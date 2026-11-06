//! Website fonts: built-in ones shipped in `public/s/fonts-builtin` plus uploaded ones in the fonts
//! directory (`public/s/fonts`, or `<STORAGE_DIR>/fonts`). Both are served under `/s/fonts/<file>`;
//! an uploaded file with the same name wins.
//!
//! Files named `<Family>-<Weight>.<ext>` (Manrope-Bold.woff2, Manrope-Regular.woff2, …) form one
//! family with a `@font-face` per weight. A user's choice is stored as one file name (the family's
//! regular weight); any file of a family selects the whole family.

use crate::config::Config;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Directory of the fonts that ship with the site.
pub fn builtin_dir(cfg: &Config) -> PathBuf {
    cfg.paths.s.join("fonts-builtin")
}

fn is_font_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".ttf", ".otf", ".woff", ".woff2"].iter().any(|e| lower.ends_with(e))
}

fn list(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|f| is_font_file(f)).collect())
        .unwrap_or_default()
}

/// CSS weight of a style suffix (`Bold` → 700); None when the suffix is no weight name.
fn weight_of(suffix: &str) -> Option<u16> {
    Some(match suffix.to_ascii_lowercase().as_str() {
        "thin" | "hairline" => 100,
        "extralight" | "ultralight" => 200,
        "light" => 300,
        "regular" | "normal" | "book" => 400,
        "medium" => 500,
        "semibold" | "demibold" => 600,
        "bold" => 700,
        "extrabold" | "ultrabold" => 800,
        "black" | "heavy" => 900,
        _ => return None,
    })
}

/// (family name, weight) of a font file.
fn split_name(file: &str) -> (String, u16) {
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    match stem.rsplit_once('-') {
        Some((family, suffix)) if !family.is_empty() => match weight_of(suffix) {
            Some(w) => (family.to_string(), w),
            None => (stem.to_string(), 400),
        },
        _ => (stem.to_string(), 400),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Family {
    pub name: String,
    /// (file, weight), lightest first
    pub faces: Vec<(String, u16)>,
}

impl Family {
    /// File stored as the user's choice: the regular weight, else the one closest to it.
    pub fn primary(&self) -> &str {
        self.faces.iter().min_by_key(|(_, w)| (*w as i32 - 400).abs()).map(|(f, _)| f.as_str()).unwrap_or("")
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "file": self.primary(),
            "files": self.faces.iter().map(|(f, _)| f).collect::<Vec<_>>(),
            "faces": self.faces.iter().map(|(f, w)| json!({ "file": f, "weight": w })).collect::<Vec<_>>(),
        })
    }
}

fn group(files: Vec<String>) -> Vec<Family> {
    let mut families: Vec<Family> = Vec::new();
    for f in files {
        let (name, weight) = split_name(&f);
        match families.iter_mut().find(|fam| fam.name == name) {
            Some(fam) => {
                // Two files for one weight (e.g. .woff and .woff2): keep the first
                if !fam.faces.iter().any(|(_, w)| *w == weight) {
                    fam.faces.push((f, weight));
                }
            }
            None => families.push(Family { name, faces: vec![(f, weight)] }),
        }
    }
    for fam in &mut families {
        fam.faces.sort_by_key(|(_, w)| *w);
    }
    families.sort_by_key(|f| f.name.to_lowercase());
    families
}

/// All fonts, uploaded ones first in case of equal file names.
fn all_files(cfg: &Config) -> Vec<String> {
    let mut files = list(&cfg.paths.fonts);
    for f in list(&builtin_dir(cfg)) {
        if !files.contains(&f) {
            files.push(f);
        }
    }
    files.sort();
    files
}

pub fn families(cfg: &Config) -> Vec<Family> {
    group(all_files(cfg))
}

/// Font list for templates: `[{ name, file, files, faces: [{ file, weight }] }]`.
pub fn families_json(cfg: &Config) -> Value {
    Value::Array(families(cfg).iter().map(Family::to_json).collect())
}

/// Faces of the family a stored choice belongs to (`[{ file, weight }]`), empty when unknown.
pub fn faces_json(cfg: &Config, file: Option<&str>) -> Value {
    let Some(file) = file.filter(|f| !f.is_empty()) else { return json!([]) };
    families(cfg)
        .into_iter()
        .find(|fam| fam.faces.iter().any(|(f, _)| f == file))
        .map(|fam| fam.to_json()["faces"].clone())
        .unwrap_or_else(|| json!([]))
}

/// URL of the navbar brand font's stylesheet (see [`brand_css`]); versioned by the font's name.
pub fn brand_css_url(file: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    format!("/brand-font.css?v={:x}", h.finish())
}

/// `/brand-font.css`: an `@font-face` per weight of the family of `file`, as 'BrandFont' (the
/// `.brand-font` class on the navbar brand text uses it; the frontends' CSS files hold that rule).
/// Empty when no font is set or the font is gone.
pub fn brand_css(cfg: &Config, file: &str) -> String {
    let enc = url_enc;
    let faces = faces_json(cfg, Some(file));
    faces
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| Some((f["file"].as_str()?, f["weight"].as_u64()?)))
                .map(|(f, w)| format!("@font-face {{ font-family: 'BrandFont'; src: url('/s/fonts/{}'); font-weight: {w}; font-display: swap; }}\n", enc(f)))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `file` is an available font (validates a user's choice).
pub fn exists(cfg: &Config, file: &str) -> bool {
    is_font_file(file) && all_files(cfg).iter().any(|f| f == file)
}

/// Directory to serve `/s/fonts/<rel>` from: the uploaded fonts when the file is there, else the built-in ones.
pub fn serve_root(cfg: &Config, rel: &str) -> PathBuf {
    match crate::http::files::safe_join(&cfg.paths.fonts, rel) {
        Some(p) if p.is_file() => cfg.paths.fonts.clone(),
        _ => builtin_dir(cfg),
    }
}

// ── admin font manager (routes/admin_media/fonts.rs) ────────────────────────

/// Style suffix written into uploaded file names for a CSS weight.
pub fn weight_name(w: u16) -> &'static str {
    match w {
        100 => "Thin",
        200 => "ExtraLight",
        300 => "Light",
        500 => "Medium",
        600 => "SemiBold",
        700 => "Bold",
        800 => "ExtraBold",
        900 => "Black",
        _ => "Regular",
    }
}

/// The weights the upload form offers.
pub const WEIGHTS: [u16; 9] = [100, 200, 300, 400, 500, 600, 700, 800, 900];

/// File extension for a font's first bytes; None when it is no single web font (font
/// collections, anything else). Browsers sanitize fonts themselves (OTS), this only keeps
/// other files out of the fonts directory.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    match head.get(..4)? {
        b"wOF2" => Some("woff2"),
        b"wOFF" => Some("woff"),
        b"OTTO" => Some("otf"),
        [0, 1, 0, 0] | b"true" => Some("ttf"),
        _ => None,
    }
}

/// A family name usable in a file name: ASCII letters, digits, `_` and inner `-`, at most 64.
pub fn clean_family(s: &str) -> String {
    let s: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(64).collect();
    s.trim_matches('-').to_string()
}

/// File name an upload is stored under: `<Family>-<Weight>.<ext>`. The family and weight come from
/// the form when given, else from the uploaded file's name (`Inter-Bold.ttf` → Inter, 700).
pub fn upload_name(orig: &str, family: Option<&str>, weight: Option<u16>, ext: &str) -> Option<String> {
    let base = orig.rsplit(['/', '\\']).next().unwrap_or("");
    let (orig_family, orig_weight) = split_name(base);
    let family = clean_family(family.filter(|f| !f.trim().is_empty()).unwrap_or(&orig_family));
    if family.is_empty() {
        return None;
    }
    Some(format!("{family}-{}.{ext}", weight_name(weight.unwrap_or(orig_weight))))
}

/// Why `name` can't be stored, if anything: an uploaded file of that name exists, or the family
/// already has that weight in another file (only the first file of a weight is used). Same name
/// as a built-in font is fine: the upload replaces it.
pub fn upload_conflict(cfg: &Config, name: &str) -> Option<String> {
    if list(&cfg.paths.fonts).iter().any(|f| f == name) {
        return Some(format!("{name} already exists, delete it first"));
    }
    let (family, weight) = split_name(name);
    all_files(cfg)
        .into_iter()
        .filter(|f| f != name)
        .find(|f| split_name(f) == (family.clone(), weight))
        .map(|f| format!("{family} already has weight {weight} ({f})"))
}

/// Whether two font files are the same family and weight (only one of them would be used).
pub fn same_face(a: &str, b: &str) -> bool {
    split_name(a) == split_name(b)
}

/// Whether `name` is an uploaded font (the only kind the font manager deletes).
pub fn is_uploaded(cfg: &Config, name: &str) -> bool {
    is_font_file(name) && crate::media::is_plain_name(name) && list(&cfg.paths.fonts).iter().any(|f| f == name)
}

/// Class/font-family suffix of a family on the font manager page (stable for a name).
pub fn preview_key(family: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    family.hash(&mut h);
    format!("{:x}", h.finish())
}

fn url_enc(f: &str) -> String {
    f.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Families for the font manager: `[{ name, key, files, faces: [{ file, weight, weight_name, uploaded,
/// builtin }] }]`. `uploaded` and `builtin` together: an upload replacing a built-in file.
pub fn admin_json(cfg: &Config) -> Value {
    let uploaded = list(&cfg.paths.fonts);
    let builtin = list(&builtin_dir(cfg));
    // Every file, also a second file of one weight (group() keeps only the first)
    let mut fams = families(cfg);
    for f in all_files(cfg) {
        let (name, weight) = split_name(&f);
        if let Some(fam) = fams.iter_mut().find(|x| x.name == name)
            && !fam.faces.iter().any(|(x, _)| *x == f)
        {
            fam.faces.push((f, weight));
            fam.faces.sort_by_key(|(_, w)| *w);
        }
    }
    Value::Array(
        fams.iter()
            .map(|fam| {
                json!({
                    "name": fam.name,
                    "key": preview_key(&fam.name),
                    "files": fam.faces.iter().map(|(f, _)| f).collect::<Vec<_>>(),
                    "faces": fam.faces.iter().map(|(f, w)| json!({
                        "file": f,
                        "weight": w,
                        "weight_name": weight_name(*w),
                        "uploaded": uploaded.contains(f),
                        "builtin": builtin.contains(f),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

/// `/admin/fonts/preview.css`: an `@font-face` per font as `FontPreview-<key>` and a
/// `.adm-font-sample-<key>` class using it (see [`preview_key`]).
pub fn preview_css(cfg: &Config) -> String {
    let mut css = String::new();
    for fam in families(cfg) {
        let key = preview_key(&fam.name);
        for (f, w) in &fam.faces {
            css.push_str(&format!(
                "@font-face {{ font-family: 'FontPreview-{key}'; src: url('/s/fonts/{}'); font-weight: {w}; font-display: swap; }}\n",
                url_enc(f)
            ));
        }
        css.push_str(&format!(".adm-font-sample-{key} {{ font-family: 'FontPreview-{key}', monospace; }}\n"));
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_group_into_one_family() {
        let fams = group(
            ["Manrope-Bold.woff2", "Manrope-Regular.woff2", "Manrope-Light.woff2", "Hack.woff", "Some-Thing.ttf"].map(String::from).to_vec(),
        );
        let names: Vec<_> = fams.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Hack", "Manrope", "Some-Thing"]);
        let manrope = &fams[1];
        assert_eq!(manrope.faces.iter().map(|(_, w)| *w).collect::<Vec<_>>(), [300, 400, 700]);
        assert_eq!(manrope.primary(), "Manrope-Regular.woff2");
        assert_eq!(fams[0].primary(), "Hack.woff");
    }

    #[test]
    fn upload_names() {
        assert_eq!(upload_name("Inter-Bold.ttf", None, None, "ttf").as_deref(), Some("Inter-Bold.ttf"));
        assert_eq!(upload_name("inter.woff2", None, None, "woff2").as_deref(), Some("inter-Regular.woff2"));
        assert_eq!(upload_name("x.bin", Some("Open Sans"), Some(300), "otf").as_deref(), Some("OpenSans-Light.otf"));
        assert_eq!(upload_name("../a/Foo-Black.woff", Some("  "), None, "woff").as_deref(), Some("Foo-Black.woff"));
        assert_eq!(upload_name("...ttf", None, None, "ttf"), None);
        assert_eq!(upload_name("ok.ttf", Some("ÄÖÜ"), None, "ttf"), None);
        // the stored name splits back into the same family and weight
        let n = upload_name("x", Some("Some-Thing"), Some(600), "ttf").unwrap();
        assert_eq!(split_name(&n), ("Some-Thing".to_string(), 600));
    }

    #[test]
    fn sniffs_font_types() {
        assert_eq!(sniff(b"wOF2...."), Some("woff2"));
        assert_eq!(sniff(b"wOFF...."), Some("woff"));
        assert_eq!(sniff(b"OTTO...."), Some("otf"));
        assert_eq!(sniff(&[0, 1, 0, 0, 9]), Some("ttf"));
        assert_eq!(sniff(b"ttcf...."), None);
        assert_eq!(sniff(b"<svg"), None);
        assert_eq!(sniff(b"wO"), None);
    }

    #[test]
    fn primary_without_regular_is_closest_to_400() {
        let fams = group(["X-Bold.woff2", "X-Light.woff2"].map(String::from).to_vec());
        assert_eq!(fams[0].primary(), "X-Light.woff2");
    }
}
