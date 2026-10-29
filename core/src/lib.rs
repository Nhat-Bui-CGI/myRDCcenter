//! Shared core for RDS-center: data model, encrypted vault and RDP launching.
//! Both UI front-ends (egui and Slint) build on this crate.

pub mod idle;
pub mod model;
pub mod rdp;
pub mod vault;

pub use model::{Connection, Secret, Settings, VaultData};
pub use vault::{default_vault_path, Vault, VaultError};
