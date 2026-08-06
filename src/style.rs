//! Raw ANSI styling.
//!
//! The panel deliberately does not use Zellij's `Text`/ribbon UI components:
//! those serialize to a DCS sequence that repositions the cursor itself, so
//! consecutive components collapse onto a single grid row. Plain ANSI with one
//! `println!` per row gives exact line control.
//!
//! Trade-off: fixed 256-colour codes do not follow the user's Zellij theme the
//! way `Text::color_range()` indices would. Correct layout is worth more here.
//!
//! Note if you ever do reach for the component API: `color_range()` indices are
//! BYTE offsets (`Text::serialize()` encodes via `as_bytes()`), not character
//! offsets. Character counts corrupt the payload for multi-byte glyphs.

pub(crate) const RESET: &str = "\u{1b}[0m";
pub(crate) const BOLD: &str = "\u{1b}[1m";
pub(crate) const DIM: &str = "\u{1b}[2m";
pub(crate) const RED: &str = "\u{1b}[38;5;203m";
pub(crate) const GREEN: &str = "\u{1b}[38;5;114m";
pub(crate) const BLUE: &str = "\u{1b}[38;5;75m";
pub(crate) const GREY: &str = "\u{1b}[38;5;245m";
pub(crate) const SEL_BG: &str = "\u{1b}[48;5;237m";
