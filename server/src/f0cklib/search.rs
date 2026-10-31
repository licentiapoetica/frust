//! Fuzzy tag search scoring (port of src/inc/routeinc/search.mjs).
//!
//! Works on UTF-16 code units like the JavaScript original, so scores are identical.

use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

fn units(s: &str) -> Vec<u16> {
    s.nfc().collect::<String>().to_lowercase().encode_utf16().collect()
}

/// Score of `word` against `tag` (0 = no match).
pub fn score(tag: &str, word: &str) -> f64 {
    let s = units(tag);
    let w = units(word);
    let mut rscore = 0.0;
    let mut startat = 0usize;
    for &c in &w {
        let Some(idx) = s.get(startat..).and_then(|rest| rest.iter().position(|&x| x == c)).map(|p| p + startat) else {
            return 0.0;
        };
        let mut cscore = if startat == idx {
            0.7
        } else {
            let mut v = 0.1;
            if idx > 0 && s[idx - 1] == u16::from(b' ') {
                v += 0.8;
            }
            v
        };
        if s[idx] == c {
            cscore += 0.1;
        }
        rscore += cscore;
        startat = idx + 1;
    }
    let mut score = 0.5 * (rscore / s.len() as f64 + rscore / w.len() as f64);
    if w.first() == s.first() && score < 0.85 {
        score += 0.15;
    }
    score
}

/// `search(obj, word)`: rows with a `tag` key, scored, without non-matches, best first.
/// `None` for non-list input (the old function returned `false`).
pub fn search(rows: &Value, word: &str) -> Option<Vec<Value>> {
    let list = rows.as_array()?;
    let mut out: Vec<Value> = list
        .iter()
        .map(|row| {
            let tag = row.get("tag").and_then(Value::as_str).unwrap_or("");
            let sc = score(tag, word);
            let mut r = match row {
                Value::Object(m) => m.clone(),
                _ => serde_json::Map::new(),
            };
            // NaN > 0 is false, as in JS
            (sc > 0.0).then(|| {
                r.insert("score".into(), json!(sc));
                Value::Object(r)
            })
        })
        .flatten()
        .collect();
    // Array.prototype.sort is stable
    out.sort_by(|a, b| {
        let (x, y) = (a["score"].as_f64().unwrap_or(0.0), b["score"].as_f64().unwrap_or(0.0));
        y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal)
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores() {
        assert_eq!(score("abc", "x"), 0.0);
        assert!(score("cat", "cat") > score("concatenate", "cat"));
        // empty word: rscore / 0 → NaN in JS as well
        assert!(score("cat", "").is_nan());
        let r = search(&json!([{"tag": "dog"}, {"tag": "cat"}, {"tag": "catalog"}]), "cat").unwrap();
        assert_eq!(r[0]["tag"], "cat");
        assert_eq!(r.len(), 2);
    }
}

#[cfg(test)]
mod parity {
    use super::*;

    /// Values computed with the old search.mjs.
    #[test]
    fn matches_js() {
        let r = search(&json!([{"tag":"cat"},{"tag":"concatenate"},{"tag":"Ünïcode cat"},{"tag":"big cat"},{"tag":"c a t"}]), "cat").unwrap();
        let got: Vec<(String, f64)> = r.iter().map(|x| (x["tag"].as_str().unwrap().to_string(), x["score"].as_f64().unwrap())).collect();
        let want = [("cat", 0.95), ("c a t", 0.8966666666666666), ("big cat", 0.619047619047619), ("concatenate", 0.5318181818181817), ("Ünïcode cat", 0.2545454545454545)];
        assert_eq!(got.len(), want.len());
        for ((t, s), (wt, ws)) in got.iter().zip(want) {
            assert_eq!(t, wt);
            assert_eq!(*s, ws, "{t}");
        }
    }
}
