//! Export connections to an encrypted .rdcx file, to move or copy them to another computer.
//!
//! The file uses the vault format, but is locked with its own export password,
//! so it can be opened on a PC that has a different master password.
//! Passwords of the connections are included.

use std::path::Path;

use crate::groups::{is_within, normalize};
use crate::import_rdm::RdmImport;
use crate::model::VaultData;
use crate::vault::Vault;

pub const EXPORT_EXTENSION: &str = "rdcx";

/// Connections (and groups) to put in an export: everything, or one group with its subgroups.
pub fn select(data: &VaultData, group: Option<&str>) -> VaultData {
    let inside = |g: &str| group.is_none_or(|sel| is_within(&normalize(g), sel));
    VaultData {
        connections: data.connections.iter().filter(|c| inside(&c.group)).cloned().collect(),
        groups: data.all_groups().into_iter().filter(|g| inside(g)).collect(),
        settings: Default::default(),
    }
}

/// Write an export file. Returns how many connections were written.
pub fn export_to(data: &VaultData, group: Option<&str>, password: &str, dest: &Path) -> Result<usize, String> {
    let subset = select(data, group);
    let bytes = Vault::seal(&subset, password).map_err(|e| e.to_string())?;
    std::fs::write(dest, bytes).map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
    Ok(subset.connections.len())
}

/// Read an export file. A wrong password gives an error.
pub fn read_export(path: &Path, password: &str) -> Result<RdmImport, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read file: {e}"))?;
    let data = Vault::unseal(&bytes, password).map_err(|e| match e {
        crate::vault::VaultError::WrongPassword => "Wrong export password".to_string(),
        e => e.to_string(),
    })?;
    Ok(RdmImport { connections: data.connections, groups: data.groups, ..Default::default() })
}

/// Remove connections after a "move": the ones that were exported.
pub fn remove_exported(data: &mut VaultData, group: Option<&str>) {
    match group {
        Some(g) => {
            data.connections.retain(|c| !is_within(&normalize(&c.group), g));
            data.delete_group_tree(g);
        }
        None => {
            data.connections.clear();
            data.groups.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Connection, Secret};

    fn sample() -> VaultData {
        let mut d = VaultData::default();
        for (n, g) in [("a", "Work/Servers"), ("b", "Work"), ("c", "Home")] {
            let mut c = Connection::new(n, g);
            c.host = format!("{n}.local");
            c.password = Secret::new(format!("pw-{n}"));
            d.connections.push(c);
        }
        d
    }

    #[test]
    fn roundtrip_group_with_passwords() {
        let d = sample();
        let path = std::env::temp_dir().join(format!("rdcx-test-{}.rdcx", std::process::id()));
        assert_eq!(export_to(&d, Some("Work"), "exportpw", &path).unwrap(), 2);
        assert!(read_export(&path, "wrong").is_err());
        let imp = read_export(&path, "exportpw").unwrap();
        assert_eq!(imp.connections.len(), 2);
        assert!(imp.connections.iter().any(|c| c.name == "a" && c.password.as_str() == "pw-a"));
        assert!(imp.groups.contains(&"Work/Servers".to_string()));

        let mut other = VaultData::default();
        assert_eq!(other.merge_import(imp).added, 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn move_removes_group() {
        let mut d = sample();
        d.add_group("Work/Empty");
        remove_exported(&mut d, Some("Work"));
        assert_eq!(d.connections.len(), 1);
        assert_eq!(d.all_groups(), ["Home"]);
    }
}
