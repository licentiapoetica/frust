//! Recommendations, user affinities and the tag feed (port of the end of
//! src/inc/routeinc/f0cklib.mjs: `getRandomRecommendations`, `getRandomVideos`,
//! `updateUserAffinity`, `updateUserTagAffinity`, `decayUserAffinities`,
//! `getPersonalizedRecommendations`, `getTagFeedItems`).

use super::{compute_base_mode, global_filter, resolve_mime_sql, resolve_numeric_item_id, xd_score_meta};
use crate::db::{Row, RowExt, Sql};
use crate::session::Session;
use crate::settings::is_anon_session;
use crate::sql;
use crate::state::AppState;
use rand::Rng;
use serde_json::{Map, Value, json};

/// Options of the recommendation / tag feed functions (`{ limit, mode, ratings, session, ... }`).
#[derive(Default, Clone)]
pub struct RecOpts<'a> {
    /// `limit` (any number; NaN/0 → default)
    pub limit: Option<f64>,
    pub mode: Option<i64>,
    pub ratings: Option<Vec<String>>,
    pub session: Option<&'a Session>,
    pub exclude: Vec<i64>,
    pub user_id: Option<i64>,
    pub is_admin: bool,
    pub mime: Option<String>,
    pub exclude_ids: Vec<i64>,
    pub session_tags: String,
    pub session_creators: String,
    pub continuation: bool,
    pub prefer_personalized: Option<bool>,
}

/// `Math.min(Math.max(1, Number(limit) || dflt), 50)`
fn clamp_limit(limit: Option<f64>, dflt: f64) -> i64 {
    let l = limit.filter(|l| l.is_finite() && *l != 0.0).unwrap_or(dflt);
    // postgres casts a fractional LIMIT parameter to bigint (rounding)
    l.clamp(1.0, 50.0).round() as i64
}

/// The filters every query here shares.
struct Common {
    modequery: String,
    globalfilter: Option<String>,
    excluded: Vec<i32>,
    visibility: Sql,
    exclude_ids: Vec<i32>,
    mime_sql: Sql,
    mime_parts: Vec<String>,
}

impl Common {
    fn new(st: &AppState, o: &RecOpts) -> Common {
        let cfg = st.cfg();
        let ratings = o.ratings.clone().filter(|r| !r.is_empty());
        let modequery = compute_base_mode(&cfg, o.mode, ratings.as_ref(), o.session);
        let globalfilter = if o.session.is_none() || is_anon_session(o.session) { global_filter(st, &cfg, o.session) } else { None };
        let excluded: Vec<i32> = if o.session.is_some() { o.exclude.iter().map(|&n| n as i32).collect() } else { vec![] };
        let uid = o.user_id.filter(|&u| u != 0);
        let visibility = if o.session.is_some() && o.is_admin {
            Sql::empty()
        } else if let (Some(_), Some(uid)) = (o.session, uid) {
            sql!(r#"AND (COALESCE(items.visibility, 0) = 0 OR items.username = (SELECT "user" FROM "user" WHERE id = "#, uid as i32, "))")
        } else {
            Sql::new("AND COALESCE(items.visibility, 0) = 0")
        };
        let exclude_ids = o.exclude_ids.iter().filter(|&&n| n > 0).map(|&n| n as i32).collect();
        let (mime_parts, mime_sql) = resolve_mime_sql(&cfg, o.mime.as_deref(), o.session, "items");
        Common { modequery, globalfilter, excluded, visibility, exclude_ids, mime_sql, mime_parts }
    }

    /// `AND modequery [globalfilter] [excluded tags]`
    fn mode_filters(&self) -> Sql {
        let mut q = Sql::new("AND ");
        q.push(&self.modequery).push(" ");
        if let Some(gf) = &self.globalfilter {
            q.push("AND NOT EXISTS (SELECT 1 FROM tags_assign WHERE item_id = items.id AND (").push(gf).push(")) ");
        }
        if !self.excluded.is_empty() {
            q.push("AND NOT EXISTS (SELECT 1 FROM tags_assign WHERE item_id = items.id AND tag_id = ANY(").bind(self.excluded.clone()).push("::int[])) ");
        }
        q
    }

    fn exclude_ids_sql(&self) -> Sql {
        if self.exclude_ids.is_empty() {
            Sql::empty()
        } else {
            sql!("AND items.id != ALL(", self.exclude_ids.clone(), "::int[])")
        }
    }
}

const ITEM_COLS: &str = "items.id, items.title, items.slug, items.mime, items.dest, items.username, items.stamp, items.xd_score, items.width, items.height, items.has_coverart";

/// The shared outer select: user options, rating tag, first 3 tags.
fn decorate(cfg: &crate::config::Config, alias: &str) -> Sql {
    let ratings: Vec<i32> = vec![1, 2, cfg.nsfl_tag_id() as i32];
    let mut q = Sql::new(&format!(
        "SELECT {alias}.*, uo.display_name, uo.username_color, uo.avatar, uo.avatar_file, \
         (SELECT ta.tag_id FROM tags_assign ta WHERE ta.item_id = {alias}.id AND ta.tag_id = ANY("
    ));
    q.bind(ratings).push(&format!(
        "::int[]) LIMIT 1) as rating_tag_id, \
         ARRAY(SELECT t.tag FROM tags_assign ta JOIN tags t ON t.id = ta.tag_id WHERE ta.item_id = {alias}.id AND ta.tag_id NOT IN (1, 2, 3) LIMIT 3) as tags "
    ));
    q
}

fn user_join(alias: &str) -> String {
    format!(r#" LEFT JOIN "user" u ON LOWER(u."user") = LOWER({alias}.username) LEFT JOIN user_options uo ON uo.user_id = u.id"#)
}

fn or_null(v: &Value) -> Value {
    if crate::config::truthy(v) { v.clone() } else { Value::Null }
}

/// Row → the item object the recommendation functions return.
fn map_item(r: &Row) -> Map<String, Value> {
    let xd = r.val("xd_score");
    let (tier, label) = xd_score_meta(xd.as_f64().unwrap_or(0.0));
    let tag_id = r.val("rating_tag_id");
    let rating_class = match tag_id.as_i64() {
        Some(1) => "sfw",
        Some(2) => "nsfw",
        Some(3) => "nsfl",
        _ => "untagged",
    };
    let username = r.val("username").clone();
    let display_name = if crate::config::truthy(r.val("display_name")) { r.val("display_name").clone() } else { username.clone() };
    let tags = match r.val("tags") {
        Value::Array(a) => Value::Array(a.clone()),
        _ => json!([]),
    };
    let mut m = Map::new();
    m.insert("id".into(), r.val("id").clone());
    m.insert("title".into(), or_null(r.val("title")));
    m.insert("slug".into(), or_null(r.val("slug")));
    m.insert("mime".into(), r.val("mime").clone());
    m.insert("dest".into(), r.val("dest").clone());
    m.insert("username".into(), username);
    m.insert("display_name".into(), display_name);
    m.insert("username_color".into(), or_null(r.val("username_color")));
    m.insert("avatar".into(), or_null(r.val("avatar")));
    m.insert("avatar_file".into(), or_null(r.val("avatar_file")));
    m.insert("stamp".into(), r.val("stamp").clone());
    m.insert("tags".into(), tags);
    m.insert("rating_tag_id".into(), tag_id.clone());
    m.insert("rating_class".into(), json!(rating_class));
    m.insert("xd_score".into(), xd.clone());
    m.insert("xd_tier".into(), json!(tier));
    m.insert("xd_label".into(), json!(label));
    m.insert("width".into(), r.val("width").clone());
    m.insert("height".into(), r.val("height").clone());
    m.insert("has_coverart".into(), json!(r.bool("has_coverart")));
    m
}

/// `getRandomRecommendations`
pub async fn get_random_recommendations(st: &AppState, o: &RecOpts<'_>) -> anyhow::Result<Vec<Map<String, Value>>> {
    let cfg = st.cfg();
    let c = Common::new(st, o);
    let max_limit = clamp_limit(o.limit, 20.0);
    let base = |extra: &str, limit: i64| {
        let mut q = Sql::new(&format!("SELECT {ITEM_COLS} FROM items WHERE items.active = true AND (items.is_deleted IS NOT TRUE) {extra} "));
        q.append(&c.mime_sql).push(" ").append(&c.visibility).push(" ").append(&c.exclude_ids_sql()).push(" ").append(&c.mode_filters());
        q.push(" ORDER BY random() LIMIT ").bind(limit);
        q
    };
    let mut q;
    if !c.mime_parts.is_empty() {
        q = Sql::new("WITH rand_items AS (");
        q.append(&base("", max_limit)).push(") ");
    } else {
        let audio_limit = ((max_limit as f64 * 0.15).floor() as i64).clamp(1, 3);
        q = Sql::new("WITH rand_audio AS (");
        q.append(&base("AND items.mime ILIKE 'audio/%'", audio_limit))
            .push("), rand_other AS (")
            .append(&base("AND items.mime NOT ILIKE 'audio/%'", max_limit))
            .push("), rand_items AS (SELECT * FROM (SELECT * FROM rand_audio UNION ALL SELECT * FROM rand_other) sub ORDER BY random() LIMIT ")
            .bind(max_limit)
            .push(") ");
    }
    q.append(&decorate(&cfg, "ri")).push("FROM rand_items ri").push(&user_join("ri"));
    let rows = q.fetch_all(&st.db).await?;
    Ok(rows
        .iter()
        .map(|r| {
            let mut m = map_item(r);
            m.insert("score".into(), json!(0));
            m.insert("rank_score".into(), json!(0));
            m
        })
        .collect())
}

/// `getRandomVideos`
#[allow(dead_code)]
pub async fn get_random_videos(st: &AppState, o: &RecOpts<'_>) -> anyhow::Result<Vec<Map<String, Value>>> {
    let mut o = o.clone();
    o.mime = Some("video".into());
    get_random_recommendations(st, &o).await
}

/// `updateUserAffinity({ user_id, item_id, scoreDelta })` — tag and creator affinities.
pub async fn update_user_affinity(st: &AppState, user_id: i64, item_id: i64, delta: f64) {
    if user_id == 0 || item_id == 0 || delta == 0.0 {
        return;
    }
    if let Err(e) = update_user_affinity_inner(st, user_id as i32, item_id as i32, delta).await {
        tracing::error!("[AFFINITY] Failed to update user affinity: {e}");
    }
}

async fn update_user_affinity_inner(st: &AppState, user_id: i32, item_id: i32, delta: f64) -> anyhow::Result<()> {
    let Some(info) = sql!(
        r#"SELECT i.username, COALESCE(array_agg(ta.tag_id) FILTER (WHERE ta.tag_id IS NOT NULL), '{}') as tag_ids
        FROM items i LEFT JOIN tags_assign ta ON ta.item_id = i.id
        WHERE i.id = "#,
        item_id,
        " GROUP BY i.id, i.username LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?
    else {
        return Ok(());
    };
    let tag_ids: Vec<i32> = info.val("tag_ids").as_array().map(|a| a.iter().filter_map(|v| v.as_i64()).map(|n| n as i32).collect()).unwrap_or_default();
    if !tag_ids.is_empty() {
        sql!(
            "INSERT INTO user_tag_affinity (user_id, tag_id, score, interaction_count, last_interacted) SELECT ",
            user_id,
            ", unnest(",
            tag_ids,
            "::int[]), ",
            delta,
            r#", 1, now()
          ON CONFLICT (user_id, tag_id) DO UPDATE SET
            score = GREATEST(-10.0, LEAST(1000.0, user_tag_affinity.score + EXCLUDED.score)),
            interaction_count = user_tag_affinity.interaction_count + 1,
            last_interacted = now()"#
        )
        .execute(&st.db)
        .await?;
    }
    let creator = info.string("username");
    let creator = creator.trim();
    if !creator.is_empty() {
        sql!(
            "INSERT INTO user_creator_affinity (user_id, creator_username, score, interaction_count, last_interacted) VALUES (",
            user_id,
            ", ",
            creator,
            ", ",
            delta,
            r#", 1, now())
          ON CONFLICT (user_id, creator_username) DO UPDATE SET
            score = GREATEST(-10.0, LEAST(1000.0, user_creator_affinity.score + EXCLUDED.score)),
            interaction_count = user_creator_affinity.interaction_count + 1,
            last_interacted = now()"#
        )
        .execute(&st.db)
        .await?;
    }
    Ok(())
}

/// Fire-and-forget [`update_user_affinity`] (`f0cklib.updateUserAffinity(...).catch(...)`).
pub fn spawn_update_user_affinity(st: &AppState, user_id: i64, item_id: i64, delta: f64) {
    let st = st.clone();
    tokio::spawn(async move { update_user_affinity(&st, user_id, item_id, delta).await });
}

/// `updateUserTagAffinity({ user_id, tag, scoreDelta })` — `tag` is a comma separated list.
pub async fn update_user_tag_affinity(st: &AppState, user_id: i64, tag: &str, delta: f64) {
    if user_id == 0 || tag.is_empty() || delta == 0.0 {
        return;
    }
    let tags: Vec<String> = tag
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty() && !t.starts_with("title:") && !t.starts_with("src:"))
        .take(10)
        .collect();
    if tags.is_empty() {
        return;
    }
    let slugs: Vec<String> = tags.iter().map(|t| crate::util::slugify(t)).filter(|s| !s.is_empty()).collect();
    let r: anyhow::Result<()> = async {
        let ids: Vec<i32> = sql!(
            "SELECT DISTINCT id FROM tags WHERE LOWER(tag) = ANY(",
            tags,
            "::text[]) OR (normalized != '' AND normalized = ANY(",
            slugs,
            "::text[]))"
        )
        .fetch_scalar_list(&st.db)
        .await?;
        if ids.is_empty() {
            return Ok(());
        }
        // Only count an interaction when the last one is more than 10 s old (page refreshes,
        // beacon + page load).
        sql!(
            "INSERT INTO user_tag_affinity (user_id, tag_id, score, interaction_count, last_interacted) SELECT ",
            user_id as i32,
            ", unnest(",
            ids,
            "::int[]), ",
            delta,
            r#", 1, now()
        ON CONFLICT (user_id, tag_id) DO UPDATE SET
          score = CASE WHEN user_tag_affinity.last_interacted < now() - interval '10 seconds'
            THEN GREATEST(-10.0, LEAST(1000.0, user_tag_affinity.score + EXCLUDED.score)) ELSE user_tag_affinity.score END,
          interaction_count = CASE WHEN user_tag_affinity.last_interacted < now() - interval '10 seconds'
            THEN user_tag_affinity.interaction_count + 1 ELSE user_tag_affinity.interaction_count END,
          last_interacted = now()"#
        )
        .execute(&st.db)
        .await?;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::error!("[AFFINITY] Failed to update user tag affinity from search: {e}");
    }
}

/// Fire-and-forget [`update_user_tag_affinity`].
pub fn spawn_update_user_tag_affinity(st: &AppState, user_id: i64, tag: &str, delta: f64) {
    let st = st.clone();
    let tag = tag.to_string();
    tokio::spawn(async move { update_user_tag_affinity(&st, user_id, &tag, delta).await });
}

/// `decayUserAffinities` — affinities untouched for a week lose 15 %, near-zero ones are dropped.
/// The old code never scheduled it; exported for a periodic job.
#[allow(dead_code)]
pub async fn decay_user_affinities(st: &AppState) {
    let r: anyhow::Result<()> = async {
        sql!("UPDATE user_tag_affinity SET score = score * 0.85 WHERE last_interacted < now() - interval '7 days' AND score > 0.1").execute(&st.db).await?;
        sql!("UPDATE user_creator_affinity SET score = score * 0.85 WHERE last_interacted < now() - interval '7 days' AND score > 0.1").execute(&st.db).await?;
        sql!("DELETE FROM user_tag_affinity WHERE score < 0.05 AND interaction_count < 2").execute(&st.db).await?;
        sql!("DELETE FROM user_creator_affinity WHERE score < 0.05 AND interaction_count < 2").execute(&st.db).await?;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::error!("[AFFINITY] Decay job error: {e}");
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// A float as a JSON number the way JavaScript prints it (integral values without `.0`).
pub fn jnum(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 { json!(v as i64) } else { serde_json::Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null) }
}

/// `getPersonalizedRecommendations`: a few items matching the user's tag/creator affinities,
/// interleaved with random ones.
pub async fn get_personalized_recommendations(st: &AppState, o: &RecOpts<'_>) -> anyhow::Result<Vec<Map<String, Value>>> {
    let cfg = st.cfg();
    let max_limit = clamp_limit(o.limit, 20.0);
    let exclude_item_ids: Vec<i64> = o.exclude_ids.iter().copied().filter(|&n| n > 0).collect();

    // 1. user & session profile (insertion ordered like the JS Map/Set)
    let mut tag_map: Vec<(i64, f64)> = Vec::new();
    let mut creators: Vec<String> = Vec::new();
    if let Some(uid) = o.user_id.filter(|&u| u != 0) {
        let r: anyhow::Result<()> = async {
            let rows = sql!("SELECT tag_id, score FROM user_tag_affinity WHERE user_id = ", uid as i32, " AND score > 0 ORDER BY score DESC LIMIT 30")
                .fetch_all(&st.db)
                .await?;
            for r in rows {
                if let Some(id) = r.i64("tag_id") {
                    let s = r.f64("score").unwrap_or(0.0);
                    match tag_map.iter_mut().find(|(t, _)| *t == id) {
                        Some(e) => e.1 = s,
                        None => tag_map.push((id, s)),
                    }
                }
            }
            let rows: Vec<String> =
                sql!("SELECT creator_username FROM user_creator_affinity WHERE user_id = ", uid as i32, " AND score > 0 ORDER BY score DESC LIMIT 15")
                    .fetch_scalar_list(&st.db)
                    .await?;
            for c in rows {
                if !c.is_empty() && !creators.contains(&c) {
                    creators.push(c);
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = r {
            tracing::error!("[RECS] Failed to load user affinity profile: {e}");
        }
    }

    let st_list: Vec<String> = o.session_tags.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).take(20).collect();
    if !st_list.is_empty() {
        match sql!("SELECT id, tag FROM tags WHERE LOWER(tag) = ANY(", st_list, "::text[])").fetch_all(&st.db).await {
            Ok(rows) => {
                for r in rows {
                    if let Some(id) = r.i64("id") {
                        match tag_map.iter_mut().find(|(t, _)| *t == id) {
                            Some(e) => e.1 += 8.0,
                            None => tag_map.push((id, 8.0)),
                        }
                    }
                }
            }
            Err(e) => tracing::error!("[RECS] Failed to resolve session tags: {e}"),
        }
    }
    for c in o.session_creators.split(',').map(str::trim).filter(|s| !s.is_empty()).take(15) {
        if !creators.iter().any(|x| x == c) {
            creators.push(c.to_string());
        }
    }

    let targets: Vec<(i64, f64)> = tag_map.into_iter().filter(|(_, s)| *s > 0.0).take(35).collect();
    let mut sub = o.clone();
    sub.limit = Some(max_limit as f64);
    sub.exclude_ids = exclude_item_ids.clone();
    if targets.is_empty() && creators.is_empty() {
        return get_random_recommendations(st, &sub).await;
    }

    // 2. how many personalized items
    let is_first_batch = !o.continuation && exclude_item_ids.is_empty();
    let personalized_target: i64 = if max_limit == 1 {
        match o.prefer_personalized {
            Some(true) => 1,
            Some(false) => 0,
            None => (rand::thread_rng().r#gen::<f64>() < 0.20) as i64,
        }
    } else if is_first_batch {
        1 + ((max_limit - 3).max(0) as f64 * 0.20).round() as i64
    } else {
        ((max_limit as f64 * 0.20).round() as i64).max(1)
    };

    let c = Common::new(st, &sub);
    let mut personalized: Vec<Map<String, Value>> = Vec::new();
    if personalized_target > 0 && !targets.is_empty() {
        let tag_ids: Vec<i32> = targets.iter().map(|(t, _)| *t as i32).collect();
        let scores: Vec<String> = targets.iter().map(|(_, s)| s.to_string()).collect();
        let pool_limit = (personalized_target * 3).max(25);
        let mut q = sql!(
            "WITH user_tags AS (SELECT unnest(",
            tag_ids,
            "::int[]) as tag_id, unnest(",
            scores,
            "::text[]::real[]) as score), candidate_pool AS (SELECT "
        );
        q.push(ITEM_COLS)
            .push(", (SUM(ut.score) * 1.5 + CASE WHEN items.username = ANY(")
            .bind(creators.clone())
            .push(
                "::text[]) THEN 20.0 ELSE 0.0 END + (random() * 12.0)) as rank_score \
                 FROM items JOIN tags_assign ta ON ta.item_id = items.id JOIN user_tags ut ON ut.tag_id = ta.tag_id \
                 WHERE items.active = true AND (items.is_deleted IS NOT TRUE) ",
            )
            .append(&c.visibility)
            .push(" ")
            .append(&c.exclude_ids_sql())
            .push(" ")
            .append(&c.mime_sql)
            .push(" ")
            .append(&c.mode_filters())
            .push(" GROUP BY ")
            .push(ITEM_COLS)
            .push(" ORDER BY rank_score DESC LIMIT ")
            .bind(pool_limit)
            .push(") ")
            .append(&decorate(&cfg, "cp"))
            .push("FROM (SELECT * FROM candidate_pool ORDER BY random() LIMIT ")
            .bind(personalized_target)
            .push(") cp")
            .push(&user_join("cp"));
        match q.fetch_all(&st.db).await {
            Ok(rows) => {
                personalized = rows
                    .iter()
                    .map(|r| {
                        let mut m = map_item(r);
                        let score = r.f64("rank_score").map(round1).unwrap_or(0.0);
                        m.insert("personalized".into(), json!(true));
                        m.insert("score".into(), jnum(score));
                        m.insert("rank_score".into(), jnum(score));
                        m
                    })
                    .collect();
            }
            Err(e) => tracing::error!("[RECS] Error querying personalized candidate pool: {e}"),
        }
    }

    // 3. random exploration items
    let needed = max_limit - personalized.len() as i64;
    let mut random_items: Vec<Map<String, Value>> = Vec::new();
    if needed > 0 {
        let mut all_exclude = exclude_item_ids.clone();
        all_exclude.extend(personalized.iter().filter_map(|p| p.get("id").and_then(Value::as_i64)));
        let mut ro = o.clone();
        ro.limit = Some((needed + 3).max(max_limit) as f64);
        ro.exclude_ids = all_exclude;
        random_items = get_random_recommendations(st, &ro).await?;
    }

    // 4. combine
    let max = max_limit as usize;
    if max_limit == 1 {
        return Ok(if !personalized.is_empty() { personalized.into_iter().take(1).collect() } else { random_items.into_iter().take(1).collect() });
    }
    if personalized.is_empty() {
        random_items.truncate(max);
        return Ok(random_items);
    }
    if random_items.is_empty() {
        personalized.truncate(max);
        return Ok(personalized);
    }
    let mut pers = std::collections::VecDeque::from(personalized);
    let mut rand_q = std::collections::VecDeque::from(random_items);
    let mut result: Vec<Map<String, Value>> = Vec::new();
    let mut rng = rand::thread_rng();
    let mut gap: i64;
    if is_first_batch {
        // exactly one recommendation among the first three
        let top = max.min(3);
        let rec_slot = rng.gen_range(0..top);
        let mut slots: Vec<Option<Map<String, Value>>> = vec![None; top];
        slots[rec_slot] = pers.pop_front();
        for (s, slot) in slots.iter_mut().enumerate() {
            if s != rec_slot {
                *slot = if !rand_q.is_empty() { rand_q.pop_front() } else { pers.pop_front() };
            }
        }
        result.extend(slots.into_iter().flatten());
        gap = top as i64 - 1 - rec_slot as i64;
    } else {
        gap = rng.gen_range(1..=2);
    }
    // later recommendations separated by 3–5 random items
    let mut target_gap: i64 = rng.gen_range(3..=5);
    while result.len() < max && (!pers.is_empty() || !rand_q.is_empty()) {
        if !pers.is_empty() && gap >= target_gap && !rand_q.is_empty() {
            result.extend(pers.pop_front());
            gap = 0;
            target_gap = rng.gen_range(3..=5);
        } else if let Some(r) = rand_q.pop_front() {
            result.push(r);
            gap += 1;
        } else if let Some(p) = pers.pop_front() {
            result.push(p);
            gap = 0;
        }
    }
    result.truncate(max);
    Ok(result)
}

/// Options of [`get_tag_feed_items`].
pub struct TagFeedOpts<'a> {
    pub tag: String,
    pub order: String,
    pub offset: f64,
    pub limit: Option<f64>,
    pub focus_id: Option<String>,
    pub strict: bool,
    pub rec: RecOpts<'a>,
}

/// `getTagFeedItems` → `{ items, total, offset, limit }`
pub async fn get_tag_feed_items(st: &AppState, o: &TagFeedOpts<'_>) -> anyhow::Result<Value> {
    if o.tag.trim().is_empty() {
        return Ok(json!({ "items": [], "total": 0 }));
    }
    let cfg = st.cfg();
    let c = Common::new(st, &o.rec);
    let max_limit = clamp_limit(o.limit, 20.0);
    let num_offset = if o.offset.is_finite() { o.offset.max(0.0).trunc() as i64 } else { 0 };

    let terms: Vec<String> = o.tag.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect();
    let is_strict = o.strict || o.tag.contains(',');
    let mut tag_cond = Sql::empty();
    if is_strict {
        if !terms.is_empty() {
            let n = terms.len() as i64;
            tag_cond = sql!(
                "and (items.id in (select ta.item_id from tags_assign ta join tags t on t.id = ta.tag_id \
                 where t.normalized = ANY(ARRAY(SELECT slugify(x) FROM unnest(",
                terms.clone(),
                "::text[]) AS x)) group by ta.item_id having count(distinct t.normalized) = ",
                n,
                ") or items.id in (select ai.item_id from album_items ai join album_items_tags_assign aita on aita.album_item_id = ai.id \
                 join tags t on t.id = aita.tag_id where t.normalized = ANY(ARRAY(SELECT slugify(x) FROM unnest(",
                terms.clone(),
                "::text[]) AS x)) group by ai.item_id having count(distinct t.normalized) = ",
                n,
                "))"
            );
        }
    } else {
        for term in &terms {
            tag_cond.append(&sql!(
                " and (items.id in (select ta.item_id from tags_assign ta join tags t on t.id = ta.tag_id \
                 where t.normalized like '%' || slugify(",
                term.as_str(),
                ") || '%') or items.id in (select ai.item_id from album_items ai join album_items_tags_assign aita on aita.album_item_id = ai.id \
                 join tags t on t.id = aita.tag_id where t.normalized like '%' || slugify(",
                term.as_str(),
                ") || '%'))"
            ));
        }
    }
    let is_desc = o.order.to_lowercase() == "desc";
    let sort_items = if is_desc { "items.id desc" } else { "items.id asc" };
    let sort_mi = if is_desc { "mi.id desc" } else { "mi.id asc" };

    let where_all = || {
        let mut q = Sql::new("WHERE items.active = true AND (items.is_deleted IS NOT TRUE) ");
        q.append(&c.visibility).push(" ").append(&c.mime_sql).push(" ").append(&c.mode_filters()).push(" ").append(&tag_cond);
        q
    };

    let mut computed_offset = num_offset;
    let mut computed_limit = max_limit;
    if let Some(focus) = o.focus_id.as_deref().filter(|f| !f.is_empty()) {
        if let Some(fid) = resolve_numeric_item_id(st, focus).await {
            let mut q = Sql::new(&format!("WITH ordered AS (SELECT items.id, ROW_NUMBER() OVER (ORDER BY {sort_items}) - 1 as row_num FROM items "));
            q.append(&where_all()).push(") SELECT row_num FROM ordered WHERE id = ").bind(fid as i32).push(" LIMIT 1");
            match q.fetch_optional(&st.db).await {
                Ok(Some(r)) => {
                    let row_num = r.i64("row_num").unwrap_or(0);
                    if row_num < 30 {
                        computed_offset = 0;
                        computed_limit = (row_num + 10).clamp(20, 50);
                    } else {
                        computed_offset = (row_num - 6).max(0);
                        computed_limit = 25;
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("[TAG-FEED] Error computing focus_id offset: {e}"),
            }
        }
    }

    let mut cq = Sql::new("SELECT count(distinct items.id) as total FROM items ");
    cq.append(&where_all());
    let total = cq.fetch_optional(&st.db).await?.and_then(|r| r.i64("total")).unwrap_or(0);

    let mut q = Sql::new(&format!("WITH matched_items AS (SELECT {ITEM_COLS}, items.is_album FROM items "));
    q.append(&where_all())
        .push(&format!(" ORDER BY {sort_items} OFFSET "))
        .bind(computed_offset)
        .push(" LIMIT ")
        .bind(computed_limit)
        .push(") ")
        .append(&decorate(&cfg, "mi"))
        .push("FROM matched_items mi")
        .push(&user_join("mi"))
        .push(&format!(" ORDER BY {sort_mi}"));
    let mut rows = q.fetch_all(&st.db).await?;

    let album_ids: Vec<i32> = rows.iter().filter(|r| r.bool("is_album")).filter_map(|r| r.i64("id")).map(|n| n as i32).collect();
    if !terms.is_empty() && !album_ids.is_empty() {
        let mut sq = sql!(
            "SELECT ai.item_id, ai.id as subf0ck_id, ai.slug as subf0ck_slug, ai.dest as subf0ck_dest, ai.mime as subf0ck_mime, ai.order_index \
             FROM album_items ai JOIN album_items_tags_assign aita ON aita.album_item_id = ai.id JOIN tags t ON t.id = aita.tag_id \
             WHERE ai.item_id = ANY(",
            album_ids,
            "::int[]) AND ("
        );
        if is_strict {
            sq.push("t.normalized = ANY(ARRAY(SELECT slugify(x) FROM unnest(").bind(terms.clone()).push("::text[]) AS x))");
        } else {
            for (i, term) in terms.iter().enumerate() {
                if i > 0 {
                    sq.push(" OR ");
                }
                sq.push("t.normalized LIKE '%' || slugify(").bind(term.as_str()).push(") || '%'");
            }
        }
        sq.push(") ORDER BY ai.item_id, ai.order_index ASC");
        match sq.fetch_all(&st.db).await {
            Ok(subs) => {
                for row in rows.iter_mut() {
                    let id = row.i64("id");
                    let Some(ms) = subs.iter().find(|s| s.i64("item_id") == id) else { continue };
                    let slug = if crate::config::truthy(ms.val("subf0ck_slug")) { ms.val("subf0ck_slug").clone() } else { ms.val("subf0ck_id").clone() };
                    row.insert("target_subf0ck_slug".into(), slug);
                    if let Some(dest) = ms.str("subf0ck_dest").filter(|d| !d.is_empty()) {
                        let base = match dest.rfind('.') {
                            Some(i) if !dest[i + 1..].is_empty() => &dest[..i],
                            _ => dest,
                        };
                        let thumb = format!("/t/{base}.webp");
                        row.insert("thumb".into(), json!(thumb));
                        row.insert("matching_sub_thumb".into(), json!(thumb));
                        row.insert("matching_sub_dest".into(), json!(dest));
                        row.insert("dest".into(), json!(dest));
                    }
                    if let Some(m) = ms.str("subf0ck_mime").filter(|m| !m.is_empty()) {
                        row.insert("matching_sub_mime".into(), json!(m));
                        row.insert("mime".into(), json!(m));
                    }
                }
            }
            Err(e) => tracing::warn!("[TAG-FEED] Error resolving matching subf0cks for tag feed: {e}"),
        }
    }

    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let base = map_item(r);
            let mut m = Map::new();
            for (k, v) in base {
                m.insert(k.clone(), v);
                match k.as_str() {
                    "dest" => {
                        let thumb = if crate::config::truthy(r.val("thumb")) { r.val("thumb").clone() } else { json!(format!("/t/{}.webp", r.i64("id").unwrap_or(0))) };
                        m.insert("thumb".into(), thumb);
                        m.insert("matching_sub_dest".into(), or_null(r.val("matching_sub_dest")));
                        m.insert("matching_sub_mime".into(), or_null(r.val("matching_sub_mime")));
                        m.insert("matching_sub_thumb".into(), or_null(r.val("matching_sub_thumb")));
                        m.insert("target_subf0ck_slug".into(), or_null(r.val("target_subf0ck_slug")));
                    }
                    _ => {}
                }
            }
            let xd = r.val("xd_score");
            m.insert("score".into(), if crate::config::truthy(xd) { xd.clone() } else { json!(0) });
            m.insert("rank_score".into(), json!(0));
            Value::Object(m)
        })
        .collect();

    Ok(json!({ "items": items, "total": total, "offset": computed_offset, "limit": computed_limit }))
}
