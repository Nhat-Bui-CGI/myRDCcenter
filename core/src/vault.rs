//! Encrypted vault file.
//!
//! Layout (all integers little-endian):
//!   magic "RDSC" | version u8 | argon2 m_cost u32 | t_cost u32 | p_cost u32
//!   | salt [16] | nonce [24] | ciphertext (XChaCha20-Poly1305 of JSON)
//! The header bytes are authenticated as associated data.
//! The key is derived from the master password with Argon2id and never written to disk.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, Zeroizing};

use crate::model::VaultData;

const MAGIC: &[u8; 4] = b"RDSC";
const FORMAT_VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 4 + 1 + 12 + SALT_LEN + NONCE_LEN;

// 64 MiB, 3 passes: roughly half a second on a normal laptop.
const DEFAULT_M_COST: u32 = 64 * 1024;
const DEFAULT_T_COST: u32 = 3;
const DEFAULT_P_COST: u32 = 1;

#[derive(Debug)]
pub enum VaultError {
    Io(std::io::Error),
    WrongPassword,
    Corrupt(String),
    Crypto(String),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::Io(e) => write!(f, "File error: {e}"),
            VaultError::WrongPassword => write!(f, "Wrong master password"),
            VaultError::Corrupt(m) => write!(f, "Vault file is damaged: {m}"),
            VaultError::Crypto(m) => write!(f, "Encryption error: {m}"),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<std::io::Error> for VaultError {
    fn from(e: std::io::Error) -> Self {
        VaultError::Io(e)
    }
}

/// Where the vault lives by default: %LOCALAPPDATA%\myRDCcenter\vault.rdsc
/// (Local, not Roaming, so it never follows a domain profile to other PCs).
/// A vault from before the rename (folder "RDS-center") is moved here on first use.
pub fn default_vault_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    let path = base.join("myRDCcenter").join("vault.rdsc");
    let old = base.join("RDS-center").join("vault.rdsc");
    if !path.exists() && old.is_file() {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if fs::rename(&old, &path).is_err() {
            let _ = fs::copy(&old, &path);
        }
    }
    path
}

#[derive(Clone, Copy)]
struct KdfParams {
    m: u32,
    t: u32,
    p: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self { m: DEFAULT_M_COST, t: DEFAULT_T_COST, p: DEFAULT_P_COST }
    }
}

/// An unlocked vault. Dropping it wipes the key from memory.
pub struct Vault {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
    salt: [u8; SALT_LEN],
    kdf: KdfParams,
    pub data: VaultData,
}

impl Vault {
    pub fn exists(path: &Path) -> bool {
        path.is_file()
    }

    /// Create a new, empty vault file protected by `password`.
    pub fn create(path: &Path, password: &str) -> Result<Self, VaultError> {
        let kdf = KdfParams::default();
        let salt = random_bytes::<SALT_LEN>()?;
        let key = derive_key(password, &salt, kdf)?;
        let vault = Vault { path: path.to_path_buf(), key, salt, kdf, data: VaultData::default() };
        vault.save()?;
        Ok(vault)
    }

    /// Open and decrypt an existing vault file.
    pub fn open(path: &Path, password: &str) -> Result<Self, VaultError> {
        Self::open_inner(path, |salt, kdf| derive_key(password, salt, kdf))
    }

    /// Open with a key remembered on this computer (see [`crate::keystore`]).
    /// Returns None when nothing is remembered for this vault.
    pub fn open_remembered(path: &Path) -> Option<Result<Self, VaultError>> {
        let key = crate::keystore::load(path)?;
        Some(Self::open_inner(path, |_, _| Ok(key.clone())))
    }

    /// True when this vault's key is remembered on this computer.
    pub fn is_remembered(&self) -> bool {
        crate::keystore::load(&self.path).is_some()
    }

    /// Remember the key on this computer, protected by the user's login, so the
    /// vault opens without the master password. Pass false to forget it again.
    pub fn set_remembered(&self, remember: bool) -> Result<(), String> {
        if remember {
            crate::keystore::save(&self.path, &self.key)
        } else {
            crate::keystore::forget(&self.path);
            Ok(())
        }
    }

    fn open_inner(
        path: &Path,
        get_key: impl FnOnce(&[u8], KdfParams) -> Result<Zeroizing<[u8; 32]>, VaultError>,
    ) -> Result<Self, VaultError> {
        let bytes = fs::read(path)?;
        Self::decrypt_bytes(path, &bytes, get_key)
    }

    fn decrypt_bytes(
        path: &Path,
        bytes: &[u8],
        get_key: impl FnOnce(&[u8], KdfParams) -> Result<Zeroizing<[u8; 32]>, VaultError>,
    ) -> Result<Self, VaultError> {
        if bytes.len() < HEADER_LEN || &bytes[0..4] != MAGIC {
            return Err(VaultError::Corrupt("not a myRDCcenter vault".into()));
        }
        if bytes[4] != FORMAT_VERSION {
            return Err(VaultError::Corrupt(format!("unsupported version {}", bytes[4])));
        }
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let kdf = KdfParams { m: u32_at(5), t: u32_at(9), p: u32_at(13) };
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&bytes[17..17 + SALT_LEN]);
        let nonce = XNonce::from_slice(&bytes[17 + SALT_LEN..HEADER_LEN]);

        let key = get_key(&salt, kdf)?;
        let cipher = XChaCha20Poly1305::new(key.as_ref().into());
        let plain = Zeroizing::new(
            cipher
                .decrypt(nonce, Payload { msg: &bytes[HEADER_LEN..], aad: &bytes[..HEADER_LEN] })
                .map_err(|_| VaultError::WrongPassword)?,
        );
        let data: VaultData =
            serde_json::from_slice(&plain).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        Ok(Vault { path: path.to_path_buf(), key, salt, kdf, data })
    }

    /// Encrypt `data` on its own with `password`, in the vault file format.
    /// Used for export files that are moved to another computer.
    pub fn seal(data: &VaultData, password: &str) -> Result<Vec<u8>, VaultError> {
        let kdf = KdfParams::default();
        let salt = random_bytes::<SALT_LEN>()?;
        let key = derive_key(password, &salt, kdf)?;
        let tmp = Vault { path: PathBuf::new(), key, salt, kdf, data: data.clone() };
        tmp.encrypt()
    }

    /// Decrypt bytes made by [`Vault::seal`] (or a vault/backup file).
    pub fn unseal(bytes: &[u8], password: &str) -> Result<VaultData, VaultError> {
        let mut v = Self::decrypt_bytes(Path::new(""), bytes, |salt, kdf| derive_key(password, salt, kdf))?;
        Ok(std::mem::take(&mut v.data))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Encrypt and write the vault. Writes to a temp file first, then renames,
    /// so a crash never leaves a half-written vault.
    pub fn save(&self) -> Result<(), VaultError> {
        let bytes = self.encrypt()?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("rdsc.tmp");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Write an encrypted copy (same master password) to another file.
    pub fn export_backup(&self, dest: &Path) -> Result<(), VaultError> {
        fs::write(dest, self.encrypt()?)?;
        Ok(())
    }

    /// Re-key the vault with a new master password and save it.
    pub fn change_password(&mut self, new_password: &str) -> Result<(), VaultError> {
        let remembered = self.is_remembered();
        let salt = random_bytes::<SALT_LEN>()?;
        let kdf = KdfParams::default();
        self.key = derive_key(new_password, &salt, kdf)?;
        self.salt = salt;
        self.kdf = kdf;
        self.save()?;
        if remembered {
            // Keep "remember on this computer" working with the new key.
            let _ = self.set_remembered(true);
        }
        Ok(())
    }

    fn encrypt(&self) -> Result<Vec<u8>, VaultError> {
        let nonce_bytes = random_bytes::<NONCE_LEN>()?;
        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(MAGIC);
        header.push(FORMAT_VERSION);
        header.extend_from_slice(&self.kdf.m.to_le_bytes());
        header.extend_from_slice(&self.kdf.t.to_le_bytes());
        header.extend_from_slice(&self.kdf.p.to_le_bytes());
        header.extend_from_slice(&self.salt);
        header.extend_from_slice(&nonce_bytes);

        let plain = Zeroizing::new(
            serde_json::to_vec(&self.data).map_err(|e| VaultError::Crypto(e.to_string()))?,
        );
        let cipher = XChaCha20Poly1305::new(self.key.as_ref().into());
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce_bytes), Payload { msg: &plain, aad: &header })
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        header.extend_from_slice(&ct);
        Ok(header)
    }
}

fn derive_key(password: &str, salt: &[u8], kdf: KdfParams) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let params = Params::new(kdf.m, kdf.t, kdf.p, Some(32)).map_err(|e| VaultError::Crypto(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| VaultError::Crypto(e.to_string()))?;
    Ok(key)
}

fn random_bytes<const N: usize>() -> Result<[u8; N], VaultError> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|e| VaultError::Crypto(e.to_string()))?;
    Ok(buf)
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.salt.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Connection;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rdsc-test-{}-{name}.rdsc", std::process::id()))
    }

    #[test]
    fn roundtrip_and_wrong_password() {
        let path = tmp_path("rt");
        let mut v = Vault::create(&path, "secret").unwrap();
        let mut c = Connection::new("Server 1", "Work");
        c.host = "10.0.0.5".into();
        c.password = crate::model::Secret::new("p@ss");
        v.data.connections.push(c);
        v.save().unwrap();

        let raw = fs::read(&path).unwrap();
        assert!(!raw.windows(4).any(|w| w == b"p@ss"), "password must not be in plain text");

        let v2 = Vault::open(&path, "secret").unwrap();
        assert_eq!(v2.data.connections[0].host, "10.0.0.5");
        assert!(matches!(Vault::open(&path, "nope"), Err(VaultError::WrongPassword)));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn change_password() {
        let path = tmp_path("cp");
        let mut v = Vault::create(&path, "old").unwrap();
        v.change_password("new").unwrap();
        assert!(Vault::open(&path, "old").is_err());
        assert!(Vault::open(&path, "new").is_ok());
        fs::remove_file(&path).ok();
    }

    #[test]
    fn tampering_is_detected() {
        let path = tmp_path("tamper");
        Vault::create(&path, "pw").unwrap();
        let mut raw = fs::read(&path).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        fs::write(&path, raw).unwrap();
        assert!(Vault::open(&path, "pw").is_err());
        fs::remove_file(&path).ok();
    }
}
