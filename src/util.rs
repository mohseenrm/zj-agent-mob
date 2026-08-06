//! Formatting helpers.

pub(crate) fn fmt_elapsed(secs: f64) -> String {
    if secs < 0.0 || !secs.is_finite() {
        return String::new();
    }
    let s = secs as u64;
    if s < 60 {
        format!("{}s", s)
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// Moves a cursor by `delta`, wrapping at both ends.
pub(crate) fn wrap(i: usize, delta: isize, len: usize) -> usize {
    let n = len as isize;
    (((i as isize + delta) % n + n) % n) as usize
}

/// Truncates on char boundaries; summaries can contain non-ASCII.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Decoding helpers for the DCS payloads Zellij's UI components emit. Tests
/// read rendered rows through these rather than asserting on raw byte lists.
#[cfg(test)]
pub(crate) mod testing {
    use zellij_tile::prelude::{NestedListItem, Text};

    /// Both components serialize the same payload shape, so the decoders below
    /// take either one.
    pub(crate) trait Serializable {
        fn payload(&self) -> String;
    }
    impl Serializable for Text {
        fn payload(&self) -> String {
            self.serialize()
        }
    }
    impl Serializable for NestedListItem {
        fn payload(&self) -> String {
            self.serialize()
        }
    }

    /// An item serializes as `|`-indent, then `z`/`x` flags, then `$`-separated
    /// colour index groups, then the text as comma-separated bytes.
    pub(crate) fn item_text<T: Serializable>(item: &T) -> String {
        let s = item.payload();
        let body = s.trim_start_matches('|').trim_start_matches(['z', 'x']);
        // Everything up to the last `$` is index groups, not text.
        let payload = match body.rfind('$') {
            Some(at) => &body[at + 1..],
            None => body,
        };
        let bytes: Vec<u8> = payload.split(',').filter_map(|b| b.trim().parse::<u8>().ok()).collect();
        String::from_utf8_lossy(&bytes).to_string()
    }

    pub(crate) fn is_selected<T: Serializable>(item: &T) -> bool {
        // `z` (opaque) is prepended before `x`, so skip it first.
        item.payload()
            .trim_start_matches('|')
            .trim_start_matches('z')
            .starts_with('x')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("日本語テスト", 3), "日本…");
        assert_eq!(truncate("émoji→ok", 4), "émo…");
        assert_eq!(truncate("abc", 1), "…");
        assert_eq!(truncate("abc", 0), "…");
    }

    #[test]
    fn elapsed_formats_by_magnitude() {
        assert_eq!(fmt_elapsed(0.0), "0s");
        assert_eq!(fmt_elapsed(45.0), "45s");
        assert_eq!(fmt_elapsed(134.0), "2m14s");
        assert_eq!(fmt_elapsed(3600.0), "1h00m");
        assert_eq!(fmt_elapsed(-5.0), "", "negative clock skew renders blank");
        assert_eq!(fmt_elapsed(f64::NAN), "");
    }
}
