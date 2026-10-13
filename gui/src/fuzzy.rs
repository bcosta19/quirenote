// Fuzzy matching for the quick switcher, the outline and [[link]] completion: the query
// letters must appear in order; accents and case do not matter (port of web/src/lib/fuzzy.ts).

use unicode_normalization::UnicodeNormalization;

fn fold(s: &str) -> Vec<char> {
    s.nfd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .flat_map(char::to_lowercase)
        .collect()
}

fn separator(c: Option<char>) -> bool {
    matches!(c, Some(c) if c.is_whitespace() || c == '/' || c == '_' || c == '-')
}

/// Score (higher is better) or None if it does not match. Favors the file name and consecutive letters.
pub fn score(query: &str, path: &str) -> Option<f64> {
    let q = fold(query.trim());
    if q.is_empty() {
        return Some(0.0);
    }
    let p = fold(path);
    let name_start = p.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);
    let mut s = 0.0;
    let mut pi = 0;
    let mut prev: isize = -2;
    for &ch in &q {
        if ch == ' ' {
            continue;
        }
        let i = pi + p[pi..].iter().position(|&c| c == ch)?;
        s += 1.0;
        if i as isize == prev + 1 {
            s += 3.0; // consecutive run
        }
        if i >= name_start {
            s += 2.0; // in the name, not the folder
        }
        if i == name_start || separator(i.checked_sub(1).map(|j| p[j])) {
            s += 4.0; // start of a word
        }
        prev = i as isize;
        pi = i + 1;
    }
    if p[name_start..].starts_with(&q) {
        s += 10.0;
    }
    Some(s - p.len() as f64 * 0.01) // tie-breaker: shorter paths first
}

/// Ranks `items` by `score(query, key(item))`, best first; drops the ones that do not match.
pub fn rank<'a, T>(query: &str, items: &'a [T], key: impl Fn(&T) -> &str) -> Vec<&'a T> {
    let mut scored: Vec<(f64, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| score(query, key(it)).map(|s| (s, i)))
        .collect();
    // Stable, like Array.prototype.sort: equal scores keep their order.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, i)| &items[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_order_ignoring_accents() {
        assert!(score("acao", "notas/Ação.md").is_some());
        assert!(score("xyz", "notas/Ação.md").is_none());
        let a = score("nov", "Novo.md").unwrap_or_default();
        let b = score("nov", "pasta/aNoVo.md").unwrap_or_default();
        assert!(a > b);
    }
}
