//! Update groups: packages that must be updated together.
//!
//! A package declares `passthru.ekapkgs-update.parent = "<attr path>";`. All
//! packages whose parent chains end at the same root form a group, which is
//! proposed as a single PR with one commit per member (in dependency order),
//! verified by building every member and the root.
//!
//! This module holds the pure parts: parent resolution, root resolution and
//! the dependency ordering of members.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Split an attr path into segments, keeping quoted segments
/// (`python3Packages."foo.bar"`) intact.
pub fn split_attr_path(attr: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    for (i, c) in attr.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '.' if !in_quotes => {
                segments.push(&attr[start..i]);
                start = i + 1;
            },
            _ => {},
        }
    }
    segments.push(&attr[start..]);
    segments
}

/// Resolve a declared parent to an attr path.
///
/// The parent is looked up relative to the child's scope first: child
/// `a.b.c` with parent `p` tries `a.b.p`, then `a.p`, then `p`, and the first
/// attr path in `known` wins. A parent that matches nothing is still used as
/// a group label (so its children are grouped together), at the closest
/// scope; `found` is false in that case so the caller can warn about it.
pub fn resolve_parent(child: &str, parent: &str, known: &HashSet<String>) -> ResolvedParent {
    let parent = parent.trim();
    let segments = split_attr_path(child);
    let scopes = segments.len().saturating_sub(1);
    let candidates: Vec<String> = (0..=scopes)
        .rev()
        .map(|depth| {
            if depth == 0 {
                parent.to_owned()
            } else {
                format!("{}.{}", segments[..depth].join("."), parent)
            }
        })
        .collect();

    match candidates.iter().find(|c| known.contains(*c)) {
        Some(found) => ResolvedParent {
            attr_path: found.clone(),
            found: true,
        },
        None => ResolvedParent {
            attr_path: candidates.into_iter().next().unwrap_or_default(),
            found: false,
        },
    }
}

/// Result of [`resolve_parent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedParent {
    pub attr_path: String,
    /// Whether the parent is a known attr path (otherwise it is a label).
    pub found: bool,
}

/// Order attr paths by length, then lexically: the canonical choice among
/// equivalent attr paths (e.g. the root of a parent cycle).
fn canonical_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Resolve every package's group root from its (resolved) parent.
///
/// `parents` maps a child attr path to its parent attr path. Chains are
/// followed to a package without a parent, which is the root. A cycle has no
/// such package; its canonical member (shortest, then lexically smallest) is
/// the root. The result maps every child and every root to the root, so a
/// root maps to itself.
pub fn resolve_roots(parents: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut roots: BTreeMap<String, String> = BTreeMap::new();

    for start in parents.keys() {
        if roots.contains_key(start) {
            continue;
        }
        // Walk the chain until a known root, a package without a parent, or
        // a cycle.
        let mut path: Vec<&str> = vec![start];
        let mut position: HashMap<&str, usize> = HashMap::from([(start.as_str(), 0)]);
        let root = loop {
            let current = *path.last().expect("path is never empty");
            if let Some(root) = roots.get(current) {
                break root.clone();
            }
            let Some(next) = parents.get(current) else {
                break current.to_owned();
            };
            if let Some(&index) = position.get(next.as_str()) {
                let cycle = &path[index..];
                break cycle
                    .iter()
                    .min_by(|a, b| canonical_cmp(a, b))
                    .map(|s| (*s).to_owned())
                    .expect("cycle is never empty");
            }
            position.insert(next, path.len());
            path.push(next);
        };
        for attr in path {
            roots.insert(attr.to_owned(), root.clone());
        }
        roots.insert(root.clone(), root);
    }

    roots
}

/// Order members so every member comes after the members it depends on.
///
/// `deps` maps a member to the members its derivation depends on (other
/// entries are ignored). Ties are broken alphabetically, and members caught
/// in a dependency cycle (impossible in a derivation graph, but `deps` may
/// be approximate) are appended alphabetically.
pub fn dependency_order(
    members: &[String],
    deps: &HashMap<String, HashSet<String>>,
) -> Vec<String> {
    let member_set: BTreeSet<&str> = members.iter().map(String::as_str).collect();
    let mut remaining: BTreeMap<&str, BTreeSet<&str>> = member_set
        .iter()
        .map(|&m| {
            let needs = deps
                .get(m)
                .map(|d| {
                    d.iter()
                        .map(String::as_str)
                        .filter(|d| *d != m && member_set.contains(d))
                        .collect()
                })
                .unwrap_or_default();
            (m, needs)
        })
        .collect();

    let mut order = Vec::with_capacity(member_set.len());
    loop {
        let ready: Vec<&str> = remaining
            .iter()
            .filter(|(_, needs)| needs.is_empty())
            .map(|(m, _)| *m)
            .collect();
        let Some(&next) = ready.first() else {
            break;
        };
        remaining.remove(next);
        for needs in remaining.values_mut() {
            needs.remove(next);
        }
        order.push(next.to_owned());
    }
    order.extend(remaining.into_keys().map(str::to_owned));
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(attrs: &[&str]) -> HashSet<String> {
        attrs.iter().map(|s| (*s).to_owned()).collect()
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn splits_attr_paths() {
        let cases: &[(&str, &[&str])] = &[
            ("hello", &["hello"]),
            ("python3Packages.requests", &["python3Packages", "requests"]),
            (
                "abseil-cpp.\"v20230802.1\"",
                &["abseil-cpp", "\"v20230802.1\""],
            ),
            ("a.\"b.c\".d", &["a", "\"b.c\"", "d"]),
        ];
        for (attr, expected) in cases {
            assert_eq!(split_attr_path(attr), *expected, "{attr}");
        }
    }

    #[test]
    fn resolves_parents_scope_first() {
        let attrs = known(&[
            "aws-crt-cpp",
            "python3Packages.boto",
            "boto",
            "python3Packages.sub.x",
            "a.\"b.c\".p",
        ]);
        let cases: &[(&str, &str, &str, bool)] = &[
            // Top-level child, top-level parent.
            ("aws-c-io", "aws-crt-cpp", "aws-crt-cpp", true),
            // The scoped parent wins over the top-level one.
            (
                "python3Packages.botocore",
                "boto",
                "python3Packages.boto",
                true,
            ),
            // Falls back to outer scopes.
            ("python3Packages.sub.y", "aws-crt-cpp", "aws-crt-cpp", true),
            // Dotted parents are looked up relative to scopes too.
            ("python3Packages.z", "sub.x", "python3Packages.sub.x", true),
            (
                "other.z",
                "python3Packages.boto",
                "python3Packages.boto",
                true,
            ),
            // Quoted segments keep their dots.
            ("a.\"b.c\".child", "p", "a.\"b.c\".p", true),
            // Unknown parents become labels at the closest scope.
            (
                "python3Packages.foo",
                "label",
                "python3Packages.label",
                false,
            ),
            ("foo", "label", "label", false),
            ("foo", " aws-crt-cpp ", "aws-crt-cpp", true),
        ];
        for (child, parent, expected, found) in cases {
            let resolved = resolve_parent(child, parent, &attrs);
            assert_eq!(resolved.attr_path, *expected, "{child} -> {parent}");
            assert_eq!(resolved.found, *found, "{child} -> {parent}");
        }
    }

    #[test]
    fn resolves_roots() {
        type Pairs<'a> = &'a [(&'a str, &'a str)];
        let cases: &[(Pairs<'_>, Pairs<'_>)] = &[
            // Star: every child points at the root.
            (
                &[("aws-c-io", "aws-crt-cpp"), ("aws-c-common", "aws-crt-cpp")],
                &[
                    ("aws-c-common", "aws-crt-cpp"),
                    ("aws-c-io", "aws-crt-cpp"),
                    ("aws-crt-cpp", "aws-crt-cpp"),
                ],
            ),
            // Chains are followed to the end.
            (
                &[("c", "b"), ("b", "a")],
                &[("a", "a"), ("b", "a"), ("c", "a")],
            ),
            // A cycle's root is its shortest, then smallest, member; a tail
            // into the cycle joins it.
            (
                &[("zz", "yyy"), ("yyy", "zz"), ("tail", "yyy")],
                &[("tail", "zz"), ("yyy", "zz"), ("zz", "zz")],
            ),
            // Self-parent.
            (&[("a", "a")], &[("a", "a")]),
            // Independent groups.
            (
                &[("x1", "x"), ("y1", "y")],
                &[("x", "x"), ("x1", "x"), ("y", "y"), ("y1", "y")],
            ),
        ];
        for (parents, expected) in cases {
            assert_eq!(resolve_roots(&map(parents)), map(expected), "{parents:?}");
        }
    }

    #[test]
    fn orders_members_by_dependencies() {
        let members: Vec<String> = ["aws-c-io", "aws-c-common", "aws-c-cal", "aws-crt-cpp"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let deps = |pairs: &[(&str, &[&str])]| -> HashMap<String, HashSet<String>> {
            pairs
                .iter()
                .map(|(m, d)| ((*m).to_owned(), d.iter().map(|s| (*s).to_owned()).collect()))
                .collect()
        };
        type Deps<'a> = &'a [(&'a str, &'a [&'a str])];
        let cases: &[(Deps<'_>, &[&str])] = &[
            // No information: alphabetical.
            (
                &[],
                &["aws-c-cal", "aws-c-common", "aws-c-io", "aws-crt-cpp"],
            ),
            // Real ordering, with non-members and self-references ignored.
            (
                &[
                    ("aws-c-cal", &["aws-c-common", "openssl"]),
                    ("aws-c-io", &["aws-c-cal", "aws-c-common", "aws-c-io"]),
                    ("aws-crt-cpp", &["aws-c-io", "aws-c-cal", "aws-c-common"]),
                ],
                &["aws-c-common", "aws-c-cal", "aws-c-io", "aws-crt-cpp"],
            ),
            // A cycle is appended alphabetically after the rest.
            (
                &[("aws-c-cal", &["aws-c-io"]), ("aws-c-io", &["aws-c-cal"])],
                &["aws-c-common", "aws-crt-cpp", "aws-c-cal", "aws-c-io"],
            ),
        ];
        for (d, expected) in cases {
            assert_eq!(dependency_order(&members, &deps(d)), *expected, "{d:?}");
        }
    }
}
