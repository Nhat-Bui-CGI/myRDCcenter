//! Screenshots of the apps for the documentation.
//!
//! Only built with the `screenshot` feature. Start an app with:
//!
//! - `MYRDCCENTER_VAULT`: a demo vault (see `examples/demo_vault.rs`)
//! - `MYRDCCENTER_SCREENSHOT`: the PNG file to write
//! - `MYRDCCENTER_SELECT`: optional, the name of the connection to select first
//!
//! The app opens the vault, selects the connection, saves the PNG and exits.

use std::path::{Path, PathBuf};

/// Window size (logical pixels) used for screenshots.
pub const SIZE: (f32, f32) = (1180.0, 720.0);

pub struct Request {
    pub out: PathBuf,
    pub select: Option<String>,
}

/// The screenshot that was asked for in the environment, if any.
pub fn request() -> Option<Request> {
    let out = std::env::var_os("MYRDCCENTER_SCREENSHOT").filter(|p| !p.is_empty())?;
    let select = std::env::var("MYRDCCENTER_SELECT").ok().filter(|s| !s.is_empty());
    Some(Request { out: PathBuf::from(out), select })
}

/// Write RGBA8 pixels (row by row, top to bottom) to a PNG file.
pub fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(rgba).map_err(|e| e.to_string())
}
