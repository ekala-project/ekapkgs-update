use std::collections::HashMap;
use anyhow::Result;

type Attr = String;
type Group = String;

/// Owns the actual group data
/// This makes the assumption that an attr only exists in one group
/// References into GroupsData
pub struct GroupsData {
    groups_by_name: HashMap<Group, Vec<Attr>>,
}

impl GroupsData {
    /// Load grouping file from disk
    /// Format: { "group-name": ["attr.path1", "attr.path2"], ... }
    pub async fn load_from_file(path: &str) -> Result<Self> {
        let content = tokio::fs::read_to_string(path).await?;
        let groups: HashMap<Group, Vec<Attr>> = serde_json::from_str(&content)?;

        Ok(Self {
            groups_by_name: groups,
        })
    }

    /// Build the index maps that reference into the owned data
    pub fn build_index(&self) -> Groupings<'_> {
        let mut attr_to_group = HashMap::new();
        let mut members_by_attr = HashMap::new();

        for (group, attrs) in &self.groups_by_name {
            for attr in attrs {
                members_by_attr.insert(attr, attrs);
                attr_to_group.insert(attr, group);
            }
        }

        Groupings {
            attr_to_group,
            members_by_attr,
        }
    }
}

/// Create index versions of the queries as it will be a common operation
pub struct Groupings<'a> {
    /// See which group an attr is a member of
    attr_to_group: HashMap<&'a Attr, &'a Group>,
    /// Given an attr, what other attrs are in the same group
    members_by_attr: HashMap<&'a Attr, &'a Vec<Attr>>,
}

impl<'a> Groupings<'a> {
    pub fn group_name(&self, attr: &Attr) -> Option<&'a Group> {
        self.attr_to_group.get(attr).map(|v| *v)
    }

    pub fn attr_in_group(&self, attr: &Attr) -> bool {
        self.attr_to_group.get(attr).is_some()
    }

    pub fn group_members(&self, attr: &Attr) -> Option<&'a Vec<Attr>> {
        self.members_by_attr.get(attr).map(|v| *v)
    }
}
