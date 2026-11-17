use std::path::{Path, PathBuf};

use anyhow::Context;
use tracing::{debug, info, warn};

use super::variants::find_version_in_siblings;
use crate::commands::migrate::{convert_to_final_attrs_pattern_generic, fix_closing_brace};
use crate::nix::is_many_variants_package;
use crate::rewrite::{
    align_ref_with_tag, find_and_update_attr, is_single_ref_from, needs_final_attrs_conversion,
    try_fixup_stale_rev, try_update_rev_attr,
};

/// The git tag of the release being updated to, for upstreams whose releases
/// are tags in a repository (GitHub, GitLab, SourceHut).
#[derive(Debug, Clone, Copy)]
pub struct ReleaseTag<'a> {
    /// Tag name, e.g. `v1.2.3`
    pub name: &'a str,
    /// Repository name, as bound by `repo = "…";` in its fetcher
    pub repo: &'a str,
}

/// Update version and hash in a Nix file
///
/// Returns the actual file path that was updated (may differ from input due to mkManyVariants)
///
/// With a `release_tag`, the file's git ref is checked against it: a stale
/// commit SHA is rewritten to reference the version, and a ref built from an
/// older tag scheme is rewritten to the release's tag. Both only happen when
/// the file's only `rev`/`tag` fetches the release's repository.
pub async fn update_nix_file(
    eval_entry_point: &str,
    attr_path: &str,
    file_path: &Path,
    old_version: &str,
    new_version: &str,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
    release_tag: Option<ReleaseTag<'_>>,
) -> anyhow::Result<PathBuf> {
    if file_path.extension().is_some_and(|ext| ext == "json") {
        return update_json_data_file(file_path, old_version, new_version, old_hash, new_hash)
            .await;
    }
    debug!(
        "Updating Nix file at {} using AST manipulation",
        file_path.display()
    );
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("read nix file {}", file_path.display()))?;

    // Try to update the version attribute
    let (updated_content, actual_file_path) =
        match find_and_update_attr(&content, "version", new_version, Some(old_version)) {
            Ok(content) => {
                debug!(
                    "Updated version attribute: {} -> {}",
                    old_version, new_version
                );
                (content, file_path.to_path_buf())
            },
            Err(e) if e.is_not_found() => {
                // Version kept in a JSON data file next to the expression
                // (beads-rust: `inherit (data) version;` from hashes.json)
                if let Some(json) = find_version_in_json(file_path, old_version).await? {
                    info!("Using data file {}", json.display());
                    return update_json_data_file(
                        &json,
                        old_version,
                        new_version,
                        old_hash,
                        new_hash,
                    )
                    .await;
                }
                // Version not found - check if this is a mkManyVariants package
                debug!(
                    "Version not found in {}, checking if mkManyVariants",
                    file_path.display()
                );

                if is_many_variants_package(eval_entry_point, attr_path).await? {
                    // This is a mkManyVariants package - search sibling files
                    let file_path_str = file_path
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid UTF-8 in path"))?;
                    match find_version_in_siblings(file_path_str, old_version, old_hash).await? {
                        Some(sibling_path) => {
                            info!("Using mkManyVariants file: {}", sibling_path);
                            let sibling_content = tokio::fs::read_to_string(&sibling_path)
                                .await
                                .with_context(|| format!("read sibling file {sibling_path}"))?;

                            // Try simple string replacement for mkManyVariants files
                            let updated = sibling_content.replace(old_version, new_version);
                            (updated, PathBuf::from(sibling_path))
                        },
                        None => {
                            // No sibling found, return original error
                            return Err(e.into());
                        },
                    }
                } else {
                    // Not a mkManyVariants package, return original error
                    return Err(e.into());
                }
            },
            Err(e) => return Err(e.into()),
        };

    // Try to update the rev attribute if it exists (non-blocking)
    // Only for normal files, not mkManyVariants (which use string replacement)
    let updated_content = if actual_file_path.as_path() == file_path {
        match try_update_rev_attr(&updated_content, old_version, new_version) {
            Ok(content) => {
                debug!(
                    "Updated rev attribute based on version change: {} -> {}",
                    old_version, new_version
                );
                content
            },
            Err(e) if e.is_not_found() => {
                // Check if this is a commit SHA that needs fixing via finalAttrs
                if let Some(tag) = release_tag {
                    try_fixup_commit_sha_rev(&updated_content, tag, new_version)
                } else {
                    debug!("No rev attribute to update (or skipped): {}", e);
                    updated_content
                }
            },
            Err(e) => {
                warn!("Failed to update rev attribute: {}", e);
                updated_content
            },
        }
    } else {
        // For mkManyVariants, we already did string replacement above
        updated_content
    };

    // Point the ref at the release's tag if the tag scheme changed
    let updated_content = match release_tag {
        Some(tag)
            if actual_file_path.as_path() == file_path
                && is_single_ref_from(&updated_content, tag.repo) =>
        {
            match align_ref_with_tag(&updated_content, tag.name, old_version, new_version) {
                Ok(aligned) => {
                    if aligned != updated_content {
                        info!("Rewrote git ref to match release tag {}", tag.name);
                    }
                    aligned
                },
                Err(e) => {
                    debug!("Git ref not aligned with tag {}: {}", tag.name, e);
                    updated_content
                },
            }
        },
        _ => updated_content,
    };

    // Update hash if provided
    let final_content = if let (Some(old_h), Some(new_h)) = (old_hash, new_hash) {
        // For mkManyVariants, use simple string replacement
        // For normal files, use AST-based replacement
        if actual_file_path.as_path() != file_path {
            // mkManyVariants file - use string replacement
            let result = updated_content.replace(old_h, new_h);
            debug!(
                "Updated hash using string replacement: {} -> {}",
                old_h, new_h
            );
            result
        } else {
            // Normal file - try AST-based replacement, then the hash's value
            // wherever it is bound (`cliHash = "…";` in a `let`)
            let hash_attrs = ["hash", "sha256", "outputHash", "src-hash"];

            hash_attrs
                .iter()
                .find_map(|&attr_name| {
                    find_and_update_attr(&updated_content, attr_name, new_h, Some(old_h))
                        .ok()
                        .inspect(|_| {
                            debug!("Updated {} attribute: {} -> {}", attr_name, old_h, new_h);
                        })
                })
                .or_else(|| replace_unique_literal(&updated_content, old_h, new_h))
                .with_context(|| {
                    format!(
                        "source hash {old_h} is not bound by {} and does not appear exactly once \
                         as a string in {}",
                        hash_attrs.join("/"),
                        file_path.display()
                    )
                })?
        }
    } else {
        updated_content
    };

    // Write back to file
    tokio::fs::write(&actual_file_path, final_content)
        .await
        .with_context(|| format!("write nix file {}", actual_file_path.display()))?;
    Ok(actual_file_path)
}

/// Replace the string literal `"old"` with `"new"` if it appears exactly once.
///
/// Hashes are unique strings, so a hash bound under any name (`cliHash`,
/// `mobyHash`) can be updated by value.
fn replace_unique_literal(content: &str, old: &str, new: &str) -> Option<String> {
    let quoted = format!("\"{old}\"");
    let offsets: Vec<usize> = content.match_indices(&quoted).map(|(i, _)| i).collect();
    // Several systems sharing one hash (`x86_64-linux = "…"; aarch64-linux =
    // "…";`, greptile-cli's single JS file): the build machine's line.
    let host = super::fod_platforms::native_system();
    let offset = match offsets.as_slice() {
        [offset] => *offset,
        [] => return None,
        several => {
            let on_host_line: Vec<usize> = several
                .iter()
                .copied()
                .filter(|&i| {
                    let line_start = content[..i].rfind('\n').map_or(0, |n| n + 1);
                    content[line_start..i]
                        .trim_start()
                        .trim_start_matches('"')
                        .starts_with(&host)
                })
                .collect();
            match on_host_line.as_slice() {
                [offset] => *offset,
                _ => return None,
            }
        },
    };
    debug!("Updated hash by value: {} -> {}", old, new);
    Some(format!(
        "{}\"{new}\"{}",
        &content[..offset],
        &content[offset + quoted.len()..]
    ))
}

/// Replace the hash bound by `attr` (`cargoHash`, `vendorHash`,
/// `npmDepsHash`, `nugetDeps`, `composerDepsHash`) from `old_hash` to
/// `new_hash`.
pub async fn update_hash_attr(
    file_path: &Path,
    attr: &str,
    old_hash: &str,
    new_hash: &str,
) -> anyhow::Result<()> {
    debug!("Updating {attr} in {}", file_path.display());
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("read nix file {}", file_path.display()))?;

    // A JSON data file has no Nix to parse.
    let bound = if file_path.extension().is_some_and(|ext| ext == "json") {
        None
    } else {
        match find_and_update_attr(&content, attr, new_hash, Some(old_hash)) {
            Ok(updated) => Some(updated),
            Err(e) if e.is_not_found() => None,
            Err(e) => return Err(e.into()),
        }
    };
    let (target, updated_content) = match bound {
        Some(updated) => (file_path.to_path_buf(), updated),
        // Not bound literally (`inherit (data) cargoHash;`): the hash's
        // only occurrence among the package's files
        None => {
            let dir = file_path.parent().unwrap_or(file_path);
            replace_unique_literal_in_dir(dir, old_hash, new_hash)
                .await?
                .with_context(|| {
                    format!(
                        "{attr} is not bound literally in {}, and {old_hash} does not \
                             appear exactly once among the package's files",
                        file_path.display()
                    )
                })?
        },
    };
    debug!(
        "Updated {attr}: {old_hash} -> {new_hash} in {}",
        target.display()
    );

    tokio::fs::write(&target, updated_content)
        .await
        .with_context(|| format!("write {}", target.display()))?;
    Ok(())
}

/// The `.nix`/`.json` files directly in `dir`, with their content.
pub(super) async fn package_files(dir: &Path) -> anyhow::Result<Vec<(PathBuf, String)>> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("read directory {}", dir.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let wanted = path
            .extension()
            .is_some_and(|ext| ext == "nix" || ext == "json");
        if wanted && entry.file_type().await?.is_file() {
            let content = tokio::fs::read_to_string(&path).await?;
            files.push((path, content));
        }
    }
    files.sort();
    Ok(files)
}

/// Replace the string literal `old` where it appears exactly once among
/// `files`, returning that file and its new content.
fn plan_unique_literal(
    files: &[(PathBuf, String)],
    old: &str,
    new: &str,
) -> Option<(PathBuf, String)> {
    let quoted = format!("\"{old}\"");
    let mut hits = files.iter().flat_map(|(path, content)| {
        std::iter::repeat_n((path, content), content.matches(&quoted).count())
    });
    let (path, content) = hits.next()?;
    if hits.next().is_some() {
        return None;
    }
    Some((
        path.clone(),
        content.replacen(&quoted, &format!("\"{new}\""), 1),
    ))
}

async fn replace_unique_literal_in_dir(
    dir: &Path,
    old: &str,
    new: &str,
) -> anyhow::Result<Option<(PathBuf, String)>> {
    Ok(plan_unique_literal(&package_files(dir).await?, old, new))
}

/// `"version": "<old>"` replaced, when it appears exactly once.
fn update_json_version(content: &str, old: &str, new: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r#""version"(\s*:\s*)"{}""#, regex::escape(old))).ok()?;
    let mut matches = re.find_iter(content);
    let found = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let replaced = re.replace(
        &content[found.start()..found.end()],
        |caps: &regex::Captures<'_>| format!(r#""version"{}"{new}""#, &caps[1]),
    );
    Some(format!(
        "{}{replaced}{}",
        &content[..found.start()],
        &content[found.end()..]
    ))
}

/// The one JSON file next to `file_path` holding `"version": "<version>"`.
async fn find_version_in_json(file_path: &Path, version: &str) -> anyhow::Result<Option<PathBuf>> {
    let Some(dir) = file_path.parent() else {
        return Ok(None);
    };
    let mut found = package_files(dir)
        .await?
        .into_iter()
        .filter(|(path, content)| {
            path.extension().is_some_and(|ext| ext == "json")
                && update_json_version(content, version, version).is_some()
        });
    let first = found.next();
    Ok(if found.next().is_some() {
        None
    } else {
        first.map(|(path, _)| path)
    })
}

/// Update the version and (by value) the hash in a JSON data file.
async fn update_json_data_file(
    path: &Path,
    old_version: &str,
    new_version: &str,
    old_hash: Option<&str>,
    new_hash: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let mut content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    if old_version != new_version {
        content = update_json_version(&content, old_version, new_version).with_context(|| {
            format!(
                "\"version\": \"{old_version}\" does not appear exactly once in {}",
                path.display()
            )
        })?;
    }
    if let (Some(old), Some(new)) = (old_hash, new_hash) {
        content = replace_unique_literal(&content, old, new)
            .with_context(|| format!("{old} does not appear exactly once in {}", path.display()))?;
    }
    tokio::fs::write(path, content)
        .await
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path.to_path_buf())
}

/// Attempt to fix a stale commit-SHA `rev` by converting to `finalAttrs`
/// (if needed) and rewriting `rev` to reference the version via the tag format.
///
/// Returns the fixed content on success, or the original content unchanged
/// if the fixup is not applicable.
fn try_fixup_commit_sha_rev(content: &str, tag: ReleaseTag<'_>, new_version: &str) -> String {
    use crate::rewrite::rev_update::{extract_rev_value, is_likely_commit_sha};

    // The SHA must belong to the release's repository, and be the only ref:
    // otherwise it may pin some other source (opencv's third-party data).
    if !is_single_ref_from(content, tag.repo) {
        debug!(
            "Rev not fixed up: not the only ref, or not from {}",
            tag.repo
        );
        return content.to_owned();
    }

    // Only attempt fixup when rev is actually a commit SHA
    let is_sha = extract_rev_value(content)
        .map(|rev| is_likely_commit_sha(&rev))
        .unwrap_or(false);
    if !is_sha {
        debug!("No rev attribute to update (not a commit SHA)");
        return content.to_owned();
    }

    info!("Detected stale commit SHA in rev, attempting to fix");

    let mut fixable = content.to_owned();

    // Convert to finalAttrs if the builder doesn't use rec or finalAttrs
    if needs_final_attrs_conversion(&fixable) {
        match convert_to_final_attrs_pattern_generic(&fixable).and_then(|c| fix_closing_brace(&c)) {
            Ok(converted) => {
                info!("Converted to finalAttrs pattern for version reference");
                fixable = converted;
            },
            Err(e) => {
                warn!("Could not convert to finalAttrs: {}", e);
                return content.to_owned();
            },
        }
    }

    // Rewrite rev to reference version
    match try_fixup_stale_rev(&fixable, tag.name, new_version) {
        Ok(fixed) => {
            info!("Fixed stale rev to reference version via tag format");
            fixed
        },
        Err(e) => {
            debug!("Rev fixup not applicable: {}", e);
            content.to_owned()
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{plan_unique_literal, replace_unique_literal, update_json_version};

    #[test]
    fn json_data_files() {
        let beads = "{\n  \"version\": \"0.7.3\",\n  \"hash\": \"sha256-src\",\n  \
                     \"frankensqlite\": { \"version\" : \"0.7.30\" }\n}\n";
        assert_eq!(
            update_json_version(beads, "0.7.3", "0.7.6").as_deref(),
            Some(beads.replacen("\"0.7.3\"", "\"0.7.6\"", 1).as_str())
        );
        // Spacing is kept; a longer version is not matched.
        assert_eq!(
            update_json_version(beads, "0.7.30", "0.8.0").as_deref(),
            Some(
                beads
                    .replace("\"version\" : \"0.7.30\"", "\"version\" : \"0.8.0\"")
                    .as_str()
            )
        );
        assert_eq!(update_json_version(beads, "0.7", "0.8"), None);
        let twice = "{\"version\": \"1.0\", \"x\": {\"version\": \"1.0\"}}";
        assert_eq!(update_json_version(twice, "1.0", "1.1"), None);

        let files = vec![
            (
                PathBuf::from("default.nix"),
                "inherit (data) cargoHash;".to_owned(),
            ),
            (
                PathBuf::from("hashes.json"),
                "{\"cargoHash\": \"sha256-old\"}".to_owned(),
            ),
        ];
        assert_eq!(
            plan_unique_literal(&files, "sha256-old", "sha256-new"),
            Some((
                PathBuf::from("hashes.json"),
                "{\"cargoHash\": \"sha256-new\"}".to_owned()
            ))
        );
        let mut twice = files.clone();
        twice[0].1 = "x = \"sha256-old\";".to_owned();
        assert_eq!(
            plan_unique_literal(&twice, "sha256-old", "sha256-new"),
            None
        );
        assert_eq!(
            plan_unique_literal(&files, "sha256-missing", "sha256-new"),
            None
        );
    }

    #[test]
    fn replace_unique_literal_cases() {
        let docker = r#"cliHash = "sha256-old"; mobyHash = "sha256-moby";"#;
        let cases = [
            (
                docker,
                "sha256-old",
                Some(r#"cliHash = "sha256-new"; mobyHash = "sha256-moby";"#),
            ),
            (docker, "sha256-missing", None),
            // Only whole string literals match.
            (r#"a = "sha256-old2";"#, "sha256-old", None),
            // Ambiguous.
            (r#"a = "sha256-old"; b = "sha256-old";"#, "sha256-old", None),
        ];
        // Shared by several systems: the build machine's line.
        let host = crate::commands::update::fod_platforms::native_system();
        let shared =
            format!("hashes = {{\n  zz-other = \"sha256-old\";\n  {host} = \"sha256-old\";\n}};\n");
        assert_eq!(
            replace_unique_literal(&shared, "sha256-old", "sha256-new"),
            Some(format!(
                "hashes = {{\n  zz-other = \"sha256-old\";\n  {host} = \"sha256-new\";\n}};\n"
            ))
        );
        for (content, old, expected) in cases {
            assert_eq!(
                replace_unique_literal(content, old, "sha256-new").as_deref(),
                expected,
                "{content}"
            );
        }
    }
}
