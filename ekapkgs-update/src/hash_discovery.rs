use regex::Regex;

/// Extract hash from Nix build error output
///
/// Nix error format: "got: sha256-<hash>"
fn extract_hash_from_error(stderr: &str) -> Option<String> {
    let hash_regex = Regex::new(r"got:\s+(sha256-[A-Za-z0-9+/=]+)").ok()?;
    let caps = hash_regex.captures(stderr)?;
    Some(caps.get(1)?.as_str().to_owned())
}

/// Discover hash without the update-verify cycle
///
/// This is useful when you just want to discover the hash without updating the file,
/// such as in the variant discovery case where you're working with temporary content.
///
/// # Arguments
/// * `eval_entry_point` - Entry point for Nix evaluation
/// * `attr_path` - Full attribute path to build (including any suffixes)
/// * `stderr_output` - The stderr output from a failed build attempt
///
/// # Returns
/// The extracted hash from the error, or None if not found
pub fn extract_hash(stderr_output: &str) -> Option<String> {
    extract_hash_from_error(stderr_output)
}

/// The `got:` hash of the mismatch whose `specified:` hash is `specified`.
///
/// A build can fail on another fixed-output derivation's mismatch (a stale
/// hash this update has not reached yet); taking the first `got:` would
/// write that FOD's hash in place of the one being discovered.
pub fn extract_hash_for(stderr_output: &str, specified: &str) -> Option<String> {
    let mut lines = stderr_output.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some(value) = line.strip_prefix("specified:") else {
            continue;
        };
        if value.trim() != specified {
            continue;
        }
        let got = lines.next()?.strip_prefix("got:")?.trim();
        if got.starts_with("sha256-") {
            return Some(got.to_owned());
        }
    }
    None
}

/// Whether `value` is written like a Nix hash: SRI (`sha256-…`), a
/// `<algo>:`-prefixed hash, or a bare nix32 / base16 sha256.
///
/// Used to tell hash-valued attributes from ones that name a file or
/// derivation, such as `nugetDeps = ./deps.json;` (evaluates to a path).
pub fn looks_like_hash(value: &str) -> bool {
    let value = value.trim();
    let is_base64 = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
    };
    for algo in ["sha1", "sha256", "sha512", "md5"] {
        if let Some(rest) = value.strip_prefix(algo) {
            if let Some(b64) = rest.strip_prefix('-') {
                return is_base64(b64);
            }
            if let Some(digest) = rest.strip_prefix(':') {
                return is_base64(digest);
            }
        }
    }
    let nix32 = value.len() == 52
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase() && !"eotu".contains(c)));
    let base16 = value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit());
    nix32 || base16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_hash_cases() {
        for (value, expected) in [
            ("sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", true),
            ("sha512-abc+/=", true),
            (
                "sha256:0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73",
                true,
            ),
            ("0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73", true),
            (
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                true,
            ),
            ("/nix/store/abc-deps.json", false),
            ("/home/user/pkgs/foo/deps.nix", false),
            ("", false),
            ("sha256-", false),
        ] {
            assert_eq!(looks_like_hash(value), expected, "{value}");
        }
    }

    #[test]
    fn extract_hash_for_the_fake_hash_only() {
        // kapacitor: the stale libflux vendor FOD fails next to go-modules.
        let stderr = r#"
error: hash mismatch in fixed-output derivation '/nix/store/a-libflux-0.201.0-vendor-staging.drv':
         specified: sha256-kbI1uUDE8JyFFtwV5k0EeeNGCZFQLXLobW/MilHX2Sg=
            got:    sha256-hSQDiFvVsFrRccC1yFV2+bmRzch6VnwChrkTW93L1ws=
error: hash mismatch in fixed-output derivation '/nix/store/b-kapacitor-1.8.9-go-modules.drv':
         specified: sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
            got:    sha256-goModulesHashxxxxxxxxxxxxxxxxxxxxxxxxxxxxx=
"#;
        assert_eq!(
            extract_hash_for(
                stderr,
                "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
            ),
            Some("sha256-goModulesHashxxxxxxxxxxxxxxxxxxxxxxxxxxxxx=".to_owned())
        );
        assert_eq!(extract_hash_for(stderr, "sha256-other"), None);
    }

    #[test]
    fn test_extract_hash_from_error() {
        let stderr = r#"
error: hash mismatch in fixed-output derivation '/nix/store/...':
  specified: sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
  got:     sha256-abcdef1234567890ABCDEF1234567890ABCDEF12=
"#;

        let hash = extract_hash_from_error(stderr);
        assert_eq!(
            hash,
            Some("sha256-abcdef1234567890ABCDEF1234567890ABCDEF12=".to_owned())
        );
    }

    #[test]
    fn test_extract_hash_no_match() {
        let stderr = "Some error without a hash";
        let hash = extract_hash_from_error(stderr);
        assert_eq!(hash, None);
    }
}
