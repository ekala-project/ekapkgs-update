//! Configuration file support for ekapkgs-update.
//!
//! Loads settings from a TOML file, allowing NixOS modules to render a
//! configuration file rather than passing everything via CLI flags or
//! environment variables.
//!
//! ## Precedence
//!
//! CLI flags > config file > environment variables > built-in defaults
//!
//! ## Config file resolution
//!
//! 1. `--config-file <path>` CLI flag
//! 2. `EKAPKGS_CONFIG_FILE` environment variable
//! 3. `~/.config/ekapkgs-update/config.toml` (XDG default)
//!
//! ## Example
//!
//! ```toml
//! database = "/var/lib/ekapkgs-update/updates.db"
//! file = "default.nix"
//!
//! [llm]
//! base_url = "http://llm-server:8080"
//! model = "qwen2.5-coder:3b"
//! max_tokens = 2048
//! temperature = 0.1
//! timeout_secs = 300
//!
//! [autofix]
//! max_attempts = 3
//!
//! [nix]
//! builders = "ssh://builder"
//! max_jobs = 4
//! ```

use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Default XDG config file name.
const CONFIG_FILENAME: &str = "config.toml";
/// Application name for XDG directory lookup.
const APP_NAME: &str = "ekapkgs-update";
/// Repo name reserved for the unnamed (`''`) scope used by `run` and
/// single-repo watch; the dashboard displays and links that scope as
/// `default`, so a real repo with this name would be indistinguishable.
pub const RESERVED_REPO_NAME: &str = "default";

/// Whether `name` is an allowed repo name: an ASCII letter or digit followed
/// by letters, digits, `-` or `_`. Repo names are joined with attr paths by
/// `+` into worktree and preserved-failure names
/// (`commands::run::preservation::repo_scoped_name`), so they must never
/// contain it, and they must be safe as a path component.
pub fn is_valid_repo_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Configuration for a single watched repository.
///
/// Used by the `watch` command to track multiple upstream repositories,
/// each with its own Nix entry point and PR target.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RepoConfig {
    /// Human-readable name for this repo (used in logs and database tagging).
    pub name: String,

    /// Upstream repository identifier. Accepts `owner/repo` shorthand (assumes
    /// GitHub) or a full URL like `github.com/owner/repo`.
    pub upstream: String,

    /// Nix file to evaluate within the repo's working directory.
    #[serde(default = "default_entrypoint")]
    pub entrypoint: String,

    /// Git remote name to push update branches to for PR creation.
    #[serde(default = "default_fork")]
    pub fork: String,

    /// Local path to the repo checkout. The daemon will clone the repo here
    /// if it does not already exist.
    pub path: Option<String>,
}

fn default_entrypoint() -> String {
    "default.nix".to_owned()
}

fn default_fork() -> String {
    "origin".to_owned()
}

/// Top-level configuration file structure.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ConfigFile {
    /// Path to the SQLite database.
    pub database: Option<String>,

    /// Nix file to evaluate (e.g., `default.nix`).
    pub file: Option<String>,

    /// LLM server configuration.
    pub llm: LlmConfig,

    /// Autofix pipeline settings.
    pub autofix: AutofixConfig,

    /// Nix build settings.
    pub nix: NixConfig,

    /// Watch (passive/event-driven) mode settings.
    pub watch: WatchFileConfig,

    /// Repositories to watch for updates. When non-empty, each repo carries
    /// its own entry point and PR target. Used by the `watch` command.
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
}

/// LLM server configuration.
///
/// All fields are optional. When a field is `None`, the corresponding
/// environment variable is checked, then the built-in default is used.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LlmConfig {
    /// Base URL of the OpenAI-compatible API server.
    /// Env fallback: `EKAPKGS_LLM_BASE_URL`
    pub base_url: Option<String>,

    /// Model name to use for completions and embeddings.
    /// Env fallback: `EKAPKGS_LLM_MODEL`
    pub model: Option<String>,

    /// Maximum tokens in the LLM response.
    /// Env fallback: `EKAPKGS_LLM_MAX_TOKENS`
    pub max_tokens: Option<u32>,

    /// Sampling temperature (0.0 = deterministic, higher = more creative).
    /// Env fallback: `EKAPKGS_LLM_TEMPERATURE`
    pub temperature: Option<f32>,

    /// Request timeout in seconds. Small models on CPU can be slow.
    /// Env fallback: `EKAPKGS_LLM_TIMEOUT_SECS`
    pub timeout_secs: Option<u64>,
}

/// Autofix pipeline settings.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AutofixConfig {
    /// Maximum LLM fix attempts per package before escalation to human.
    pub max_attempts: Option<i64>,
}

/// Nix build settings applied globally to all `nix-build` invocations.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct NixConfig {
    /// Nix builders string (e.g., `ssh://builder`).
    /// Passed as `--builders` to `nix-build`.
    pub builders: Option<String>,

    /// Maximum number of local build jobs.
    /// Passed as `--max-jobs` to `nix-build`.
    pub max_jobs: Option<usize>,
}

/// Watch (passive/event-driven) mode settings.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct WatchFileConfig {
    /// Feed poll interval in minutes (default: 10).
    pub poll_interval_minutes: Option<u64>,

    /// Full index refresh interval in hours (default: 24).
    pub index_refresh_hours: Option<u64>,
}

impl ConfigFile {
    /// Load a config file, returning the default if no file exists.
    ///
    /// Resolution order:
    /// 1. Explicit path (from `--config-file`)
    /// 2. `EKAPKGS_CONFIG_FILE` env var
    /// 3. XDG default: `~/.config/ekapkgs-update/config.toml`
    pub fn load(explicit_path: Option<&str>) -> anyhow::Result<Self> {
        let path = resolve_config_path(explicit_path);

        let Some(path) = path else {
            debug!("No config file found, using defaults");
            return Ok(Self::default());
        };

        if !path.exists() {
            if explicit_path.is_some() {
                bail!("Config file not found: {}", path.display());
            }
            debug!(
                "Default config file not found at {}, using defaults",
                path.display()
            );
            return Ok(Self::default());
        }

        info!("Loading config from {}", path.display());
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("read config file {}", path.display()))?;

        let config: Self = toml::from_str(&content)
            .with_context(|| format!("parse config file {}", path.display()))?;

        config.validate()?;

        Ok(config)
    }

    /// Validate configuration values.
    fn validate(&self) -> anyhow::Result<()> {
        if let Some(ref url) = self.llm.base_url {
            if url.is_empty() {
                bail!("llm.base_url cannot be empty");
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                bail!("llm.base_url must start with http:// or https://");
            }
        }

        if let Some(temp) = self.llm.temperature {
            if !(0.0..=2.0).contains(&temp) {
                bail!("llm.temperature must be between 0.0 and 2.0, got {temp}");
            }
        }

        if let Some(tokens) = self.llm.max_tokens {
            if tokens == 0 {
                bail!("llm.max_tokens must be greater than 0");
            }
        }

        if let Some(attempts) = self.autofix.max_attempts {
            if attempts < 1 {
                bail!("autofix.max_attempts must be at least 1, got {attempts}");
            }
        }

        // Validate repo configs
        let mut seen_names = std::collections::HashSet::new();
        for repo in &self.repos {
            if repo.name.is_empty() {
                bail!("repos: each repo must have a non-empty name");
            }
            if !is_valid_repo_name(&repo.name) {
                bail!(
                    "repos: invalid repo name '{}' (use ASCII letters, digits, '-' and '_', \
                     starting with a letter or digit)",
                    repo.name
                );
            }
            if repo.name.eq_ignore_ascii_case(RESERVED_REPO_NAME) {
                bail!(
                    "repos: '{}' is reserved (it labels the single-repo / `run` scope)",
                    repo.name
                );
            }
            if !seen_names.insert(&repo.name) {
                bail!("repos: duplicate repo name '{}'", repo.name);
            }
            if repo.upstream.is_empty() {
                bail!("repos.{}: upstream cannot be empty", repo.name);
            }
        }

        Ok(())
    }
}

impl LlmConfig {
    /// Resolve the base URL from config file, then env var.
    pub fn resolve_base_url(&self) -> Option<String> {
        self.base_url
            .clone()
            .or_else(|| std::env::var("EKAPKGS_LLM_BASE_URL").ok())
    }

    /// Resolve the model name from config file, then env var.
    pub fn resolve_model(&self) -> Option<String> {
        self.model
            .clone()
            .or_else(|| std::env::var("EKAPKGS_LLM_MODEL").ok())
    }

    /// Resolve max tokens from config file, then env var.
    pub fn resolve_max_tokens(&self) -> Option<u32> {
        self.max_tokens.or_else(|| {
            std::env::var("EKAPKGS_LLM_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
        })
    }

    /// Resolve temperature from config file, then env var.
    pub fn resolve_temperature(&self) -> Option<f32> {
        self.temperature.or_else(|| {
            std::env::var("EKAPKGS_LLM_TEMPERATURE")
                .ok()
                .and_then(|s| s.parse().ok())
        })
    }

    /// Resolve timeout from config file, then env var.
    pub fn resolve_timeout_secs(&self) -> Option<u64> {
        self.timeout_secs.or_else(|| {
            std::env::var("EKAPKGS_LLM_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
        })
    }
}

/// Determine the config file path.
fn resolve_config_path(explicit: Option<&str>) -> Option<PathBuf> {
    // 1. Explicit path from --config-file
    if let Some(path) = explicit {
        return Some(PathBuf::from(shellexpand::tilde(path).as_ref()));
    }

    // 2. EKAPKGS_CONFIG_FILE env var
    if let Ok(path) = std::env::var("EKAPKGS_CONFIG_FILE") {
        return Some(PathBuf::from(shellexpand::tilde(&path).as_ref()));
    }

    // 3. XDG default
    let dirs = directories::ProjectDirs::from("", "", APP_NAME)?;
    Some(dirs.config_dir().join(CONFIG_FILENAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_is_valid() {
        let config: ConfigFile = toml::from_str("").unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn test_full_config_roundtrip() {
        let toml_str = r#"
database = "/var/lib/ekapkgs-update/updates.db"
file = "default.nix"

[llm]
base_url = "http://llm-server:8080"
model = "qwen2.5-coder:3b"
max_tokens = 2048
temperature = 0.1
timeout_secs = 300

[autofix]
max_attempts = 5

[nix]
builders = "ssh://builder"
max_jobs = 4
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.database.as_deref(),
            Some("/var/lib/ekapkgs-update/updates.db")
        );
        assert_eq!(
            config.llm.base_url.as_deref(),
            Some("http://llm-server:8080")
        );
        assert_eq!(config.llm.model.as_deref(), Some("qwen2.5-coder:3b"));
        assert_eq!(config.llm.max_tokens, Some(2048));
        assert_eq!(config.llm.temperature, Some(0.1));
        assert_eq!(config.llm.timeout_secs, Some(300));
        assert_eq!(config.autofix.max_attempts, Some(5));
        assert_eq!(config.nix.builders.as_deref(), Some("ssh://builder"));
        assert_eq!(config.nix.max_jobs, Some(4));

        config.validate().unwrap();
    }

    #[test]
    fn test_invalid_base_url() {
        let config = ConfigFile {
            llm: LlmConfig {
                base_url: Some("not-a-url".to_owned()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_temperature() {
        let config = ConfigFile {
            llm: LlmConfig {
                temperature: Some(3.0),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_max_attempts() {
        let config = ConfigFile {
            autofix: AutofixConfig {
                max_attempts: Some(0),
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_partial_config() {
        let toml_str = r#"
[llm]
base_url = "http://localhost:11434"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.llm.base_url.as_deref(),
            Some("http://localhost:11434")
        );
        assert!(config.llm.model.is_none());
        assert!(config.database.is_none());
        config.validate().unwrap();
    }

    #[test]
    fn test_unknown_keys_ignored() {
        let toml_str = r#"
unknown_key = "value"

[llm]
base_url = "http://localhost:8080"
unknown_llm_key = "value"
"#;
        // serde default ignores unknown keys
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn test_repos_config() {
        let toml_str = r#"
database = "/var/lib/ekapkgs-update/updates.db"

[[repos]]
name = "core-pkgs"
upstream = "github.com/ekala-project/core-pkgs"
entrypoint = "default.nix"
path = "/var/lib/ekapkgs-update/repos/core-pkgs"

[[repos]]
name = "ekapkgs"
upstream = "github.com/ekala-project/ekapkgs"
path = "/var/lib/ekapkgs-update/repos/ekapkgs"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.repos.len(), 2);
        assert_eq!(config.repos[0].name, "core-pkgs");
        assert_eq!(
            config.repos[0].upstream,
            "github.com/ekala-project/core-pkgs"
        );
        assert_eq!(config.repos[0].entrypoint, "default.nix");
        assert_eq!(config.repos[1].name, "ekapkgs");
        assert_eq!(config.repos[1].fork, "origin"); // default
        config.validate().unwrap();
    }

    #[test]
    fn test_repos_duplicate_name_rejected() {
        let toml_str = r#"
[[repos]]
name = "core-pkgs"
upstream = "github.com/ekala-project/core-pkgs"

[[repos]]
name = "core-pkgs"
upstream = "github.com/ekala-project/ekapkgs"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_repos_reserved_name_rejected() {
        for name in ["default", "Default"] {
            let toml_str =
                format!("[[repos]]\nname = \"{name}\"\nupstream = \"ekala-project/core-pkgs\"\n");
            let config: ConfigFile = toml::from_str(&toml_str).unwrap();
            assert!(config.validate().is_err(), "{name} should be rejected");
        }
    }

    #[test]
    fn test_repo_name_charset() {
        for name in ["core-pkgs", "ekapkgs", "a_b", "a-b", "9lives"] {
            assert!(is_valid_repo_name(name), "{name} should be accepted");
        }
        for name in ["", "a+b", "a.b", "a/b", "-a", "_a", ".", "a b"] {
            assert!(!is_valid_repo_name(name), "{name} should be rejected");
        }
        let toml_str = "[[repos]]\nname = \"a+b\"\nupstream = \"ekala-project/core-pkgs\"\n";
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_repos_empty_name_rejected() {
        let toml_str = r#"
[[repos]]
name = ""
upstream = "github.com/ekala-project/core-pkgs"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }
}
