//! Every glyph the panel draws, in one table.

pub(crate) struct Icons {
    pub(crate) waiting: &'static str,
    pub(crate) idle_wait: &'static str,
    pub(crate) failed: &'static str,
    pub(crate) done: &'static str,
    pub(crate) idle: &'static str,
    pub(crate) discovered: &'static str,
    pub(crate) gone: &'static str,
    pub(crate) compact: &'static str,
    pub(crate) marker: &'static str,
    pub(crate) notified: &'static str,
    pub(crate) pinned: &'static str,
    pub(crate) subagent: &'static str,
    pub(crate) installed: &'static str,
    pub(crate) absent: &'static str,
    pub(crate) busy: &'static str,
    pub(crate) spinner: &'static [&'static str],
}

pub(crate) const NERD: Icons = Icons {
    waiting: "\u{f140}",
    idle_wait: "\u{eaa2}",
    failed: "\u{f530}",
    done: "\u{ebb3}",
    idle: "\u{f186}",
    discovered: "\u{ea6d}",
    gone: "\u{eb32}",
    compact: "\u{ea98}",
    marker: "\u{eb70}",
    notified: "\u{f444}",
    pinned: "\u{f08d}",
    subagent: "\u{ebba}",
    installed: "\u{ebb3}",
    absent: "\u{eabc}",
    busy: "\u{ee06}",
    spinner: &["\u{ee06}", "\u{ee07}", "\u{ee08}", "\u{ee09}", "\u{ee0a}", "\u{ee0b}"],
};

pub(crate) const UNICODE: Icons = Icons {
    waiting: "\u{25cf}",
    idle_wait: "\u{25d0}",
    failed: "\u{2717}",
    done: "\u{2713}",
    idle: "\u{25cb}",
    discovered: "\u{25cc}",
    gone: "?",
    compact: "\u{25d1}",
    marker: "\u{25b6}",
    notified: "!",
    pinned: "^",
    subagent: "\u{2442}",
    installed: "\u{2713}",
    absent: "\u{25cb}",
    busy: "\u{2219}",
    spinner: &crate::SPINNER,
};

impl Default for &'static Icons {
    fn default() -> Self {
        &NERD
    }
}

pub(crate) fn from_config(spec: Option<&String>) -> &'static Icons {
    match spec.map(|s| s.as_str()) {
        Some("unicode") => &UNICODE,
        _ => &NERD,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_glyph(set: &'static Icons) -> Vec<(&'static str, &'static str)> {
        let mut v = vec![
            ("waiting", set.waiting),
            ("idle_wait", set.idle_wait),
            ("failed", set.failed),
            ("done", set.done),
            ("idle", set.idle),
            ("discovered", set.discovered),
            ("gone", set.gone),
            ("compact", set.compact),
            ("marker", set.marker),
            ("notified", set.notified),
            ("pinned", set.pinned),
            ("subagent", set.subagent),
            ("installed", set.installed),
            ("absent", set.absent),
            ("busy", set.busy),
        ];
        v.extend(set.spinner.iter().map(|f| ("spinner", *f)));
        v
    }

    /// Every column offset in a row is computed with `style::chars`, which counts
    /// characters. A glyph of two chars shifts every field to its right and the
    /// colour ranges land on the wrong word.
    #[test]
    fn every_icon_is_exactly_one_character() {
        for (name, set) in [("nerd", &NERD), ("unicode", &UNICODE)] {
            for (field, glyph) in every_glyph(set) {
                assert_eq!(
                    glyph.chars().count(),
                    1,
                    "{} icon {} is {:?}, which is {} chars",
                    name,
                    field,
                    glyph,
                    glyph.chars().count()
                );
            }
        }
    }

    /// A codepoint above U+FFFF is two UTF-16 units, and an East-Asian-Wide one
    /// takes two terminal cells. Either breaks the one-char-one-cell assumption
    /// the whole layout rests on, which is why `nf-md-*` and emoji are excluded.
    #[test]
    fn no_icon_is_wide_or_outside_the_bmp() {
        for (name, set) in [("nerd", &NERD), ("unicode", &UNICODE)] {
            for (field, glyph) in every_glyph(set) {
                let cp = glyph.chars().next().unwrap() as u32;
                assert!(cp <= 0xFFFF, "{} icon {} is U+{:05X}, outside the BMP", name, field, cp);
                let wide = (0x1100..=0x115F).contains(&cp)
                    || (0x2E80..=0xA4CF).contains(&cp)
                    || (0xAC00..=0xD7A3).contains(&cp)
                    || (0xF900..=0xFAFF).contains(&cp)
                    || (0xFE30..=0xFE6F).contains(&cp)
                    || (0xFF00..=0xFF60).contains(&cp)
                    || (0xFFE0..=0xFFE6).contains(&cp);
                assert!(!wide, "{} icon {} is U+{:05X}, double-width", name, field, cp);
            }
        }
    }

    /// `icons unicode` must not silently lose an icon, so the named fields are
    /// compared one for one. The spinners are deliberately different lengths -
    /// six Nerd frames against ten braille ones - so they are compared as
    /// "both non-empty" rather than by count.
    #[test]
    fn the_two_sets_have_the_same_shape() {
        let named = |set: &'static Icons| -> Vec<&'static str> {
            every_glyph(set)
                .into_iter()
                .filter(|(f, _)| *f != "spinner")
                .map(|(f, _)| f)
                .collect()
        };
        assert_eq!(named(&NERD), named(&UNICODE));
        assert!(!NERD.spinner.is_empty() && !UNICODE.spinner.is_empty());
    }

    /// Nerd Font glyphs live in the Private Use Area. A glyph that escaped it is
    /// a typo'd codepoint that would render as whatever the font has there.
    #[test]
    fn every_nerd_glyph_is_in_the_private_use_area() {
        for (field, glyph) in every_glyph(&NERD) {
            let cp = glyph.chars().next().unwrap() as u32;
            assert!(
                (0xE000..=0xF8FF).contains(&cp),
                "nerd icon {} is U+{:05X}, outside the PUA",
                field,
                cp
            );
        }
    }

    #[test]
    fn the_config_key_selects_a_set_and_defaults_to_nerd() {
        let pick = |v: Option<&str>| from_config(v.map(|s| s.to_string()).as_ref()).waiting;
        assert_eq!(pick(None), NERD.waiting, "no key means the nerd set");
        assert_eq!(pick(Some("nerd")), NERD.waiting);
        assert_eq!(pick(Some("unicode")), UNICODE.waiting);
        assert_eq!(
            pick(Some("nonsense")),
            NERD.waiting,
            "an unknown value is not a third set"
        );
    }

    /// No two status glyphs may be equal, and no gutter marker may reuse one.
    ///
    /// Exhaustive rather than a hand-listed set of pairs: a duplicate shipped
    /// once already (the notified gutter and `waiting` were both a bell, which
    /// put two bells on one row) and a list of pairs is exactly what missed it.
    #[test]
    fn no_two_glyphs_collide() {
        for (name, set) in [("nerd", &NERD), ("unicode", &UNICODE)] {
            let statuses = [
                ("waiting", set.waiting),
                ("idle_wait", set.idle_wait),
                ("failed", set.failed),
                ("done", set.done),
                ("idle", set.idle),
                ("discovered", set.discovered),
                ("gone", set.gone),
                ("compact", set.compact),
            ];
            for (i, (an, a)) in statuses.iter().enumerate() {
                for (bn, b) in statuses.iter().skip(i + 1) {
                    assert_ne!(a, b, "{} set: {} and {} are the same glyph", name, an, bn);
                }
            }
            // The gutter sits immediately left of the status icon, so a marker
            // that reuses a status glyph draws the same symbol twice on one row.
            for (gn, g) in [("notified", set.notified), ("pinned", set.pinned)] {
                for (sn, st) in statuses.iter() {
                    assert_ne!(&g, st, "{} set: gutter {} reuses the {} icon", name, gn, sn);
                }
            }
            assert_ne!(set.notified, set.pinned, "{} set: the two gutter markers match", name);
        }
    }
}
