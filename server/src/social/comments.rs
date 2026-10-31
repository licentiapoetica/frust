//! Comment loading and processing (port of the comment parts of f0cklib.mjs).

use super::helpers::resolve_numeric_item_id;
use crate::db::{Row, RowExt};
use crate::sql;
use crate::state::AppState;
use fancy_regex::Regex as FancyRegex;
use once_cell::sync::Lazy;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

/// Sort order of `getComments`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    New,
    Old,
}

impl Sort {
    /// `sort !== 'new'` means oldest first.
    pub fn from_query(q: Option<&str>) -> Sort {
        match q {
            None | Some("new") => Sort::New,
            Some(_) => Sort::Old,
        }
    }
}

/// `f0cklib.getComments(itemId, sort, process)` — comments of an item (id or slug) with author
/// info, attachments and polls. Errors are logged and yield an empty list, like the old code.
pub async fn get_comments(st: &AppState, item: &str, sort: Sort, process: bool) -> Vec<Row> {
    let Some(id) = resolve_numeric_item_id(st, item).await else { return vec![] };
    let started = std::time::Instant::now();
    match fetch_comments(st, id, sort).await {
        Ok(rows) => {
            tracing::info!("[GETCOMMENTS] Fetched {} comments for item {item} in {}ms", rows.len(), started.elapsed().as_millis());
            if process { process_mentions(st, rows).await } else { rows }
        }
        Err(e) => {
            tracing::error!("[F0CKLIB] Error fetching comments: {e:#}");
            vec![]
        }
    }
}

async fn fetch_comments(st: &AppState, item_id: i64, sort: Sort) -> anyhow::Result<Vec<Row>> {
    let order = if sort == Sort::New { "c.created_at DESC" } else { "c.created_at ASC" };
    let mut q = sql!(
        r#"SELECT
                c.id, c.parent_id, c.content, c.created_at, c.vote_score, c.is_deleted,
                COALESCE(c.is_pinned, false) as is_pinned,
                c.video_time,
                u.user as username, u.id as user_id, uo.avatar, uo.avatar_file, uo.username_color, uo.display_name, uo.banner_file, uo.banner_position, uo.banner_size, uo.banner_repeat,
                (SELECT count(*) FROM comments r WHERE r.parent_id = c.id) as reply_count,
                ai.fingerprint as anon_fingerprint
            FROM comments c
            JOIN "user" u ON c.user_id = u.id
            LEFT JOIN user_options uo ON uo.user_id = u.id
            LEFT JOIN anon_identities ai ON ai.user_id = u.id
            WHERE c.item_id = "#,
        item_id,
        " AND c.is_deleted = false ORDER BY COALESCE(c.is_pinned, false) DESC, "
    );
    q.push(order);
    let mut comments = q.fetch_all(&st.db).await?;

    for c in comments.iter_mut() {
        if let Some(fp) = c.str("anon_fingerprint").filter(|f| !f.is_empty()).map(str::to_string) {
            c.insert("is_anon".into(), json!(true));
            c.insert("username".into(), json!("anonymous"));
            c.insert("display_name".into(), json!("Anonymous"));
            c.insert("anon_short_fingerprint".into(), json!(js_slice(&fp, 7, 15)));
        }
    }

    if !comments.is_empty() {
        let ids = comment_ids(&comments);
        attach_files(st, &mut comments, &ids).await;
        match load_polls(st, &ids, true).await {
            Ok(mut polls) => {
                for c in comments.iter_mut() {
                    let p = c.i64("id").and_then(|id| polls.remove(&id)).unwrap_or(Value::Null);
                    c.insert("poll".into(), p);
                }
            }
            Err(e) => {
                tracing::error!("[POLLS] getComments poll fetch error: {e:#}");
                for c in comments.iter_mut() {
                    c.insert("poll".into(), Value::Null);
                }
            }
        }
    }
    Ok(comments)
}

/// `str.slice(a, b)` on UTF-16 code units (fingerprints are ASCII; chars are close enough).
fn js_slice(s: &str, a: usize, b: usize) -> String {
    s.chars().skip(a).take(b.saturating_sub(a)).collect()
}

pub fn comment_ids(rows: &[Row]) -> Vec<i32> {
    rows.iter().filter_map(|c| c.i64("id")).filter_map(|i| i32::try_from(i).ok()).collect()
}

/// Attachment rows (`comment_files`) per comment id.
pub async fn load_files(st: &AppState, ids: &[i32]) -> anyhow::Result<HashMap<i64, Vec<Value>>> {
    let files = sql!(
        "SELECT id, comment_id, dest, mime, size, original_filename FROM comment_files WHERE comment_id = ANY(",
        ids.to_vec(),
        "::int[]) ORDER BY id ASC"
    )
    .fetch_all(&st.db)
    .await?;
    let mut map: HashMap<i64, Vec<Value>> = HashMap::new();
    for f in files {
        if let Some(cid) = f.i64("comment_id") {
            map.entry(cid).or_default().push(Value::Object(f));
        }
    }
    Ok(map)
}

/// Sets `files` on every comment (empty on error, like the old code).
pub async fn attach_files(st: &AppState, comments: &mut [Row], ids: &[i32]) {
    let mut map = load_files(st, ids).await.unwrap_or_default();
    for c in comments.iter_mut() {
        let files = c.i64("id").and_then(|id| map.remove(&id)).unwrap_or_default();
        c.insert("files".into(), Value::Array(files));
    }
}

/// Polls of the given comments, keyed by comment id, in the shape the frontend expects.
/// `with_voters` adds voter lists to options of non-anonymous polls.
pub async fn load_polls(st: &AppState, ids: &[i32], with_voters: bool) -> anyhow::Result<HashMap<i64, Value>> {
    let rows = sql!(
        r#"SELECT
              cp.id as poll_id,
              cp.comment_id,
              cp.question,
              cp.expires_at,
              COALESCE(cp.is_anonymous, true) as is_anonymous,
              json_agg(
                json_build_object(
                  'id', cpo.id,
                  'text', cpo.text,
                  'sort_order', cpo.sort_order,
                  'vote_count', COALESCE(vote_counts.cnt, 0)
                ) ORDER BY cpo.sort_order ASC, cpo.id ASC
              ) AS options,
              COALESCE(SUM(vote_counts.cnt), 0)::int AS total_votes
            FROM comment_polls cp
            JOIN comment_poll_options cpo ON cpo.poll_id = cp.id
            LEFT JOIN (
              SELECT option_id, COUNT(*) AS cnt
              FROM comment_poll_votes
              GROUP BY option_id
            ) vote_counts ON vote_counts.option_id = cpo.id
            WHERE cp.comment_id = ANY("#,
        ids.to_vec(),
        "::int[]) GROUP BY cp.id, cp.comment_id, cp.question, cp.expires_at, cp.is_anonymous"
    )
    .fetch_all(&st.db)
    .await?;

    let mut voters: HashMap<i64, Vec<Value>> = HashMap::new();
    if with_voters {
        let non_anon: Vec<i32> = rows.iter().filter(|p| !p.bool("is_anonymous")).filter_map(|p| p.i64("poll_id")).map(|i| i as i32).collect();
        if !non_anon.is_empty() {
            voters = load_voters(st, &non_anon).await?;
        }
    }

    let mut map = HashMap::new();
    for p in rows {
        let anonymous = p.bool("is_anonymous");
        let mut options = p.val("options").clone();
        if with_voters && !anonymous {
            add_voters(&mut options, &voters);
        }
        let Some(cid) = p.i64("comment_id") else { continue };
        map.insert(
            cid,
            json!({
                "id": p.val("poll_id"),
                "question": p.val("question"),
                "expires_at": p.val("expires_at"),
                "is_anonymous": p.val("is_anonymous"),
                "options": options,
                "total_votes": p.i64("total_votes").unwrap_or(0),
                "user_vote_option_id": null
            }),
        );
    }
    Ok(map)
}

/// Voters per option id for the given polls.
pub async fn load_voters(st: &AppState, poll_ids: &[i32]) -> anyhow::Result<HashMap<i64, Vec<Value>>> {
    let rows = sql!(
        r#"SELECT cpv.option_id, u."user" as username, uo.avatar, uo.avatar_file
              FROM comment_poll_votes cpv
              JOIN public."user" u ON u.id = cpv.user_id
              LEFT JOIN public.user_options uo ON uo.user_id = cpv.user_id
              WHERE cpv.poll_id = ANY("#,
        poll_ids.to_vec(),
        "::int[])"
    )
    .fetch_all(&st.db)
    .await?;
    let mut m: HashMap<i64, Vec<Value>> = HashMap::new();
    for v in rows {
        if let Some(o) = v.i64("option_id") {
            m.entry(o).or_default().push(json!({ "username": v.val("username"), "avatar": v.val("avatar"), "avatar_file": v.val("avatar_file") }));
        }
    }
    Ok(m)
}

/// `options.map(o => ({ ...o, voters: votersByOption.get(o.id) || [] }))`
pub fn add_voters(options: &mut Value, voters: &HashMap<i64, Vec<Value>>) {
    if let Value::Array(opts) = options {
        for o in opts.iter_mut() {
            if let Value::Object(om) = o {
                let id = om.get("id").and_then(crate::config::as_int).unwrap_or(-1);
                om.insert("voters".into(), Value::Array(voters.get(&id).cloned().unwrap_or_default()));
            }
        }
    }
}

/// `f0cklib.getComment(id, process)` — one non-deleted comment with attachments.
pub async fn get_comment(st: &AppState, id: i64, process: bool) -> Option<Row> {
    if id == 0 {
        return None;
    }
    let row = sql!(
        r#"SELECT
                c.id, c.parent_id, c.item_id, c.content, c.created_at, c.vote_score, c.is_deleted,
                COALESCE(c.is_pinned, false) as is_pinned,
                c.video_time,
                u.user as username, u.id as user_id, uo.avatar, uo.avatar_file, uo.username_color, uo.display_name, uo.banner_file, uo.banner_position, uo.banner_size, uo.banner_repeat
            FROM comments c
            JOIN "user" u ON c.user_id = u.id
            LEFT JOIN user_options uo ON uo.user_id = u.id
            WHERE c.id = "#,
        id,
        " AND c.is_deleted = false LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await;
    let mut comment = match row {
        Ok(Some(c)) => c,
        Ok(None) => return None,
        Err(e) => {
            tracing::error!("[F0CKLIB] Error fetching comment: {e:#}");
            return None;
        }
    };
    let files = match i32::try_from(id) {
        Ok(i) => load_files(st, &[i]).await.map(|mut m| m.remove(&id).unwrap_or_default()).unwrap_or_default(),
        Err(_) => vec![],
    };
    comment.insert("files".into(), Value::Array(files));
    if process { process_mentions(st, vec![comment]).await.into_iter().next() } else { Some(comment) }
}

static MENTION_RE: Lazy<FancyRegex> = Lazy::new(|| FancyRegex::new(r"(?<!\[)@([a-zA-Z0-9_\-\.]+)(?!\])").expect("mention regex"));

/// `processMentions(comments)` — turns `@name` of existing users into markdown links
/// `[@name](/user/name)`, then runs [`process_embeds`]. Comment text stays markdown; it is
/// rendered (and sanitized) in the browser.
pub async fn process_mentions(st: &AppState, comments: Vec<Row>) -> Vec<Row> {
    if comments.is_empty() {
        return comments;
    }
    let mut all: Vec<String> = Vec::new();
    for c in &comments {
        let content = c.string("content");
        for m in MENTION_RE.captures_iter(&content).flatten() {
            if let Some(name) = m.get(1) {
                let n = name.as_str().to_lowercase();
                if !all.contains(&n) {
                    all.push(n);
                }
            }
        }
    }
    if all.is_empty() {
        return process_embeds(st, comments);
    }
    let valid: HashSet<String> = match sql!(r#"SELECT login FROM "user" WHERE login = ANY("#, all, ")").fetch_all(&st.db).await {
        Ok(rows) => rows.iter().map(|r| r.string("login")).collect(),
        Err(e) => {
            tracing::error!("Error verifying mentions: {e}");
            return process_embeds(st, comments);
        }
    };
    let processed = comments
        .into_iter()
        .map(|mut c| {
            let content = c.string("content");
            let new = MENTION_RE.replace_all(&content, |caps: &fancy_regex::Captures| {
                let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                if valid.contains(&name.to_lowercase()) { format!("[@{name}](/user/{name})") } else { whole.to_string() }
            });
            c.insert("content".into(), Value::String(new.into_owned()));
            c
        })
        .collect();
    process_embeds(st, processed)
}

/// `processEmbeds(comments)` — bare links to images on this site become markdown images.
pub fn process_embeds(st: &AppState, comments: Vec<Row>) -> Vec<Row> {
    if comments.is_empty() {
        return comments;
    }
    let cfg = st.cfg();
    let site = cfg.str_or("main.url.full", "").to_string();
    if site.is_empty() {
        return comments;
    }
    let pattern = format!(r"(?i)({}(?:/\S+\.(?:jpg|jpeg|png|gif|webp)))", regex::escape(&site));
    let Ok(re) = regex::Regex::new(&pattern) else { return comments };
    comments
        .into_iter()
        .map(|mut c| {
            let content = c.string("content");
            if content.is_empty() {
                return c;
            }
            let new = re.replace_all(&content, "![site image](${1})").into_owned();
            c.insert("content".into(), Value::String(new));
            c
        })
        .collect()
}

static XD_RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)x(D+)").expect("xd regex"));

/// `computeXdScore(comments)` — total length of all `xD+` runs in non-deleted comments.
pub fn compute_xd_score(comments: &[Row]) -> i64 {
    let mut score = 0i64;
    for c in comments {
        if c.bool("is_deleted") {
            continue;
        }
        let Some(content) = c.str("content").filter(|s| !s.is_empty()) else { continue };
        for m in XD_RE.captures_iter(content) {
            score += m.get(1).map(|d| d.as_str().chars().count() as i64).unwrap_or(0);
        }
    }
    score
}

/// `xdScoreMeta(score)` → `(tier, label)`.
pub fn xd_score_meta(score: i64) -> (i64, &'static str) {
    match score {
        s if s < 1 => (0, ""),
        s if s < 200 => (1, "xD"),
        s if s < 1000 => (2, "xDD"),
        s if s < 100_000 => (3, "xDDD"),
        s if s < 20_000_000 => (4, "xDDDD"),
        _ => (5, "xDDDDD+"),
    }
}

/// `xdScoreMeta` as the JSON object `{ tier, label }`.
pub fn xd_score_meta_value(score: i64) -> Value {
    let (tier, label) = xd_score_meta(score);
    json!({ "tier": tier, "label": label })
}

/// `f0cklib.getSubscriptionStatus(userId, itemId)`
pub async fn get_subscription_status(st: &AppState, user_id: i64, item: &str) -> bool {
    if st.cfg().get("enable_comments") == &Value::Bool(false) {
        return false;
    }
    let Some(id) = resolve_numeric_item_id(st, item).await else { return false };
    if user_id == 0 {
        return false;
    }
    sql!(
        "SELECT 1 FROM comment_subscriptions WHERE user_id = ",
        user_id,
        " AND item_id = ",
        id,
        " AND is_subscribed = true"
    )
    .fetch_optional(&st.db)
    .await
    .map(|r| r.is_some())
    .unwrap_or(false)
}

/// `f0cklib.markNotificationsRead(userId, itemId)`
pub async fn mark_notifications_read(st: &AppState, user_id: i64, item: &str) {
    let Some(id) = resolve_numeric_item_id(st, item).await else { return };
    if user_id == 0 {
        return;
    }
    if let Err(e) = sql!("UPDATE notifications SET is_read = true WHERE user_id = ", user_id, " AND item_id = ", id, " AND is_read = false")
        .execute(&st.db)
        .await
    {
        tracing::error!("[F0CKLIB] Error marking notifications as read: {e}");
    }
}

/// Values the item page needs for its xD badge: `(xd_score, xd_tier, xd_label)` computed from the
/// item's comments (the `commentsForScore` block in index.mjs / ajax.mjs / user_halls.mjs).
pub async fn item_xd_score(st: &AppState, item: &str) -> (i64, i64, &'static str) {
    let comments = get_comments(st, item, Sort::Old, false).await;
    let score = compute_xd_score(&comments);
    let (tier, label) = xd_score_meta(score);
    (score, tier, label)
}

/// Anonymized copy of a comment row (`isAnonymizeSession`), as in GET /api/comments.
pub fn anonymize(mut c: Row) -> Row {
    for (k, v) in [("username", json!("anonymous")), ("display_name", Value::Null), ("username_color", Value::Null), ("avatar", Value::Null), ("avatar_file", Value::Null)] {
        c.insert(k.into(), v);
    }
    c
}

pub fn rows_to_value(rows: Vec<Row>) -> Value {
    Value::Array(rows.into_iter().map(Value::Object).collect())
}

pub fn empty_obj() -> Map<String, Value> {
    Map::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(content: &str, deleted: bool) -> Row {
        let mut r = Row::new();
        r.insert("content".into(), json!(content));
        r.insert("is_deleted".into(), json!(deleted));
        r
    }

    #[test]
    fn xd_score() {
        let rows = vec![row("xD xDDD xddd", false), row("xDDDDD", true), row("nothing", false)];
        assert_eq!(compute_xd_score(&rows), 1 + 3 + 3);
        assert_eq!(xd_score_meta(0), (0, ""));
        assert_eq!(xd_score_meta(199), (1, "xD"));
        assert_eq!(xd_score_meta(200), (2, "xDD"));
        assert_eq!(xd_score_meta(20_000_000), (5, "xDDDDD+"));
    }

    #[test]
    fn mention_regex_matches_js() {
        let found: Vec<String> = MENTION_RE.captures_iter("hi @bob and [@alice] mail a@b.c @x]").flatten().map(|c| c[1].to_string()).collect();
        // JS: ['bob', 'b.c'] plus backtracking '@x]' → no match for 'x' (only char before ']')
        assert_eq!(found, vec!["bob", "b.c"]);
    }
}
