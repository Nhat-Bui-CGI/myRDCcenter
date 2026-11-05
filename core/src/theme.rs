//! Colours shared by both front-ends, so they look the same.
//!
//! Every top-level group gets its own colour from [`GROUP_COLORS`]. Its subgroups
//! and connections use the same colour, so you can see at a glance where a
//! connection belongs.

use crate::groups::TreeRow;

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Window background.
pub const BG: Rgb = Rgb(0xf4, 0xf5, 0xfb);
/// Side panel, toolbar and status bar.
pub const PANEL: Rgb = Rgb(0xff, 0xff, 0xff);
/// Cards, dialogs and input fields.
pub const CARD: Rgb = Rgb(0xff, 0xff, 0xff);
/// Hovered rows and buttons.
pub const HOVER: Rgb = Rgb(0xec, 0xea, 0xff);
/// Thin lines between areas.
pub const BORDER: Rgb = Rgb(0xe1, 0xe4, 0xef);
/// Normal text.
pub const TEXT: Rgb = Rgb(0x1b, 0x1e, 0x2e);
/// Secondary text.
pub const TEXT_WEAK: Rgb = Rgb(0x6a, 0x70, 0x8c);

/// Main accent: selection, primary buttons, focus.
pub const ACCENT: Rgb = Rgb(0x6d, 0x4a, 0xff);
/// The Connect button.
pub const CONNECT: Rgb = Rgb(0x05, 0x96, 0x69);
/// Delete buttons and errors.
pub const DANGER: Rgb = Rgb(0xe1, 0x1d, 0x48);
/// Confirmations such as "Saved".
pub const SUCCESS: Rgb = Rgb(0x10, 0xb9, 0x81);

/// Colours for top-level groups. Dark enough to read on white.
pub const GROUP_COLORS: &[Rgb] = &[
    Rgb(0x7c, 0x3a, 0xed), // violet
    Rgb(0x08, 0x91, 0xb2), // cyan
    Rgb(0xd9, 0x77, 0x06), // amber
    Rgb(0xdb, 0x27, 0x77), // pink
    Rgb(0x16, 0xa3, 0x4a), // green
    Rgb(0x25, 0x63, 0xeb), // blue
    Rgb(0xea, 0x58, 0x0c), // orange
    Rgb(0x93, 0x33, 0xea), // purple
];

/// Colour of connections that are not in any group.
pub const UNGROUPED: Rgb = Rgb(0x64, 0x74, 0x8b);

/// Colour of a group (or a connection in it), from its top-level group name.
/// The same name always gets the same colour.
pub fn group_color(path: &str) -> Rgb {
    let top = path.split('/').next().unwrap_or("").trim();
    if top.is_empty() {
        return UNGROUPED;
    }
    // FNV-1a: small, and stable between runs and versions.
    let mut h: u32 = 0x811c_9dc5;
    for b in top.to_lowercase().bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    GROUP_COLORS[h as usize % GROUP_COLORS.len()]
}

/// Colour of the dot in front of a status message: red for problems,
/// green while connecting, otherwise the "done" colour.
pub fn status_color(status: &str) -> Rgb {
    let s = status.to_lowercase();
    let bad = ["could not", "error", "failed", "not found", "don't match", "at least", "wrong", "denied"];
    if bad.iter().any(|b| s.contains(b)) {
        DANGER
    } else if s.starts_with("connecting") {
        CONNECT
    } else {
        SUCCESS
    }
}

/// Colour for each row of the tree, in the same order.
pub fn row_colors(rows: &[TreeRow]) -> Vec<Rgb> {
    let mut current = UNGROUPED;
    rows.iter()
        .map(|r| match r {
            TreeRow::Group { path, .. } => {
                current = group_color(path);
                current
            }
            TreeRow::Connection { depth: 0, .. } => UNGROUPED,
            TreeRow::Connection { .. } => current,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subgroups_share_the_top_colour() {
        assert_eq!(group_color("Work"), group_color("Work/Servers/DB"));
        assert_eq!(group_color(""), UNGROUPED);
    }
}
