use regex::Regex;

/// Extract hash from Nix build error output
///
/// Nix error format: "got: sha256-<hash>"
fn extract_hash_from_error(stderr: &str) -> Option<String> {
    let hash_regex = Regex::new(r"got:\s+(sha256-[A-Za-z0-9+/=]+)").ok()?;
    let caps = hash_regex.captures(stderr)?;
    Some(caps.get(1)?.as_str().to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

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
            Some("sha256-abcdef1234567890ABCDEF1234567890ABCDEF12=".to_string())
        );
    }

    #[test]
    fn test_extract_hash_no_match() {
        let stderr = "Some error without a hash";
        let hash = extract_hash_from_error(stderr);
        assert_eq!(hash, None);
    }
}
