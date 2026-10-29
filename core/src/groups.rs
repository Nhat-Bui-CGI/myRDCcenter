//! Connection groups: a tree built from "/" separated paths like "Work/Servers".

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::model::VaultData;

/// Clean up a group path: trim each part, drop empty parts. "  a / /b " -> "a/b".
pub fn normalize(path: &str) -> String {
    path.split('/').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
}

/// Last part of a path: "Work/Servers" -> "Servers".
pub fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Parent of a path: "Work/Servers" -> "Work", "Work" -> "".
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

/// True if `path` is `group` itself or inside it.
pub fn is_within(path: &str, group: &str) -> bool {
    path == group || path.starts_with(&format!("{group}/"))
}

/// One visible line in the connection tree.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeRow {
    Group { path: String, name: String, depth: usize, expanded: bool, count: usize },
    Connection { id: Uuid, name: String, address: String, depth: usize },
}

impl VaultData {
    /// Every group: the ones created by the user, the ones connections use, and all their parents.
    pub fn all_groups(&self) -> Vec<String> {
        let mut set = BTreeSet::new();
        for g in self.groups.iter().map(String::as_str).chain(self.connections.iter().map(|c| c.group.as_str())) {
            let mut g = normalize(g);
            while !g.is_empty() {
                set.insert(g.clone());
                g = parent(&g).to_string();
            }
        }
        let mut list: Vec<String> = set.into_iter().collect();
        list.sort_by_key(|g| g.to_lowercase());
        list
    }

    /// Create a group (and its parents). Returns the cleaned path.
    pub fn add_group(&mut self, path: &str) -> String {
        let path = normalize(path);
        if !path.is_empty() && !self.groups.contains(&path) {
            self.groups.push(path.clone());
        }
        path
    }

    /// Rename or move a group, with everything inside it.
    pub fn rename_group(&mut self, old: &str, new: &str) -> Result<(), String> {
        let new = normalize(new);
        if new.is_empty() {
            return Err("Group name can't be empty".into());
        }
        if is_within(&new, old) && new != old {
            return Err("A group can't be moved into itself".into());
        }
        let rewrite = |p: &str| -> Option<String> {
            if is_within(p, old) { Some(format!("{new}{}", &p[old.len()..])) } else { None }
        };
        for g in self.groups.iter_mut() {
            if let Some(n) = rewrite(g) {
                *g = n;
            }
        }
        for c in self.connections.iter_mut() {
            if let Some(n) = rewrite(&c.group) {
                c.group = n;
            }
        }
        for g in self.settings.collapsed_groups.iter_mut() {
            if let Some(n) = rewrite(g) {
                *g = n;
            }
        }
        self.groups.sort();
        self.groups.dedup();
        if !self.groups.contains(&new) {
            self.groups.push(new);
        }
        Ok(())
    }

    /// Delete a group. Its connections and subgroups move up to the parent group,
    /// so deleting a group never deletes connections.
    pub fn delete_group(&mut self, path: &str) {
        let up = parent(path).to_string();
        let lift = |p: &str| -> Option<String> {
            if p == path {
                Some(up.clone())
            } else if is_within(p, path) {
                let rest = &p[path.len() + 1..];
                Some(if up.is_empty() { rest.to_string() } else { format!("{up}/{rest}") })
            } else {
                None
            }
        };
        let mut groups = Vec::new();
        for g in &self.groups {
            match lift(g) {
                Some(n) if !n.is_empty() => groups.push(n),
                Some(_) => {}
                None => groups.push(g.clone()),
            }
        }
        groups.sort();
        groups.dedup();
        self.groups = groups;
        for c in self.connections.iter_mut() {
            if let Some(n) = lift(&c.group) {
                c.group = n;
            }
        }
        self.settings.collapsed_groups.retain(|g| !is_within(g, path));
    }

    pub fn move_connection(&mut self, id: Uuid, group: &str) {
        let group = normalize(group);
        if let Some(c) = self.find_mut(id) {
            c.group = group;
        }
    }

    pub fn is_collapsed(&self, path: &str) -> bool {
        self.settings.collapsed_groups.iter().any(|g| g == path)
    }

    pub fn set_collapsed(&mut self, path: &str, collapsed: bool) {
        self.settings.collapsed_groups.retain(|g| g != path);
        if collapsed {
            self.settings.collapsed_groups.push(path.to_string());
        }
    }

    /// Number of connections in a group, including its subgroups.
    pub fn count_in(&self, path: &str) -> usize {
        self.connections.iter().filter(|c| is_within(&normalize(&c.group), path)).count()
    }

    /// The visible tree: subgroups first, then connections, sorted by name.
    /// While searching, only matching connections (and the groups holding them) are shown,
    /// and every group is expanded.
    pub fn tree_rows(&self, query: &str) -> Vec<TreeRow> {
        let searching = !query.trim().is_empty();
        let groups = self.all_groups();
        let mut rows = Vec::new();
        self.push_level("", 0, &groups, query, searching, &mut rows);
        rows
    }

    fn push_level(&self, at: &str, depth: usize, groups: &[String], query: &str, searching: bool, rows: &mut Vec<TreeRow>) {
        for g in groups.iter().filter(|g| parent(g) == at) {
            let matching = self
                .connections
                .iter()
                .filter(|c| is_within(&normalize(&c.group), g) && c.matches(query))
                .count();
            if searching && matching == 0 && !g.to_lowercase().contains(&query.trim().to_lowercase()) {
                continue;
            }
            let expanded = searching || !self.is_collapsed(g);
            rows.push(TreeRow::Group { path: g.clone(), name: leaf(g).to_string(), depth, expanded, count: self.count_in(g) });
            if expanded {
                self.push_level(g, depth + 1, groups, query, searching, rows);
            }
        }
        let mut conns: Vec<_> = self.connections.iter().filter(|c| normalize(&c.group) == at && c.matches(query)).collect();
        conns.sort_by_key(|c| c.display_name().to_lowercase());
        for c in conns {
            rows.push(TreeRow::Connection { id: c.id, name: c.display_name().to_string(), address: c.address(), depth });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Connection;

    fn sample() -> VaultData {
        let mut d = VaultData::default();
        for (name, group) in [("web", "Work/Servers"), ("dc", "Work/Servers"), ("pc", "Work"), ("nas", "Home"), ("x", "")] {
            let mut c = Connection::new(name, group);
            c.host = format!("{name}.local");
            d.connections.push(c);
        }
        d.add_group("Empty");
        d
    }

    fn names(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                TreeRow::Group { name, depth, .. } => format!("{}[{name}]", " ".repeat(*depth)),
                TreeRow::Connection { name, depth, .. } => format!("{}{name}", " ".repeat(*depth)),
            })
            .collect()
    }

    #[test]
    fn tree_layout() {
        let d = sample();
        assert_eq!(
            names(&d.tree_rows("")),
            ["[Empty]", "[Home]", " nas", "[Work]", " [Servers]", "  dc", "  web", " pc", "x"]
        );
        assert_eq!(d.count_in("Work"), 3);
    }

    #[test]
    fn collapse_and_search() {
        let mut d = sample();
        d.set_collapsed("Work", true);
        assert_eq!(names(&d.tree_rows("")), ["[Empty]", "[Home]", " nas", "[Work]", "x"]);
        // Searching opens everything and hides groups without matches.
        assert_eq!(names(&d.tree_rows("web")), ["[Work]", " [Servers]", "  web"]);
    }

    #[test]
    fn rename_moves_contents() {
        let mut d = sample();
        d.rename_group("Work", "Job").unwrap();
        assert!(d.connections.iter().any(|c| c.group == "Job/Servers"));
        assert!(!d.all_groups().iter().any(|g| g.starts_with("Work")));
        assert!(d.rename_group("Job", "Job/Inner").is_err());
    }

    #[test]
    fn delete_keeps_connections() {
        let mut d = sample();
        d.delete_group("Work");
        assert_eq!(d.connections.len(), 5);
        assert!(d.connections.iter().any(|c| c.name == "pc" && c.group.is_empty()));
        assert!(d.connections.iter().any(|c| c.name == "web" && c.group == "Servers"));
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize(" a / /b "), "a/b");
        assert_eq!(leaf("a/b"), "b");
        assert_eq!(parent("a/b"), "a");
        assert_eq!(parent("a"), "");
    }
}
