//! Turning update requests into jobs: requests for members of an update
//! group become one [`GroupRequest`] per group (see [`crate::groups`]), the
//! rest stay single updates.
//!
//! Shared by `run` (after the checker finished, see [`RunGroups`]) and
//! `watch` (per poll cycle).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use tracing::{info, warn};

use super::types::{GroupRequest, UpdateJob, UpdateRequest};
use crate::database::Database;
use crate::groups::{resolve_parent, resolve_roots};

/// Group `requests` (all in `repo`) by their group root in `roots`
/// (attr → root, as stored by [`Database::replace_groups`]).
///
/// A group's job includes every member with a newer known upstream release
/// (from the database), not just the requested ones, so a regenerated group
/// PR keeps the members it proposed before. Requested versions win over the
/// database's. Every tracked member and the root are verified.
pub async fn group_jobs(
    db: &Database,
    repo: &str,
    requests: Vec<UpdateRequest>,
    roots: &BTreeMap<String, String>,
) -> Vec<UpdateJob> {
    let mut jobs = Vec::new();
    let mut grouped: BTreeMap<&str, Vec<UpdateRequest>> = BTreeMap::new();
    for req in requests {
        match roots.get(&req.attr_path) {
            Some(root) => grouped.entry(root.as_str()).or_default().push(req),
            None => jobs.push(UpdateJob::Single(req)),
        }
    }

    for (root, triggers) in grouped {
        let members = match db.group_members(repo, root).await {
            Ok(members) => members,
            Err(e) => {
                warn!(
                    "group {}: failed to load members ({e:#}); updating alone",
                    root
                );
                jobs.extend(triggers.into_iter().map(UpdateJob::Single));
                continue;
            },
        };
        let template = triggers[0].clone();
        let mut by_attr: HashMap<String, UpdateRequest> = triggers
            .into_iter()
            .map(|r| (r.attr_path.clone(), r))
            .collect();

        let mut group_members = Vec::new();
        let mut verify = Vec::new();
        for member in &members {
            verify.push(member.attr_path.clone());
            if let Some(req) = by_attr.remove(&member.attr_path) {
                group_members.push(req);
            } else if let Some((current, latest)) = member.pending_update() {
                group_members.push(UpdateRequest {
                    attr_path: member.attr_path.clone(),
                    drv_path: None,
                    current_version: current.to_owned(),
                    new_version: latest.to_owned(),
                    ..template.clone()
                });
            }
        }
        // Requested members the database does not list (not tracked yet).
        for (attr, req) in by_attr {
            verify.push(attr);
            group_members.push(req);
        }
        if !verify.iter().any(|a| a == root) {
            verify.push(root.to_owned());
        }
        group_members.sort_by(|a, b| a.attr_path.cmp(&b.attr_path));

        info!(
            "group {}: {} member(s) to update ({}), {} to verify",
            root,
            group_members.len(),
            group_members
                .iter()
                .map(|m| format!("{} {} -> {}", m.attr_path, m.current_version, m.new_version))
                .collect::<Vec<_>>()
                .join(", "),
            verify.len()
        );
        jobs.push(UpdateJob::Group(GroupRequest {
            root: root.to_owned(),
            members: group_members,
            verify,
        }));
    }

    jobs.sort_by_key(UpdateJob::label);
    jobs
}

/// Resolve raw `passthru.ekapkgs-update.parent` declarations against the
/// known attr paths, warning about parents that match no package (they
/// still group their children, as a label).
pub fn resolve_declarations<'a>(
    declared: impl IntoIterator<Item = (&'a str, &'a str)>,
    known: &HashSet<String>,
    repo_label: &str,
) -> BTreeMap<String, String> {
    declared
        .into_iter()
        .map(|(child, parent)| {
            let resolved = resolve_parent(child, parent, known);
            if !resolved.found {
                warn!(
                    "[{}] {}: update-group parent '{}' is not a package; grouping by label '{}'",
                    repo_label, child, parent, resolved.attr_path
                );
            }
            (child.to_owned(), resolved.attr_path)
        })
        .collect()
}

/// Update-group bookkeeping for one `run`.
///
/// The checker only evaluates packages that are out of backoff, so group
/// declarations are the previous run's (from the database) updated with the
/// ones evaluated now. Requests for packages that are (or may be) in a group
/// are held back until the checker finished, then dispatched per group;
/// everything else is dispatched immediately.
#[derive(Default)]
pub(super) struct RunGroups {
    /// Every attr path the enumeration produced.
    known: Mutex<HashSet<String>>,
    /// Declarations evaluated by this run: attr → raw parent (`None`: the
    /// package declares no parent any more).
    declared: Mutex<HashMap<String, Option<String>>>,
    /// Group roots stored by previous runs (attr → root).
    previous_roots: BTreeMap<String, String>,
    /// Declared parents stored by previous runs (child → parent).
    previous_parents: BTreeMap<String, String>,
}

impl RunGroups {
    pub(super) async fn load(db: &Database, repo: &str) -> Self {
        let previous_roots = db.group_roots(repo).await.unwrap_or_else(|e| {
            warn!("Failed to load update groups: {e:#}");
            BTreeMap::new()
        });
        let previous_parents = db.group_parents(repo).await.unwrap_or_default();
        Self {
            previous_roots,
            previous_parents,
            ..Self::default()
        }
    }

    pub(super) fn note_known(&self, attr: &str) {
        lock(&self.known).insert(attr.to_owned());
    }

    pub(super) fn note_declaration(&self, attr: &str, parent: Option<String>) {
        lock(&self.declared).insert(attr.to_owned(), parent);
    }

    /// Whether a request for `attr` must wait for the groups to be resolved:
    /// it declares a parent, was grouped before, or is named as a parent.
    pub(super) fn should_hold(&self, attr: &str) -> bool {
        let declared = lock(&self.declared);
        if let Some(parent) = declared.get(attr) {
            if parent.is_some() {
                return true;
            }
        } else if self.previous_roots.contains_key(attr) {
            return true;
        }
        if self.previous_roots.values().any(|root| root == attr) {
            return true;
        }
        let tail = crate::groups::split_attr_path(attr)
            .last()
            .copied()
            .unwrap_or(attr);
        declared
            .values()
            .flatten()
            .any(|p| p == attr || p.as_str() == tail)
    }

    /// Resolve and store the groups, then turn the held requests into jobs.
    pub(super) async fn finish(
        &self,
        db: &Database,
        repo: &str,
        held: Vec<UpdateRequest>,
    ) -> Vec<UpdateJob> {
        let known = lock(&self.known).clone();
        let declared = lock(&self.declared).clone();

        let mut parents = self.previous_parents.clone();
        let fresh: Vec<(&str, &str)> = declared
            .iter()
            .filter_map(|(attr, parent)| parent.as_deref().map(|p| (attr.as_str(), p)))
            .collect();
        for (attr, parent) in &declared {
            if parent.is_none() {
                parents.remove(attr);
            }
        }
        // Packages gone from the repo leave their groups.
        parents.retain(|attr, _| known.is_empty() || known.contains(attr));
        parents.extend(resolve_declarations(fresh, &known, "run"));

        let roots = resolve_roots(&parents);
        if let Err(e) = db.replace_groups(repo, &parents, &roots).await {
            warn!("Failed to store update groups: {e:#}");
        }
        group_jobs(db, repo, held, &roots).await
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
