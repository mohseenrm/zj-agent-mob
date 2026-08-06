//! Small formatting helpers.

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

/// Truncate on character boundaries, not bytes: summaries can contain non-ASCII.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
        // Multi-byte: must not panic or split a char.
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
