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

/// Number of **characters** in `s`.
///
/// Every `color_range`/`dim_range` index Zellij consumes is a character offset,
/// not a byte offset - see `Text::color_substring`, which converts a byte
/// position with `chars().count()` before handing it to `color_range`. Using
/// `str::len` for a string containing the cursor marker, the spinner, or the
/// enter glyph shifts the range right by the extra UTF-8 bytes and colours only
/// part of the intended word.
pub(crate) fn chars(s: &str) -> usize {
    s.chars().count()
}
