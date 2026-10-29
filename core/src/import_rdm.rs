//! Import from .rdm export files (XML).
//!
//! Only RDP entries are imported. Folders become groups ("A\B" -> "A/B"),
//! and usernames are taken from the entry itself or from its linked credential entry.
//! Passwords in these files are encrypted by the exporting program, so they are
//! not imported and must be typed in again.

use std::collections::HashMap;
use std::path::Path;

use roxmltree::{Document, Node};

use crate::groups::normalize;
use crate::model::{Connection, VaultData};

#[derive(Debug, Default)]
pub struct RdmImport {
    pub connections: Vec<Connection>,
    pub groups: Vec<String>,
    /// Entries that had a saved password we could not import.
    pub passwords_skipped: usize,
    /// Non-RDP entries that were skipped (SSH, web, etc.).
    pub other_skipped: usize,
}

/// Result of merging an import into a vault.
#[derive(Debug, Default, PartialEq)]
pub struct MergeStats {
    pub added: usize,
    pub duplicates: usize,
}

fn child<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text(n: Node, name: &str) -> String {
    child(n, name).and_then(|c| c.text()).unwrap_or("").trim().to_string()
}

fn group_path(raw: &str) -> String {
    normalize(&raw.replace('\\', "/"))
}

/// Parse the XML text of an .rdm export.
pub fn parse_rdm(xml: &str) -> Result<RdmImport, String> {
    let doc = Document::parse(xml.trim_start_matches('\u{feff}')).map_err(|e| format!("Not a valid .rdm file: {e}"))?;
    let root = doc.root_element();
    let entries: Vec<Node> = root
        .descendants()
        .filter(|n| n.has_tag_name("Connection") && n.parent().is_some_and(|p| p.has_tag_name("Connections")))
        .collect();
    if entries.is_empty() && !root.has_tag_name("RDMExport") && !root.has_tag_name("ArrayOfConnection") {
        return Err("No entries found in this file".into());
    }

    // Credential entries by id, so RDP entries can pick up their username.
    let mut creds: HashMap<String, (String, String, bool)> = HashMap::new();
    for e in &entries {
        if text(*e, "ConnectionType") == "Credential" {
            let c = child(*e, "Credentials");
            let user = c.map(|c| text(c, "UserName")).unwrap_or_default();
            let domain = c.map(|c| text(c, "Domain")).unwrap_or_default();
            let has_pw = c.is_some_and(|c| !text(c, "SafePassword").is_empty() || !text(c, "Password").is_empty());
            creds.insert(text(*e, "ID").to_lowercase(), (user, domain, has_pw));
        }
    }

    let mut out = RdmImport::default();
    for e in &entries {
        let kind = text(*e, "ConnectionType");
        let group = group_path(&text(*e, "Group"));
        match kind.as_str() {
            "Group" => {
                if !group.is_empty() && !out.groups.contains(&group) {
                    out.groups.push(group);
                }
            }
            "RDPConfigured" | "RDP" => out.connections.push(parse_rdp_entry(*e, &group, &creds, &mut out.passwords_skipped)),
            "Credential" => {}
            _ => out.other_skipped += 1,
        }
    }
    Ok(out)
}

fn parse_rdp_entry(e: Node, group: &str, creds: &HashMap<String, (String, String, bool)>, pw_skipped: &mut usize) -> Connection {
    let mut c = Connection::new(&text(e, "Name"), group);
    let url = text(e, "Url");
    match url.rsplit_once(':').filter(|(h, p)| !h.contains(':') && p.parse::<u16>().is_ok()) {
        Some((h, p)) => {
            c.host = h.to_string();
            c.port = p.parse().unwrap_or(3389);
        }
        None => c.host = url.trim_matches(['[', ']']).to_string(),
    }
    c.admin_session = text(e, "Console").eq_ignore_ascii_case("true");

    let rdp = child(e, "RDP");
    // Username/domain can sit on the entry, in its <RDP> block, or in a linked credential entry.
    let mut user = text(e, "UserName");
    let mut domain = text(e, "Domain");
    if let Some(r) = rdp {
        if user.is_empty() {
            user = text(r, "UserName");
        }
        if domain.is_empty() {
            domain = text(r, "Domain");
        }
        let gw = text(r, "GatewayHostname");
        if !gw.is_empty() {
            c.gateway = gw;
        }
        if text(r, "UseMultipleMonitors").eq_ignore_ascii_case("true") || text(r, "UseMultimon").eq_ignore_ascii_case("true") {
            c.multimon = true;
        }
    }
    let mut has_pw = !text(e, "SafePassword").is_empty() || rdp.is_some_and(|r| !text(r, "SafePassword").is_empty());
    for link in ["PersonalConnectionID", "CredentialConnectionID"] {
        if let Some((u, d, pw)) = creds.get(&text(e, link).to_lowercase()) {
            if user.is_empty() {
                user = u.clone();
            }
            if domain.is_empty() {
                domain = d.clone();
            }
            has_pw |= *pw;
        }
    }
    // "DOMAIN\user" in the username field.
    if domain.is_empty() {
        if let Some((d, u)) = user.split_once('\\') {
            domain = d.to_string();
            user = u.to_string();
        }
    }
    c.username = user;
    c.domain = domain;
    if has_pw {
        *pw_skipped += 1;
    }
    let desc = text(e, "Description");
    if !desc.is_empty() {
        c.notes = desc;
    }
    c
}

/// Read an .rdm file from disk.
pub fn import_rdm(path: &Path) -> Result<RdmImport, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read file: {e}"))?;
    let xml = if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    parse_rdm(&xml)
}

impl VaultData {
    /// Add imported groups and connections. A connection with the same name, host and
    /// group as an existing one is skipped, so importing the same file twice is harmless.
    pub fn merge_import(&mut self, import: RdmImport) -> MergeStats {
        let mut stats = MergeStats::default();
        for g in import.groups {
            self.add_group(&g);
        }
        for c in import.connections {
            let dup = self.connections.iter().any(|e| {
                e.name.eq_ignore_ascii_case(&c.name) && e.host.eq_ignore_ascii_case(&c.host) && normalize(&e.group) == c.group
            });
            if dup {
                stats.duplicates += 1;
            } else {
                self.connections.push(c);
                stats.added += 1;
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<RDMExport>
  <Connections>
    <Connection>
      <ConnectionType>RDPConfigured</ConnectionType>
      <CredentialConnectionID>245A4245-48E7-4DF5-9C4C-11861D8E1F81</CredentialConnectionID>
      <Group>Test\Subfolder</Group>
      <ID>aaaa0001-0000-0000-0000-000000000001</ID>
      <Name>Server A</Name>
      <PersonalConnectionID>cccc0001-0000-0000-0000-000000000001</PersonalConnectionID>
      <Console>true</Console>
      <Url>host1:55307</Url>
    </Connection>
    <Connection>
      <ConnectionType>Group</ConnectionType>
      <Group>Test\Subfolder</Group>
      <ID>bbbb0001-0000-0000-0000-000000000001</ID>
      <Name>Subfolder</Name>
    </Connection>
    <Connection>
      <ConnectionType>Credential</ConnectionType>
      <Group>Test\Subfolder</Group>
      <ID>cccc0001-0000-0000-0000-000000000001</ID>
      <Name>Server A_2</Name>
      <ParentID>aaaa0001-0000-0000-0000-000000000001</ParentID>
      <Credentials>
        <SafePassword>AAAAAAAAAAA=</SafePassword>
        <UserName>CORP\alice</UserName>
      </Credentials>
    </Connection>
    <Connection>
      <ConnectionType>RDPConfigured</ConnectionType>
      <Group>Test\Subfolder2</Group>
      <ID>aaaa0002-0000-0000-0000-000000000002</ID>
      <Name>Server B</Name>
      <Url>10.0.0.9</Url>
      <RDP><ScreenSizingMode>FitToWindow</ScreenSizingMode></RDP>
    </Connection>
    <Connection>
      <ConnectionType>Group</ConnectionType>
      <Group>Empty</Group>
      <ID>bbbb0002-0000-0000-0000-000000000002</ID>
      <Name>Empty</Name>
    </Connection>
    <Connection>
      <ConnectionType>SSHShell</ConnectionType>
      <Group>Test</Group>
      <ID>dddd0001-0000-0000-0000-000000000001</ID>
      <Name>Linux box</Name>
      <Url>linux1</Url>
    </Connection>
  </Connections>
  <Version>2</Version>
</RDMExport>"#;

    #[test]
    fn parses_sample() {
        let imp = parse_rdm(SAMPLE).unwrap();
        assert_eq!(imp.connections.len(), 2);
        let a = &imp.connections[0];
        assert_eq!((a.name.as_str(), a.group.as_str(), a.host.as_str(), a.port), ("Server A", "Test/Subfolder", "host1", 55307));
        assert_eq!((a.domain.as_str(), a.username.as_str()), ("CORP", "alice"));
        assert!(a.admin_session);
        assert!(a.password.is_empty());
        let b = &imp.connections[1];
        assert_eq!((b.host.as_str(), b.port, b.group.as_str()), ("10.0.0.9", 3389, "Test/Subfolder2"));
        assert_eq!(imp.groups, ["Test/Subfolder", "Empty"]);
        assert_eq!(imp.passwords_skipped, 1);
        assert_eq!(imp.other_skipped, 1);
    }

    #[test]
    fn merge_skips_duplicates() {
        let mut d = VaultData::default();
        assert_eq!(d.merge_import(parse_rdm(SAMPLE).unwrap()), MergeStats { added: 2, duplicates: 0 });
        assert_eq!(d.merge_import(parse_rdm(SAMPLE).unwrap()), MergeStats { added: 0, duplicates: 2 });
        assert!(d.all_groups().contains(&"Empty".to_string()));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_rdm("not xml").is_err());
    }
}
