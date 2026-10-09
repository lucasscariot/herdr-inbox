//! Fuzzy ranking for pickers: a few typed letters reach a long model id.
//!
//! An exact prefix wins, then a match at a word start, then any substring,
//! then the letters in order with gaps. Fewer gaps and earlier matches rank
//! higher; ties keep the original order.

/// Lower is better; `None` means no match. Case-insensitive.
pub fn score(query: &str, text: &str) -> Option<u32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    if let Some(position) = text.find(&query) {
        if position == 0 {
            return Some(0);
        }
        let before = text[..position].chars().last();
        let at_word = before.is_some_and(|c| " -_/.:·".contains(c));
        return Some(if at_word { 1 } else { 2_000 + position as u32 });
    }
    let chars: Vec<char> = text.chars().collect();
    let mut cursor = 0;
    let mut gaps = 0;
    let mut previous: Option<usize> = None;
    for wanted in query.chars() {
        let found = chars[cursor..].iter().position(|&c| c == wanted)? + cursor;
        if previous.is_some_and(|p| found != p + 1) {
            gaps += 1;
        }
        previous = Some(found);
        cursor = found + 1;
    }
    Some(10_000 + gaps * 1_000 + previous.unwrap_or(0) as u32)
}

/// The items matching `query`, best first; equal scores keep their order.
pub fn rank<'a, T>(query: &str, items: &'a [T], key: impl Fn(&T) -> String) -> Vec<&'a T> {
    let mut scored: Vec<(u32, usize, &T)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| score(query, &key(item)).map(|s| (s, index, item)))
        .collect();
    scored.sort_by_key(|(score, index, _)| (*score, *index));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_beats_word_start_beats_substring_beats_subsequence() {
        let prefix = score("open", "opencode/big-pickle").unwrap();
        let word = score("big", "opencode/big-pickle").unwrap();
        let inner = score("pick", "opencode/big-pickle").unwrap();
        let scattered = score("bgpk", "opencode/big-pickle").unwrap();
        assert!(prefix < word, "{prefix} {word}");
        assert!(word < inner || inner == 1, "pick follows a dash, so it is a word start too");
        assert!(score("ode", "opencode").unwrap() > word, "a mid-word substring ranks below a word start");
        assert!(word < scattered && inner < scattered);
    }

    #[test]
    fn subsequences_with_fewer_gaps_rank_higher() {
        let tight = score("gpt", "gpt-5").unwrap();
        let loose = score("gpt", "great-pumpkin-tart").unwrap();
        assert!(tight < loose);
    }

    #[test]
    fn no_match_and_empty_queries() {
        assert_eq!(score("xyz", "claude"), None);
        assert_eq!(score("claudex", "claude"), None, "every letter must be present");
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("  ", "anything"), Some(0));
    }

    #[test]
    fn matching_ignores_case_and_handles_unicode() {
        assert_eq!(score("OPUS", "claude-opus"), Some(1));
        assert!(score("été", "Résumé de l'été").is_some());
        assert!(score("rsm", "Résumé").is_some(), "a subsequence across accented letters");
        assert_eq!(score("rme", "Résumé"), None, "é is not e");
    }

    #[test]
    fn rank_orders_by_score_and_keeps_ties_stable() {
        let items = ["sonnet", "opus", "haiku", "fable", "opus-mini"];
        let ranked: Vec<&&str> = rank("o", &items, |s| s.to_string());
        assert_eq!(ranked, [&"opus", &"opus-mini", &"sonnet"]);
        let all: Vec<&&str> = rank("", &items, |s| s.to_string());
        assert_eq!(all.len(), items.len(), "an empty query keeps everything in order");
        assert_eq!(*all[0], "sonnet");
    }
}
