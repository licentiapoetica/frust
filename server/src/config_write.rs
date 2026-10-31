//! Writing config.yaml from the admin config editor while keeping its comments and layout
//! (port of `writeConfigFile` / `syncNode` in `src/inc/config.mjs`).
//!
//! The old code used the `yaml` package's document model. No comment-preserving YAML crate is
//! available here, so this is a line-based editor for the block-style subset config.yaml uses
//! (block mappings, block sequences, flow `[]`/`{}`, plain/quoted scalars, comments):
//!
//! * like `syncNode`, it walks block mappings key by key: keys missing from the new object are
//!   removed, keys whose value is unchanged are left untouched (comments and formatting kept),
//!   changed values are re-emitted in place (a trailing `# comment` on a scalar line is kept),
//!   nested block mappings are recursed into, and new keys are appended to their mapping;
//! * sequences and anything else are replaced as a whole when they differ (same as the old code).
//!
//! The result is always parsed back and must equal the requested config exactly; if the file uses a
//! construct this editor gets wrong, the write is refused instead of saving something different.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::path::Path;

/// Save a full config object to `<base>/config.yaml`, keeping the file's comments and layout.
/// The file is replaced atomically (temp file + rename) and keeps its permissions.
pub fn write_config_file(base: &Path, new_config: &Value) -> Result<()> {
    let Value::Object(_) = new_config else { bail!("Invalid configuration payload") };
    let path = base.join("config.yaml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("Cannot read {}", path.display()))?;
    let out = sync_yaml(&text, new_config)?;
    if out == text {
        return Ok(());
    }
    let tmp = base.join(format!(".config.yaml.{}.tmp", crate::util::random_hex(6)));
    let res = (|| -> Result<()> {
        std::fs::write(&tmp, &out)?;
        if let Ok(meta) = std::fs::metadata(&path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res.with_context(|| format!("Cannot write {}", path.display()))
}

/// Parse YAML text into the JSON tree the server works with.
pub fn parse_yaml(text: &str) -> Result<Value> {
    let y: serde_yaml::Value = serde_yaml::from_str(text)?;
    Ok(serde_json::to_value(y)?)
}

/// Bring YAML text in line with `new`, touching only what differs.
pub fn sync_yaml(text: &str, new: &Value) -> Result<String> {
    let Value::Object(new_map) = new else { bail!("Invalid configuration payload") };
    let current = parse_yaml(text).map_err(|e| anyhow::anyhow!("config.yaml has syntax errors: {e}"))?;
    let trailing_newline = text.ends_with('\n') || text.is_empty();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    match &current {
        Value::Object(cur_map) if first_content(&lines, 0, lines.len()).is_some() => {
            let ind = indent(&lines[first_content(&lines, 0, lines.len()).unwrap_or(0)]);
            let end = lines.len();
            sync_map(&mut lines, 0, end, ind, cur_map, new_map)?;
        }
        _ => {
            // Empty document (or not a mapping): keep the comments, write the mapping below them.
            lines.retain(|l| is_blank_or_comment(l));
            for (k, v) in new_map {
                lines.extend(render_entry(0, &render_key(k), v, None));
            }
        }
    }

    let mut out = lines.join("\n");
    if trailing_newline && !out.is_empty() {
        out.push('\n');
    }
    let check = parse_yaml(&out).context("Edited config.yaml does not parse")?;
    if &check != new {
        bail!("config.yaml uses YAML this editor can't update safely; edit the file by hand");
    }
    Ok(out)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

fn first_content(lines: &[String], start: usize, end: usize) -> Option<usize> {
    (start..end).find(|&i| !is_blank_or_comment(&lines[i]))
}

fn is_seq_item(content: &str) -> bool {
    content == "-" || content.starts_with("- ")
}

/// A `key: value` line inside a block mapping.
struct Entry {
    line: usize,
    /// One past the last line belonging to this entry.
    end: usize,
    key: String,
    /// The key as written (kept on rewrite).
    key_token: String,
    /// Everything after the `:` on the key line.
    rest: String,
}

/// Split `key: rest` (plain, `"double"` or `'single'` quoted keys). `None` for non-entries.
fn split_key(content: &str) -> Option<(String, String, String)> {
    if is_seq_item(content) || content.starts_with('#') {
        return None;
    }
    let (key, token_len) = if let Some(q @ ('"' | '\'')) = content.chars().next() {
        let body = &content[1..];
        let mut key = String::new();
        let mut chars = body.char_indices();
        let mut close = None;
        while let Some((i, c)) = chars.next() {
            if c == q {
                if q == '\'' && body[i + 1..].starts_with('\'') {
                    key.push('\'');
                    chars.next();
                    continue;
                }
                close = Some(i);
                break;
            }
            if q == '"' && c == '\\' {
                match chars.next() {
                    Some((_, 'n')) => key.push('\n'),
                    Some((_, 't')) => key.push('\t'),
                    Some((_, e)) => key.push(e),
                    None => return None,
                }
                continue;
            }
            key.push(c);
        }
        let close = close?;
        (key, close + 2)
    } else {
        let b = content.as_bytes();
        let pos = (0..b.len()).find(|&i| b[i] == b':' && (i + 1 == b.len() || b[i + 1] == b' '))?;
        (content[..pos].trim_end().to_string(), pos)
    };
    let after = &content[token_len..];
    let after = after.strip_prefix(':')?;
    if !(after.is_empty() || after.starts_with(' ')) {
        return None;
    }
    Some((key, content[..token_len].to_string(), after.to_string()))
}

/// Entries of the block mapping at indentation `ind` within `[start, end)`.
fn entries(lines: &[String], start: usize, end: usize, ind: usize) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut i = start;
    while i < end {
        let l = &lines[i];
        if is_blank_or_comment(l) || indent(l) != ind {
            i += 1;
            continue;
        }
        let Some((key, key_token, rest)) = split_key(&l[ind..]) else {
            i += 1;
            continue;
        };
        // The entry runs over deeper lines (and a compact `- item` list at the same indentation).
        let mut j = i + 1;
        while j < end {
            let lj = &lines[j];
            if is_blank_or_comment(lj) || indent(lj) > ind || (indent(lj) == ind && is_seq_item(&lj[ind..])) {
                j += 1;
            } else {
                break;
            }
        }
        // Blank lines and comments at this level before the next key belong to that key.
        let mut e = j;
        while e > i + 1 && (lines[e - 1].trim().is_empty() || (lines[e - 1].trim_start().starts_with('#') && indent(&lines[e - 1]) <= ind)) {
            e -= 1;
        }
        out.push(Entry { line: i, end: e, key, key_token, rest });
        i = j;
    }
    out
}

/// Byte offset of a trailing `# comment` in the value part of a line (outside quotes).
fn comment_start(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        match quote {
            Some(q) if b[i] == q => quote = None,
            Some(b'"') if b[i] == b'\\' => i += 1,
            Some(_) => {}
            None if (b[i] == b'"' || b[i] == b'\'') && rest[..i].trim().is_empty() => quote = Some(b[i]),
            None if b[i] == b'#' && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t') => return Some(i),
            None => {}
        }
        i += 1;
    }
    None
}

fn render_key(k: &str) -> String {
    let s = serde_yaml::to_string(&Value::String(k.to_string())).unwrap_or_default();
    let s = s.trim_end();
    if s.contains('\n') { serde_json::to_string(k).unwrap_or_default() } else { s.to_string() }
}

/// Lines for `key: value` at indentation `ind`. A one-line scalar keeps `comment`.
fn render_entry(ind: usize, key_token: &str, v: &Value, comment: Option<&str>) -> Vec<String> {
    let pad = " ".repeat(ind);
    let child = " ".repeat(ind + 2);
    let dumped = serde_yaml::to_string(v).unwrap_or_else(|_| "null\n".into());
    let dumped = dumped.strip_prefix("---\n").unwrap_or(&dumped);
    let body: Vec<&str> = dumped.trim_end_matches('\n').split('\n').collect();
    let collection = matches!(v, Value::Object(m) if !m.is_empty()) || matches!(v, Value::Array(a) if !a.is_empty());
    if collection {
        let mut out = vec![format!("{pad}{key_token}:")];
        out.extend(body.iter().map(|l| if l.is_empty() { String::new() } else { format!("{child}{l}") }));
        return out;
    }
    let mut out = vec![match (body.len(), comment) {
        (1, Some(c)) => format!("{pad}{key_token}: {} {c}", body[0]),
        _ => format!("{pad}{key_token}: {}", body[0]),
    }];
    // Multi-line scalars (`|-` block) continue on deeper lines.
    out.extend(body[1..].iter().map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") }));
    out
}

/// Child mapping of an entry, when its value is a block mapping: `(start, indentation)`.
fn block_map_child(lines: &[String], e: &Entry, ind: usize) -> Option<(usize, usize)> {
    let rest = e.rest.trim();
    if !(rest.is_empty() || rest.starts_with('#')) {
        return None;
    }
    let first = first_content(lines, e.line + 1, e.end)?;
    let ci = indent(&lines[first]);
    if ci <= ind || is_seq_item(&lines[first][ci..]) {
        return None;
    }
    Some((e.line + 1, ci))
}

/// Sync the block mapping at indentation `ind` in `[start, end)`; returns the new `end`.
fn sync_map(lines: &mut Vec<String>, start: usize, mut end: usize, ind: usize, cur: &Map<String, Value>, new: &Map<String, Value>) -> Result<usize> {
    // Removed keys (bottom up so earlier indices stay valid).
    let removed: Vec<Entry> = entries(lines, start, end, ind).into_iter().filter(|e| !new.contains_key(&e.key)).collect();
    for e in removed.iter().rev() {
        lines.drain(e.line..e.end);
        end -= e.end - e.line;
    }

    for (k, v) in new {
        let found = entries(lines, start, end, ind).into_iter().find(|e| &e.key == k);
        match found {
            Some(e) => {
                let old = cur.get(k).unwrap_or(&Value::Null);
                if let (Value::Object(new_child), Value::Object(cur_child), Some((cs, ci))) = (v, old, block_map_child(lines, &e, ind)) {
                    if !new_child.is_empty() {
                        let new_end = sync_map(lines, cs, e.end, ci, cur_child, new_child)?;
                        end = end + new_end - e.end;
                        continue;
                    }
                }
                if old == v {
                    continue;
                }
                // A trailing comment of a one-line entry stays on the rewritten line.
                let comment = if e.end == e.line + 1 { comment_start(&e.rest).map(|p| e.rest[p..].trim_end().to_string()) } else { None };
                let rendered = render_entry(ind, &e.key_token, v, comment.as_deref());
                let n = rendered.len();
                lines.splice(e.line..e.end, rendered);
                end = end + n - (e.end - e.line);
            }
            None => {
                let at = entries(lines, start, end, ind).last().map(|e| e.end).unwrap_or(end);
                let rendered = render_entry(ind, &render_key(k), v, None);
                let n = rendered.len();
                lines.splice(at..at, rendered);
                end += n;
            }
        }
    }
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DOC: &str = "# Top comment\nmain:\n  url:\n    full: https://example.com # the site\n    domain: example.com\n  # upload size\n  maxfilesize: 100\n  ignored:\n    - a.net\n    - b.org\n  secret: \"x: y\"\nallowedModes:\n- sfw\n- nsfw\n\n# Bot clients\nclients:\n  - type: tg\n    enabled: false\napis: {}\nsql:\n  host: localhost\n";

    fn cur() -> Value {
        parse_yaml(DOC).unwrap()
    }

    #[test]
    fn unchanged_is_identical() {
        assert_eq!(sync_yaml(DOC, &cur()).unwrap(), DOC);
    }

    #[test]
    fn scalar_change_keeps_comments() {
        let mut v = cur();
        v["main"]["url"]["full"] = json!("https://f0ck.me");
        v["main"]["maxfilesize"] = json!(200);
        let out = sync_yaml(DOC, &v).unwrap();
        assert!(out.contains("    full: https://f0ck.me # the site\n"), "{out}");
        assert!(out.contains("  # upload size\n  maxfilesize: 200\n"), "{out}");
        assert!(out.starts_with("# Top comment\n"));
        assert!(out.contains("# Bot clients\n"));
        assert_eq!(parse_yaml(&out).unwrap(), v);
    }

    #[test]
    fn add_remove_and_replace() {
        let mut v = cur();
        v["main"].as_object_mut().unwrap().remove("ignored");
        v["main"]["new_flag"] = json!(true);
        v["allowedModes"] = json!(["sfw"]);
        v["clients"][0]["enabled"] = json!(true);
        v["apis"] = json!({ "key": "abc" });
        v["websrv"] = json!({ "port": 1337, "halls_enabled": false });
        v.as_object_mut().unwrap().remove("sql");
        let out = sync_yaml(DOC, &v).unwrap();
        assert!(!out.contains("a.net"));
        assert!(out.contains("  new_flag: true\n"), "{out}");
        assert!(out.contains("# Bot clients\nclients:\n"), "{out}");
        assert!(!out.contains("host: localhost"));
        assert_eq!(parse_yaml(&out).unwrap(), v);
    }

    #[test]
    fn tricky_strings_round_trip() {
        let mut v = cur();
        v["main"]["secret"] = json!("a # not a comment: 'q' \"d\"");
        v["main"]["multi"] = json!("line1\nline2");
        v["main"]["looks_bool"] = json!("yes");
        v["main"]["num_str"] = json!("123");
        v["main"]["empty"] = json!("");
        v["main"]["nothing"] = Value::Null;
        let out = sync_yaml(DOC, &v).unwrap();
        assert_eq!(parse_yaml(&out).unwrap(), v);
    }

    #[test]
    fn collection_to_scalar_and_back() {
        let mut v = cur();
        v["main"]["url"] = json!("plain");
        let out = sync_yaml(DOC, &v).unwrap();
        assert_eq!(parse_yaml(&out).unwrap(), v);
        let mut w = v.clone();
        w["main"]["url"] = json!({ "full": "x", "nested": { "a": [1, 2] } });
        let out2 = sync_yaml(&out, &w).unwrap();
        assert_eq!(parse_yaml(&out2).unwrap(), w);
    }

    #[test]
    fn rejects_non_objects() {
        assert!(sync_yaml(DOC, &json!([1])).is_err());
        assert!(sync_yaml("main: [unclosed", &json!({})).is_err());
    }

    #[test]
    fn real_example_config_round_trips() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config_example.yaml");
        let Ok(text) = std::fs::read_to_string(p) else { return };
        let mut v = parse_yaml(&text).unwrap();
        assert_eq!(sync_yaml(&text, &v).unwrap(), text);
        v["websrv"]["halls_enabled"] = json!(false);
        v["main"]["upload_limit"] = json!(5);
        let out = sync_yaml(&text, &v).unwrap();
        assert_eq!(parse_yaml(&out).unwrap(), v);
        assert_eq!(text.matches('#').count(), out.matches('#').count());
        let changed = text.lines().zip(out.lines()).filter(|(a, b)| a != b).count();
        assert!(changed <= 2, "only the edited lines change");
    }
}
