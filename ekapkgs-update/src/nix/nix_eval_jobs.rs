use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum NixEvalItem {
    Error(NixEvalError),
    Drv(Box<NixEvalDrv>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NixMeta {
    pub homepage: Option<String>,
    pub changelog: Option<String>,
    pub description: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NixEvalDrv {
    /// String of full attr path
    /// E.g. "python.pkgs.setuptools"
    pub attr: String,

    /// List of attrs to access drv.
    /// "python.pkgs.setuptools" -> [ "python" "pkgs" "setuptools" ]
    #[serde(rename = "attrPath")]
    pub attr_path: Vec<String>,

    /// Store path to drv. E.g. "/nix/store/<hash>-<name>.drv"
    #[serde(rename = "drvPath")]
    pub drv_path: String,

    /// Direct references/dependencies of this drv
    #[serde(rename = "inputDrvs")]
    pub input_drvs: Option<HashMap<String, Vec<String>>>,

    /// Name of drv. Usually includes "${pname}-${version}", but doesn't need to
    pub name: String,

    /// A mapping of the multiple outputs and their respective nix store paths
    pub outputs: HashMap<String, String>,

    /// Build platform system
    pub system: String,

    /// Meta information about the package
    pub meta: Option<NixMeta>,

    /// Result of the `--apply` function for this derivation, when one was given.
    #[serde(
        rename = "extraValue",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub extra_value: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NixEvalError {
    pub attr: String,
    #[serde(rename = "attrPath")]
    pub attr_path: Vec<String>,
    pub error: String,
}

impl NixEvalError {
    /// Whether this error is from a top-level function attribute (not a real failure).
    pub fn is_function_error(&self) -> bool {
        self.error
            .contains("cannot evaluate a function that has an argument without a value")
    }
}

impl NixEvalItem {
    /// Reinterpret dotted attribute names as attribute paths.
    ///
    /// `--select` is typically used to pick a flat attribute set whose keys are
    /// dotted package paths (e.g. corepkgs' `buildable` contains `"cmake.v4"`).
    /// nix-eval-jobs reports such a key as a single quoted component, which
    /// cannot be spliced into `with import <file> { }; <attr>` expressions.
    /// This splits those keys into real components and re-renders `attr`.
    pub fn flatten_dotted_attrs(&mut self) {
        let (attr, attr_path) = match self {
            NixEvalItem::Error(e) => (&mut e.attr, &mut e.attr_path),
            NixEvalItem::Drv(d) => (&mut d.attr, &mut d.attr_path),
        };
        *attr_path = split_dotted_components(attr_path);
        *attr = render_attr_path(attr_path);
    }
}

/// Split each component on `.` (e.g. `["cmake.v4"]` -> `["cmake", "v4"]`).
///
/// Components that would produce an empty segment (`"a..b"`, `".a"`) are kept
/// verbatim, since they cannot be valid dotted paths.
pub fn split_dotted_components(components: &[String]) -> Vec<String> {
    components
        .iter()
        .flat_map(|component| {
            let parts: Vec<&str> = component.split('.').collect();
            if parts.iter().any(|p| p.is_empty()) {
                vec![component.clone()]
            } else {
                parts.into_iter().map(str::to_owned).collect()
            }
        })
        .collect()
}

/// Render attribute path components as a Nix attribute path.
///
/// Components that are plain Nix identifiers are emitted bare; anything else
/// (leading digit, dots, keywords, ...) is double-quoted, mirroring how
/// nix-eval-jobs renders its `attr` field.
pub fn render_attr_path(components: &[String]) -> String {
    components
        .iter()
        .map(|c| {
            if is_nix_identifier(c) {
                c.clone()
            } else {
                let escaped = c
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace("${", "\\${");
                format!("\"{escaped}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Whether `s` can appear unquoted as an attribute name in a Nix select path.
fn is_nix_identifier(s: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "assert", "else", "if", "in", "inherit", "let", "or", "rec", "then", "with",
    ];
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '\'' | '-'))
        && !KEYWORDS.contains(&s)
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn test_deserialization() -> anyhow::Result<()> {
        // Generated with:
        // `nix-eval-jobs --expr 'with import ./. {}; { inherit grpc; }'`
        // while in nixpkgs directory
        let eval_drv = r#"{"attr":"cmake","attrPath":["cmake"],"drvPath":"/nix/store/3fr8b3xlygv2a64ff7fq7564j4sxv4lc-cmake-3.29.6.drv","inputDrvs":{"/nix/store/08s4j5nvddsbrjpachqwzai83xngxnc0-pkg-config-wrapper-0.29.2.drv":["out"],"/nix/store/0cgbdlz63qiqf5f8i1sljak1dfbzyrl5-openssl-3.0.14.drv":["dev"],"/nix/store/265x0i426vnqjma9khcfpi86m6hx4smr-bash-5.2p32.drv":["out"],"/nix/store/27zlixdsk0kx585j4dcjm53636mx7cis-libuv-1.48.0.drv":["dev"],"/nix/store/2vyizsckka60lhh0kylhbpdd1flb998v-cmake-3.29.6.tar.gz.drv":["out"],"/nix/store/4hzjv6r5v7h6hzad718jgc0hrm1gz8r1-gcc-wrapper-13.3.0.drv":["out"],"/nix/store/860zddz386bk0441flrg940ipbp0jp1z-xz-5.6.2.drv":["dev"],"/nix/store/9jvlq6qg9j1222w3zm3wgfv5qyqfqmxz-bzip2-1.0.8.drv":["dev"],"/nix/store/ax4q30iyf9wi95hswil021lg0cdqq6rl-libarchive-3.7.4.drv":["dev"],"/nix/store/bxq3kjf71wn92yisdbq18fzpvcl5pn31-expat-2.6.2.drv":["dev"],"/nix/store/kh6mps96srqgdvn03vq4gmqzl51s9w8h-glibc-2.39-52.drv":["bin","dev","out"],"/nix/store/lzc503qcc7f6ibq8sdbcri73wb62dj4r-zlib-1.3.1.drv":["dev"],"/nix/store/mzw7jzs6ix17ajh3z4kqzvh8l7abj4yr-rhash-1.4.4.drv":["out"],"/nix/store/v288gxsg679gyi9zpg0mhrv26vfmw4kr-stdenv-linux.drv":["out"],"/nix/store/vnq47hr4nwry8kgvfgmx0229id3q49dr-binutils-2.42.drv":["out"],"/nix/store/y99v9h2mcqbw91g7p3lnk292k0np0djr-curl-8.9.0.drv":["dev"]},"name":"cmake-3.29.6","outputs":{"debug":"/nix/store/xrh9g28kmsyjlw6qf46ngkvhac1llgvz-cmake-3.29.6-debug","out":"/nix/store/rz7j0kdkq8j522vpw6n8wjq2qv3if24g-cmake-3.29.6"},"system":"x86_64-linux"}"#;

        let _drv: NixEvalDrv = serde_json::from_str(eval_drv)
            .context("nix-eval-jobs sample drv fixture must parse as NixEvalDrv")?;
        Ok(())
    }

    #[test]
    fn test_error() -> anyhow::Result<()> {
        let err = r##"{"attr":"adoptopenjdk-openj9-bin-15","attrPath":["adoptopenjdk-openj9-bin-15"],"error":"error:\n       … from call site\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:217:7:\n          216|     lib.mapAttrs (\n          217|       n: alias: removeDistribute (removeRecurseForDerivations (checkInPkgs n alias))\n             |       ^\n          218|     ) aliases;\n\n       … while calling anonymous lambda\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:217:10:\n          216|     lib.mapAttrs (\n          217|       n: alias: removeDistribute (removeRecurseForDerivations (checkInPkgs n alias))\n             |          ^\n          218|     ) aliases;\n\n       … from call site\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:217:17:\n          216|     lib.mapAttrs (\n          217|       n: alias: removeDistribute (removeRecurseForDerivations (checkInPkgs n alias))\n             |                 ^\n          218|     ) aliases;\n\n       … while calling 'removeDistribute'\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:34:22:\n           33|   # sets from building on Hydra.\n           34|   removeDistribute = alias: if lib.isDerivation alias then lib.dontDistribute alias else alias;\n             |                      ^\n           35|\n\n       … while evaluating a branch condition\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:34:29:\n           33|   # sets from building on Hydra.\n           34|   removeDistribute = alias: if lib.isDerivation alias then lib.dontDistribute alias else alias;\n             |                             ^\n           35|\n\n       … from call site\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:34:32:\n           33|   # sets from building on Hydra.\n           34|   removeDistribute = alias: if lib.isDerivation alias then lib.dontDistribute alias else alias;\n             |                                ^\n           35|\n\n       … while calling 'isDerivation'\n         at /home/jon/projects/nixpkgs/lib/attrsets.nix:1251:18:\n         1250|   */\n         1251|   isDerivation = value: value.type or null == \"derivation\";\n             |                  ^\n         1252|\n\n       … from call site\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:217:35:\n          216|     lib.mapAttrs (\n          217|       n: alias: removeDistribute (removeRecurseForDerivations (checkInPkgs n alias))\n             |                                   ^\n          218|     ) aliases;\n\n       … while calling 'removeRecurseForDerivations'\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:26:5:\n           25|   removeRecurseForDerivations =\n           26|     alias:\n             |     ^\n           27|     if alias.recurseForDerivations or false then\n\n       … while evaluating a branch condition\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:27:5:\n           26|     alias:\n           27|     if alias.recurseForDerivations or false then\n             |     ^\n           28|       lib.removeAttrs alias [ \"recurseForDerivations\" ]\n\n       … from call site\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:217:64:\n          216|     lib.mapAttrs (\n          217|       n: alias: removeDistribute (removeRecurseForDerivations (checkInPkgs n alias))\n             |                                                                ^\n          218|     ) aliases;\n\n       … while calling 'checkInPkgs'\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:211:8:\n          210|   checkInPkgs =\n          211|     n: alias:\n             |        ^\n          212|     if builtins.hasAttr n super then throw \"Alias ${n} is still in all-packages.nix\" else alias;\n\n       … while calling the 'throw' builtin\n         at /home/jon/projects/nixpkgs/pkgs/top-level/aliases.nix:257:32:\n          256|   adoptopenjdk-openj9-bin-11 = throw \"adoptopenjdk has been removed as the upstream project is deprecated. Consider using `semeru-bin-11`.\"; # Added 2024-05-09\n          257|   adoptopenjdk-openj9-bin-15 = throw \"adoptopenjdk has been removed as the upstream project is deprecated. JDK 15 is also EOL. Consider using `semeru-bin-17`.\"; # Added 2024-05-09\n             |                                ^\n          258|   adoptopenjdk-openj9-bin-16 = throw \"adoptopenjdk has been removed as the upstream project is deprecated. JDK 16 is also EOL. Consider using `semeru-bin-17`.\"; # Added 2024-05-09\n\n       error: adoptopenjdk has been removed as the upstream project is deprecated. JDK 15 is also EOL. Consider using `semeru-bin-17`."}"##;
        let _item: NixEvalItem = serde_json::from_str(err)
            .context("nix-eval-jobs sample error fixture must parse as NixEvalItem")?;
        Ok(())
    }

    #[test]
    fn test_function_error_detection() {
        let function_error = NixEvalError {
            attr: "myHelper".to_owned(),
            attr_path: vec!["myHelper".to_owned()],
            error: "error: cannot evaluate a function that has an argument without a value \
                    ('name')"
                .to_owned(),
        };
        assert!(function_error.is_function_error());

        let real_error = NixEvalError {
            attr: "brokenPkg".to_owned(),
            attr_path: vec!["brokenPkg".to_owned()],
            error: "error: attribute 'src' missing".to_owned(),
        };
        assert!(!real_error.is_function_error());
    }

    #[test]
    fn test_render_attr_path() {
        let cases: &[(&[&str], &str)] = &[
            (&["cmake"], "cmake"),
            (
                &["python3", "v3_12", "pkgs", "requests"],
                "python3.v3_12.pkgs.requests",
            ),
            (&["389-ds-base"], "\"389-ds-base\""),
            (&["cmake.v4"], "\"cmake.v4\""),
            (&["foo'", "bar-baz"], "foo'.bar-baz"),
            (&["with"], "\"with\""),
            (&["a\"b${c}"], "\"a\\\"b\\${c}\""),
        ];
        for (components, expected) in cases {
            let owned: Vec<String> = components.iter().map(|s| (*s).to_owned()).collect();
            assert_eq!(render_attr_path(&owned), *expected, "{components:?}");
        }
    }

    #[test]
    fn test_split_dotted_components() {
        let cases: &[(&[&str], &[&str])] = &[
            (&["cmake.v4"], &["cmake", "v4"]),
            (&["zlib"], &["zlib"]),
            (&["linux", "pkgs.zfs"], &["linux", "pkgs", "zfs"]),
            (&["a..b"], &["a..b"]),
            (&[".hidden"], &[".hidden"]),
        ];
        for (input, expected) in cases {
            let owned: Vec<String> = input.iter().map(|s| (*s).to_owned()).collect();
            assert_eq!(split_dotted_components(&owned), *expected, "{input:?}");
        }
    }

    #[test]
    fn test_flatten_dotted_attrs_on_select_output() -> anyhow::Result<()> {
        // Shape produced by `nix-eval-jobs --select 'x: x.buildable'` for a
        // variant key in corepkgs' flat `buildable` set.
        let line = r#"{"attr":"\"abseil-cpp.v20230802\"","attrPath":["abseil-cpp.v20230802"],"drvPath":"/nix/store/aaaa-abseil-cpp-20230802.1.drv","name":"abseil-cpp-20230802.1","outputs":{"out":"/nix/store/bbbb-abseil-cpp-20230802.1"},"system":"x86_64-linux","extraValue":{"version":"20230802.1"}}"#;
        let mut item: NixEvalItem =
            serde_json::from_str(line).context("select output line must parse")?;
        item.flatten_dotted_attrs();
        let NixEvalItem::Drv(drv) = item else {
            anyhow::bail!("expected a derivation");
        };
        assert_eq!(drv.attr, "abseil-cpp.v20230802");
        assert_eq!(drv.attr_path, vec!["abseil-cpp", "v20230802"]);
        assert_eq!(
            drv.extra_value
                .as_ref()
                .and_then(|v| v.get("version"))
                .and_then(serde_json::Value::as_str),
            Some("20230802.1")
        );
        Ok(())
    }
}
