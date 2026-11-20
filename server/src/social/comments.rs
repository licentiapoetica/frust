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
/// info and attachments (polls depend on the viewer: [`crate::social::polls::attach_polls`]). Errors are logged and yield an empty list, like the old code.
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
        // The full fingerprint is the ban key of the identity: only the short form leaves the server
        c.remove("anon_fingerprint");
    }

    if !comments.is_empty() {
        let ids = comment_ids(&comments);
        attach_files(st, &mut comments, &ids).await;
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
            let new = MENTION_RE.replace_all(&content, |caps: &fancy_regex::Captures<str>| {
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

    #[test]
    fn mention_regex_matches_js() {
        let found: Vec<String> = MENTION_RE.captures_iter("hi @bob and [@alice] mail a@b.c @x]").flatten().map(|c| c[1].to_string()).collect();
        // JS: ['bob', 'b.c'] plus backtracking '@x]' → no match for 'x' (only char before ']')
        assert_eq!(found, vec!["bob", "b.c"]);
    }
}
