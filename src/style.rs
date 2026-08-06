//! Raw ANSI, not Zellij's `Text`/ribbon components: those emit a DCS sequence
//! that moves the cursor itself, collapsing consecutive rows onto one line.
//! Cost is that these fixed colours ignore the user's theme.

pub(crate) const RESET: &str = "\u{1b}[0m";
pub(crate) const BOLD: &str = "\u{1b}[1m";
pub(crate) const DIM: &str = "\u{1b}[2m";
pub(crate) const RED: &str = "\u{1b}[38;5;203m";
pub(crate) const GREEN: &str = "\u{1b}[38;5;114m";
pub(crate) const BLUE: &str = "\u{1b}[38;5;75m";
pub(crate) const GREY: &str = "\u{1b}[38;5;245m";
pub(crate) const SEL_BG: &str = "\u{1b}[48;5;237m";

/// Zellij's colour-index level that renders dim. Levels 0-3 are the theme's
/// four accent colours; 4 is dim and 5 is unbold. `Text` exposes `dim_all`,
/// but `NestedListItem` only forwards `color_range`, so it needs the number.
pub(crate) const DIM_LEVEL: usize = 4;
