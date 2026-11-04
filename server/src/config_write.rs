//! Line-based editing of config.yaml that keeps its comments and layout, used by
//! `f0ckm config-update` to add the settings `config_example.yaml` has and config.yaml lacks.
//!
//! No comment-preserving YAML crate is available here, so this works on the block-style subset
//! config.yaml uses (block mappings, block sequences, flow `[]`/`{}`, plain/quoted scalars,
//! comments). The result is always parsed back and must equal the expected config; if the file
//! uses a construct this editor gets wrong, the write is refused instead of saving something else.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::path::Path;

/// Replace a file's contents atomically (temp file + rename next to the real file, so a symlinked
/// config.yaml stays a symlink), keeping its permissions. Where a rename isn't possible (a
/// bind-mounted single file in Docker) it is written in place.
pub fn replace_file(path: &Path, out: &str) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".config.yaml.{}.tmp", crate::util::random_hex(6)));
    let atomic = (|| -> Result<()> {
        std::fs::write(&tmp, out)?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if atomic.is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(path, out).with_context(|| format!("Cannot write {}", path.display()))
}

/// Parse YAML text into the JSON tree the server works with.
pub fn parse_yaml(text: &str) -> Result<Value> {
    let y: serde_yaml::Value = serde_yaml::from_str(text)?;
    Ok(serde_json::to_value(y)?)
}

/// Add the settings `example` has and `text` lacks, copied with the comments above them and placed
/// after the setting that precedes them in the example. Nested mappings are merged key by key;
/// existing values (lists included) are never changed. Returns the new text and the added paths.
pub fn add_missing(text: &str, example: &str) -> Result<(String, Vec<String>)> {
    let current = parse_yaml(text).map_err(|e| anyhow::anyhow!("config.yaml has syntax errors: {e}"))?;
    let ex_value = parse_yaml(example).map_err(|e| anyhow::anyhow!("the example config has syntax errors: {e}"))?;
    let (Value::Object(cur_map), Value::Object(ex_map)) = (&current, &ex_value) else {
        bail!("config.yaml and the example config must both be mappings");
    };
    let trailing_newline = text.ends_with('\n') || text.is_empty();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let ex: Vec<String> = example.lines().map(str::to_string).collect();
    let ind = first_content(&lines, 0, lines.len()).map(|i| indent(&lines[i])).unwrap_or(0);
    let ex_ind = first_content(&ex, 0, ex.len()).map(|i| indent(&ex[i])).unwrap_or(0);
    let mut added: Vec<Vec<String>> = Vec::new();
    let end = lines.len();
    merge_map(&mut lines, 0, end, ind, &ex, 0, ex.len(), ex_ind, cur_map, ex_map, &[], &mut added);
    if added.is_empty() {
        return Ok((text.to_string(), Vec::new()));
    }

    let mut out = lines.join("\n");
    if trailing_newline && !out.is_empty() {
        out.push('\n');
    }
    // The result must be the old config plus exactly the added settings.
    let mut expected = current.clone();
    for path in &added {
        let (last, parents) = path.split_last().expect("non-empty path");
        let mut src = &ex_value;
        let mut dst = &mut expected;
        for k in parents {
            src = &src[k.as_str()];
            dst = dst.get_mut(k.as_str()).expect("parent exists");
        }
        dst.as_object_mut().expect("parent is a mapping").insert(last.clone(), src[last.as_str()].clone());
    }
    let check = parse_yaml(&out).context("Updated config.yaml does not parse")?;
    if check != expected {
        bail!("config.yaml uses YAML this editor can't update safely; add the missing settings by hand");
    }
    Ok((out, added.iter().map(|p| p.join(".")).collect()))
}

/// Shift a line's indentation by `delta` spaces (comments and content alike).
fn reindent(line: &str, delta: isize) -> String {
    if line.trim().is_empty() || delta == 0 {
        return line.to_string();
    }
    if delta > 0 {
        return format!("{}{line}", " ".repeat(delta as usize));
    }
    let strip = (-delta as usize).min(indent(line));
    line[strip..].to_string()
}

/// [`add_missing`] for the block mapping at `ind` in `lines[start..end)`, against the example's
/// mapping at `ex_ind` in `ex[ex_start..ex_end)`. Returns the new `end`.
#[allow(clippy::too_many_arguments)]
fn merge_map(
    lines: &mut Vec<String>,
    start: usize,
    mut end: usize,
    ind: usize,
    ex: &[String],
    ex_start: usize,
    ex_end: usize,
    ex_ind: usize,
    cur: &Map<String, Value>,
    ex_map: &Map<String, Value>,
    path: &[String],
    added: &mut Vec<Vec<String>>,
) -> usize {
    let mut prev_key: Option<String> = None;
    let mut prev_ex_end = ex_start;
    for ee in entries(ex, ex_start, ex_end, ex_ind) {
        let key = ee.key.clone();
        let mut key_path = path.to_vec();
        key_path.push(key.clone());
        match (cur.get(&key), ex_map.get(&key)) {
            (Some(cur_v), Some(ex_v)) => {
                if let (Value::Object(cur_child), Value::Object(ex_child)) = (cur_v, ex_v) {
                    let found = entries(lines, start, end, ind).into_iter().find(|e| e.key == key);
                    if let (Some(e), Some((ecs, eci))) = (found, block_map_child(ex, &ee, ex_ind))
                        && let Some((cs, ci)) = block_map_child(lines, &e, ind)
                    {
                        let new_end = merge_map(lines, cs, e.end, ci, ex, ecs, ee.end, eci, cur_child, ex_child, &key_path, added);
                        end = end + new_end - e.end;
                    }
                }
            }
            (None, Some(_)) => {
                // The comments directly above the setting in the example come along.
                let mut from = ee.line;
                while from > prev_ex_end && ex[from - 1].trim_start().starts_with('#') {
                    from -= 1;
                }
                let delta = ind as isize - ex_ind as isize;
                let block: Vec<String> = ex[from..ee.end].iter().map(|l| reindent(l, delta)).collect();
                let cur_entries = entries(lines, start, end, ind);
                let at = match prev_key.as_ref().and_then(|p| cur_entries.iter().find(|e| &e.key == p)) {
                    Some(e) => e.end,
                    // First setting of the example's mapping: before everything else in it.
                    None if prev_key.is_none() => cur_entries.first().map(|e| {
                        let mut s = e.line;
                        while s > start && lines[s - 1].trim_start().starts_with('#') && indent(&lines[s - 1]) <= ind {
                            s -= 1;
                        }
                        s
                    }).unwrap_or(end),
                    None => cur_entries.last().map(|e| e.end).unwrap_or(end),
                };
                let n = block.len();
                lines.splice(at..at, block);
                end += n;
                added.push(key_path);
            }
            _ => {}
        }
        prev_key = Some(key);
        prev_ex_end = ee.end;
    }
    end
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
    /// Everything after the `:` on the key line.
    rest: String,
}

/// Split `key: rest` (plain, `"double"` or `'single'` quoted keys). `None` for non-entries.
fn split_key(content: &str) -> Option<(String, String)> {
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
    Some((key, after.to_string()))
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
        let Some((key, rest)) = split_key(&l[ind..]) else {
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
        out.push(Entry { line: i, end: e, key, rest });
        i = j;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn add_missing_keeps_existing_and_copies_comments() {
        let cur = "# mine\nmain:\n  url:\n    full: https://f0ck.example # site\n  socks: socks5h://tor:9050\nallowedModes:\n- sfw\nwebsrv:\n  port: 1\n";
        let ex = "main:\n  url:\n    full: https://example.com\n    domain: example.com\n  # the proxy\n  socks: x\n  # new flag\n  # second line\n  fresh: true\nallowedModes:\n  - sfw\n  - nsfw\nwebsrv:\n  # first\n  lang: en\n  port: 2\n  csp:\n    enabled: true\n# smtp\nsmtp:\n  enabled: false\n";
        let (out, added) = add_missing(cur, ex).unwrap();
        assert_eq!(added, ["main.url.domain", "main.fresh", "websrv.lang", "websrv.csp", "smtp"]);
        assert!(out.starts_with("# mine\nmain:\n  url:\n    full: https://f0ck.example # site\n    domain: example.com\n"), "{out}");
        assert!(out.contains("  socks: socks5h://tor:9050\n  # new flag\n  # second line\n  fresh: true\nallowedModes:\n- sfw\n"), "{out}");
        assert!(out.contains("websrv:\n  # first\n  lang: en\n  port: 1\n  csp:\n    enabled: true\n# smtp\nsmtp:\n"), "{out}");
        // Running again changes nothing
        let (again, none) = add_missing(&out, ex).unwrap();
        assert_eq!(again, out);
        assert!(none.is_empty());
    }

    #[test]
    fn add_missing_reindents() {
        let cur = "main:\n    url: a\n";
        let ex = "main:\n  url: b\n  # c\n  extra:\n    deep: 1\n";
        let (out, added) = add_missing(cur, ex).unwrap();
        assert_eq!(added, ["main.extra"]);
        assert_eq!(out, "main:\n    url: a\n    # c\n    extra:\n      deep: 1\n");
    }

    #[test]
    fn example_config_adds_to_trimmed_copy() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config_example.yaml");
        let Ok(text) = std::fs::read_to_string(p) else { return };
        assert!(add_missing(&text, &text).unwrap().1.is_empty());
        let trimmed: String = text.lines().filter(|l| !l.starts_with("  socks:") && !l.starts_with("    enabled: true") && !l.starts_with("  meme_creator:")).map(|l| format!("{l}\n")).collect();
        let (out, added) = add_missing(&trimmed, &text).unwrap();
        assert_eq!(added, ["main.socks", "websrv.meme_creator", "websrv.csp.enabled"]);
        assert_eq!(parse_yaml(&out).unwrap(), parse_yaml(&text).unwrap());
    }
}
