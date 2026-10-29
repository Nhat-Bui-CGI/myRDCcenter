//! Building, importing and launching .rdp files with the built-in Windows client (mstsc).

use std::path::Path;

use crate::model::Connection;

/// How long the temporary .rdp file and the session credential live after launch.
/// Long enough for mstsc to read them, even if a certificate prompt is shown first.
#[cfg(windows)]
const CLEANUP_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Text of an .rdp file for this connection (no password inside).
pub fn build_rdp_file(c: &Connection) -> String {
    let mut lines = vec![
        format!("full address:s:{}", c.address()),
        format!("username:s:{}", c.full_username()),
        format!("screen mode id:i:{}", if c.fullscreen { 2 } else { 1 }),
        format!("use multimon:i:{}", c.multimon as u8),
        format!("redirectclipboard:i:{}", c.redirect_clipboard as u8),
        format!("drivestoredirect:s:{}", if c.redirect_drives { "*" } else { "" }),
        format!("administrative session:i:{}", c.admin_session as u8),
        "prompt for credentials:i:0".into(),
        "authentication level:i:2".into(),
        "enablecredsspsupport:i:1".into(),
    ];
    if !c.fullscreen {
        lines.push(format!("desktopwidth:i:{}", c.width));
        lines.push(format!("desktopheight:i:{}", c.height));
    }
    if !c.gateway.trim().is_empty() {
        lines.push(format!("gatewayhostname:s:{}", c.gateway.trim()));
        lines.push("gatewayusagemethod:i:1".into());
        lines.push("gatewaycredentialssource:i:4".into());
        lines.push("promptcredentialonce:i:1".into());
    }
    lines.join("\r\n") + "\r\n"
}

/// Read an existing .rdp file (UTF-16 or UTF-8) into a new connection.
/// Passwords are never imported; .rdp files from mstsc don't hold usable ones.
pub fn parse_rdp_file(bytes: &[u8], file_name: &str) -> Connection {
    let text = decode_text(bytes);
    let mut c = Connection::new(file_name, "Imported");
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let (Some(key), Some(_ty), Some(val)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let val = val.trim();
        match key.trim().to_lowercase().as_str() {
            "full address" => {
                // host, host:port, or [ipv6]:port
                if let Some((h, p)) = val.rsplit_once(':').filter(|(h, _)| !h.contains(':') || h.ends_with(']')) {
                    c.host = h.trim_matches(['[', ']']).to_string();
                    c.port = p.parse().unwrap_or(3389);
                } else {
                    c.host = val.to_string();
                }
            }
            "username" => {
                if let Some((d, u)) = val.split_once('\\') {
                    c.domain = d.into();
                    c.username = u.into();
                } else {
                    c.username = val.into();
                }
            }
            "gatewayhostname" => c.gateway = val.into(),
            "screen mode id" => c.fullscreen = val == "2",
            "desktopwidth" => c.width = val.parse().unwrap_or(c.width),
            "desktopheight" => c.height = val.parse().unwrap_or(c.height),
            "use multimon" => c.multimon = val == "1",
            "redirectclipboard" => c.redirect_clipboard = val == "1",
            "drivestoredirect" => c.redirect_drives = !val.is_empty(),
            "administrative session" => c.admin_session = val == "1",
            _ => {}
        }
    }
    c
}

/// Import a .rdp file from disk.
pub fn import_rdp(path: &Path) -> std::io::Result<Connection> {
    let bytes = std::fs::read(path)?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(parse_rdp_file(&bytes, &name))
}

fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        String::from_utf8_lossy(b).into_owned()
    }
}

/// Start a Remote Desktop session.
///
/// Writes a temporary .rdp file, stores the password as a session-only Windows
/// credential (TERMSRV/host) so mstsc logs in without asking, starts mstsc, and
/// removes both again shortly after.
#[cfg(windows)]
pub fn launch(c: &Connection) -> Result<(), String> {
    use std::process::Command;

    if c.host.trim().is_empty() {
        return Err("No host set".into());
    }
    let path = std::env::temp_dir().join(format!("rds-center-{}.rdp", uuid::Uuid::new_v4()));
    let mut bytes = vec![0xFF, 0xFE];
    for u in build_rdp_file(c).encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&path, bytes).map_err(|e| format!("Could not write .rdp file: {e}"))?;

    let target = format!("TERMSRV/{}", c.host.trim());
    let has_cred = !c.password.is_empty() && !c.username.trim().is_empty();
    if has_cred {
        win_cred::write(&target, &c.full_username(), &c.password)?;
    }

    let spawned = Command::new("mstsc.exe").arg(&path).spawn();

    std::thread::spawn(move || {
        std::thread::sleep(CLEANUP_AFTER);
        let _ = std::fs::remove_file(&path);
        if has_cred {
            win_cred::delete(&target);
        }
    });

    spawned.map(|_| ()).map_err(|e| format!("Could not start mstsc: {e}"))
}

#[cfg(not(windows))]
pub fn launch(_c: &Connection) -> Result<(), String> {
    Err("Connecting only works on Windows".into())
}

#[cfg(windows)]
mod win_cred {
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredWriteW, CREDENTIALW, CRED_PERSIST_SESSION, CRED_TYPE_GENERIC,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn write(target: &str, user: &str, password: &str) -> Result<(), String> {
        let mut target_w = wide(target);
        let mut user_w = wide(user);
        let mut blob: Vec<u8> = password.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
        cred.Type = CRED_TYPE_GENERIC;
        cred.TargetName = target_w.as_mut_ptr();
        cred.UserName = user_w.as_mut_ptr();
        cred.CredentialBlobSize = blob.len() as u32;
        cred.CredentialBlob = blob.as_mut_ptr();
        cred.Persist = CRED_PERSIST_SESSION;
        let ok = unsafe { CredWriteW(&cred, 0) };
        zeroize::Zeroize::zeroize(&mut blob);
        if ok == 0 {
            Err(format!("Could not store credential ({})", std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }

    pub fn delete(target: &str) {
        let target_w = wide(target);
        unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdp_roundtrip() {
        let mut c = Connection::new("Srv", "");
        c.host = "srv.example.com".into();
        c.port = 3390;
        c.domain = "CORP".into();
        c.username = "nhat".into();
        c.fullscreen = false;
        c.width = 1600;
        c.height = 900;
        c.gateway = "gw.example.com".into();
        let text = build_rdp_file(&c);
        assert!(text.contains("full address:s:srv.example.com:3390"));

        let back = parse_rdp_file(text.as_bytes(), "Srv");
        assert_eq!(back.host, "srv.example.com");
        assert_eq!(back.port, 3390);
        assert_eq!(back.domain, "CORP");
        assert_eq!(back.username, "nhat");
        assert!(!back.fullscreen);
        assert_eq!(back.width, 1600);
        assert_eq!(back.gateway, "gw.example.com");
    }

    #[test]
    fn utf16_import() {
        let text = "full address:s:10.1.2.3\r\nusername:s:admin\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let c = parse_rdp_file(&bytes, "x");
        assert_eq!(c.host, "10.1.2.3");
        assert_eq!(c.port, 3389);
        assert_eq!(c.username, "admin");
    }
}
