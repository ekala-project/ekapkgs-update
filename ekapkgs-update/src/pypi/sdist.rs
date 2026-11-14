//! Matching a package's `fetchPypi` filename against the sdists PyPI lists.
//!
//! Many projects have switched their sdist filenames to the normalised form
//! (PEP 625): `sphinxcontrib-openapi` 0.10.0 ships
//! `sphinxcontrib_openapi-0.10.0.tar.gz`, while older releases were named
//! `sphinxcontrib-openapi-0.8.4.tar.gz`. `fetchPypi` builds its URL from
//! `pname`, so bumping only `version` produces a URL that 404s.
//!
//! Wheel sources break the same way when a project changes its wheel's python
//! tag (`cloudsmith_api-2.0.43-py2.py3-none-any.whl` → `…-py3-none-any.whl`):
//! `fetchPypi`'s `python` and `dist` must follow.
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

/// What to do about a package's wheel `src` after a version bump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WheelFix {
    /// The current filename and directory are one of the release's wheels.
    Matches,
    /// The release has a wheel for the same name and ABI/platform under a
    /// different name spelling, python tag or directory. Only the parts that
    /// differ are set.
    Rewrite {
        /// New `pname` for `fetchPypi`, when the name part differs.
        pname: Option<String>,
        /// New `python` tag, when it differs.
        python: Option<String>,
        /// New `dist` directory, when it differs.
        dist: Option<String>,
        /// The wheel filename the rewrite should produce.
        filename: String,
    },
    /// No wheel of the release has the same name and ABI/platform tags.
    NoMatch,
}

/// A wheel PyPI lists for a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wheel<'a> {
    pub filename: &'a str,
    /// Directory `fetchPypi` requests it from (PyPI's `python_version`).
    pub dist: &'a str,
}

/// Split `<name>-<version>-<python>-<abi>-<platform>.whl` into
/// `(name, python, "<abi>-<platform>")`.
///
/// Wheels with a build tag (`<name>-<version>-<build>-…`) are not recognised:
/// `fetchPypi` cannot request them.
pub fn split_wheel_filename<'a>(
    filename: &'a str,
    version: &str,
) -> Option<(&'a str, &'a str, &'a str)> {
    let stem = filename.strip_suffix(".whl")?;
    let (name, rest) = stem.split_once(&format!("-{version}-"))?;
    let (python, abi_platform) = rest.split_once('-')?;
    let well_formed = !name.is_empty()
        && !python.is_empty()
        && abi_platform.split('-').count() == 2
        && abi_platform.split('-').all(|part| !part.is_empty());
    well_formed.then_some((name, python, abi_platform))
}

/// Directory segment of a wheel URL
/// (`…/packages/<dist>/<letter>/<pname>/<filename>`).
pub fn wheel_url_dist(url: &str) -> Option<&str> {
    let mut segments = url.rsplit('/');
    let _filename = segments.next()?;
    let _pname = segments.next()?;
    let _letter = segments.next()?;
    segments.next().filter(|dist| !dist.is_empty())
}

/// Decide how the wheel `src` (`current` requested from `current_dist`) must
/// change to match one of `wheels`, the wheels PyPI lists for `version`.
///
/// Candidates must have the same name (PEP 503 normalised) and the same
/// ABI/platform tags; among them, one with the current python tag wins.
pub fn plan_wheel_fix(
    current: &str,
    current_dist: &str,
    version: &str,
    wheels: &[Wheel<'_>],
) -> WheelFix {
    if wheels
        .iter()
        .any(|w| w.filename == current && w.dist == current_dist)
    {
        return WheelFix::Matches;
    }
    let Some((current_name, current_python, current_abi_platform)) =
        split_wheel_filename(current, version)
    else {
        return WheelFix::NoMatch;
    };
    let wanted = normalize_name(current_name);

    let candidates: Vec<(Wheel<'_>, &str, &str)> = wheels
        .iter()
        .filter_map(|w| {
            let (name, python, abi_platform) = split_wheel_filename(w.filename, version)?;
            (abi_platform == current_abi_platform && normalize_name(name) == wanted)
                .then_some((*w, name, python))
        })
        .collect();
    let Some(&(wheel, name, python)) = candidates
        .iter()
        .find(|(_, _, python)| *python == current_python)
        .or_else(|| candidates.first())
    else {
        return WheelFix::NoMatch;
    };

    WheelFix::Rewrite {
        pname: (name != current_name).then(|| name.to_owned()),
        python: (python != current_python).then(|| python.to_owned()),
        dist: (wheel.dist != current_dist).then(|| wheel.dist.to_owned()),
        filename: wheel.filename.to_owned(),
    }
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

    #[test]
    fn test_split_wheel_filename() {
        let cases = [
            (
                "cloudsmith_api-2.0.43-py3-none-any.whl",
                "2.0.43",
                Some(("cloudsmith_api", "py3", "none-any")),
            ),
            (
                "foo-1.0-py2.py3-none-any.whl",
                "1.0",
                Some(("foo", "py2.py3", "none-any")),
            ),
            (
                "numpy-2.1.0-cp312-cp312-manylinux_2_17_x86_64.whl",
                "2.1.0",
                Some(("numpy", "cp312", "cp312-manylinux_2_17_x86_64")),
            ),
            // Build tag, wrong version, sdist, missing parts
            ("foo-1.0-1-py3-none-any.whl", "1.0", None),
            ("foo-1.0-py3-none-any.whl", "1.1", None),
            ("foo-1.0.tar.gz", "1.0", None),
            ("foo-1.0-py3-none.whl", "1.0", None),
            ("-1.0-py3-none-any.whl", "1.0", None),
        ];
        for (filename, version, expected) in cases {
            assert_eq!(
                split_wheel_filename(filename, version),
                expected,
                "filename: {filename}"
            );
        }
    }

    #[test]
    fn test_wheel_url_dist() {
        let cases = [
            (
                "https://files.pythonhosted.org/packages/py2.py3/c/cloudsmith_api/cloudsmith_api-2.0.43-py2.py3-none-any.whl",
                Some("py2.py3"),
            ),
            (
                "https://files.pythonhosted.org/packages/py3/f/foo/foo-1.0-py3-none-any.whl",
                Some("py3"),
            ),
            ("foo-1.0-py3-none-any.whl", None),
            ("c/foo/foo-1.0-py3-none-any.whl", None),
        ];
        for (url, expected) in cases {
            assert_eq!(wheel_url_dist(url), expected, "url: {url}");
        }
    }

    fn wheel_rewrite(
        pname: Option<&str>,
        python: Option<&str>,
        dist: Option<&str>,
        filename: &str,
    ) -> WheelFix {
        WheelFix::Rewrite {
            pname: pname.map(str::to_owned),
            python: python.map(str::to_owned),
            dist: dist.map(str::to_owned),
            filename: filename.to_owned(),
        }
    }

    #[test]
    fn test_plan_wheel_fix() {
        let w = |filename, dist| Wheel { filename, dist };
        let cases: &[(&str, &str, &str, &[Wheel<'_>], WheelFix)] = &[
            (
                "foo-1.0-py3-none-any.whl",
                "py3",
                "1.0",
                &[w("foo-1.0-py3-none-any.whl", "py3")],
                WheelFix::Matches,
            ),
            // python tag (and so the directory) changed
            (
                "cloudsmith_api-2.0.43-py2.py3-none-any.whl",
                "py2.py3",
                "2.0.43",
                &[w("cloudsmith_api-2.0.43-py3-none-any.whl", "py3")],
                wheel_rewrite(
                    None,
                    Some("py3"),
                    Some("py3"),
                    "cloudsmith_api-2.0.43-py3-none-any.whl",
                ),
            ),
            // name spelling changed
            (
                "Foo.Bar-2.0-py3-none-any.whl",
                "py3",
                "2.0",
                &[w("foo_bar-2.0-py3-none-any.whl", "py3")],
                wheel_rewrite(Some("foo_bar"), None, None, "foo_bar-2.0-py3-none-any.whl"),
            ),
            // only the directory is wrong
            (
                "foo-1.0-py3-none-any.whl",
                "py2.py3",
                "1.0",
                &[w("foo-1.0-py3-none-any.whl", "py3")],
                wheel_rewrite(None, None, Some("py3"), "foo-1.0-py3-none-any.whl"),
            ),
            // sdist-only release
            (
                "foo-1.1-py2.py3-none-any.whl",
                "py2.py3",
                "1.1",
                &[],
                WheelFix::NoMatch,
            ),
            // the current python tag is preferred among candidates
            (
                "foo-1.1-py3-none-any.whl",
                "py2.py3",
                "1.1",
                &[
                    w("foo-1.1-py2-none-any.whl", "py2"),
                    w("foo-1.1-py3-none-any.whl", "py3"),
                ],
                wheel_rewrite(None, None, Some("py3"), "foo-1.1-py3-none-any.whl"),
            ),
            // different platform only
            (
                "foo-1.0-py3-none-any.whl",
                "py3",
                "1.0",
                &[w("foo-1.0-cp312-cp312-manylinux_2_17_x86_64.whl", "cp312")],
                WheelFix::NoMatch,
            ),
            // different project
            (
                "foo-1.0-py2.py3-none-any.whl",
                "py2.py3",
                "1.0",
                &[w("bar-1.0-py3-none-any.whl", "py3")],
                WheelFix::NoMatch,
            ),
        ];
        for (current, dist, version, wheels, expected) in cases {
            assert_eq!(
                &plan_wheel_fix(current, dist, version, wheels),
                expected,
                "current: {current}"
            );
        }
    }
}
