//! The fuzzy matcher behind the `/` prompt. Hand-rolled rather than a crate:
//! the runtime dependency tree is exactly `zellij-tile`, the release profile is
//! tuned for wasm size, and a subsequence scorer is small enough to own.

/// Smartcase, as vim spells it: an all-lowercase query matches any case, one
/// capital makes the whole query exact.
fn is_case_sensitive(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

/// Chars that start a word inside a path or a task summary, so `/wt1` can rank
/// `repo/wt1` above `sawtooth`.
fn is_boundary(prev: char) -> bool {
    matches!(prev, '/' | '-' | '_' | '.' | ' ' | ':')
}

/// `Some(score)` when every query char appears in order in `haystack`, higher
/// meaning a better match. Greedy left-to-right over chars, not bytes: rows
/// hold non-ASCII (icons, accented paths), and a byte walk would split them.
///
/// An empty query matches everything at score zero, which is what lets the
/// prompt open onto the full list before anything is typed.
pub(crate) fn score(query: &str, haystack: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let fold = |s: &str| -> Vec<char> {
        match is_case_sensitive(query) {
            true => s.chars().collect(),
            false => s.chars().flat_map(char::to_lowercase).collect(),
        }
    };
    let hay = fold(haystack);
    let mut total: u32 = 0;
    let mut from = 0usize;
    let mut last: Option<usize> = None;
    for qc in fold(query) {
        let at = (from..hay.len()).find(|&i| hay[i] == qc)?;
        // A run of adjacent matches reads as "the word I typed"; a match that
        // starts a word beats one buried mid-word.
        let consecutive = last == Some(at.wrapping_sub(1));
        let boundary = at == 0 || is_boundary(hay[at - 1]);
        total += 1 + 2 * u32::from(consecutive) + 3 * u32::from(boundary);
        last = Some(at);
        from = at + 1;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn every_query_char_must_appear_in_order() {
        assert!(score("auth", "feat-auth-worktree").is_some());
        assert!(score("htua", "feat-auth-worktree").is_none());
        assert!(score("authz", "feat-auth-worktree").is_none());
    }

    #[test]
    fn an_empty_query_matches_everything() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("", ""), Some(0));
        assert!(score("a", "").is_none());
    }

    #[test]
    fn a_boundary_match_outscores_a_mid_word_one() {
        let boundary = score("wt", "repo/wt1").unwrap();
        let buried = score("wt", "sawtooth").unwrap();
        assert!(boundary > buried, "{} <= {}", boundary, buried);
    }

    #[test]
    fn a_consecutive_run_outscores_a_scattered_one() {
        let run = score("auth", "xauthx").unwrap();
        let scattered = score("auth", "xaxuxtxhx").unwrap();
        assert!(run > scattered, "{} <= {}", run, scattered);
    }

    #[test]
    fn smartcase_ignores_case_until_a_capital() {
        assert!(score("auth", "Feat-AUTH").is_some());
        assert!(score("AUTH", "Feat-AUTH").is_some());
        assert!(score("Auth", "feat-auth").is_none());
    }

    #[test]
    fn non_ascii_haystacks_match_by_char() {
        assert!(score("café", "fix café menu").is_some());
        assert!(score("cafm", "fix café menu").is_some());
    }
}
