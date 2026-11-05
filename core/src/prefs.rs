//! Settings of this computer, not of the vault.
//!
//! They are in `prefs.json` in the app data folder (next to the default vault),
//! not encrypted, and not in exports. They also work when the vault is locked.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Text sizes in percent, on top of the Windows / macOS display scale.
pub const TEXT_SIZES: [u32; 10] = [70, 80, 90, 100, 110, 125, 150, 175, 200, 250];
pub const DEFAULT_TEXT_SIZE: u32 = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Text size in percent. The whole UI scales with it.
    pub text_size: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { text_size: DEFAULT_TEXT_SIZE }
    }
}

impl Prefs {
    /// The prefs of this computer, or the defaults when there is no (valid) file.
    pub fn load() -> Self {
        // Screenshots for the docs are always at 100 %, unless a prefs file is given.
        #[cfg(feature = "screenshot")]
        if crate::screenshot::request().is_some() && std::env::var_os("MYRDCCENTER_PREFS").is_none() {
            return Self::default();
        }
        let mut p: Self = path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        p.text_size = nearest_text_size(p.text_size);
        p
    }

    pub fn save(&self) -> Result<(), String> {
        let path = path().ok_or("No app data folder")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    /// The text size as a factor (1.0 = 100 %).
    pub fn zoom(&self) -> f32 {
        self.text_size as f32 / 100.0
    }

    /// Go one text size up (`steps > 0`) or down (`steps < 0`).
    /// Returns true if the size changed.
    pub fn step_text_size(&mut self, steps: i32) -> bool {
        let i = TEXT_SIZES.iter().position(|&s| s == self.text_size).unwrap_or(3) as i32;
        let new = TEXT_SIZES[(i + steps).clamp(0, TEXT_SIZES.len() as i32 - 1) as usize];
        let changed = new != self.text_size;
        self.text_size = new;
        changed
    }

    /// Back to 100 %. Returns true if the size changed.
    pub fn reset_text_size(&mut self) -> bool {
        let changed = self.text_size != DEFAULT_TEXT_SIZE;
        self.text_size = DEFAULT_TEXT_SIZE;
        changed
    }

    /// Short label for the toolbar, e.g. "125 %".
    pub fn text_size_label(&self) -> String {
        format!("{} %", self.text_size)
    }
}

/// What a keyboard shortcut does with the text size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSizeKey {
    Larger,
    Smaller,
    Reset,
}

impl TextSizeKey {
    /// Ctrl (Cmd on macOS) with `+` / `=`, `-`, or `0`. `text` is the key's character.
    pub fn from_text(text: &str) -> Option<Self> {
        match text {
            "+" | "=" => Some(Self::Larger),
            "-" | "_" => Some(Self::Smaller),
            "0" => Some(Self::Reset),
            _ => None,
        }
    }

    /// Apply to `prefs` and save. Returns true if the size changed.
    pub fn apply(self, prefs: &mut Prefs) -> bool {
        let changed = match self {
            Self::Larger => prefs.step_text_size(1),
            Self::Smaller => prefs.step_text_size(-1),
            Self::Reset => prefs.reset_text_size(),
        };
        if changed {
            let _ = prefs.save();
        }
        changed
    }
}

fn nearest_text_size(v: u32) -> u32 {
    *TEXT_SIZES.iter().min_by_key(|&&s| s.abs_diff(v)).unwrap()
}

/// `prefs.json` in the app data folder. Always on this computer, also when
/// `MYRDCCENTER_VAULT` points to a vault somewhere else.
fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MYRDCCENTER_PREFS").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    Some(dirs::data_local_dir()?.join("myRDCcenter").join("prefs.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_stop_at_the_ends() {
        let mut p = Prefs::default();
        assert!(p.step_text_size(1));
        assert_eq!(p.text_size, 110);
        assert!(p.step_text_size(-2));
        assert_eq!(p.text_size, 90);
        for _ in 0..20 {
            p.step_text_size(1);
        }
        assert_eq!(p.text_size, 250);
        assert!(!p.step_text_size(1));
        assert!(p.reset_text_size());
        assert_eq!(p.text_size, 100);
        assert!(!p.reset_text_size());
    }

    #[test]
    fn odd_values_snap_to_a_step() {
        assert_eq!(nearest_text_size(0), 70);
        assert_eq!(nearest_text_size(120), 125);
        assert_eq!(nearest_text_size(9999), 250);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let p: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, Prefs::default());
    }

    #[test]
    fn keys() {
        assert_eq!(TextSizeKey::from_text("="), Some(TextSizeKey::Larger));
        assert_eq!(TextSizeKey::from_text("-"), Some(TextSizeKey::Smaller));
        assert_eq!(TextSizeKey::from_text("0"), Some(TextSizeKey::Reset));
        assert_eq!(TextSizeKey::from_text("a"), None);
    }
}
