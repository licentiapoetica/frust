//! Comment polls: reading them for a viewer, checking a new poll, creating it.
//!
//! A poll belongs to one comment (`comment_polls.comment_id`, unique) and has 2–10 options; every
//! user has at most one vote per poll (`comment_poll_votes (poll_id, user_id)` unique).
//!
//! The poll object the API and the templates get ([`load_polls`]):
//! ```json
//! { "id", "comment_id", "item_id", "question", "is_anonymous", "expires_at", "is_closed",
//!   "options": [{ "id", "text", "vote_count", "pct", "voters": [{ "username", "display_name", "avatar", "avatar_file" }] }],
//!   "total_votes", "user_vote_option_id", "can_vote", "can_unvote", "can_delete" }
//! ```
//! `voters` (the [`VOTERS_SHOWN`] newest; everyone: `GET /api/polls/:id/voters`, or `voters_all` for
//! server-rendered pages, [`Viewer::all_voters`]) only for public polls (`is_anonymous: false`) and never for a viewer who sees the site
//! anonymized. The last four fields are the viewer's own, so a poll is never broadcast whole: the
//! live event (`{ type: "poll", poll_id, comment_id, item_id }` on the `comments` channel) only
//! says which poll changed, and every page fetches it again (`GET /api/polls/:id`).

use crate::db::RowExt;
use crate::session::Session;
use crate::settings::is_anonymize_session;
use crate::social::helpers::js_str;
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};
use std::collections::HashMap;

pub const QUESTION_MAX: usize = 200;
pub const OPTION_MAX: usize = 100;
pub const OPTIONS_MIN: usize = 2;
pub const OPTIONS_MAX: usize = 10;
/// Voters shown under an option; the rest sit behind "+N" (a dialog with everyone)
pub const VOTERS_SHOWN: usize = 5;
/// Longest voter list one request returns
const VOTERS_MAX: i64 = 2000;

pub fn enabled(st: &AppState) -> bool {
    st.cfg().truthy("websrv.enable_comment_polls")
}

/// `websrv.comment_polls_unvote`: may a vote be taken back (`remove=1`)? On when unset. Moving the
/// vote to another option stays possible either way.
pub fn unvote_allowed(st: &AppState) -> bool {
    let cfg = st.cfg();
    let v = cfg.get("websrv.comment_polls_unvote");
    v.is_null() || crate::config::truthy(v)
}

/// Who looks at the polls: decides `user_vote_option_id`, `can_vote`, `can_delete` and whether
/// the voters are shown.
#[derive(Clone, Copy, Default)]
pub struct Viewer {
    pub user_id: Option<i64>,
    pub is_mod: bool,
    pub may_vote: bool,
    pub anonymized: bool,
    /// Every voter as `voters_all` too, for the no-JavaScript dialog of server-rendered pages
    /// (scripts fetch the list when it is opened)
    pub all_voters: bool,
}

impl Viewer {
    pub fn of(st: &AppState, session: Option<&Session>) -> Viewer {
        let cfg = st.cfg();
        let may_vote = crate::perms::can(st, session, crate::perms::P::PollVote);
        Viewer {
            user_id: session.map(|s| s.id),
            is_mod: crate::perms::can(st, session, crate::perms::P::ModComments),
            may_vote,
            anonymized: is_anonymize_session(&cfg, session),
            all_voters: false,
        }
    }

    /// For a server-rendered page: also every voter (see `all_voters`)
    pub fn page(st: &AppState, session: Option<&Session>) -> Viewer {
        Viewer { all_voters: true, ..Viewer::of(st, session) }
    }
}

/// The polls of the given comments, keyed by comment id. Polls of deleted comments are left out.
pub async fn load_polls(st: &AppState, comment_ids: &[i32], viewer: Viewer) -> anyhow::Result<HashMap<i64, Value>> {
    if comment_ids.is_empty() || !enabled(st) {
        return Ok(HashMap::new());
    }
    let polls = sql!(
        r#"SELECT cp.id, cp.comment_id, c.item_id, c.user_id AS author_id, cp.question, cp.is_anonymous, cp.expires_at,
                  (cp.expires_at IS NOT NULL AND cp.expires_at <= now()) AS is_closed
             FROM comment_polls cp
             JOIN comments c ON c.id = cp.comment_id
            WHERE cp.comment_id = ANY("#,
        comment_ids.to_vec(),
        "::int[]) AND c.is_deleted = false"
    )
    .fetch_all(&st.db)
    .await?;
    if polls.is_empty() {
        return Ok(HashMap::new());
    }
    let poll_ids: Vec<i32> = polls.iter().filter_map(|p| p.i64("id")).map(|i| i as i32).collect();

    // Counted only for these polls (the old query grouped the whole votes table on every call)
    let options = sql!(
        r#"SELECT o.id, o.poll_id, o.text, COUNT(v.id)::int AS vote_count
             FROM comment_poll_options o
             LEFT JOIN comment_poll_votes v ON v.option_id = o.id
            WHERE o.poll_id = ANY("#,
        poll_ids.clone(),
        "::int[]) GROUP BY o.id ORDER BY o.poll_id, o.sort_order, o.id"
    )
    .fetch_all(&st.db)
    .await?;

    let mut mine: HashMap<i64, i64> = HashMap::new();
    if let Some(me) = viewer.user_id {
        let rows = sql!("SELECT poll_id, option_id FROM comment_poll_votes WHERE poll_id = ANY(", poll_ids.clone(), "::int[]) AND user_id = ", me as i32)
            .fetch_all(&st.db)
            .await?;
        mine = rows.iter().filter_map(|r| Some((r.i64("poll_id")?, r.i64("option_id")?))).collect();
    }

    let public: Vec<i32> = polls.iter().filter(|p| !p.bool("is_anonymous")).filter_map(|p| p.i64("id")).map(|i| i as i32).collect();
    let limit = if viewer.all_voters { VOTERS_MAX } else { VOTERS_SHOWN as i64 };
    let voters = if viewer.anonymized || public.is_empty() { HashMap::new() } else { load_voters(st, &public, None, limit).await? };

    let mut by_poll: HashMap<i64, Vec<&crate::db::Row>> = HashMap::new();
    for o in &options {
        if let Some(pid) = o.i64("poll_id") {
            by_poll.entry(pid).or_default().push(o);
        }
    }

    let unvote = unvote_allowed(st);
    let mut out = HashMap::new();
    for p in &polls {
        let (Some(id), Some(comment_id)) = (p.i64("id"), p.i64("comment_id")) else { continue };
        let opts = by_poll.remove(&id).unwrap_or_default();
        let total: i64 = opts.iter().filter_map(|o| o.i64("vote_count")).sum();
        let anonymous = p.bool("is_anonymous");
        let closed = p.bool("is_closed");
        let options: Vec<Value> = opts
            .iter()
            .map(|o| {
                let count = o.i64("vote_count").unwrap_or(0);
                let oid = o.i64("id").unwrap_or(0);
                let mut v = json!({ "id": oid, "text": o.val("text"), "vote_count": count, "pct": pct(count, total) });
                if !anonymous && !viewer.anonymized {
                    let all = voters.get(&oid).cloned().unwrap_or_default();
                    v["voters"] = Value::Array(all.iter().take(VOTERS_SHOWN).cloned().collect());
                    if viewer.all_voters && all.len() > VOTERS_SHOWN {
                        v["voters_all"] = Value::Array(all);
                    }
                }
                v
            })
            .collect();
        let is_author = viewer.user_id.is_some() && viewer.user_id == p.i64("author_id");
        out.insert(
            comment_id,
            json!({
                "id": id,
                "comment_id": comment_id,
                "item_id": p.val("item_id"),
                "question": p.val("question"),
                "is_anonymous": anonymous,
                "expires_at": p.val("expires_at"),
                "is_closed": closed,
                "options": options,
                "total_votes": total,
                "user_vote_option_id": mine.get(&id),
                "can_vote": viewer.may_vote && !closed,
                "can_unvote": viewer.may_vote && !closed && unvote,
                "can_delete": is_author || (viewer.is_mod && viewer.user_id.is_some()),
            }),
        );
    }
    Ok(out)
}

/// One poll by its id, for the viewer (`None`: no such poll, or its comment is deleted).
pub async fn load_poll(st: &AppState, poll_id: i32, viewer: Viewer) -> anyhow::Result<Option<Value>> {
    let Some(cid) = sql!("SELECT comment_id FROM comment_polls WHERE id = ", poll_id).fetch_optional(&st.db).await?.and_then(|r| r.i64("comment_id")) else {
        return Ok(None);
    };
    Ok(load_polls(st, &[cid as i32], viewer).await?.remove(&cid))
}

/// Puts each comment's poll under `poll` (null without one). Errors are logged: the comments still show.
pub async fn attach_polls(st: &AppState, comments: &mut [crate::db::Row], viewer: Viewer) {
    let ids: Vec<i32> = comments.iter().filter_map(|c| c.i64("id")).map(|i| i as i32).collect();
    let mut polls = match load_polls(st, &ids, viewer).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[POLLS] load error: {e:#}");
            HashMap::new()
        }
    };
    for c in comments.iter_mut() {
        let p = c.i64("id").and_then(|id| polls.remove(&id)).unwrap_or(Value::Null);
        c.insert("poll".into(), p);
    }
}

/// Whole percent of the total, the same rounding as the old frontend (`Math.round`).
fn pct(count: i64, total: i64) -> i64 {
    if total <= 0 { 0 } else { ((count as f64 / total as f64) * 100.0).round() as i64 }
}

/// Voters per option id, newest vote first, at most `limit` per option (`option`: only that one).
async fn load_voters(st: &AppState, poll_ids: &[i32], option: Option<i32>, limit: i64) -> anyhow::Result<HashMap<i64, Vec<Value>>> {
    let mut q = sql!(
        r#"SELECT option_id, username, display_name, avatar, avatar_file FROM (
             SELECT cpv.option_id, u."user" AS username, uo.display_name, uo.avatar, uo.avatar_file,
                    row_number() OVER (PARTITION BY cpv.option_id ORDER BY cpv.created_at DESC, cpv.id DESC) AS rn
               FROM comment_poll_votes cpv
               JOIN public."user" u ON u.id = cpv.user_id
               LEFT JOIN public.user_options uo ON uo.user_id = cpv.user_id
              WHERE cpv.poll_id = ANY("#,
        poll_ids.to_vec(),
        "::int[])"
    );
    if let Some(o) = option {
        q.push(" AND cpv.option_id = ").bind(o);
    }
    q.push(") v WHERE rn <= ").bind(limit).push(" ORDER BY option_id, rn");
    let rows = q.fetch_all(&st.db).await?;
    let mut m: HashMap<i64, Vec<Value>> = HashMap::new();
    for v in rows {
        if let Some(o) = v.i64("option_id") {
            let display = v.str("display_name").filter(|d| !d.trim().is_empty()).map(|d| json!(d)).unwrap_or(Value::Null);
            m.entry(o).or_default().push(json!({ "username": v.val("username"), "display_name": display, "avatar": v.val("avatar"), "avatar_file": v.val("avatar_file") }));
        }
    }
    Ok(m)
}

/// Everyone who picked one option of a public poll, newest first (the "+N" dialog). `None` when
/// the option isn't this poll's, the poll is anonymous or the viewer sees the site anonymized.
pub async fn option_voters(st: &AppState, poll_id: i32, option_id: i32, viewer: Viewer) -> anyhow::Result<Option<Value>> {
    if viewer.anonymized {
        return Ok(None);
    }
    let Some(o) = sql!(
        "SELECT o.id, o.text, (SELECT count(*) FROM comment_poll_votes v WHERE v.option_id = o.id)::int AS vote_count FROM comment_poll_options o JOIN comment_polls p ON p.id = o.poll_id WHERE o.id = ",
        option_id,
        " AND o.poll_id = ",
        poll_id,
        " AND p.is_anonymous = false"
    )
    .fetch_optional(&st.db)
    .await?
    else {
        return Ok(None);
    };
    let voters = load_voters(st, &[poll_id], Some(option_id), VOTERS_MAX).await?.remove(&(option_id as i64)).unwrap_or_default();
    Ok(Some(json!({ "option": { "id": o.val("id"), "text": o.val("text"), "vote_count": o.val("vote_count") }, "voters": voters })))
}

/// A checked new poll.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub question: String,
    pub options: Vec<String>,
    pub is_anonymous: bool,
}

/// The poll a comment form sends, if any. Two shapes:
/// * `poll`: JSON `{ question, options: [..], is_anonymous }` (the scripts),
/// * `poll_question`, `poll_options` (one option per line), `poll_anonymous` (the plain forms; an
///   unticked box sends nothing, so a missing `poll_anonymous` means public votes).
///
/// `Ok(None)`: no poll asked for (no question and no options). `Err`: a message for the visitor.
pub fn draft_from_body(body: &Value) -> Result<Option<Draft>, String> {
    let (question, options, anonymous) = match body.get("poll") {
        Some(Value::String(s)) if !s.trim().is_empty() => match serde_json::from_str::<Value>(s) {
            Ok(v) => from_json(&v)?,
            Err(_) => return Err("Invalid poll".into()),
        },
        Some(v @ Value::Object(_)) => from_json(v)?,
        _ => {
            let q = js_str(body.get("poll_question")).unwrap_or_default();
            let opts: Vec<String> = match body.get("poll_options") {
                Some(Value::Array(a)) => a.iter().filter_map(|o| js_str(Some(o))).collect(),
                v => js_str(v).unwrap_or_default().lines().map(str::to_string).collect(),
            };
            let anon = body.get("poll_anonymous").is_some_and(crate::config::truthy);
            (q, opts, anon)
        }
    };
    check(question, options, anonymous)
}

fn from_json(v: &Value) -> Result<(String, Vec<String>, bool), String> {
    let question = js_str(v.get("question")).unwrap_or_default();
    let options = match v.get("options") {
        Some(Value::Array(a)) => a.iter().filter_map(|o| js_str(Some(o))).collect(),
        _ => vec![],
    };
    Ok((question, options, v.get("is_anonymous") != Some(&Value::Bool(false))))
}

fn check(question: String, options: Vec<String>, is_anonymous: bool) -> Result<Option<Draft>, String> {
    let question = question.trim().to_string();
    let options: Vec<String> = options.into_iter().map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
    if question.is_empty() && options.is_empty() {
        return Ok(None);
    }
    if question.is_empty() {
        return Err("The poll needs a question".into());
    }
    if question.chars().count() > QUESTION_MAX {
        return Err(format!("Poll question too long (max {QUESTION_MAX} characters)"));
    }
    if options.len() < OPTIONS_MIN || options.len() > OPTIONS_MAX {
        return Err(format!("A poll needs {OPTIONS_MIN} to {OPTIONS_MAX} options"));
    }
    if options.iter().any(|o| o.chars().count() > OPTION_MAX) {
        return Err(format!("Poll option too long (max {OPTION_MAX} characters)"));
    }
    let mut seen: Vec<String> = Vec::with_capacity(options.len());
    for o in &options {
        let k = o.to_lowercase();
        if seen.contains(&k) {
            return Err("Poll options must be different".into());
        }
        seen.push(k);
    }
    Ok(Some(Draft { question, options, is_anonymous }))
}

/// Runs the word filter over the question and the options, as over the comment text.
pub async fn filter_words(st: &AppState, d: Draft) -> Draft {
    let question = crate::wordfilter::apply_word_filter(st, &d.question).await;
    let mut options = Vec::with_capacity(d.options.len());
    for o in &d.options {
        options.push(crate::wordfilter::apply_word_filter(st, o).await);
    }
    Draft { question, options, ..d }
}

/// Creates the poll for a comment in one statement (poll and options together).
pub async fn create<'e, E: sqlx::PgExecutor<'e>>(db: E, comment_id: i32, d: &Draft) -> anyhow::Result<i64> {
    let row = sql!(
        "WITH p AS (INSERT INTO comment_polls (comment_id, question, is_anonymous) VALUES (",
        comment_id,
        ", ",
        d.question.as_str(),
        ", ",
        d.is_anonymous,
        ") RETURNING id), o AS (INSERT INTO comment_poll_options (poll_id, text, sort_order) SELECT p.id, t.text, (t.ord - 1)::smallint FROM p, unnest(",
        d.options.clone(),
        "::text[]) WITH ORDINALITY AS t(text, ord)) SELECT id FROM p"
    )
    .fetch_optional(db)
    .await?;
    row.and_then(|r| r.i64("id")).ok_or_else(|| anyhow::anyhow!("poll insert returned nothing"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts() {
        // nothing asked for
        assert_eq!(draft_from_body(&json!({ "content": "hi" })), Ok(None));
        assert_eq!(draft_from_body(&json!({ "poll_question": "  ", "poll_options": "\n \n" })), Ok(None));
        // the plain form: one option per line, box unticked = public
        assert_eq!(
            draft_from_body(&json!({ "poll_question": " Best? ", "poll_options": "a\r\n\n b \nc" })),
            Ok(Some(Draft { question: "Best?".into(), options: vec!["a".into(), "b".into(), "c".into()], is_anonymous: false }))
        );
        assert_eq!(draft_from_body(&json!({ "poll_question": "q", "poll_options": "a\nb", "poll_anonymous": "1" })).unwrap().unwrap().is_anonymous, true);
        // the scripts' JSON
        let d = draft_from_body(&json!({ "poll": "{\"question\":\"q\",\"options\":[\"x\",\"y\"]}" })).unwrap().unwrap();
        assert!(d.is_anonymous);
        assert_eq!(d.options, vec!["x", "y"]);
        // mistakes are reported, not dropped
        assert!(draft_from_body(&json!({ "poll_question": "q", "poll_options": "only one" })).is_err());
        assert!(draft_from_body(&json!({ "poll_options": "a\nb" })).is_err());
        assert!(draft_from_body(&json!({ "poll_question": "q", "poll_options": "A\na" })).is_err());
        assert!(draft_from_body(&json!({ "poll_question": "q", "poll_options": (0..11).map(|i| i.to_string()).collect::<Vec<_>>().join("\n") })).is_err());
        assert!(draft_from_body(&json!({ "poll_question": "q".repeat(QUESTION_MAX + 1), "poll_options": "a\nb" })).is_err());
        assert!(draft_from_body(&json!({ "poll": "{nope" })).is_err());
    }

    #[test]
    fn percentages() {
        assert_eq!(pct(0, 0), 0);
        assert_eq!(pct(1, 3), 33);
        assert_eq!(pct(2, 3), 67);
        assert_eq!(pct(5, 5), 100);
    }
}
