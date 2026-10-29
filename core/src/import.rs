//! Import .rdp and .rdm files into a vault. Shared by both front-ends.

use std::path::PathBuf;

use uuid::Uuid;

use crate::import_rdm::{import_rdm, MergeStats, RdmImport};
use crate::model::VaultData;
use crate::rdp::import_rdp;

#[derive(Debug, Default)]
pub struct ImportReport {
    pub added: usize,
    pub duplicates: usize,
    pub passwords_skipped: usize,
    pub other_skipped: usize,
    pub errors: Vec<String>,
    /// Last connection added, to select it afterwards.
    pub last_id: Option<Uuid>,
}

impl ImportReport {
    /// One line for the status bar.
    pub fn message(&self) -> String {
        let mut parts = vec![format!("Imported {} connection(s)", self.added)];
        if self.duplicates > 0 {
            parts.push(format!("{} already existed", self.duplicates));
        }
        if self.passwords_skipped > 0 {
            parts.push(format!("{} password(s) need to be typed in again", self.passwords_skipped));
        }
        if self.other_skipped > 0 {
            parts.push(format!("{} non-RDP entr(ies) skipped", self.other_skipped));
        }
        if let Some(e) = self.errors.first() {
            parts.push(e.clone());
        }
        parts.join(". ")
    }
}

/// File types the Import button accepts.
pub const IMPORT_EXTENSIONS: &[&str] = &["rdp", "rdm"];

pub fn import_files(data: &mut VaultData, paths: &[PathBuf]) -> ImportReport {
    let mut report = ImportReport::default();
    for path in paths {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let result: Result<RdmImport, String> = if ext == "rdm" {
            import_rdm(path)
        } else {
            import_rdp(path)
                .map(|c| RdmImport { connections: vec![c], ..Default::default() })
                .map_err(|e| e.to_string())
        };
        match result {
            Ok(imp) => {
                report.passwords_skipped += imp.passwords_skipped;
                report.other_skipped += imp.other_skipped;
                let last = imp.connections.last().map(|c| c.id);
                let MergeStats { added, duplicates } = data.merge_import(imp);
                report.added += added;
                report.duplicates += duplicates;
                if added > 0 {
                    report.last_id = last.filter(|id| data.find(*id).is_some()).or(report.last_id);
                }
            }
            Err(e) => report.errors.push(format!("{}: {e}", path.display())),
        }
    }
    report
}
