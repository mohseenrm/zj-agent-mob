//! Colour levels for Zellij's UI components.
//!
//! Nothing here is raw ANSI any more. Every row is a `Text` component, so
//! colours resolve from the user's theme rather than fixed 256-colour codes,
//! and Zellij owns the cursor positioning that hand-written escapes used to
//! fight with.

/// Zellij's colour-index level that renders dim. Levels 0-3 are the theme's
/// four accent colours; 4 is dim and 5 is unbold. `Text` exposes `dim_range`,
/// but `NestedListItem` only forwards `color_range`, so the number is needed.
pub(crate) const DIM_LEVEL: usize = 4;
