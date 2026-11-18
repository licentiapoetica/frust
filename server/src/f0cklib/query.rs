//! The search language of `/search` (every frontend's search page): words, phrases and operators,
//! parsed into a [`Query`] and turned into SQL conditions on `items`.
//!
//! - words: each must be part of one of the f0ck's tags (`black cat`: a tag with "black" and one
//!   with "cat", or one with both); in strict mode ("Verbatim") the words between commas name one
//!   whole tag. `"a phrase"` / `tag:x`: a whole tag. `cat*` / `c?t`: wildcards over a whole tag.
//! - `-x` leaves out what `x` finds; `a OR b` / `a | b`: either one.
//! - operators (`key:value`, the value may be quoted): who (`user:`, `fav:`, `commenter:`,
//!   `tagger:`), what (`title:`, `src:`, `type:`, `ext:`, `rating:`, `hall:`, `is:`, `has:`),
//!   numbers (`views:`, `favs:`, `comments:`, `tags:`, `duration:`, `size:`, `width:`, `height:`,
//!   `id:` with `>`, `>=`, `<`, `<=`, `a..b`), time (`year:`, `date:`, `before:`, `after:`, `age:`),
//!   `related:<item>` and `sort:`. An unknown key is a plain word (tags may contain colons).
//!
//! The f0ckgle page "How Search works" (views/frontends/f0ckgle/howsearch.html) explains it to
//! visitors; keep the two in step.

use crate::db::Sql;
use crate::sql;
use chrono::{NaiveDate, TimeZone, Utc};

/// The rating tags (normalized), as the item pages pick an item's rating.
const RATINGS: [&str; 3] = ["sfw", "nsfw", "nsfl"];

/// One search term: what an item must have.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    /// a tag containing the text (exact = the whole tag)
    Tag { text: String, exact: bool },
    /// a tag matching the pattern (`*` any text, `?` one character)
    Wild(String),
    Title(String),
    Src(String),
    User(String),
    Fav(String),
    Commenter(String),
    Tagger(String),
    /// a mime condition: `video/%`, `image/gif`, ...
    Mime(String),
    Ext(String),
    /// sfw / nsfw / nsfl, or "untagged" (none of them)
    Rating(String),
    Hall(String),
    Is(Flag),
    Num { field: NumField, lo: Option<f64>, lo_incl: bool, hi: Option<f64>, hi_incl: bool },
    /// upload time in [from, to) (unix seconds)
    Time { from: Option<i64>, to: Option<i64> },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Flag {
    Oc,
    Pinned,
    MyFav,
    Mine,
    Public,
    Unlisted,
    Private,
    Expiring,
    HasTitle,
    HasSrc,
    HasComments,
    HasFavs,
    HasTags,
    HasCover,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NumField {
    Views,
    Favs,
    Comments,
    Tags,
    Duration,
    Size,
    Width,
    Height,
    Id,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sort {
    New,
    Old,
    Views,
    Favs,
    Comments,
    Long,
    Short,
    Big,
    Small,
    Random,
    Related,
}

/// A term, maybe negated (`-x`).
#[derive(Debug, Clone, PartialEq)]
pub struct Atom {
    pub neg: bool,
    pub term: Term,
}

/// A parsed search: every group must match, a group when any of its atoms does.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub groups: Vec<Vec<Atom>>,
    pub sort: Option<Sort>,
    /// `related:<id or slug>`: the item whose tags the results share
    pub related: Option<String>,
    /// what the search understood, for the results page ("tagged cat", "uploaded by al", ...)
    pub explain: Vec<String>,
    /// parts it couldn't use (an unknown type, a who-operator on an anonymized site, ...)
    pub notes: Vec<String>,
}

impl Query {
    /// The first wanted tag term (a group of its own, not negated): the results show the tag
    /// matching it.
    pub fn first_tag(&self) -> Option<&Term> {
        self.groups.iter().filter(|g| g.len() == 1 && !g[0].neg).map(|g| &g[0].term).find(|t| matches!(t, Term::Tag { .. } | Term::Wild(_)))
    }

    /// The text of the first wanted tag term: the results of a page are ranked by it.
    pub fn first_word(&self) -> Option<&str> {
        match self.first_tag()? {
            Term::Tag { text, .. } | Term::Wild(text) => Some(text.as_str()),
            _ => None,
        }
    }

    /// The tag texts the visitor looked for (for their interests), without negated ones.
    pub fn wanted_tags(&self) -> Vec<String> {
        self.groups
            .iter()
            .flatten()
            .filter(|a| !a.neg)
            .filter_map(|a| match &a.term {
                Term::Tag { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.sort.is_none() && self.related.is_none()
    }
}

// ── tokens ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// a plain word, or a "quoted phrase" (quoted)
    Word { text: String, quoted: bool, neg: bool },
    /// key:value
    Op { key: String, value: String, neg: bool },
    Or,
    /// a comma: ends a run of words (one tag in strict mode)
    Sep,
}

fn tokenize(input: &str) -> Vec<Tok> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let quoted_until = |i: usize| -> (String, usize) {
        // chars[i] is the opening quote
        let mut j = i + 1;
        let mut s = String::new();
        while j < chars.len() && chars[j] != '"' {
            s.push(chars[j]);
            j += 1;
        }
        (s, (j + 1).min(chars.len()))
    };
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == ',' {
            out.push(Tok::Sep);
            i += 1;
            continue;
        }
        if c == '|' {
            out.push(Tok::Or);
            i += 1;
            continue;
        }
        let neg = c == '-' && i + 1 < chars.len() && !chars[i + 1].is_whitespace() && chars[i + 1] != ',';
        if neg {
            i += 1;
        }
        if chars[i] == '"' {
            let (s, next) = quoted_until(i);
            i = next;
            if !s.trim().is_empty() {
                out.push(Tok::Word { text: s.trim().to_string(), quoted: true, neg });
            }
            continue;
        }
        // a bare token, up to whitespace / comma / |; key:"quoted value" keeps its spaces
        let mut s = String::new();
        let mut quoted_value: Option<String> = None;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != ',' && chars[i] != '|' {
            if chars[i] == '"' && s.ends_with(':') {
                let (v, next) = quoted_until(i);
                quoted_value = Some(v);
                i = next;
                break;
            }
            s.push(chars[i]);
            i += 1;
        }
        if s == "OR" && !neg {
            out.push(Tok::Or);
            continue;
        }
        if let Some((key, value)) = s.split_once(':') {
            let value = quoted_value.unwrap_or_else(|| value.to_string());
            if !key.is_empty() && !value.trim().is_empty() {
                out.push(Tok::Op { key: key.to_lowercase(), value: value.trim().to_string(), neg });
                continue;
            }
        }
        if !s.is_empty() {
            out.push(Tok::Word { text: s, quoted: false, neg });
        }
    }
    out
}

// ── parsing ─────────────────────────────────────────────────────────────────

/// Parse a search. `strict`: "Verbatim", the words between commas name whole tags. `who_ok`: the
/// who-operators (user, fav, commenter, tagger) may be used (not on an anonymized site).
pub fn parse(input: &str, strict: bool, who_ok: bool) -> Query {
    let mut q = Query::default();
    // atoms with the "OR" markers between them
    let mut items: Vec<Option<Atom>> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let flush = |run: &mut Vec<String>, items: &mut Vec<Option<Atom>>, q: &mut Query| {
        if !run.is_empty() {
            let text = run.join(" ");
            q.explain.push(format!("tagged \u{201c}{text}\u{201d}"));
            items.push(Some(Atom { neg: false, term: Term::Tag { text, exact: true } }));
            run.clear();
        }
    };
    for tok in tokenize(input) {
        match tok {
            Tok::Word { text, quoted: false, neg: false } if strict && !text.contains(['*', '?']) => run.push(text),
            Tok::Sep => flush(&mut run, &mut items, &mut q),
            Tok::Or => {
                flush(&mut run, &mut items, &mut q);
                items.push(None);
            }
            Tok::Word { text, quoted, neg } => {
                flush(&mut run, &mut items, &mut q);
                let term = if text.contains(['*', '?']) && !quoted {
                    Term::Wild(text.clone())
                } else {
                    Term::Tag { text: text.clone(), exact: quoted || strict }
                };
                q.explain.push(describe(neg, &term));
                items.push(Some(Atom { neg, term }));
            }
            Tok::Op { key, value, neg } => {
                flush(&mut run, &mut items, &mut q);
                match operator(&key, &value, who_ok, &mut q) {
                    Some(Op::Term(term)) => {
                        q.explain.push(describe(neg, &term));
                        items.push(Some(Atom { neg, term }));
                    }
                    Some(Op::Done) => {}
                    None => {
                        // not an operator: a word with a colon in it
                        let text = format!("{key}:{value}");
                        let term = Term::Tag { text, exact: strict };
                        q.explain.push(describe(neg, &term));
                        items.push(Some(Atom { neg, term }));
                    }
                }
            }
        }
    }
    flush(&mut run, &mut items, &mut q);

    // group: atom (OR atom)*
    let mut join_next = false;
    for it in items {
        match it {
            None => join_next = !q.groups.is_empty(),
            Some(a) => {
                if join_next {
                    q.groups.last_mut().expect("a group before OR").push(a);
                } else {
                    q.groups.push(vec![a]);
                }
                join_next = false;
            }
        }
    }
    q
}

enum Op {
    Term(Term),
    /// handled (sort, related) or noted as unusable
    Done,
}

fn operator(key: &str, value: &str, who_ok: bool, q: &mut Query) -> Option<Op> {
    let v = value.trim();
    let lv = v.to_lowercase();
    let who = |q: &mut Query, t: Term| {
        if who_ok {
            Op::Term(t)
        } else {
            q.notes.push(format!("{key}: isn't available here (names are hidden)"));
            Op::Done
        }
    };
    let num = |field: NumField| parse_range(v, field).map(|(lo, lo_incl, hi, hi_incl)| Op::Term(Term::Num { field, lo, lo_incl, hi, hi_incl }));
    Some(match key {
        "tag" => Op::Term(Term::Tag { text: v.to_string(), exact: true }),
        "title" | "intitle" => Op::Term(Term::Title(v.to_string())),
        "src" | "source" | "url" | "site" => Op::Term(Term::Src(v.to_string())),
        "user" | "from" | "by" | "uploader" => who(q, Term::User(v.to_string())),
        "fav" | "favby" | "faved" | "likedby" => who(q, Term::Fav(v.to_string())),
        "commenter" | "commentedby" => who(q, Term::Commenter(v.to_string())),
        "tagger" | "taggedby" => who(q, Term::Tagger(v.to_string())),
        // favs:3 / comments:>2 count; favs:name / comments:name are people
        "favs" => match num(NumField::Favs) {
            Some(op) => op,
            None => who(q, Term::Fav(v.to_string())),
        },
        "comments" => match num(NumField::Comments) {
            Some(op) => op,
            None => who(q, Term::Commenter(v.to_string())),
        },
        "type" | "mime" | "filetype" => match kind_of(&lv) {
            Some(t) => Op::Term(t),
            None => {
                q.notes.push(format!("unknown type \u{201c}{v}\u{201d} (video, image, gif, audio, flash, or a file ending like mp4)"));
                Op::Done
            }
        },
        "ext" => Op::Term(Term::Ext(lv.trim_start_matches('.').to_string())),
        "rating" => match lv.as_str() {
            "sfw" | "nsfw" | "nsfl" | "untagged" => Op::Term(Term::Rating(lv)),
            _ => {
                q.notes.push(format!("unknown rating \u{201c}{v}\u{201d} (sfw, nsfw, nsfl, untagged)"));
                Op::Done
            }
        },
        "hall" | "in" => Op::Term(Term::Hall(v.to_string())),
        "is" | "has" => match flag_of(&lv) {
            Some(Flag::MyFav | Flag::Mine) if !who_ok => {
                q.notes.push(format!("{key}:{lv} isn't available here"));
                Op::Done
            }
            Some(f) => Op::Term(Term::Is(f)),
            // is:mp4 yes, but no unknown word as a file ending (is:xyz is a mistake)
            None => match (kind_of(&lv).filter(|t| !matches!(t, Term::Ext(_))), lv.as_str()) {
                (Some(t), _) => Op::Term(t),
                (None, "sfw" | "nsfw" | "nsfl" | "untagged") => Op::Term(Term::Rating(lv)),
                _ => {
                    q.notes.push(format!("unknown {key}:{lv}"));
                    Op::Done
                }
            },
        },
        "views" => num(NumField::Views)?,
        "tags" => num(NumField::Tags)?,
        "duration" | "length" | "len" => num(NumField::Duration)?,
        "size" => num(NumField::Size)?,
        "width" | "w" => num(NumField::Width)?,
        "height" | "h" => num(NumField::Height)?,
        "id" => num(NumField::Id)?,
        "year" | "date" | "on" => {
            let (from, to) = period(v)?;
            Op::Term(Term::Time { from: Some(from), to: Some(to) })
        }
        "before" => Op::Term(Term::Time { from: None, to: Some(period(v)?.0) }),
        "after" | "since" => Op::Term(Term::Time { from: Some(period(v)?.0), to: None }),
        "age" => {
            // age:<7d = newer than 7 days, age:>1y older than a year
            let (lo, _, hi, _) = age_range(v)?;
            let now = Utc::now().timestamp();
            Op::Term(Term::Time { from: hi.map(|h| now - h as i64), to: lo.map(|l| now - l as i64) })
        }
        "sort" | "order" => {
            let s = match lv.as_str() {
                "new" | "newest" | "latest" | "date" => Sort::New,
                "old" | "oldest" => Sort::Old,
                "views" | "popular" | "top" => Sort::Views,
                "favs" | "fav" | "likes" | "liked" => Sort::Favs,
                "comments" | "discussed" => Sort::Comments,
                "long" | "longest" | "duration" => Sort::Long,
                "short" | "shortest" => Sort::Short,
                "big" | "biggest" | "size" | "large" => Sort::Big,
                "small" | "smallest" | "tiny" => Sort::Small,
                "random" | "shuffle" | "lucky" => Sort::Random,
                "related" | "relevance" => Sort::Related,
                _ => {
                    q.notes.push(format!("unknown sort \u{201c}{v}\u{201d}"));
                    return Some(Op::Done);
                }
            };
            q.sort = Some(s);
            q.explain.push(format!("sorted by {}", sort_name(s)));
            Op::Done
        }
        "related" | "like" | "similar" => {
            q.related = Some(v.trim_start_matches('/').to_string());
            q.explain.push(format!("sharing tags with f0ck {v}"));
            Op::Done
        }
        _ => return None,
    })
}

/// What `type:` (and `is:`) take: a kind (video, image, gif, audio, flash, youtube), a mime type
/// (video/mp4, image/*), or a file ending: the common ones as their mime type (mp4 → video/mp4,
/// jpg → image/jpeg, ...), any other short one as the ending of the file (like `ext:`).
fn kind_of(v: &str) -> Option<Term> {
    if let Some(m) = mime_of(v) {
        return Some(Term::Mime(m));
    }
    let v = v.trim_start_matches('.');
    let mime = match v {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "youtube" | "yt" => "video/youtube",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        e if (2..=5).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()) => return Some(Term::Ext(e.to_string())),
        _ => return None,
    };
    Some(Term::Mime(mime.to_string()))
}

fn mime_of(v: &str) -> Option<String> {
    Some(
        match v {
            "video" | "videos" | "vid" => "video/%",
            "image" | "images" | "img" | "picture" | "pic" => "image/%",
            "gif" | "gifs" => "image/gif",
            "audio" | "music" | "sound" => "audio/%",
            "flash" | "swf" => "application/x-shockwave-flash",
            m if m.contains('/') && m.len() <= 100 => return Some(m.replace('*', "%")),
            _ => return None,
        }
        .to_string(),
    )
}

fn flag_of(v: &str) -> Option<Flag> {
    Some(match v {
        "oc" | "original" => Flag::Oc,
        "pinned" => Flag::Pinned,
        "fav" | "faved" | "favorite" | "liked" => Flag::MyFav,
        "mine" | "my" | "own" => Flag::Mine,
        "public" => Flag::Public,
        "unlisted" => Flag::Unlisted,
        "private" => Flag::Private,
        "expiring" | "temporary" | "temp" => Flag::Expiring,
        "title" => Flag::HasTitle,
        "src" | "source" => Flag::HasSrc,
        "comments" | "comment" => Flag::HasComments,
        "favs" | "favorites" => Flag::HasFavs,
        "tags" => Flag::HasTags,
        "cover" | "coverart" => Flag::HasCover,
        _ => return None,
    })
}

fn flag_name(f: Flag) -> &'static str {
    match f {
        Flag::Oc => "OC",
        Flag::Pinned => "pinned",
        Flag::MyFav => "in my favs",
        Flag::Mine => "mine",
        Flag::Public => "public",
        Flag::Unlisted => "unlisted",
        Flag::Private => "private",
        Flag::Expiring => "expiring",
        Flag::HasTitle => "with a title",
        Flag::HasSrc => "with a source",
        Flag::HasComments => "with comments",
        Flag::HasFavs => "with favs",
        Flag::HasTags => "with tags",
        Flag::HasCover => "with cover art",
    }
}

fn sort_name(s: Sort) -> &'static str {
    match s {
        Sort::New => "newest",
        Sort::Old => "oldest",
        Sort::Views => "views",
        Sort::Favs => "favs",
        Sort::Comments => "comments",
        Sort::Long => "longest",
        Sort::Short => "shortest",
        Sort::Big => "biggest",
        Sort::Small => "smallest",
        Sort::Random => "chance",
        Sort::Related => "shared tags",
    }
}

fn field_name(f: NumField) -> &'static str {
    match f {
        NumField::Views => "views",
        NumField::Favs => "favs",
        NumField::Comments => "comments",
        NumField::Tags => "tags",
        NumField::Duration => "seconds long",
        NumField::Size => "bytes",
        NumField::Width => "px wide",
        NumField::Height => "px high",
        NumField::Id => "as id",
    }
}

/// How the results page says what a term looks for.
pub(crate) fn describe(neg: bool, t: &Term) -> String {
    let d = match t {
        Term::Tag { text, exact: true } => format!("tagged exactly \u{201c}{text}\u{201d}"),
        Term::Tag { text, .. } => format!("tagged \u{201c}{text}\u{201d}"),
        Term::Wild(p) => format!("a tag like \u{201c}{p}\u{201d}"),
        Term::Title(s) => format!("title has \u{201c}{s}\u{201d}"),
        Term::Src(s) => format!("source has \u{201c}{s}\u{201d}"),
        Term::User(s) => format!("uploaded by {s}"),
        Term::Fav(s) => format!("faved by {s}"),
        Term::Commenter(s) => format!("commented on by {s}"),
        Term::Tagger(s) => format!("tagged by {s}"),
        Term::Mime(m) => format!("type {}", m.trim_end_matches("/%")),
        Term::Ext(e) => format!(".{e} files"),
        Term::Rating(r) => r.to_uppercase(),
        Term::Hall(h) => format!("in the hall {h}"),
        Term::Is(f) => flag_name(*f).to_string(),
        Term::Num { field, lo, lo_incl, hi, hi_incl } => {
            let n = |v: f64| if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") };
            match (lo, hi) {
                (Some(a), Some(b)) if a == b => format!("{} {}", n(*a), field_name(*field)),
                (Some(a), Some(b)) => format!("{}–{} {}", n(*a), n(*b), field_name(*field)),
                (Some(a), None) => format!("{}{} {}", if *lo_incl { "≥ " } else { "> " }, n(*a), field_name(*field)),
                (None, Some(b)) => format!("{}{} {}", if *hi_incl { "≤ " } else { "< " }, n(*b), field_name(*field)),
                (None, None) => field_name(*field).to_string(),
            }
        }
        Term::Time { from, to } => {
            let d = |t: i64| Utc.timestamp_opt(t, 0).single().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
            match (from, to) {
                (Some(a), Some(b)) => format!("uploaded {} – {}", d(*a), d(*b - 1)),
                (Some(a), None) => format!("uploaded since {}", d(*a)),
                (None, Some(b)) => format!("uploaded before {}", d(*b)),
                (None, None) => "any time".into(),
            }
        }
    };
    if neg { format!("not {d}") } else { d }
}

// ── the catalog (for the search boxes' help and autocomplete) ───────────────

/// One operator as the search boxes explain and complete it (GET /api/v2/search/syntax). Every
/// key and alias here is one `operator` understands (tests: catalog_matches_the_parser), so a new
/// operator shows up in the f0ckm search overlay by being added here.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct OpDoc {
    pub key: &'static str,
    pub aliases: &'static [&'static str],
    /// what goes after the colon ("name", "n", ...)
    pub arg: &'static str,
    pub about: &'static str,
    /// the values it takes, for completion (empty: free text or a number)
    pub values: &'static [&'static str],
    pub example: &'static str,
    /// people, file, numbers, time, order
    pub group: &'static str,
    /// the value is a user name (completed from /api/v2/users/suggest; off on anonymized sites)
    pub people: bool,
    /// the value is a tag (completed from /api/v2/tags/suggest)
    pub tag: bool,
    /// shown as a quick chip in the search overlay
    pub featured: bool,
}

const fn op_doc(key: &'static str, arg: &'static str, about: &'static str, example: &'static str, group: &'static str) -> OpDoc {
    OpDoc { key, aliases: &[], arg, about, values: &[], example, group, people: false, tag: false, featured: false }
}

const TYPES: &[&str] = &["video", "image", "gif", "audio", "flash", "youtube", "mp4", "webm", "jpg", "png", "webp", "mp3"];
const IS_VALUES: &[&str] = &["oc", "fav", "mine", "pinned", "expiring", "public", "unlisted", "private", "video", "image", "gif", "audio", "flash", "sfw", "nsfw", "nsfl", "untagged"];
const HAS_VALUES: &[&str] = &["title", "src", "comments", "favs", "tags", "cover"];
const SORTS: &[&str] = &["new", "old", "views", "favs", "comments", "long", "short", "big", "small", "random", "related"];

/// The groups of the catalog, in order: (id, title).
pub const GROUPS: &[(&str, &str)] = &[("tags", "Tags"), ("people", "People"), ("file", "The f0ck"), ("numbers", "Numbers"), ("time", "Time"), ("order", "Order")];

pub const OPERATORS: &[OpDoc] = &[
    OpDoc { tag: true, ..op_doc("tag", "name", "a whole tag, like \"quotes\"", "tag:cat", "tags") },
    OpDoc { featured: true, people: true, aliases: &["from", "by", "uploader"], ..op_doc("user", "name", "uploaded by them", "user:name", "people") },
    OpDoc { featured: true, people: true, aliases: &["favby", "faved", "likedby"], ..op_doc("fav", "name", "in their favs (unless private)", "fav:name", "people") },
    OpDoc { people: true, aliases: &["commentedby"], ..op_doc("commenter", "name", "they commented on it", "commenter:name", "people") },
    OpDoc { people: true, aliases: &["taggedby"], ..op_doc("tagger", "name", "they gave it a tag", "tagger:name", "people") },
    OpDoc { featured: true, values: TYPES, aliases: &["mime", "filetype"], ..op_doc("type", "kind", "video, image, gif, audio, flash, youtube, or a file ending (mp4, png, ...)", "type:video", "file") },
    OpDoc { values: &["webm", "mp4", "gif", "jpg", "png", "webp", "mp3", "swf"], ..op_doc("ext", "end", "the file ending", "ext:webm", "file") },
    OpDoc { values: &["sfw", "nsfw", "nsfl", "untagged"], ..op_doc("rating", "r", "sfw, nsfw, nsfl, or untagged", "rating:sfw", "file") },
    OpDoc { aliases: &["intitle"], ..op_doc("title", "text", "the title contains the text", "title:fail", "file") },
    OpDoc { aliases: &["source", "url", "site"], ..op_doc("src", "text", "the source address contains the text", "src:youtube", "file") },
    OpDoc { aliases: &["in"], ..op_doc("hall", "name", "in that hall", "hall:best", "file") },
    OpDoc { featured: true, values: IS_VALUES, ..op_doc("is", "what", "oc, fav (yours), mine, pinned, expiring, private, a type or a rating", "is:oc", "file") },
    OpDoc { featured: true, values: HAS_VALUES, ..op_doc("has", "what", "title, src, comments, favs, tags or cover", "has:comments", "file") },
    OpDoc { aliases: &["like", "similar"], ..op_doc("related", "f0ck", "shares tags with that f0ck (number or link name)", "related:1", "file") },
    OpDoc { featured: true, ..op_doc("views", "n", "how often it was watched: >100, 10..50, 2k", "views:>100", "numbers") },
    op_doc("favs", "n", "how many favs (favs:name: in their favs)", "favs:>=2", "numbers"),
    op_doc("comments", "n", "how many comments", "comments:>0", "numbers"),
    op_doc("tags", "n", "how many tags (without the rating)", "tags:<2", "numbers"),
    OpDoc { featured: true, aliases: &["length", "len"], ..op_doc("duration", "time", "length: 90, 1m30s, 1:30, 2h", "duration:<15s", "numbers") },
    op_doc("size", "bytes", "file size: 500k, 10mb, 1gb", "size:>10mb", "numbers"),
    OpDoc { aliases: &["w"], ..op_doc("width", "px", "width in pixels", "width:>=1920", "numbers") },
    OpDoc { aliases: &["h"], ..op_doc("height", "px", "height in pixels", "height:>=1080", "numbers") },
    op_doc("id", "n", "the f0ck's number", "id:1..100", "numbers"),
    OpDoc { aliases: &["date", "on"], ..op_doc("year", "2024", "uploaded then: 2024, 2024-06, 2024-06-15", "year:2025", "time") },
    OpDoc { aliases: &["since"], ..op_doc("after", "date", "uploaded on that date or later", "after:2025-01", "time") },
    op_doc("before", "date", "uploaded before that date", "before:2025", "time"),
    OpDoc { featured: true, values: &["<1d", "<7d", "<30d", "<1y", ">1y"], ..op_doc("age", "time", "how old: <7d, >1y (h, d, w, mo, y)", "age:<7d", "time") },
    OpDoc { featured: true, values: SORTS, aliases: &["order"], ..op_doc("sort", "how", "new, old, views, favs, comments, long, short, big, small, random, related", "sort:views", "order") },
];

/// The syntax beyond operators, for the help: (what you type, what it does, example).
pub const TRICKS: &[(&str, &str, &str)] = &[
    ("word", "a tag containing the word; every word must match", "cat"),
    ("\"tag\"", "a whole tag", "\"black cat\""),
    ("*  ?", "wildcards over a whole tag", "cat*"),
    ("-x", "leave out what x finds", "cat -dog"),
    ("a OR b", "either one (also a | b)", "cat OR dog"),
];

// ── numbers and dates ───────────────────────────────────────────────────────

/// `>5`, `>=5`, `<5`, `<=5`, `5..10`, `5` (with the field's units: 1m30s / 1:30, 10mb, 2k)
/// → (low, low inclusive, high, high inclusive)
fn parse_range(v: &str, field: NumField) -> Option<(Option<f64>, bool, Option<f64>, bool)> {
    let v = v.trim();
    let val = |s: &str| number_with_unit(s.trim(), field);
    if let Some(r) = v.strip_prefix(">=") {
        return Some((Some(val(r)?), true, None, false));
    }
    if let Some(r) = v.strip_prefix("<=") {
        return Some((None, false, Some(val(r)?), true));
    }
    if let Some(r) = v.strip_prefix('>') {
        return Some((Some(val(r)?), false, None, false));
    }
    if let Some(r) = v.strip_prefix('<') {
        return Some((None, false, Some(val(r)?), false));
    }
    if let Some((a, b)) = v.split_once("..") {
        let lo = if a.trim().is_empty() { None } else { Some(val(a)?) };
        let hi = if b.trim().is_empty() { None } else { Some(val(b)?) };
        return Some((lo, true, hi, true));
    }
    let n = val(v)?;
    Some((Some(n), true, Some(n), true))
}

fn number_with_unit(s: &str, field: NumField) -> Option<f64> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    match field {
        NumField::Duration => duration_secs(&s),
        NumField::Size => {
            let (num, mult) = [("gb", 1e9), ("g", 1e9), ("mb", 1e6), ("m", 1e6), ("kb", 1e3), ("k", 1e3), ("b", 1.0)]
                .iter()
                .find_map(|(u, m)| s.strip_suffix(u).map(|n| (n.trim().to_string(), *m)))
                .unwrap_or((s.clone(), 1.0));
            num.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0).map(|n| n * mult)
        }
        _ => {
            let (num, mult) = [("k", 1e3), ("m", 1e6)].iter().find_map(|(u, m)| s.strip_suffix(u).map(|n| (n.to_string(), *m))).unwrap_or((s.clone(), 1.0));
            num.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0).map(|n| n * mult)
        }
    }
}

/// `90`, `90s`, `1m30s`, `1:30`, `1h`, `2.5m`
fn duration_secs(s: &str) -> Option<f64> {
    if s.contains(':') {
        let mut total = 0.0;
        for part in s.split(':') {
            total = total * 60.0 + part.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)?;
        }
        return Some(total);
    }
    if let Ok(n) = s.parse::<f64>() {
        return (n.is_finite() && n >= 0.0).then_some(n);
    }
    let mut total = 0.0;
    let mut num = String::new();
    let mut any = false;
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            let n: f64 = num.parse().ok()?;
            total += n * match c {
                'h' => 3600.0,
                'm' => 60.0,
                's' => 1.0,
                _ => return None,
            };
            num.clear();
            any = true;
        }
    }
    if !num.is_empty() {
        return None;
    }
    any.then_some(total)
}

/// `7d`, `12h`, `2w`, `3mo`, `1y` (in seconds) for age:
fn age_secs(s: &str) -> Option<f64> {
    let s = s.trim().to_lowercase();
    let (num, mult) = [("mo", 2_592_000.0), ("y", 31_536_000.0), ("w", 604_800.0), ("d", 86_400.0), ("h", 3_600.0)]
        .iter()
        .find_map(|(u, m)| s.strip_suffix(u).map(|n| (n.trim().to_string(), *m)))?;
    num.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0).map(|n| n * mult)
}

fn age_range(v: &str) -> Option<(Option<f64>, bool, Option<f64>, bool)> {
    let v = v.trim();
    if let Some(r) = v.strip_prefix("<=").or_else(|| v.strip_prefix('<')) {
        return Some((None, false, Some(age_secs(r)?), true));
    }
    if let Some(r) = v.strip_prefix(">=").or_else(|| v.strip_prefix('>')) {
        return Some((Some(age_secs(r)?), true, None, false));
    }
    // age:7d = within the last 7 days
    Some((None, false, Some(age_secs(v)?), true))
}

/// `2024`, `2024-06`, `2024-06-15` → [start, end) in unix seconds (UTC)
fn period(v: &str) -> Option<(i64, i64)> {
    let parts: Vec<&str> = v.trim().split(['-', '/', '.']).collect();
    let y: i32 = parts.first()?.parse().ok().filter(|y| (1970..=9999).contains(y))?;
    let ts = |d: NaiveDate| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight")).timestamp();
    match parts.len() {
        1 => Some((ts(NaiveDate::from_ymd_opt(y, 1, 1)?), ts(NaiveDate::from_ymd_opt(y + 1, 1, 1)?))),
        2 => {
            let m: u32 = parts[1].parse().ok()?;
            let start = NaiveDate::from_ymd_opt(y, m, 1)?;
            let end = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1)? } else { NaiveDate::from_ymd_opt(y, m + 1, 1)? };
            Some((ts(start), ts(end)))
        }
        3 => {
            let d = NaiveDate::from_ymd_opt(y, parts[1].parse().ok()?, parts[2].parse().ok()?)?;
            Some((ts(d), ts(d.succ_opt()?)))
        }
        _ => None,
    }
    .filter(|(a, b)| a < b)
}

// ── SQL ─────────────────────────────────────────────────────────────────────

/// Who is searching (for is:fav / is:mine and private favourites).
pub struct Viewer<'a> {
    pub user_id: i64,
    pub owner: Option<&'a str>,
    pub admin: bool,
}

/// `%text%` for ILIKE, the user's own `%`, `_` and `\` taken literally.
fn contains_pattern(s: &str) -> String {
    format!("%{}%", escape_like(s))
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// A wildcard pattern (`*`, `?`) over the whole text, for ILIKE.
fn wild_pattern(s: &str) -> String {
    escape_like(s).replace('*', "%").replace('?', "_")
}

/// A name for ILIKE: exact (any case), or with `*` as a wildcard.
fn name_pattern(s: &str) -> String {
    wild_pattern(s.trim_start_matches('@'))
}

/// The condition on `t` (tags) for a tag term.
pub fn tag_match(term: &Term) -> Option<Sql> {
    Some(match term {
        Term::Tag { text, exact: false } => sql!(
            "(t.tag ILIKE ",
            contains_pattern(text),
            " OR ((select slugify(",
            text.as_str(),
            ")) <> '' AND t.normalized LIKE (select '%' || slugify(",
            text.as_str(),
            ") || '%')))"
        ),
        Term::Tag { text, exact: true } => sql!("(lower(t.tag) = lower(", text.as_str(), ") OR ((select slugify(", text.as_str(), ")) <> '' AND t.normalized = (select slugify(", text.as_str(), "))))"),
        Term::Wild(p) => sql!("t.tag ILIKE ", wild_pattern(p)),
        _ => return None,
    })
}

fn people(col_user: &str, col_login: &str, name: &str) -> Sql {
    let p = name_pattern(name);
    let mut q = Sql::new("(");
    q.push(col_user).push(" ILIKE ").bind(p.clone()).push(" OR ").push(col_login).push(" ILIKE ").bind(p).push(")");
    q
}

pub fn rating_tags_sql() -> String {
    format!("({})", RATINGS.iter().map(|r| format!("'{r}'")).collect::<Vec<_>>().join(", "))
}

fn num_expr(f: NumField) -> String {
    match f {
        NumField::Views => "items.views".into(),
        NumField::Favs => "(SELECT count(*) FROM favorites fc WHERE fc.item_id = items.id)".into(),
        NumField::Comments => "(SELECT count(*) FROM comments cc WHERE cc.item_id = items.id AND cc.is_deleted IS NOT TRUE)".into(),
        NumField::Tags => format!(
            "(SELECT count(*) FROM tags_assign tc JOIN tags tt ON tt.id = tc.tag_id WHERE tc.item_id = items.id AND tt.normalized NOT IN {})",
            rating_tags_sql()
        ),
        NumField::Duration => "COALESCE(items.duration, 0)".into(),
        NumField::Size => "items.size".into(),
        NumField::Width => "COALESCE(items.width, 0)".into(),
        NumField::Height => "COALESCE(items.height, 0)".into(),
        NumField::Id => "items.id".into(),
    }
}

/// The condition for one term (on `items`).
fn term_sql(term: &Term, v: &Viewer) -> Sql {
    let in_tags = |m: Sql| {
        let mut q = Sql::new("items.id IN (SELECT ta.item_id FROM tags_assign ta JOIN tags t ON t.id = ta.tag_id WHERE ");
        q.append(&m).push(")");
        q
    };
    match term {
        Term::Tag { .. } | Term::Wild(_) => in_tags(tag_match(term).expect("a tag term")),
        Term::Title(s) => sql!("items.title ILIKE ", contains_pattern(s)),
        Term::Src(s) => sql!("items.src ILIKE ", contains_pattern(s)),
        Term::User(n) => {
            let p = name_pattern(n);
            sql!(r#"(items.username ILIKE "#, p.clone(), r#" OR items.username IN (SELECT "user" FROM "user" WHERE login ILIKE "#, p, "))")
        }
        Term::Fav(n) => {
            let mut q = Sql::new(r#"items.id IN (SELECT f.item_id FROM favorites f JOIN "user" u ON u.id = f.user_id LEFT JOIN user_options uo ON uo.user_id = u.id WHERE "#);
            q.append(&people(r#"u."user""#, "u.login", n));
            // private favourites only for their owner and admins
            q.push(" AND (uo.favorites_private IS NOT TRUE OR u.id = ").bind(v.user_id as i32).push(" OR ").bind(v.admin).push("))");
            q
        }
        Term::Commenter(n) => {
            let mut q = Sql::new(r#"items.id IN (SELECT c.item_id FROM comments c JOIN "user" u ON u.id = c.user_id WHERE c.is_deleted IS NOT TRUE AND "#);
            q.append(&people(r#"u."user""#, "u.login", n)).push(")");
            q
        }
        Term::Tagger(n) => {
            let mut q = Sql::new(r#"items.id IN (SELECT tg.item_id FROM tags_assign tg JOIN "user" u ON u.id = tg.user_id WHERE "#);
            q.append(&people(r#"u."user""#, "u.login", n)).push(")");
            q
        }
        Term::Mime(m) => sql!("items.mime ILIKE ", m.as_str()),
        Term::Ext(e) => sql!("items.dest ILIKE ", format!("%.{}", escape_like(e))),
        Term::Rating(r) if r == "untagged" => {
            Sql::new(&format!("items.id NOT IN (SELECT ta.item_id FROM tags_assign ta JOIN tags t ON t.id = ta.tag_id WHERE t.normalized IN {})", rating_tags_sql()))
        }
        Term::Rating(r) => in_tags(sql!("t.normalized = ", r.as_str())),
        Term::Hall(h) => {
            let p = name_pattern(h);
            sql!("items.id IN (SELECT ha.item_id FROM halls_assign ha JOIN halls hh ON hh.id = ha.hall_id WHERE hh.slug ILIKE ", p.clone(), " OR hh.name ILIKE ", p, ")")
        }
        Term::Is(f) => match f {
            Flag::Oc => Sql::new("items.is_oc IS TRUE"),
            Flag::Pinned => Sql::new("items.is_pinned IS TRUE"),
            Flag::MyFav => sql!("items.id IN (SELECT item_id FROM favorites WHERE user_id = ", v.user_id as i32, ")"),
            Flag::Mine => sql!("lower(items.username) = lower(", v.owner.unwrap_or("").to_string(), ")"),
            Flag::Public => Sql::new("COALESCE(items.visibility, 0) = 0"),
            Flag::Unlisted => Sql::new("items.visibility = 1"),
            Flag::Private => Sql::new("items.visibility = 2"),
            Flag::Expiring => Sql::new("items.expires_at IS NOT NULL"),
            Flag::HasTitle => Sql::new("COALESCE(btrim(items.title), '') <> ''"),
            Flag::HasSrc => Sql::new("items.src ILIKE 'http%'"),
            Flag::HasComments => Sql::new("EXISTS (SELECT 1 FROM comments hc WHERE hc.item_id = items.id AND hc.is_deleted IS NOT TRUE)"),
            Flag::HasFavs => Sql::new("EXISTS (SELECT 1 FROM favorites hf WHERE hf.item_id = items.id)"),
            Flag::HasTags => Sql::new(&format!(
                "EXISTS (SELECT 1 FROM tags_assign ht JOIN tags htt ON htt.id = ht.tag_id WHERE ht.item_id = items.id AND htt.normalized NOT IN {})",
                rating_tags_sql()
            )),
            Flag::HasCover => Sql::new("items.has_coverart IS TRUE"),
        },
        Term::Num { field, lo, lo_incl, hi, hi_incl } => {
            let e = num_expr(*field);
            let mut q = Sql::new("(TRUE");
            if let Some(a) = lo {
                q.push(" AND ").push(&e).push(if *lo_incl { " >= " } else { " > " }).bind(*a);
            }
            if let Some(b) = hi {
                q.push(" AND ").push(&e).push(if *hi_incl { " <= " } else { " < " }).bind(*b);
            }
            q.push(")");
            q
        }
        Term::Time { from, to } => {
            let mut q = Sql::new("(TRUE");
            if let Some(a) = from {
                q.push(" AND items.stamp >= ").bind(*a);
            }
            if let Some(b) = to {
                q.push(" AND items.stamp < ").bind(*b);
            }
            q.push(")");
            q
        }
    }
}

/// `AND (...) AND (...)` for every group of the query (empty without any).
pub fn conditions(q: &Query, v: &Viewer, related_id: Option<i64>) -> Sql {
    let mut out = Sql::empty();
    for g in &q.groups {
        out.push(" AND (");
        for (i, a) in g.iter().enumerate() {
            if i > 0 {
                out.push(" OR ");
            }
            out.push(if a.neg { "NOT (" } else { "(" }).append(&term_sql(&a.term, v)).push(")");
        }
        out.push(")");
    }
    if let Some(id) = related_id {
        out.push(" AND items.id <> ").bind(id as i32);
        out.push(" AND items.id IN (SELECT ra.item_id FROM tags_assign ra WHERE ra.tag_id IN (SELECT rb.tag_id FROM tags_assign rb JOIN tags rt ON rt.id = rb.tag_id WHERE rb.item_id = ")
            .bind(id as i32)
            .push(&format!(" AND rt.normalized NOT IN {}))", rating_tags_sql()));
    }
    out
}

/// ORDER BY for the sort (the newest first without one).
pub fn order_by(sort: Option<Sort>, related_id: Option<i64>, seed: &str) -> Sql {
    let shared = |id: i64| sql!("(SELECT count(*) FROM tags_assign sa WHERE sa.item_id = items.id AND sa.tag_id IN (SELECT tag_id FROM tags_assign WHERE item_id = ", id as i32, ")) DESC, ");
    let mut q = Sql::new(" ORDER BY ");
    match sort {
        Some(Sort::Related) | None if related_id.is_some() => {
            q.append(&shared(related_id.expect("checked")));
        }
        Some(Sort::Old) => {
            q.push("items.id ASC");
            return q;
        }
        Some(Sort::Views) => {
            q.push("items.views DESC, ");
        }
        Some(Sort::Favs) => {
            q.push(&num_expr(NumField::Favs)).push(" DESC, ");
        }
        Some(Sort::Comments) => {
            q.push(&num_expr(NumField::Comments)).push(" DESC, ");
        }
        Some(Sort::Long) => {
            q.push("items.duration DESC NULLS LAST, ");
        }
        Some(Sort::Short) => {
            q.push("(items.duration IS NULL OR items.duration <= 0), items.duration ASC, ");
        }
        Some(Sort::Big) => {
            q.push("items.size DESC, ");
        }
        Some(Sort::Small) => {
            q.push("items.size ASC, ");
        }
        // the same order on every page of one search (and a new one each day)
        Some(Sort::Random) => {
            q.push("md5(items.id::text || ").bind(seed.to_string()).push("), ");
        }
        Some(Sort::New | Sort::Related) | None => {}
    }
    q.push("items.id DESC");
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(q: &Query) -> Vec<Vec<(bool, Term)>> {
        q.groups.iter().map(|g| g.iter().map(|a| (a.neg, a.term.clone())).collect()).collect()
    }
    fn tag(t: &str, exact: bool) -> Term {
        Term::Tag { text: t.into(), exact }
    }

    #[test]
    fn words_and_phrases() {
        let q = parse("black cat", false, true);
        assert_eq!(terms(&q), vec![vec![(false, tag("black", false))], vec![(false, tag("cat", false))]]);
        assert_eq!(q.first_word(), Some("black"));
        // strict: the words between commas are one whole tag
        let q = parse("black cat, funny", true, true);
        assert_eq!(terms(&q), vec![vec![(false, tag("black cat", true))], vec![(false, tag("funny", true))]]);
        let q = parse("\"black cat\" -dog cat*", false, true);
        assert_eq!(terms(&q), vec![vec![(false, tag("black cat", true))], vec![(true, tag("dog", false))], vec![(false, Term::Wild("cat*".into()))]]);
        // OR / | join a group
        let q = parse("cat OR dog | bird fish", false, true);
        assert_eq!(terms(&q), vec![vec![(false, tag("cat", false)), (false, tag("dog", false)), (false, tag("bird", false))], vec![(false, tag("fish", false))]]);
        // a leading OR has nothing to join
        assert_eq!(terms(&parse("OR cat", false, true)), vec![vec![(false, tag("cat", false))]]);
        // a lone dash is a word
        assert_eq!(terms(&parse("a - b", false, true)).len(), 3);
    }

    #[test]
    fn operators() {
        let q = parse("user:al title:\"two words\" type:video -rating:nsfw sort:views", false, true);
        assert_eq!(
            terms(&q),
            vec![
                vec![(false, Term::User("al".into()))],
                vec![(false, Term::Title("two words".into()))],
                vec![(false, Term::Mime("video/%".into()))],
                vec![(true, Term::Rating("nsfw".into()))],
            ]
        );
        assert_eq!(q.sort, Some(Sort::Views));
        // type: kinds, file endings (as their mime type, else as the ending) and mime types
        assert_eq!(terms(&parse("type:mp4", false, true)), vec![vec![(false, Term::Mime("video/mp4".into()))]]);
        assert_eq!(terms(&parse("type:.jpg", false, true)), vec![vec![(false, Term::Mime("image/jpeg".into()))]]);
        assert_eq!(terms(&parse("type:opus", false, true)), vec![vec![(false, Term::Ext("opus".into()))]]);
        assert_eq!(terms(&parse("is:youtube", false, true)), vec![vec![(false, Term::Mime("video/youtube".into()))]]);
        assert_eq!(terms(&parse("type:image/*", false, true)), vec![vec![(false, Term::Mime("image/%".into()))]]);
        let q = parse("type:mp4 sort:random", false, true);
        assert!(q.notes.is_empty() && q.sort == Some(Sort::Random));
        assert!(!parse("type:not-a-type!", false, true).notes.is_empty());
        // favs: a number or a person
        assert!(matches!(&terms(&parse("favs:>3", false, true))[0][0].1, Term::Num { field: NumField::Favs, lo: Some(l), .. } if *l == 3.0));
        assert_eq!(terms(&parse("favs:bob", false, true)), vec![vec![(false, Term::Fav("bob".into()))]]);
        // an unknown key is a word
        assert_eq!(terms(&parse("re:zero", false, true)), vec![vec![(false, tag("re:zero", false))]]);
        // the who-operators are off on an anonymized site
        let q = parse("user:al cat", false, false);
        assert_eq!(terms(&q), vec![vec![(false, tag("cat", false))]]);
        assert_eq!(q.notes.len(), 1);
        let q = parse("related:abc", false, true);
        assert_eq!(q.related.as_deref(), Some("abc"));
        assert!(q.groups.is_empty() && !q.is_empty());
    }

    /// Every key and alias of the catalog is an operator the parser knows, with its example.
    #[test]
    fn catalog_matches_the_parser() {
        for op in OPERATORS {
            for key in std::iter::once(&op.key).chain(op.aliases.iter()) {
                let q = parse(&format!("{key}:{}", op.example.split_once(':').map(|(_, v)| v).unwrap_or("x")), false, true);
                let plain_word = q.groups.iter().flatten().any(|a| matches!(&a.term, Term::Tag { text, .. } if text.starts_with(&format!("{key}:"))));
                assert!(!plain_word && q.notes.is_empty() && !q.is_empty(), "{key}: {:?} {:?}", q.groups, q.notes);
            }
            assert!(op.example.starts_with(&format!("{}:", op.key)), "{}", op.key);
            for v in op.values {
                let q = parse(&format!("{}:{v}", op.key), false, true);
                assert!(q.notes.is_empty() && !q.is_empty(), "{}:{v}: {:?}", op.key, q.notes);
            }
            assert!(GROUPS.iter().any(|(g, _)| *g == op.group), "{}", op.key);
        }
        for (_, _, ex) in TRICKS {
            assert!(!parse(ex, false, true).groups.is_empty(), "{ex}");
        }
    }

    /// F0CKM_SEARCH_SYNTAX=<file>: write the catalog as GET /api/v2/search/syntax answers it.
    #[test]
    fn dump_syntax() {
        let Ok(out) = std::env::var("F0CKM_SEARCH_SYNTAX") else { return };
        let v = serde_json::json!({ "success": true, "people": true,
            "groups": GROUPS.iter().map(|(id, title)| serde_json::json!({ "id": id, "title": title })).collect::<Vec<_>>(),
            "operators": OPERATORS,
            "tricks": TRICKS.iter().map(|(syntax, about, example)| serde_json::json!({ "syntax": syntax, "about": about, "example": example })).collect::<Vec<_>>() });
        std::fs::write(out, v.to_string()).unwrap();
    }

    #[test]
    fn numbers_and_dates() {
        assert_eq!(parse_range(">=10", NumField::Views), Some((Some(10.0), true, None, false)));
        assert_eq!(parse_range("5..10", NumField::Views), Some((Some(5.0), true, Some(10.0), true)));
        assert_eq!(parse_range("2k", NumField::Views), Some((Some(2000.0), true, Some(2000.0), true)));
        assert_eq!(parse_range(">1m30s", NumField::Duration), Some((Some(90.0), false, None, false)));
        assert_eq!(parse_range("<1:30", NumField::Duration), Some((None, false, Some(90.0), false)));
        assert_eq!(parse_range(">10mb", NumField::Size), Some((Some(10e6), false, None, false)));
        assert_eq!(parse_range("abc", NumField::Views), None);
        assert_eq!(period("2024"), Some((1_704_067_200, 1_735_689_600)));
        assert_eq!(period("2024-12").map(|p| p.1), Some(1_735_689_600));
        assert_eq!(period("2024-02-29").map(|(a, b)| b - a), Some(86_400));
        assert_eq!(period("2023-02-29"), None);
        assert_eq!(period("cat"), None);
        // before:/after: take the start of the period
        assert!(matches!(&terms(&parse("after:2024", false, true))[0][0].1, Term::Time { from: Some(1_704_067_200), to: None }));
        // age:<7d = since 7 days ago
        assert!(matches!(&terms(&parse("age:<7d", false, true))[0][0].1, Term::Time { from: Some(_), to: None }));
        // year:abc isn't an operator: a word
        assert_eq!(terms(&parse("year:abc", false, true)), vec![vec![(false, tag("year:abc", false))]]);
    }

    #[test]
    fn sql_is_bound() {
        let v = Viewer { user_id: 1, owner: Some("al"), admin: false };
        let q = parse("cat% -user:a_b views:>3", false, true);
        let c = conditions(&q, &v, Some(9));
        let text = c.sql();
        assert!(!text.contains("cat"), "user text is bound, not inlined: {text}");
        assert!(text.contains("NOT ("));
        // the user's % and _ are literal in LIKE
        assert_eq!(contains_pattern("cat%"), "%cat\\%%");
        assert_eq!(wild_pattern("a_b*"), "a\\_b%");
    }
}
