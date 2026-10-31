//! Word filter (`src/inc/wordfilter.mjs`): case-preserving replacements from the `wordfilter`
//! table, applied to comments and upload comments. Admin routes are in `routes/wordfilter.rs`.

use crate::db::RowExt;
use crate::sql;
use crate::state::AppState;

/// `applyWordFilter(content)`. On a database error the content is returned unchanged.
pub async fn apply_word_filter(st: &AppState, content: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    let rows = match sql!("SELECT word, replacement FROM wordfilter").fetch_all(&st.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[WORDFILTER] Error applying filter: {e}");
            return content.to_string();
        }
    };
    let pairs: Vec<(String, String)> = rows.iter().map(|r| (r.string("word"), r.string("replacement"))).collect();
    word_filter(content, &pairs)
}

/// Replace every (case-insensitive, literal) occurrence of each word, keeping the match's case:
/// ALL CAPS → replacement upper-cased, Capitalized → first letter upper-cased, lowercase →
/// lower-cased, anything else → replacement as stored.
pub fn word_filter(content: &str, filters: &[(String, String)]) -> String {
    let mut out = content.to_string();
    for (word, repl) in filters {
        if word.is_empty() {
            continue;
        }
        let Ok(re) = regex::RegexBuilder::new(&regex::escape(word)).case_insensitive(true).build() else { continue };
        out = re
            .replace_all(&out, |c: &regex::Captures| {
                let m = &c[0];
                let (up, low) = (m.to_uppercase(), m.to_lowercase());
                if m == up && m != low {
                    return repl.to_uppercase();
                }
                let first: String = m.chars().take(1).collect();
                if first == first.to_uppercase() && first != first.to_lowercase() {
                    let mut ch = repl.chars();
                    return match ch.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                        None => String::new(),
                    };
                }
                if m == low {
                    return repl.to_lowercase();
                }
                repl.clone()
            })
            .into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_filter_cases() {
        let f = vec![("darn".to_string(), "heck".to_string())];
        assert_eq!(word_filter("darn it", &f), "heck it");
        assert_eq!(word_filter("Darn it", &f), "Heck it");
        assert_eq!(word_filter("DARN it", &f), "HECK it");
        assert_eq!(word_filter("dArN", &f), "heck");
        assert_eq!(word_filter("x darnDARN", &f), "x heckHECK");
        let f = vec![("a.b".to_string(), "x".to_string())];
        assert_eq!(word_filter("a.b acb", &f), "x acb", "word is literal, not a regex");
        let f = vec![("ab".to_string(), "Cd".to_string())];
        assert_eq!(word_filter("aB", &f), "Cd", "mixed case keeps the stored replacement");
        assert_eq!(word_filter("", &f), "");
    }
}
