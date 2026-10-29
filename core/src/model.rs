use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

/// Everything stored inside the encrypted vault file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultData {
    pub connections: Vec<Connection>,
    /// Groups created by the user, as "/" separated paths. Kept so empty groups survive.
    pub groups: Vec<String>,
    pub settings: Settings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Lock the vault after this many idle minutes (0 = never).
    pub auto_lock_minutes: u32,
    /// Group paths that are folded in the tree.
    pub collapsed_groups: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_lock_minutes: 10, collapsed_groups: Vec::new() }
    }
}

/// One remote desktop connection profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub id: Uuid,
    pub name: String,
    /// Folder path, "/" separated, e.g. "Customers/ACME". Empty = root.
    pub group: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub password: Secret,
    pub gateway: String,
    pub fullscreen: bool,
    pub width: u32,
    pub height: u32,
    pub multimon: bool,
    pub redirect_clipboard: bool,
    pub redirect_drives: bool,
    pub admin_session: bool,
    pub notes: String,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            group: String::new(),
            host: String::new(),
            port: 3389,
            username: String::new(),
            domain: String::new(),
            password: Secret::default(),
            gateway: String::new(),
            fullscreen: true,
            width: 1920,
            height: 1080,
            multimon: false,
            redirect_clipboard: true,
            redirect_drives: false,
            admin_session: false,
            notes: String::new(),
        }
    }
}

/// A string that is wiped from memory when dropped.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into())
    }
}

impl std::ops::Deref for Secret {
    type Target = String;
    fn deref(&self) -> &String {
        &self.0
    }
}

impl std::ops::DerefMut for Secret {
    fn deref_mut(&mut self) -> &mut String {
        &mut self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Connection {
    pub fn new(name: &str, group: &str) -> Self {
        Self { name: name.into(), group: group.into(), ..Default::default() }
    }

    /// Name shown in lists; falls back to the host.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() { &self.host } else { &self.name }
    }

    /// "DOMAIN\user", or just "user" when no domain is set.
    pub fn full_username(&self) -> String {
        if self.domain.trim().is_empty() {
            self.username.clone()
        } else {
            format!("{}\\{}", self.domain.trim(), self.username.trim())
        }
    }

    /// "host" or "host:port" when the port is not the default.
    pub fn address(&self) -> String {
        if self.port == 0 || self.port == 3389 {
            self.host.trim().to_string()
        } else {
            format!("{}:{}", self.host.trim(), self.port)
        }
    }

    /// Case-insensitive match on name, host, group, user and notes.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        [&self.name, &self.host, &self.group, &self.username, &self.notes]
            .iter()
            .any(|f| f.to_lowercase().contains(&q))
    }
}

impl VaultData {
    pub fn find(&self, id: Uuid) -> Option<&Connection> {
        self.connections.iter().find(|c| c.id == id)
    }

    pub fn find_mut(&mut self, id: Uuid) -> Option<&mut Connection> {
        self.connections.iter_mut().find(|c| c.id == id)
    }

    pub fn remove(&mut self, id: Uuid) {
        self.connections.retain(|c| c.id != id);
    }

    /// Connections sorted by group then name, filtered by a search query.
    pub fn sorted_filtered(&self, query: &str) -> Vec<&Connection> {
        let mut list: Vec<&Connection> = self.connections.iter().filter(|c| c.matches(query)).collect();
        list.sort_by(|a, b| {
            (a.group.to_lowercase(), a.display_name().to_lowercase())
                .cmp(&(b.group.to_lowercase(), b.display_name().to_lowercase()))
        });
        list
    }
}
