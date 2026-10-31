//! Fuzzy matching, ported from Ulauncher so results are ranked the same way.

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn lower(s: &str) -> Vec<char> {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// Score between 0 and 100 (Levenshtein based, boosted for word-start matches).
pub fn score(query: &str, text: &str) -> f32 {
    let q = lower(query);
    let t = lower(text);
    if q.is_empty() || t.is_empty() {
        return 0.0;
    }
    let ql = q.len() as f32;
    // Counter-weight the distance so short queries can match long texts.
    let diff = levenshtein(&q, &t) as f32 - (t.len().saturating_sub(q.len()) as f32 * 0.95);
    let mut score = 100.0 * (ql - diff).max(0.0) / ql;

    // +30 if the query fully matches the beginning of a word.
    let text_lower: String = t.iter().collect();
    let query_lower: String = q.iter().collect();
    if text_lower.split(' ').any(|w| w.starts_with(&query_lower)) {
        score += 30.0;
    }

    // +10 for each query char matching the start of the text or of a word.
    let mut i = 0;
    for (j, c) in t.iter().enumerate() {
        if i < q.len() && q[i] == *c {
            if j == 0 || " .(-_+)".contains(t[j - 1]) {
                score += 10.0;
            }
            i += 1;
        } else if i == q.len() {
            break;
        }
    }
    score.min(100.0)
}

/// Whether the query chars appear in order in `text` (ignoring spaces), allowing one
/// missing char for longer queries to tolerate typos. Filters out the far-fetched matches
/// a pure edit-distance score lets through.
pub fn plausible(query: &str, text: &str) -> bool {
    let q: Vec<char> = lower(query).into_iter().filter(|c| !c.is_whitespace()).collect();
    let t = lower(text);
    let allowed_missing = usize::from(q.len() >= 5);
    let mut pos = 0;
    let mut missing = 0;
    for c in &q {
        match t[pos..].iter().position(|x| x == c) {
            Some(i) => pos += i + 1,
            None => {
                missing += 1;
                if missing > allowed_missing {
                    return false;
                }
            }
        }
    }
    true
}

/// Positions (char indices into `text`) of the query chars, using the longest common
/// substring approach from Ulauncher. Used for highlighting.
pub fn matching_indexes(query: &str, text: &str) -> Vec<usize> {
    let q = lower(query);
    let t: Vec<char> = text.chars().collect();
    let tl: Vec<char> = t.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let (m, n) = (q.len(), tl.len());
    if m == 0 || n == 0 {
        return Vec::new();
    }
    let mut counter = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..m {
        for j in 0..n {
            if q[i] == tl[j] {
                counter[i + 1][j + 1] = counter[i][j] + 1;
            }
        }
    }
    let mut positions = std::collections::BTreeSet::new();
    let mut i = m;
    while i > 0 {
        // first maximum wins, like Python's max()
        let (mut best_j, mut best_c) = (0, 0);
        for (j, &c) in counter[i].iter().enumerate() {
            if c > best_c {
                best_j = j;
                best_c = c;
            }
        }
        if best_c > 0 {
            for p in best_j - best_c..best_j {
                positions.insert(p);
            }
            i -= best_c;
        } else {
            i -= 1;
        }
    }
    positions.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring() {
        assert!(score("fir", "Firefox") > score("fir", "Thunderbird"));
        assert!(score("fiwebr", "Firefox Web Browser") >= 50.0);
        assert_eq!(score("", "x"), 0.0);
    }

    #[test]
    fn plausibility() {
        assert!(plausible("fire", "Firefox"));
        assert!(plausible("fiwebr", "Firefox Web Browser"));
        assert!(!plausible("fire", "Bitwarden"));
        assert!(!plausible("Doc", "Desktop"));
        assert!(plausible("thunderbrid", "Thunderbird"));
    }

    #[test]
    fn indexes() {
        assert_eq!(matching_indexes("fox", "Firefox"), vec![4, 5, 6]);
        assert_eq!(matching_indexes("fiwe", "Firefox Web"), vec![0, 1, 8, 9]);
    }
}
