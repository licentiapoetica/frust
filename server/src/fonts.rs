//! Website fonts: built-in ones shipped in `public/s/fonts-builtin` plus uploaded ones in the fonts
//! directory (`public/s/fonts`, or `<STORAGE_DIR>/fonts`). Both are served under `/s/fonts/<file>`;
//! an uploaded file with the same name wins.
//!
//! Files named `<Family>-<Weight>.<ext>` (Manrope-Bold.woff2, Manrope-Regular.woff2, …) form one
//! family with a `@font-face` per weight. A variable font (one file with a `wght` axis, read from
//! the file) offers every weight of its range. A user's choice is stored as one file name (any
//! file selects its whole family) plus an optional weight (`user_options.font_weight`): the weight
//! plain text is drawn in, see [`Family::faces_at`].

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
    /// A variable font of the family: (file, lightest, heaviest weight of its `wght` axis)
    pub variable: Option<(String, u16, u16)>,
}

impl Family {
    /// File stored as the user's choice: the regular weight, else the one closest to it.
    pub fn primary(&self) -> &str {
        if let Some((f, _, _)) = &self.variable {
            return f;
        }
        self.faces.iter().min_by_key(|(_, w)| (*w as i32 - 400).abs()).map(|(f, _)| f.as_str()).unwrap_or("")
    }

    /// The weights a user can pick: every hundred of a variable font's range, else one per file.
    pub fn weights(&self) -> Vec<u16> {
        match &self.variable {
            Some((_, lo, hi)) => WEIGHTS.iter().copied().filter(|w| w >= lo && w <= hi).collect(),
            None => self.faces.iter().map(|(_, w)| *w).collect(),
        }
    }

    /// Weight plain text uses when the user picked none: the stored file's (a non-regular file
    /// was how a weight was chosen before `font_weight`), 400 for a variable font.
    pub fn default_weight(&self, file: &str) -> u16 {
        let ws = self.weights();
        let want = match &self.variable {
            Some(_) => 400,
            None => self.faces.iter().find(|(f, _)| f == file).map(|(_, w)| *w).unwrap_or(400),
        };
        ws.iter().copied().min_by_key(|w| (*w as i32 - want as i32).abs()).unwrap_or(want)
    }

    /// The `@font-face` (file, weight) list for plain text in `weight`; light and bold text
    /// keep lighter / heavier faces.
    ///
    /// Separate files: the file of `weight` is declared as the normal weight (400); faces lighter
    /// and heavier than both stay, the ones in between go (they would take the 400 slot or sit
    /// between it and bold). A variable font renders at the weight its rule declares, so it gets
    /// one rule per hundred, and the slots the browser would pick for normal text before
    /// `weight` are left out (CSS font matching: 400 looks at 400–500, then lighter, then heavier).
    pub fn faces_at(&self, weight: u16) -> Vec<(String, u16)> {
        if let Some((file, _, _)) = &self.variable {
            let w = weight;
            return self
                .weights()
                .into_iter()
                .filter(|s| match w {
                    0..400 => *s <= w || *s >= 600,
                    400 => true,
                    401..=500 => *s != 400,
                    _ => *s >= w,
                })
                .map(|s| (file.clone(), s))
                .collect();
        }
        let file = self.faces.iter().min_by_key(|(_, w)| (*w as i32 - weight as i32).abs()).map(|(f, _)| f.clone()).unwrap_or_default();
        self.faces_for(&file)
    }

    /// [`Family::faces_at`] for separate files: `file` is declared as the normal weight.
    pub fn faces_for(&self, file: &str) -> Vec<(String, u16)> {
        let Some(&(_, chosen)) = self.faces.iter().find(|(f, _)| f == file) else { return self.faces.clone() };
        let (lo, hi) = (chosen.min(400), chosen.max(400));
        let mut out: Vec<(String, u16)> = self.faces.iter().filter(|(f, w)| f != file && (*w < lo || *w > hi)).cloned().collect();
        out.push((file.to_string(), 400));
        out.sort_by_key(|(_, w)| *w);
        out
    }

    /// Every face at its own weight (a variable font: one rule per hundred of its range).
    pub fn real_faces(&self) -> Vec<(String, u16)> {
        match &self.variable {
            Some((file, _, _)) => self.weights().into_iter().map(|w| (file.clone(), w)).collect(),
            None => self.faces.clone(),
        }
    }

    fn has(&self, file: &str) -> bool {
        self.faces.iter().any(|(f, _)| f == file)
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "file": self.primary(),
            "files": self.faces.iter().map(|(f, _)| f).collect::<Vec<_>>(),
            "variable": self.variable.is_some(),
            "weights": self.weights().into_iter().map(|w| json!({ "weight": w, "name": weight_name(w) })).collect::<Vec<_>>(),
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
            None => families.push(Family { name, faces: vec![(f, weight)], variable: None }),
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

/// Path of a font file (an upload wins over a built-in file of the same name).
fn path_of(cfg: &Config, file: &str) -> PathBuf {
    serve_root(cfg, file).join(file)
}

pub fn families(cfg: &Config) -> Vec<Family> {
    let mut fams = group(all_files(cfg));
    for fam in &mut fams {
        fam.variable = fam.faces.iter().find_map(|(f, _)| wght_range(&path_of(cfg, f)).map(|(lo, hi)| (f.clone(), lo, hi)));
    }
    fams
}

fn family_of(cfg: &Config, file: &str) -> Option<Family> {
    families(cfg).into_iter().find(|fam| fam.has(file))
}

/// Font list for templates: `[{ name, file, files, variable, weights: [{ weight, name }] }]`.
pub fn families_json(cfg: &Config) -> Value {
    Value::Array(families(cfg).iter().map(Family::to_json).collect())
}

/// `@font-face` list (`[{ file, weight }]`) for a stored choice in `weight` (None: the family's
/// default, see [`Family::default_weight`]). Empty when unknown.
pub fn faces_json(cfg: &Config, file: Option<&str>, weight: Option<u16>) -> Value {
    let Some(file) = file.filter(|f| !f.is_empty()) else { return json!([]) };
    let Some(fam) = family_of(cfg, file) else { return json!([]) };
    let w = valid_weight(&fam, weight).unwrap_or_else(|| fam.default_weight(file));
    Value::Array(fam.faces_at(w).into_iter().map(|(f, w)| json!({ "file": f, "weight": w })).collect())
}

fn valid_weight(fam: &Family, weight: Option<u16>) -> Option<u16> {
    weight.filter(|w| fam.weights().contains(w))
}

/// A user's weight choice for the font `file`: Some when the family offers it.
pub fn weight_for(cfg: &Config, file: &str, weight: Option<i64>) -> Option<u16> {
    let w = weight.and_then(|w| u16::try_from(w).ok())?;
    family_of(cfg, file).and_then(|fam| valid_weight(&fam, Some(w)))
}

/// The weight plain text is drawn in for a stored choice (the weight select's value).
pub fn effective_weight(cfg: &Config, file: &str, weight: Option<i64>) -> Option<u16> {
    let fam = family_of(cfg, file)?;
    Some(valid_weight(&fam, weight.and_then(|w| u16::try_from(w).ok())).unwrap_or_else(|| fam.default_weight(file)))
}

/// Text sizes (percent) a user can pick besides 100 (stored as NULL).
pub const SIZES: [u16; 7] = [80, 90, 110, 120, 130, 140, 150];

/// A user's text size choice: Some when it is one of [`SIZES`].
pub fn size_for(size: Option<i64>) -> Option<u16> {
    size.and_then(|s| u16::try_from(s).ok()).filter(|s| SIZES.contains(s))
}

/// URL of the navbar brand font's stylesheet (see [`brand_css`]); versioned by the font's name and
/// the chosen weight.
pub fn brand_css_url(file: &str, weight: Option<u16>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    weight.hash(&mut h);
    format!("/brand-font.css?v={:x}", h.finish())
}

/// `/brand-font.css`: an `@font-face` per weight of the family of `file`, as 'BrandFont' (the
/// `.brand-font` class on the navbar brand text uses it; the frontends' CSS files hold that rule).
/// With a chosen `weight` (a `.brand-weight-<n>` class sets it) every face keeps its real weight,
/// so the class picks exactly that face; without one the frontend's own weight is drawn in the
/// family's default weight like a user's font ([`faces_json`]). Empty when no font is set or the
/// font is gone.
pub fn brand_css(cfg: &Config, file: &str, weight: Option<u16>) -> String {
    match weight {
        Some(w) => match family_of(cfg, file).filter(|fam| fam.weights().contains(&w)) {
            Some(fam) => faces_css(&fam.real_faces(), "BrandFont"),
            None => family_css(cfg, file, None, "BrandFont"),
        },
        None => family_css(cfg, file, None, "BrandFont"),
    }
}

/// URL of the stylesheet declaring a visitor's chosen font as 'CustomUserFont' (see
/// [`user_css`]), linked as `#user-font-css` in the page head (settings.js swaps it live).
pub fn user_css_url(file: &str, weight: Option<u16>) -> String {
    match weight {
        Some(w) => format!("/user-font.css?f={}&w={w}", url_enc(file)),
        None => format!("/user-font.css?f={}", url_enc(file)),
    }
}

/// `/user-font.css?f=<file>&w=<weight>`: the family of `file` as 'CustomUserFont', plain text in
/// `weight` ([`faces_json`]). Empty for an unknown font.
pub fn user_css(cfg: &Config, file: &str, weight: Option<u16>) -> String {
    family_css(cfg, file, weight, "CustomUserFont")
}

/// The `@font-face` rules of [`faces_json`], named `alias`.
fn family_css(cfg: &Config, file: &str, weight: Option<u16>, alias: &str) -> String {
    let faces: Vec<(String, u16)> = faces_json(cfg, Some(file), weight)
        .as_array()
        .map(|a| a.iter().filter_map(|f| Some((f["file"].as_str()?.to_string(), u16::try_from(f["weight"].as_u64()?).ok()?))).collect())
        .unwrap_or_default();
    faces_css(&faces, alias)
}

/// One `@font-face` rule per (file, weight), named `alias`.
fn faces_css(faces: &[(String, u16)], alias: &str) -> String {
    faces
        .iter()
        .map(|(f, w)| format!("@font-face {{ font-family: '{alias}'; src: url('/s/fonts/{}'); font-weight: {w}; font-display: swap; }}\n", url_enc(f)))
        .collect()
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

/// Families for the font manager: `[{ name, key, files, faces: [{ file, weight, weight_name,
/// variable ("100–900" or null), uploaded, builtin }] }]`. `uploaded` and `builtin` together: an upload replacing a built-in file.
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
                    "faces": fam.faces.iter().map(|(f, w)| {
                        // A variable font: its weight range instead of one weight
                        let range = fam.variable.as_ref().filter(|(vf, _, _)| vf == f).map(|(_, lo, hi)| format!("{lo}–{hi}"));
                        json!({
                            "file": f,
                            "weight": w,
                            "weight_name": weight_name(*w),
                            "variable": range,
                            "uploaded": uploaded.contains(f),
                            "builtin": builtin.contains(f),
                        })
                    }).collect::<Vec<_>>(),
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
            // A variable font covers its whole range (the samples are drawn in each weight)
            let weight = match &fam.variable {
                Some((vf, lo, hi)) if vf == f => format!("{lo} {hi}"),
                _ => w.to_string(),
            };
            css.push_str(&format!(
                "@font-face {{ font-family: 'FontPreview-{key}'; src: url('/s/fonts/{}'); font-weight: {weight}; font-display: swap; }}\n",
                url_enc(f)
            ));
        }
        css.push_str(&format!(".adm-font-sample-{key} {{ font-family: 'FontPreview-{key}', monospace; }}\n"));
    }
    css
}


// ── variable fonts: the `wght` axis of a font file ──────────────────────────

/// Weight range of a variable font's `wght` axis, None for a static font. Read from the file
/// (ttf/otf, woff, woff2) once per file version.
pub fn wght_range(path: &Path) -> Option<(u16, u16)> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    /// path → (modified, size, range)
    type Cache = HashMap<PathBuf, (std::time::SystemTime, u64, Option<(u16, u16)>)>;
    static CACHE: once_cell::sync::Lazy<Mutex<Cache>> = once_cell::sync::Lazy::new(Default::default);
    let meta = std::fs::metadata(path).ok()?;
    let stamp = (meta.modified().ok()?, meta.len());
    if let Some((m, l, r)) = CACHE.lock().ok()?.get(path)
        && (*m, *l) == stamp
    {
        return *r;
    }
    let r = std::fs::read(path).ok().and_then(|d| fvar_table(&d)).and_then(|t| wght_axis(&t));
    if let Ok(mut c) = CACHE.lock() {
        c.insert(path.to_path_buf(), (stamp.0, stamp.1, r));
    }
    r
}

fn be16(d: &[u8], o: usize) -> Option<usize> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?) as usize)
}

fn be32(d: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// `(lightest, heaviest)` of the `wght` axis in an `fvar` table.
fn wght_axis(t: &[u8]) -> Option<(u16, u16)> {
    let (axes, count, size) = (be16(t, 4)?, be16(t, 8)?, be16(t, 10)?);
    (0..count).find_map(|i| {
        let a = t.get(axes + i * size..axes + i * size + 16)?;
        if &a[0..4] != b"wght" {
            return None;
        }
        let fixed = |o: usize| i32::from_be_bytes(a[o..o + 4].try_into().unwrap()) / 65536;
        let (lo, hi) = (fixed(4).clamp(1, 1000) as u16, fixed(12).clamp(1, 1000) as u16);
        (lo < hi).then_some((lo, hi))
    })
}

/// The `fvar` table of a font file (uncompressed), if it has one.
fn fvar_table(d: &[u8]) -> Option<Vec<u8>> {
    const MAX: u64 = 1 << 20;
    match d.get(..4)? {
        b"wOFF" => {
            for i in 0..be16(d, 12)? {
                let e = 44 + i * 20;
                if d.get(e..e + 4)? != b"fvar" {
                    continue;
                }
                let (off, comp, orig) = (be32(d, e + 4)?, be32(d, e + 8)?, be32(d, e + 12)?);
                let data = d.get(off..off + comp)?;
                if comp >= orig {
                    return Some(data.to_vec());
                }
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut std::io::Read::take(flate2::read::ZlibDecoder::new(data), MAX), &mut out).ok()?;
                return Some(out);
            }
            None
        }
        b"wOF2" => woff2_fvar(d, MAX),
        b"ttcf" => None,
        _ => {
            for i in 0..be16(d, 4)? {
                let r = 12 + i * 16;
                if d.get(r..r + 4)? == b"fvar" {
                    let (off, len) = (be32(d, r + 8)?, be32(d, r + 12)?);
                    return d.get(off..off + len).map(<[u8]>::to_vec);
                }
            }
            None
        }
    }
}

/// WOFF2: the table directory gives each table's length in the brotli stream (tables follow each
/// other unpadded, in directory order); only the stream up to the end of `fvar` is decompressed.
fn woff2_fvar(d: &[u8], max: u64) -> Option<Vec<u8>> {
    if d.get(4..8)? == b"ttcf" {
        return None;
    }
    let base128 = |pos: &mut usize| -> Option<usize> {
        let mut acc: u32 = 0;
        for i in 0..5 {
            let b = *d.get(*pos)?;
            *pos += 1;
            if (i == 0 && b == 0x80) || acc & 0xFE00_0000 != 0 {
                return None;
            }
            acc = (acc << 7) | u32::from(b & 0x7f);
            if b & 0x80 == 0 {
                return Some(acc as usize);
            }
        }
        None
    };
    let (num, compressed) = (be16(d, 12)?, be32(d, 20)?);
    let mut pos = 48;
    let mut offset = 0usize;
    let mut found = None;
    for _ in 0..num {
        let flags = *d.get(pos)?;
        pos += 1;
        let tag: [u8; 4] = match flags & 0x3f {
            63 => {
                let t = d.get(pos..pos + 4)?.try_into().ok()?;
                pos += 4;
                t
            }
            10 => *b"glyf",
            11 => *b"loca",
            47 => *b"fvar",
            _ => *b"    ",
        };
        let version = flags >> 6;
        let orig = base128(&mut pos)?;
        let transformed = if &tag == b"glyf" || &tag == b"loca" { version == 0 } else { version != 0 };
        let len = if transformed { base128(&mut pos)? } else { orig };
        if &tag == b"fvar" && found.is_none() {
            found = Some((offset, len));
        }
        offset += len;
    }
    let (off, len) = found?;
    if (off + len) as u64 > max * 64 {
        return None;
    }
    let stream = d.get(pos..pos + compressed)?;
    let mut out = Vec::new();
    let reader = brotli_decompressor::Decompressor::new(stream, 4096);
    std::io::Read::read_to_end(&mut std::io::Read::take(reader, (off + len) as u64), &mut out).ok()?;
    out.get(off..off + len).map(<[u8]>::to_vec)
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
    fn chosen_weight_becomes_normal() {
        let fams = group(["M-ExtraLight.woff2", "M-Light.woff2", "M-Regular.woff2", "M-Medium.woff2", "M-Bold.woff2", "M-ExtraBold.woff2"].map(String::from).to_vec());
        let m = &fams[0];
        let w = |file: &str| m.faces_for(file).iter().map(|(f, w)| format!("{f}:{w}")).collect::<Vec<_>>().join(" ");
        // the regular face: everything as it is
        assert_eq!(w("M-Regular.woff2"), "M-ExtraLight.woff2:200 M-Light.woff2:300 M-Regular.woff2:400 M-Medium.woff2:500 M-Bold.woff2:700 M-ExtraBold.woff2:800");
        // light text: Light takes 400, bold stays bold
        assert_eq!(w("M-Light.woff2"), "M-ExtraLight.woff2:200 M-Light.woff2:400 M-Medium.woff2:500 M-Bold.woff2:700 M-ExtraBold.woff2:800");
        // heavy text: Bold takes 400, bold text gets ExtraBold
        assert_eq!(w("M-Bold.woff2"), "M-ExtraLight.woff2:200 M-Light.woff2:300 M-Bold.woff2:400 M-ExtraBold.woff2:800");
    }

    /// A minimal sfnt with an `fvar` table (wdth 75–100, wght 200–800).
    fn fvar_font() -> Vec<u8> {
        let mut fvar = vec![0, 1, 0, 0, 0, 16, 0, 2, 0, 2, 0, 20, 0, 0, 0, 0];
        for (tag, lo, def, hi) in [(b"wdth", 75, 100, 100), (b"wght", 200, 400, 800)] {
            fvar.extend_from_slice(tag);
            for v in [lo, def, hi] {
                fvar.extend_from_slice(&((v as i32) << 16).to_be_bytes());
            }
            fvar.extend_from_slice(&[0, 0, 1, 0]);
        }
        let mut d = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        d.extend_from_slice(b"fvar");
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&28u32.to_be_bytes());
        d.extend_from_slice(&(fvar.len() as u32).to_be_bytes());
        d.extend_from_slice(&fvar);
        d
    }

    #[test]
    fn reads_the_weight_axis() {
        let font = fvar_font();
        assert_eq!(fvar_table(&font).and_then(|t| wght_axis(&t)), Some((200, 800)));
        // no fvar: a static font
        let mut plain = font.clone();
        plain[12..16].copy_from_slice(b"name");
        assert_eq!(fvar_table(&plain).and_then(|t| wght_axis(&t)), None);
        assert_eq!(fvar_table(b"wOF2"), None);
    }

    #[test]
    fn variable_weights_and_slots() {
        let fam = Family { name: "S".into(), faces: vec![("S-Regular.ttf".into(), 400)], variable: Some(("S-Regular.ttf".into(), 100, 900)) };
        assert_eq!(fam.weights(), WEIGHTS);
        assert_eq!(fam.default_weight("S-Regular.ttf"), 400);
        let slots = |w: u16| fam.faces_at(w).iter().map(|(_, s)| *s).collect::<Vec<_>>();
        assert_eq!(slots(400), WEIGHTS);
        // light: normal text falls to 300 (nothing at 400-500), bold still 700
        assert_eq!(slots(300), [100, 200, 300, 600, 700, 800, 900]);
        assert_eq!(slots(500), [100, 200, 300, 500, 600, 700, 800, 900]);
        // heavy: normal text goes up to 600 (nothing lighter), bold 700
        assert_eq!(slots(600), [600, 700, 800, 900]);
        // separate files: the chosen weight's file, the closest one for others
        let fams = group(["M-Light.woff2", "M-Regular.woff2", "M-Bold.woff2"].map(String::from).to_vec());
        assert_eq!(fams[0].weights(), [300, 400, 700]);
        assert_eq!(fams[0].default_weight("M-Light.woff2"), 300);
        assert!(fams[0].faces_at(300).contains(&("M-Light.woff2".to_string(), 400)));
    }

    /// The brand font with a chosen weight: every face at its own weight, so the
    /// `.brand-weight-<n>` class picks exactly that face.
    #[test]
    fn real_faces_keep_their_weights() {
        let fams = group(["M-ExtraLight.woff2", "M-Light.woff2", "M-Bold.woff2"].map(String::from).to_vec());
        assert_eq!(fams[0].real_faces(), [("M-ExtraLight.woff2".to_string(), 200), ("M-Light.woff2".to_string(), 300), ("M-Bold.woff2".to_string(), 700)]);
        let var = Family { name: "S".into(), faces: vec![("S.ttf".into(), 400)], variable: Some(("S.ttf".into(), 300, 700)) };
        assert_eq!(var.real_faces().iter().map(|(_, w)| *w).collect::<Vec<_>>(), [300, 400, 500, 600, 700]);
        assert_eq!(faces_css(&var.real_faces()[..1], "BrandFont"), "@font-face { font-family: 'BrandFont'; src: url('/s/fonts/S.ttf'); font-weight: 300; font-display: swap; }\n");
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

