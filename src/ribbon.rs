//! Footer key hints. Ribbons are Zellij's own mode-indicator component, so they
//! follow the user's theme rather than the fixed colours the panel once used.

/// One footer entry: the key to press, and what it does.
pub(crate) struct Hint {
    pub(crate) key: &'static str,
    pub(crate) action: &'static str,
}

impl Hint {
    pub(crate) const fn new(key: &'static str, action: &'static str) -> Self {
        Hint { key, action }
    }

    /// No angle brackets: they crowd the enter glyph into an unreadable smudge.
    /// Colour separates the key from the action instead.
    pub(crate) fn text(&self) -> String {
        format!("{} {}", self.key, self.action)
    }

    /// **Character** offsets, which is what Zellij's colour ranges index. The
    /// enter glyph is 3 bytes, so `key.len()` would bleed the colour into the
    /// action word.
    pub(crate) fn key_range(&self) -> std::ops::Range<usize> {
        0..crate::style::chars(self.key)
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

/// Shown while the selected agent is blocked on you, so the keys that type into
/// its pane appear only when there is a prompt there to answer.
pub(crate) const REPLY_HINTS: &[Hint] = &[
    Hint::new("y", "yes"),
    Hint::new("m", "message"),
    Hint::new("\u{21b5}", "jump"),
    Hint::new("x", "kill"),
    Hint::new("q", "hide"),
];

/// The one-line editor owns the keyboard while it is up.
pub(crate) const REPLY_EDIT_HINTS: &[Hint] = &[Hint::new("\u{21b5}", "send"), Hint::new("esc", "cancel")];

/// Shown only while the selected agent has a permission prompt parked. Keeping
/// approve and reject out of the default footer means they cannot be pressed
/// by muscle memory when no prompt is waiting.
pub(crate) const ASK_HINTS: &[Hint] = &[
    Hint::new("a", "approve"),
    Hint::new("r", "reject"),
    Hint::new("\u{21b5}", "jump"),
    Hint::new("x", "kill"),
    Hint::new("q", "hide"),
];

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

/// A space each side of every segment, plus the two joining arrow glyphs.
const RIBBON_PADDING: usize = 4;

pub(crate) fn ribbon_width(hints: &[Hint]) -> usize {
    hints.iter().map(|h| h.text().chars().count() + RIBBON_PADDING).sum()
}

/// Fallback for panes too narrow for ribbons, which drop whole segments from
/// the middle rather than truncating, silently losing a key.
pub(crate) fn plain_line(hints: &[Hint]) -> String {
    let joined: Vec<String> = hints.iter().map(|h| format!("{} {}", h.key, h.action)).collect();
    format!(" {}", joined.join("  "))
}

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

    /// The highlighted range is in characters and must cover exactly the key,
    /// stopping before the separating space. A byte-length range would spill
    /// into the action word for the multi-byte enter glyph.
    #[test]
    fn key_range_covers_the_key_only() {
        for h in LIST_HINTS
            .iter()
            .chain(INSTALL_HINTS)
            .chain(SETUP_HINTS)
            .chain(ASK_HINTS)
            .chain(REPLY_HINTS)
            .chain(REPLY_EDIT_HINTS)
        {
            let text = h.text();
            let r = h.key_range();
            let covered: String = text.chars().skip(r.start).take(r.end - r.start).collect();
            assert_eq!(covered, h.key, "range must cover exactly the key in {:?}", text);
            assert_eq!(
                text.chars().nth(r.end),
                Some(' '),
                "the char after the range must be the separating space in {:?}",
                text
            );
        }
    }

    #[test]
    fn every_hint_has_a_distinct_key() {
        for set in [LIST_HINTS, INSTALL_HINTS, ASK_HINTS, REPLY_HINTS, REPLY_EDIT_HINTS] {
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
        for (name, set) in [
            ("list", LIST_HINTS),
            ("setup", SETUP_HINTS),
            ("install", INSTALL_HINTS),
            ("ask", ASK_HINTS),
            ("reply", REPLY_HINTS),
            ("reply-edit", REPLY_EDIT_HINTS),
        ] {
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
