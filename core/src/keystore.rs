//! "Remember on this computer": keeps the vault key protected by the user's OS login,
//! so the vault opens without the master password.
//!
//! - Windows: DPAPI (CryptProtectData), stored as `vault.key` next to the vault.
//!   Only the same Windows user on the same PC can decrypt it.
//! - macOS: the login Keychain.
//! - Linux: not supported yet; `SUPPORTED` is false.

use std::path::Path;

use zeroize::Zeroizing;

pub type Key = Zeroizing<[u8; 32]>;

#[cfg(any(windows, target_os = "macos"))]
pub const SUPPORTED: bool = true;
#[cfg(not(any(windows, target_os = "macos")))]
pub const SUPPORTED: bool = false;

/// Text for the checkbox, naming what protects the key.
pub fn label() -> &'static str {
    if cfg!(windows) {
        "Remember on this computer (unlock with my Windows login)"
    } else {
        "Remember on this computer (unlock with my Mac login)"
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn key_file(vault: &Path) -> std::path::PathBuf {
        vault.with_extension("key")
    }

    const ENTROPY: &[u8] = b"myRDCcenter vault key";

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    pub fn save(vault: &Path, key: &[u8; 32]) -> Result<(), String> {
        let input = blob(key);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err(format!("Windows could not protect the key ({})", std::io::Error::last_os_error()));
        }
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        std::fs::write(key_file(vault), bytes).map_err(|e| format!("Could not save key: {e}"))
    }

    pub fn load(vault: &Path) -> Option<Key> {
        let data = std::fs::read(key_file(vault)).ok()?;
        let input = blob(&data);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return None;
        }
        let plain = unsafe { std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize) };
        let mut key = Zeroizing::new([0u8; 32]);
        let good = plain.len() == 32;
        if good {
            key.copy_from_slice(plain);
        }
        zeroize::Zeroize::zeroize(plain);
        unsafe { LocalFree(out.pbData as _) };
        good.then_some(key)
    }

    pub fn forget(vault: &Path) {
        let _ = std::fs::remove_file(key_file(vault));
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    fn entry(vault: &Path) -> Option<keyring::Entry> {
        keyring::Entry::new("myRDCcenter", &vault.to_string_lossy()).ok()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn save(vault: &Path, key: &[u8; 32]) -> Result<(), String> {
        let text = Zeroizing::new(hex(key));
        entry(vault)
            .ok_or("Keychain not available")?
            .set_password(&text)
            .map_err(|e| format!("Could not save to Keychain: {e}"))
    }

    pub fn load(vault: &Path) -> Option<Key> {
        let text = Zeroizing::new(entry(vault)?.get_password().ok()?);
        if text.len() != 64 {
            return None;
        }
        let mut key = Zeroizing::new([0u8; 32]);
        for (i, k) in key.iter_mut().enumerate() {
            *k = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(key)
    }

    pub fn forget(vault: &Path) {
        if let Some(e) = entry(vault) {
            let _ = e.delete_credential();
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::*;

    pub fn save(_vault: &Path, _key: &[u8; 32]) -> Result<(), String> {
        Err("Remembering the vault is not supported on this system yet".into())
    }

    pub fn load(_vault: &Path) -> Option<Key> {
        None
    }

    pub fn forget(_vault: &Path) {}
}

pub fn save(vault: &Path, key: &[u8; 32]) -> Result<(), String> {
    imp::save(vault, key)
}

pub fn load(vault: &Path) -> Option<Key> {
    imp::load(vault)
}

pub fn forget(vault: &Path) {
    imp::forget(vault)
}
