//! Shared core for myRDCcenter: data model, encrypted vault and RDP launching.
//! Both UI front-ends (egui and Slint) build on this crate.

pub mod groups;
pub mod idle;
pub mod import;
pub mod import_rdm;
pub mod keystore;
pub mod model;
pub mod rdp;
pub mod vault;

pub use groups::TreeRow;
pub use model::{Connection, Secret, Settings, VaultData};
pub use vault::{default_vault_path, Vault, VaultError};
