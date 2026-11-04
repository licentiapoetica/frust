//! `f0ckm build-css`: `public/s/css/bundle.css` from `f0ckm.css` + `v0ck.css` (`scripts/build-css.mjs`).
//!
//! Same minification as the old script (comments removed, whitespace collapsed and dropped around
//! `{ } : ; ,`) with two fixes: whitespace *before* `:` is kept, because in selectors it is the
//! descendant combinator (`.gapRight :is(a, button)` became `.gapRight:is(a,button)`, which matches
//! something else), and quoted strings are copied unchanged.

use super::Args;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const SOURCES: [&str; 3] = ["f0ckm.css", "privacy.css", "v0ck.css"];
pub const OUTPUT: &str = "bundle.css";

fn css_dir(base: &Path) -> PathBuf {
    base.join("public/s/css")
}

/// JavaScript's `\s` (what the old regexes matched).
fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

pub fn minify(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut pending_space = false;
    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            c if is_space(c) => pending_space = true,
            '"' | '\'' => {
                if pending_space && !out.is_empty() && !out.ends_with(['{', '}', ':', ';', ',']) {
                    out.push(' ');
                }
                pending_space = false;
                out.push(c);
                let mut escaped = false;
                for s in chars.by_ref() {
                    out.push(s);
                    if escaped {
                        escaped = false;
                    } else if s == '\\' {
                        escaped = true;
                    } else if s == c || s == '\n' {
                        break;
                    }
                }
            }
            '{' | '}' | ';' | ',' => {
                pending_space = false;
                out.push(c);
            }
            ':' => {
                if pending_space && !out.is_empty() && !out.ends_with(['{', '}', ':', ';', ',']) {
                    out.push(' ');
                }
                pending_space = false;
                out.push(c);
            }
            c => {
                if pending_space && !out.is_empty() && !out.ends_with(['{', '}', ':', ';', ',']) {
                    out.push(' ');
                }
                pending_space = false;
                out.push(c);
            }
        }
    }
    out
}

/// The bundle as it should be on disk.
fn build(base: &Path) -> Result<(String, usize)> {
    let dir = css_dir(base);
    let mut combined = String::new();
    for f in SOURCES {
        let p = dir.join(f);
        combined.push_str(&std::fs::read_to_string(&p).with_context(|| format!("cannot read {}", p.display()))?);
        combined.push('\n');
    }
    // Sizes in UTF-16 units, like the old script's `string.length`
    Ok((minify(&combined), combined.encode_utf16().count()))
}

pub fn run(base: &Path, args: &Args) -> Result<i32> {
    let (css, input_len) = build(base)?;
    let out = css_dir(base).join(OUTPUT);
    if args.flag(&["--check"]) {
        let current = std::fs::read_to_string(&out).unwrap_or_default();
        if current == css {
            println!("{} is up to date", out.display());
            return Ok(0);
        }
        println!("{} is out of date (run `f0ckm build-css`)", out.display());
        return Ok(1);
    }
    std::fs::write(&out, &css).with_context(|| format!("cannot write {}", out.display()))?;
    println!("Bundle created: {} ({input_len} -> {} bytes)", out.display(), css.encode_utf16().count());
    Ok(0)
}

/// Server start: rebuild the bundle when it is missing or older than a source file.
pub fn ensure_bundle(base: &Path) {
    let dir = css_dir(base);
    let mtime = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let newest_source = SOURCES.iter().filter_map(|f| mtime(dir.join(f))).max();
    let stale = match (mtime(dir.join(OUTPUT)), newest_source) {
        (_, None) => return,
        (None, _) => true,
        (Some(out), Some(src)) => src > out,
    };
    if !stale {
        return;
    }
    match build(base).and_then(|(css, _)| std::fs::write(dir.join(OUTPUT), css).context("write")) {
        Ok(()) => tracing::info!("[BOOT] Rebuilt public/s/css/{OUTPUT}"),
        Err(e) => tracing::warn!("[BOOT] Could not rebuild public/s/css/{OUTPUT}: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::minify;

    #[test]
    fn minifies_like_the_old_script() {
        assert_eq!(minify("/* c */\n.a ,\n .b {\n  color : red ;\n  margin: 0  auto;\n}\n"), ".a,.b{color :red;margin:0 auto;}");
        assert_eq!(minify("  a{b:c}  \n"), "a{b:c}");
        assert_eq!(minify("@media (max-width: 600px) {\n  .x { top: 0 }\n}"), "@media (max-width:600px){.x{top:0}}");
        assert_eq!(minify("x{transition: a 1s, b 2s;}"), "x{transition:a 1s,b 2s;}");
    }

    #[test]
    fn keeps_descendant_pseudo_selectors_and_strings() {
        assert_eq!(minify(".gapRight :is(a, b):hover { x: y }"), ".gapRight :is(a,b):hover{x:y}");
        assert_eq!(minify(".pagination :first-child{}"), ".pagination :first-child{}");
        assert_eq!(minify("a::after { content: \" /* x */ ; , \" attr(t) }"), "a::after{content:\" /* x */ ; , \" attr(t)}");
        assert_eq!(minify("a{content:'it\\'s  {'}"), "a{content:'it\\'s  {'}");
    }
}
