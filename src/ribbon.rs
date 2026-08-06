//! The footer key hints, rendered as Zellij ribbons.
//!
//! Ribbons are the same component Zellij uses for its own mode indicators, so
//! they pick up the user's theme instead of the fixed 256-colour codes the rest
//! of the panel falls back to. Each action is its own ribbon segment with the
//! key highlighted, which is what makes the actions distinguishable at a glance.

/// One footer entry: the key to press, and what it does.
pub(crate) struct Hint {
    pub(crate) key: &'static str,
    pub(crate) action: &'static str,
}

impl Hint {
    pub(crate) const fn new(key: &'static str, action: &'static str) -> Self {
        Hint { key, action }
    }

    /// `key action`. No angle brackets: the ribbon already reads as a discrete
    /// chip, and the brackets crowded the enter glyph badly enough to look like
    /// one smudged character. The key is told apart by colour instead.
    pub(crate) fn text(&self) -> String {
        format!("{} {}", self.key, self.action)
    }

    /// Byte range covering the key, for `color_range`.
    ///
    /// `Text::serialize` encodes via `as_bytes()` while the indices are built as
    /// a plain range, so these are **byte** offsets. Keys are ASCII, except the
    /// enter glyph, so the range is computed from the encoded length rather than
    /// the character count.
    pub(crate) fn key_range(&self) -> std::ops::Range<usize> {
        0..self.key.len()
    }
}

pub(crate) const LIST_HINTS: &[Hint] = &[
    Hint::new("\u{21b5}", "jump"),
    Hint::new("1-9", "quick"),
    Hint::new("x", "kill"),
    Hint::new("d", "dismiss"),
    Hint::new("i", "install"),
    Hint::new("q", "hide"),
];

/// Columns Zellij pads around each ribbon segment (one space each side, plus
/// the two arrow glyphs that join them).
const RIBBON_PADDING: usize = 4;

/// Columns a hint set needs when drawn as ribbons.
pub(crate) fn ribbon_width(hints: &[Hint]) -> usize {
    hints.iter().map(|h| h.text().chars().count() + RIBBON_PADDING).sum()
}

/// The same hints as one plain line, for panes too narrow for ribbons.
///
/// Zellij drops whole ribbon segments that don't fit rather than truncating,
/// and it drops from the middle, so an over-wide set silently loses a key. The
/// plain row keeps every key visible at the cost of the themed styling.
pub(crate) fn plain_line(hints: &[Hint]) -> String {
    let joined: Vec<String> = hints.iter().map(|h| format!("{} {}", h.key, h.action)).collect();
    format!(" {}", joined.join("  "))
}

pub(crate) const SETUP_HINTS: &[Hint] = &[
    Hint::new("1", "claude"),
    Hint::new("2", "codex"),
    Hint::new("3", "both"),
    Hint::new("q", "quit"),
];

pub(crate) const INSTALL_HINTS: &[Hint] = &[
    Hint::new("c", "claude"),
    Hint::new("x", "codex"),
    Hint::new("p", "plugin"),
    Hint::new("r", "refresh"),
    Hint::new("esc", "back"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// No angle brackets: they crowded the enter glyph into an unreadable
    /// smudge. Colour separates the key from the action instead.
    #[test]
    fn hint_text_is_the_bare_key_and_action() {
        assert_eq!(Hint::new("x", "kill").text(), "x kill");
        assert_eq!(Hint::new("1-9", "quick").text(), "1-9 quick");
        assert_eq!(Hint::new("\u{21b5}", "jump").text(), "\u{21b5} jump");
    }

    /// The highlighted range must cover exactly the key and stop before the
    /// space, and must land on a char boundary so multi-byte keys don't corrupt
    /// the serialized payload.
    #[test]
    fn key_range_covers_the_key_only() {
        for h in LIST_HINTS.iter().chain(INSTALL_HINTS).chain(SETUP_HINTS) {
            let text = h.text();
            let r = h.key_range();
            assert!(
                text.is_char_boundary(r.start) && text.is_char_boundary(r.end),
                "range {:?} splits a char in {:?}",
                r,
                text
            );
            assert_eq!(&text[r.clone()], h.key, "range must cover exactly the key");
            assert_eq!(
                text.as_bytes().get(r.end),
                Some(&b' '),
                "the byte after the range must be the separating space in {:?}",
                text
            );
        }
    }

    #[test]
    fn every_hint_has_a_distinct_key() {
        for set in [LIST_HINTS, INSTALL_HINTS] {
            let mut keys: Vec<&str> = set.iter().map(|h| h.key).collect();
            keys.sort_unstable();
            let before = keys.len();
            keys.dedup();
            assert_eq!(keys.len(), before, "duplicate key in a hint set");
        }
    }

    /// The plain fallback is what a narrow pane actually shows, so it has to be
    /// narrower than the ribbons it replaces and still name every key.
    #[test]
    fn plain_fallback_is_narrower_and_keeps_every_key() {
        for set in [LIST_HINTS, SETUP_HINTS, INSTALL_HINTS] {
            let plain = plain_line(set);
            assert!(
                plain.chars().count() < ribbon_width(set),
                "the fallback must save columns: {:?}",
                plain
            );
            for h in set {
                assert!(plain.contains(h.key), "missing key {:?} in {:?}", h.key, plain);
                assert!(plain.contains(h.action), "missing action {:?}", h.action);
            }
        }
    }

    /// The README documents this exact footer, and it is the only place the
    /// install screen is advertised.
    #[test]
    fn list_footer_matches_the_documented_row() {
        assert_eq!(
            plain_line(LIST_HINTS),
            " \u{21b5} jump  1-9 quick  x kill  d dismiss  i install  q hide"
        );
    }

    /// Dropping the angle brackets bought back two columns per hint, which is
    /// what lets the full list footer render as ribbons in a typical floating
    /// pane instead of falling back to plain text.
    #[test]
    fn every_hint_row_fits_a_typical_pane_as_ribbons() {
        for (name, set) in [("list", LIST_HINTS), ("setup", SETUP_HINTS), ("install", INSTALL_HINTS)] {
            assert!(
                ribbon_width(set) <= 72,
                "{} hints need {} columns; Zellij would silently drop one",
                name,
                ribbon_width(set)
            );
        }
    }

    /// The footer is the only discoverability surface for these keys, so every
    /// key the list screen handles should appear in it.
    #[test]
    fn list_hints_cover_the_documented_keys() {
        let keys: Vec<&str> = LIST_HINTS.iter().map(|h| h.key).collect();
        for expect in ["x", "d", "i", "q", "1-9"] {
            assert!(keys.contains(&expect), "missing hint for {:?}", expect);
        }
    }
}
