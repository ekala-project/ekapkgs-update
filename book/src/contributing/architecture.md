# Architecture

This document describes the high-level architecture and design of ekapkgs-update.

## Overview

ekapkgs-update is a Rust application that automates package updates in Nix ecosystems. It follows a modular architecture with clear separation of concerns.

**Technology Stack:**

- **Language**: Rust (stable)
- **Async Runtime**: Tokio
- **CLI Framework**: Clap v4
- **Error Handling**: anyhow
- **HTTP Client**: reqwest
- **JSON**: serde_json
- **Database**: rusqlite (SQLite)
- **Nix**: Process execution (`nix-instantiate`, `nix-build`)

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        CLI (clap)                           │
│                      src/cli.rs                             │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│                     Commands Layer                          │
│              src/commands/{update,run,log}                  │
├─────────────────────────────────────────────────────────────┤
│  • Update Command - Single package updates                 │
│  • Run Command - Daemon mode, continuous updates           │
│  • Log Command - View failure logs                         │
└────────┬───────────────────────────────────┬────────────────┘
         │                                   │
         ▼                                   ▼
┌──────────────────────────┐      ┌────────────────────────────┐
│  Package Metadata Layer  │      │   VCS Sources Layer        │
│     src/package/         │      │   src/vcs_sources/         │
├──────────────────────────┤      ├────────────────────────────┤
│ • Nix evaluation         │      │ • GitHub API client        │
│ • Metadata extraction    │      │ • GitLab API client        │
│ • Passthru attributes    │      │ • PyPI API client          │
│ • Flake support          │      │ • Git tag fetching         │
└───────┬──────────────────┘      └──────┬─────────────────────┘
        │                                │
        │                                │
        └────────────┬───────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│                   Rewrite Layer                             │
│                   src/rewrite/                              │
├─────────────────────────────────────────────────────────────┤
│ • Nix AST parsing                                           │
│ • Version field updates                                     │
│ • Hash field updates                                        │
│ • Dependency hash updates                                   │
└─────────────────────────────────────────────────────────────┘
```

## Core Components

### 1. CLI Layer (`src/cli.rs`)

**Responsibility:** Parse command-line arguments and dispatch to commands.

**Key types:**

```rust
pub struct Cli {
    pub command: Command,
    pub color: ColorWhen,
}

pub enum Command {
    Update(UpdateArgs),
    Run(RunArgs),
    Log(LogArgs),
    Migrate(MigrateArgs),
    PruneMaintainers(PruneMaintainersArgs),
}
```

**Dependencies:** `clap`

**Flow:**

1. Parse CLI arguments
2. Dispatch to appropriate command handler
3. Return exit code

### 2. Package Metadata Layer (`src/package/`)

**Responsibility:** Extract package information from Nix files.

**Key types:**

```rust
pub struct PackageMetadata {
    pub version: String,
    pub pname: String,
    pub attr_path: String,
    pub file_path: PathBuf,
    pub version_source: Option<VersionSource>,
    pub version_prefix: Option<String>,

    // Passthru attributes (EEP-0039)
    pub skip: Option<bool>,
    pub semver_strategy: Option<SemverStrategy>,
    pub include_prereleases: Option<bool>,
    pub version_regex: Option<String>,

    // ... other fields
}

pub enum VersionSource {
    GitHub { owner: String, repo: String },
    GitLab { owner: String, repo: String },
    PyPI { package: String },
    Git { url: String },
}
```

**Implementation:**

- **Evaluation**: Calls `nix-instantiate --eval` to query package attributes
- **Passthru attributes**: Queries `passthru.ekapkgs-update.*` for configuration
- **Source detection**: Analyzes `src` attribute to determine version source
- **Flake support**: Special handling for flake packages

**Files:**

- `src/package/mod.rs` - Main metadata extraction
- `src/package/flake.rs` - Flake-specific logic

### 3. VCS Sources Layer (`src/vcs_sources/`)

**Responsibility:** Fetch available versions from upstream sources.

**Key types:**

```rust
pub struct Release {
    pub tag_name: String,
    pub version: String,
    pub is_prerelease: bool,
    pub published_at: Option<DateTime<Utc>>,
}

pub enum SemverStrategy {
    Latest,  // Accept any version
    Major,   // Latest within same major version
    Minor,   // Latest within same minor version
    Patch,   // Latest within same patch version
}
```

**Implementations:**

#### GitHub (`src/vcs_sources/github.rs`)

- **API**: GitHub REST API v3
- **Endpoints**:
  - `/repos/{owner}/{repo}/releases` - Get releases
  - `/repos/{owner}/{repo}/tags` - Get tags (fallback)
- **Rate limiting**: 60/hour unauthenticated, 5000/hour with token
- **Authentication**: `GITHUB_TOKEN` environment variable

#### GitLab (`src/vcs_sources/gitlab.rs`)

- **API**: GitLab API v4
- **Endpoints**:
  - `/projects/{id}/releases` - Get releases
  - `/projects/{id}/repository/tags` - Get tags
- **Authentication**: `GITLAB_TOKEN` environment variable

#### PyPI (`src/vcs_sources/pypi.rs`)

- **API**: PyPI JSON API
- **Endpoint**: `/pypi/{package}/json`
- **No authentication required**

#### Git (`src/vcs_sources/git.rs`)

- **Method**: Direct `git ls-remote --tags`
- **Use case**: Repositories without GitHub/GitLab APIs
- **No authentication** (uses public repos)

**Version Matching Logic:**

```rust
impl Release {
    pub fn matches(
        &self,
        current_version: &str,
        strategy: SemverStrategy,
        version_prefix: Option<&str>,
        version_regex: Option<&str>,
        include_prereleases: bool,
    ) -> bool {
        // 1. Filter prereleases
        if self.is_prerelease && !include_prereleases {
            return false;
        }

        // 2. Apply version regex if provided
        let version = if let Some(regex) = version_regex {
            extract_version_with_regex(&self.tag_name, regex)?
        } else {
            &self.version
        };

        // 3. Apply semver strategy
        match strategy {
            Latest => true,
            Major => same_major(current_version, version),
            Minor => same_minor(current_version, version),
            Patch => same_patch(current_version, version),
        }
    }
}
```

### 4. Rewrite Layer (`src/rewrite/`)

**Responsibility:** Modify Nix files while preserving structure and formatting.

**Approach:**

ekapkgs-update uses **regex-based rewriting** rather than full AST parsing. This preserves:
- Comments
- Formatting
- Indentation
- Non-semantic whitespace

**Key operations:**

```rust
pub fn update_version(file: &str, old_version: &str, new_version: &str) -> String;
pub fn update_hash(file: &str, old_hash: &str, new_hash: &str) -> String;
pub fn update_dependency_hash(file: &str, dep: &str, old_hash: &str, new_hash: &str) -> String;
```

**Pattern matching:**

```nix
# Original
version = "1.2.3";

# Regex pattern
version\s*=\s*"([^"]+)";

# Replacement
version = "1.3.0";
```

**Hash updates:**

Handles multiple hash formats:
- `sha256 = "sha256-...";`
- `hash = "sha256-...";`
- `cargoSha256 = "sha256-...";`
- `vendorSha256 = "sha256-...";`
- Legacy: `sha256 = "1abc...";` (base32)

**Files:**

- `src/rewrite/mod.rs` - Main rewriting logic
- `src/rewrite/tests.rs` - Extensive test suite

**Testing:**

The rewrite module has comprehensive tests ensuring:
- Version updates work correctly
- Hash updates don't corrupt other hashes
- Formatting is preserved
- Edge cases are handled

### 5. Update Command (`src/commands/update/`)

**Responsibility:** Orchestrate single package updates.

**Architecture:**

```
Update Command
├── config.rs       - Configuration and argument parsing
├── mod.rs          - Main update workflow
├── file_update.rs  - File-level update operations
├── hash_workflows.rs - Hash computation workflows
├── build.rs        - Package building
├── git.rs          - Git commit creation
├── pr.rs           - Pull request creation
├── format.rs       - Nixfmt integration
└── variants.rs     - mkManyVariants support
```

**Update Workflow:**

```rust
async fn update_package(args: UpdateArgs) -> Result<()> {
    // 1. Extract metadata
    let metadata = PackageMetadata::from_attr_path(&file, &attr_path).await?;

    // 2. Check skip flag
    if metadata.skip == Some(true) && !args.force {
        bail!("Package has skip=true. Use --force to override.");
    }

    // 3. Fetch available versions
    let releases = fetch_releases(&metadata).await?;

    // 4. Find compatible version
    let semver_strategy = metadata.semver_strategy.unwrap_or(args.semver);
    let compatible_release = find_best_release(
        &releases,
        &metadata.version,
        semver_strategy,
        metadata.include_prereleases.unwrap_or(false),
        metadata.version_regex.as_deref(),
    )?;

    // 5. Update file
    let mut content = fs::read_to_string(&metadata.file_path)?;
    content = rewrite::update_version(&content, &metadata.version, &compatible_release.version);

    // 6. Compute new hash
    fs::write(&metadata.file_path, &content)?;
    let new_hash = compute_hash(&metadata).await?;
    content = rewrite::update_hash(&content, &old_hash, &new_hash);

    // 7. Update dependency hashes (if not --src-only)
    if !args.src_only {
        content = update_dependency_hashes(&content, &metadata).await?;
    }

    // 8. Build to verify
    build_package(&metadata).await?;

    // 9. Commit (if --commit or --create-pr)
    if args.commit || args.create_pr {
        git::commit(&metadata, &old_version, &new_version).await?;
    }

    // 10. Create PR (if --create-pr)
    if args.create_pr {
        pr::create(&metadata, &old_version, &new_version, &args).await?;
    }

    Ok(())
}
```

**Hash Computation:**

```rust
async fn compute_hash(metadata: &PackageMetadata) -> Result<String> {
    // Attempt 1: Build and read from error message
    let output = Command::new("nix-build")
        .args(&["-A", &metadata.attr_path])
        .output()
        .await?;

    // Parse hash from error message like:
    // "got:    sha256-ABC..."
    if let Some(hash) = extract_hash_from_output(&output) {
        return Ok(hash);
    }

    // Attempt 2: Use nix-prefetch-url
    let hash = nix_prefetch_url(&metadata.src_url).await?;
    Ok(hash)
}
```

### 6. Run Command (Daemon Mode) (`src/commands/run/`)

**Responsibility:** Continuous automated updates for many packages.

**Architecture:**

```
Run Command
├── mod.rs      - Main daemon loop
├── checker.rs  - Check packages for updates
├── updater.rs  - Execute updates
├── config.rs   - Configuration
└── types.rs    - Shared types
```

**Daemon Workflow:**

```rust
async fn run_daemon(args: RunArgs) -> Result<()> {
    // 1. Open database
    let db = Database::open(&args.database)?;

    // 2. Enumerate packages
    let packages = enumerate_packages(&args.file).await?;
    info!("Found {} packages", packages.len());

    // 3. Check each package for updates
    let updates: Vec<UpdateTask> = packages
        .into_iter()
        .filter_map(|pkg| check_package(&pkg, &db, &args).await.ok())
        .collect();

    // 4. Execute updates concurrently
    let concurrent_updates = args.concurrent_updates.unwrap_or(num_cpus() / 4);
    let results = futures::stream::iter(updates)
        .map(|update| execute_update(update, &args))
        .buffer_unordered(concurrent_updates)
        .collect::<Vec<_>>()
        .await;

    // 5. Report summary
    let successful = results.iter().filter(|r| r.is_ok()).count();
    let failed = results.iter().filter(|r| r.is_err()).count();
    info!("Summary: {} updated, {} failed", successful, failed);

    Ok(())
}
```

**Database Schema:**

```sql
CREATE TABLE updates (
    attr_path TEXT PRIMARY KEY,
    last_checked_at INTEGER,  -- Unix timestamp
    last_updated_at INTEGER,
    last_version TEXT,
    last_error TEXT
);

CREATE TABLE failure_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    attr_path TEXT,
    timestamp INTEGER,
    error_message TEXT,
    FOREIGN KEY (attr_path) REFERENCES updates(attr_path)
);
```

## Data Flow

### Update Flow (Manual Update)

```
User Input
    │
    ▼
CLI Parsing (clap)
    │
    ▼
UpdateArgs
    │
    ▼
PackageMetadata::from_attr_path()
    │
    ├─> nix-instantiate --eval (query Nix)
    └─> Extract passthru.ekapkgs-update attributes
    │
    ▼
Fetch Releases (VCS Sources)
    │
    ├─> GitHub API (if GitHub source)
    ├─> GitLab API (if GitLab source)
    ├─> PyPI API (if PyPI source)
    └─> git ls-remote (if Git source)
    │
    ▼
Find Compatible Release (version matching)
    │
    ├─> Filter prereleases
    ├─> Apply version regex
    └─> Apply semver strategy
    │
    ▼
Rewrite File
    │
    ├─> Update version
    ├─> Update main hash
    └─> Update dependency hashes
    │
    ▼
Build Package (verification)
    │
    └─> nix-build -A attr-path
    │
    ▼
Git Commit (optional)
    │
    └─> git commit -m "..."
    │
    ▼
Create PR (optional)
    │
    └─> gh pr create
    │
    ▼
Success
```

### Daemon Flow (Continuous Updates)

```
Daemon Start
    │
    ▼
Open Database
    │
    ▼
Enumerate Packages
    │
    └─> nix-instantiate --eval (get all packages)
    │
    ▼
For each package:
    │
    ├─> Check if skip=true (skip if true)
    ├─> Check last_checked_at (skip if recent)
    └─> Fetch latest version
    │
    ▼
Collect Update Tasks
    │
    ▼
Execute Updates (concurrent)
    │
    ├─> Task 1: update package1
    ├─> Task 2: update package2
    ├─> ...
    └─> Task N: update packageN
    │
    ▼
Update Database
    │
    ├─> Record successful updates
    └─> Log failures
    │
    ▼
Report Summary
```

## Design Patterns

### 1. Builder Pattern

Used for configuration:

```rust
let config = UpdateConfig::builder()
    .semver(SemverStrategy::Minor)
    .include_prereleases(false)
    .force(true)
    .build();
```

### 2. Strategy Pattern

SemverStrategy allows different version selection strategies:

```rust
trait VersionMatcher {
    fn matches(&self, current: &str, candidate: &str) -> bool;
}

impl VersionMatcher for SemverStrategy {
    fn matches(&self, current: &str, candidate: &str) -> bool {
        match self {
            Latest => true,
            Major => same_major(current, candidate),
            Minor => same_minor(current, candidate),
            Patch => same_patch(current, candidate),
        }
    }
}
```

### 3. Repository Pattern

VCS sources abstract data access:

```rust
trait ReleaseRepository {
    async fn fetch_releases(&self) -> Result<Vec<Release>>;
}

impl ReleaseRepository for GitHubClient { ... }
impl ReleaseRepository for GitLabClient { ... }
impl ReleaseRepository for PyPIClient { ... }
```

### 4. Command Pattern

Each CLI command is a separate module with a `run()` function:

```rust
pub async fn run(args: UpdateArgs) -> Result<()> {
    // Command implementation
}
```

## Error Handling

**Strategy:** Use `anyhow` for error propagation with context.

**Example:**

```rust
use anyhow::{Context, Result};

fn update_file(path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)
        .context("Failed to read file")?;

    let updated = rewrite::update_version(&content, "1.0.0", "2.0.0")
        .context("Failed to rewrite version")?;

    fs::write(path, updated)
        .context("Failed to write updated file")?;

    Ok(())
}
```

**User-facing errors:**

- Clear error messages with context
- Actionable suggestions when possible
- Debug logging with RUST_LOG for detailed errors

## Performance Considerations

### Concurrency

- **Async/await**: All I/O operations are async
- **Tokio runtime**: Efficient task scheduling
- **Concurrent updates**: Daemon mode uses `buffer_unordered` for parallelism
- **Rate limiting**: Respect GitHub/GitLab API limits

### Caching

- **Database**: Prevents re-checking recently checked packages
- **HTTP caching**: Respects Cache-Control headers (via reqwest)
- **Build caching**: Relies on Nix store for build caching

### Optimization

- **Lazy evaluation**: Only fetch releases when needed
- **Early termination**: Stop searching if compatible version found
- **Minimal rebuilds**: Only update hashes when necessary

## Testing Strategy

### Unit Tests

- Located in `#[cfg(test)]` modules
- Test individual functions in isolation
- Mock external dependencies where needed

### Integration Tests

- Located in `tests/` directory
- Test full workflows end-to-end
- Use real Nix files and packages

### Test Coverage

Aim for:
- **Core logic**: 90%+ coverage
- **Rewrite logic**: 100% coverage (critical for correctness)
- **VCS sources**: High coverage with mocked HTTP clients

## Security Considerations

### API Tokens

- Never log tokens
- Read from environment variables only
- Support multiple tokens for different services

### Nix Evaluation

- Use `--read-only-mode` when possible
- Sanitize file paths
- Validate attribute paths

### File Rewriting

- Atomic writes (write to temp file, then move)
- Backup before modifying (optional)
- Validate before writing

## Future Architecture

### Planned Improvements

1. **Plugin system** - Allow custom VCS sources
2. **Configuration file** - `.ekapkgs-update.toml`
3. **Webhook support** - Trigger updates on upstream releases
4. **Improved caching** - Cache GitHub/GitLab API responses
5. **Parallel Nix evaluation** - Speed up package enumeration

### Extensibility Points

- **New VCS sources**: Implement `ReleaseRepository` trait
- **New commands**: Add variant to `Command` enum
- **New passthru attributes**: Add field to `PackageMetadata`

## See Also

- [Development Guide](./development.md) - How to contribute
- [Source Code Documentation](https://docs.rs/ekapkgs-update) - Rust API docs
- [EEP-0039](https://github.com/ekala-project/eeps/blob/main/eeps/0039-ekapkgs-update-passthru.md) - Passthru attributes specification
