//! Matching a package's `fetchPypi` filename against the sdists PyPI lists.
//!
//! Many projects have switched their sdist filenames to the normalised form
//! (PEP 625): `sphinxcontrib-openapi` 0.10.0 ships
//! `sphinxcontrib_openapi-0.10.0.tar.gz`, while older releases were named
//! `sphinxcontrib-openapi-0.8.4.tar.gz`. `fetchPypi` builds its URL from
//! `pname`, so bumping only `version` produces a URL that 404s.
//!
//! These helpers are pure so they can be tested without network access; the
//! update workflow (`commands::update::hash_workflows`) supplies the evaluated
//! `src` filename and PyPI's file list.

/// Sdist archive extensions, longest first so `tar.gz` wins over `gz`.
const SDIST_EXTENSIONS: &[&str] = &["tar.gz", "tar.bz2", "tar.xz", "tgz", "zip"];

/// What to do about a package's sdist filename after a version bump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdistFix {
    /// The current filename is one of the release's sdists.
    Matches,
    /// The release has a sdist under a different spelling of the same name
    /// (or with a different extension). Only the parts that differ are set.
    Rewrite {
        /// New `pname` for `fetchPypi`, when the name part differs.
        pname: Option<String>,
        /// New `extension` for `fetchPypi`, when the extension differs.
        extension: Option<String>,
        /// The sdist filename the rewrite should produce.
        filename: String,
    },
    /// The release publishes only wheels.
    NoSdist,
    /// The release has sdists, but none is a spelling of the current name.
    NoMatch,
}

/// Normalise a distribution name as PEP 503 does: lowercase, and every run of
/// `-`, `_` and `.` becomes a single `-`.
pub fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_separator = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !in_separator {
                out.push('-');
            }
            in_separator = true;
        } else {
            out.extend(c.to_lowercase());
            in_separator = false;
        }
    }
    out
}

/// Split `<name>-<version>.<ext>` into `(name, ext)`.
///
/// Returns `None` unless the filename ends in `-<version>` followed by a known
/// sdist extension.
pub fn split_sdist_filename<'a>(filename: &'a str, version: &str) -> Option<(&'a str, &'a str)> {
    SDIST_EXTENSIONS.iter().find_map(|ext| {
        let stem = filename.strip_suffix(ext)?.strip_suffix('.')?;
        let name = stem.strip_suffix(version)?.strip_suffix('-')?;
        if name.is_empty() {
            return None;
        }
        Some((name, &filename[filename.len() - ext.len()..]))
    })
}

/// Decide how the `src` filename `current` must change to match one of
/// `sdists`, the sdist filenames PyPI lists for `version`.
pub fn plan_sdist_fix(current: &str, version: &str, sdists: &[&str]) -> SdistFix {
    if sdists.is_empty() {
        return SdistFix::NoSdist;
    }
    if sdists.contains(&current) {
        return SdistFix::Matches;
    }
    let Some((current_name, current_ext)) = split_sdist_filename(current, version) else {
        return SdistFix::NoMatch;
    };
    let wanted = normalize_name(current_name);

    sdists
        .iter()
        .filter_map(|sdist| Some((*sdist, split_sdist_filename(sdist, version)?)))
        .find(|(_, (name, _))| normalize_name(name) == wanted)
        .map_or(SdistFix::NoMatch, |(sdist, (name, ext))| {
            SdistFix::Rewrite {
                pname: (name != current_name).then(|| name.to_owned()),
                extension: (ext != current_ext).then(|| ext.to_owned()),
                filename: sdist.to_owned(),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_name() {
        let cases = [
            ("requests", "requests"),
            ("Django", "django"),
            ("sphinxcontrib_openapi", "sphinxcontrib-openapi"),
            ("wheezy.template", "wheezy-template"),
            ("XStatic-jQuery", "xstatic-jquery"),
            ("web.py", "web-py"),
            ("a__-.b", "a-b"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_name(input), expected, "input: {input}");
        }
    }

    #[test]
    fn test_split_sdist_filename() {
        let cases = [
            (
                "requests-2.28.1.tar.gz",
                "2.28.1",
                Some(("requests", "tar.gz")),
            ),
            (
                "sphinxcontrib_openapi-0.10.0.tar.gz",
                "0.10.0",
                Some(("sphinxcontrib_openapi", "tar.gz")),
            ),
            (
                "ufonormalizer-0.6.2.zip",
                "0.6.2",
                Some(("ufonormalizer", "zip")),
            ),
            ("foo-bar-1.0.tgz", "1.0", Some(("foo-bar", "tgz"))),
            ("foo-1.0.tar.bz2", "1.0", Some(("foo", "tar.bz2"))),
            // Wrong version, wheel, missing name, unknown extension
            ("requests-2.28.1.tar.gz", "2.28.2", None),
            ("requests-2.28.1-py3-none-any.whl", "2.28.1", None),
            ("-1.0.tar.gz", "1.0", None),
            ("foo-1.0.rar", "1.0", None),
            // A version that is a suffix of another must not match
            ("foo-11.0.tar.gz", "1.0", None),
        ];
        for (filename, version, expected) in cases {
            assert_eq!(
                split_sdist_filename(filename, version),
                expected,
                "filename: {filename}, version: {version}"
            );
        }
    }

    fn rewrite(pname: Option<&str>, extension: Option<&str>, filename: &str) -> SdistFix {
        SdistFix::Rewrite {
            pname: pname.map(str::to_owned),
            extension: extension.map(str::to_owned),
            filename: filename.to_owned(),
        }
    }

    #[test]
    fn test_plan_sdist_fix() {
        let cases: &[(&str, &str, &[&str], SdistFix)] = &[
            (
                "requests-2.32.0.tar.gz",
                "2.32.0",
                &["requests-2.32.0.tar.gz"],
                SdistFix::Matches,
            ),
            // Hyphens became underscores
            (
                "sphinxcontrib-openapi-0.10.0.tar.gz",
                "0.10.0",
                &["sphinxcontrib_openapi-0.10.0.tar.gz"],
                rewrite(
                    Some("sphinxcontrib_openapi"),
                    None,
                    "sphinxcontrib_openapi-0.10.0.tar.gz",
                ),
            ),
            // Case and separators changed
            (
                "XStatic-jquery-ui-1.13.0.2.tar.gz",
                "1.13.0.2",
                &["xstatic_jquery_ui-1.13.0.2.tar.gz"],
                rewrite(
                    Some("xstatic_jquery_ui"),
                    None,
                    "xstatic_jquery_ui-1.13.0.2.tar.gz",
                ),
            ),
            // Dots in the name
            (
                "web.py-0.76.tar.gz",
                "0.76",
                &["web_py-0.76.tar.gz"],
                rewrite(Some("web_py"), None, "web_py-0.76.tar.gz"),
            ),
            // Only the extension changed
            (
                "ufonormalizer-0.6.3.zip",
                "0.6.3",
                &["ufonormalizer-0.6.3.tar.gz"],
                rewrite(None, Some("tar.gz"), "ufonormalizer-0.6.3.tar.gz"),
            ),
            // Both changed
            (
                "Foo-Bar-2.0.zip",
                "2.0",
                &["foo_bar-2.0.tar.gz"],
                rewrite(Some("foo_bar"), Some("tar.gz"), "foo_bar-2.0.tar.gz"),
            ),
            // Wheel-only release
            (
                "yoyo-migrations-9.0.0.tar.gz",
                "9.0.0",
                &[],
                SdistFix::NoSdist,
            ),
            // A sdist exists but is for a different name
            (
                "foo-1.0.tar.gz",
                "1.0",
                &["unrelated-1.0.tar.gz"],
                SdistFix::NoMatch,
            ),
            // The current URL is not a sdist filename for this version
            (
                "foo-1.0-py3-none-any.whl",
                "1.0",
                &["foo-1.0.tar.gz"],
                SdistFix::NoMatch,
            ),
        ];
        for (current, version, sdists, expected) in cases {
            assert_eq!(
                &plan_sdist_fix(current, version, sdists),
                expected,
                "current: {current}"
            );
        }
    }
}
