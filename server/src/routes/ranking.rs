//! User tag ranking and site statistics (`/ranking`) — port of src/inc/routes/ranking.mjs.

use super::toptags::{member_session, no_store};
use crate::db::{Row, RowExt};
use crate::http::{Guard, Req, Res, Router, html};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::time::Duration;

pub fn register(r: &mut Router) {
    r.get_g(r"^\/ranking$", Guard::LoggedIn, ranking);
}

/// Last computed size of the media directories (formatted), like the old module-level variable.
static DISK_SIZE: OnceLock<parking_lot::RwLock<String>> = OnceLock::new();

fn disk_size() -> &'static parking_lot::RwLock<String> {
    DISK_SIZE.get_or_init(|| parking_lot::RwLock::new("Calculating...".into()))
}

/// Boot: load the last stored size, then recompute it now and every 30 minutes.
pub fn spawn_disk_size_task(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        match sql!("select value from site_settings where key = 'disk_size'").fetch_scalar::<String, _>(&st.db).await {
            Ok(Some(v)) => {
                if let Some(n) = crate::social::helpers::js_parse_int(&v) {
                    *disk_size().write() = crate::util::format_size_f(n as f64);
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!("Error reading disk size from DB: {e}"),
        }
        loop {
            update_disk_size(&st).await;
            tokio::time::sleep(Duration::from_secs(30 * 60)).await;
        }
    });
}

async fn update_disk_size(st: &AppState) {
    let cfg = st.cfg();
    let dirs = [&cfg.paths.b, &cfg.paths.t, &cfg.paths.c].map(|p| p.to_string_lossy().to_string());
    let mut args: Vec<&str> = vec!["-sb"];
    args.extend(dirs.iter().map(String::as_str));
    let out = match crate::proc::spawn("du", &args).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("Error calculating disk size: {e}");
            return;
        }
    };
    let total: i64 = out
        .stdout_str()
        .lines()
        .filter_map(|l| l.split_whitespace().next().and_then(|n| n.parse::<i64>().ok()))
        .sum();
    *disk_size().write() = crate::util::format_size_f(total as f64);
    let stored = sql!("select value from site_settings where key = 'disk_size'").fetch_scalar::<String, _>(&st.db).await.ok().flatten();
    if stored.as_deref() == Some(total.to_string().as_str()) {
        return;
    }
    let r = sql!(
        "insert into site_settings (key, value) values ('disk_size', ",
        total.to_string(),
        ") on conflict (key) do update set value = excluded.value"
    )
    .execute(&st.db)
    .await;
    if let Err(e) = r {
        tracing::error!("Error saving disk size to DB: {e}");
    }
}

/// `lib.countf0cks()` — item totals by rating and state.
pub async fn count_f0cks(st: &AppState) -> anyhow::Result<Value> {
    let cfg = st.cfg();
    let nsfl = cfg.nsfl_tag_id() as i32;
    let row = sql!(
        "SELECT
           count(*) FILTER (WHERE active AND NOT is_deleted AND EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id)) AS tagged,
           count(*) FILTER (WHERE active AND NOT is_deleted AND NOT EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id IN (1, 2, ", nsfl, "))) AS untagged,
           count(*) FILTER (WHERE active AND NOT is_deleted AND EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id = 1)) AS sfw,
           count(*) FILTER (WHERE active AND NOT is_deleted AND EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id = 2)) AS nsfw,
           count(*) FILTER (WHERE active AND NOT is_deleted AND EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id = ", nsfl, ")) AS nsfl,
           count(*) FILTER (WHERE NOT active AND is_deleted) AS deleted,
           count(*) FILTER (WHERE NOT active AND NOT is_deleted) AS pending,
           max(id) AS lastf0ck
         FROM items i"
    )
    .fetch_optional(&st.db)
    .await?
    .unwrap_or_default();
    let n = |k: &str| row.i64(k).unwrap_or(0);
    let (tagged, untagged, deleted, pending) = (n("tagged"), n("untagged"), n("deleted"), n("pending"));
    Ok(json!({
        "tagged": tagged,
        "untagged": untagged,
        "total": tagged + untagged,
        "deleted": deleted,
        "pending": pending,
        "untracked": n("lastf0ck") - (tagged + untagged + deleted + pending),
        "sfw": n("sfw"),
        "nsfw": n("nsfw"),
        "nsfl": if cfg.enable_nsfl() { n("nsfl") } else { 0 },
    }))
}

/// `lib.getDefaultAvatar()` — the column default of `user_options.avatar` (an integer or NULL).
async fn default_avatar(st: &AppState) -> Option<i32> {
    let d: Option<String> = sql!(
        "select column_default from information_schema.columns where table_schema = 'public' and table_name = 'user_options' and column_name = 'avatar'"
    )
    .fetch_scalar(&st.db)
    .await
    .ok()
    .flatten();
    d.and_then(|v| v.trim_matches(|c: char| c == '\'' || c == '(' || c == ')').split("::").next().and_then(|n| n.trim_matches('\'').parse().ok()))
}

async fn ranking(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let list: Vec<Row> = sql!(
        r#"select
          "user".user, "user".admin,
          coalesce("user_options".avatar, "#,
        default_avatar(&st).await,
        r#") as avatar,
          "user_options".avatar_file,
          "user_options".display_name,
          count(distinct(tag_id, item_id)) as count
        from "tags_assign"
        left join "user" on "user".id = "tags_assign".user_id
        left join "user_options" on "user_options".user_id = "user".id
        left join "anon_identities" on "anon_identities".user_id = "user".id
        where "user".id is not null
          and "anon_identities".id is null
          and "user".login not like 'anon_%'
          and "user".user != 'anonymous'
        group by "user".user, "user_options".avatar, "user_options".avatar_file, "user".admin, "user_options".display_name
        order by count desc"#
    )
    .fetch_all(&st.db)
    .await?;
    let stats = count_f0cks(&st).await?;
    let total_comments: i64 = sql!(
        "select count(*) from comments join items on comments.item_id = items.id where comments.is_deleted = false and items.active = true"
    )
    .fetch_scalar(&st.db)
    .await?
    .unwrap_or(0);
    let total_favs: i64 = sql!("select count(*) from favorites join items on favorites.item_id = items.id where items.active = true")
        .fetch_scalar(&st.db)
        .await?
        .unwrap_or(0);
    let total_users: i64 = sql!(
        r#"select count(*) from "user" left join "anon_identities" on "anon_identities".user_id = "user".id
        where "anon_identities".id is null and "user".login not like 'anon_%' and "user".user != 'anonymous'"#
    )
    .fetch_scalar(&st.db)
    .await?
    .unwrap_or(0);
    let total_anon: i64 = sql!("select count(distinct pubkey) from anon_identities").fetch_scalar(&st.db).await?.unwrap_or(0);
    let hoster = sql!(
        "with t as (select split_part(substring(src, position('//' in src)+2), '/', 1) part from items)
        select t.part, count(t.part) as c from t group by t.part order by c desc limit 20"
    )
    .fetch_all(&st.db)
    .await?;
    let favotop = sql!(
        "select favorites.item_id as id, items.slug, count(*) favs
        from favorites join items on items.id = favorites.item_id
        where items.active = true
        group by favorites.item_id, items.slug
        having count(*) > 1
        order by favs desc
        limit 10"
    )
    .fetch_all(&st.db)
    .await?;
    let mut xdtop: Vec<Value> = Vec::new();
    if cfg.truthy("websrv.enable_xd_score") {
        let rows = sql!("select id, slug, xd_score from items where active = true and is_deleted = false and xd_score > 0 order by xd_score desc limit 10")
            .fetch_all(&st.db)
            .await?;
        for mut r in rows {
            let (tier, label) = crate::social::comments::xd_score_meta(r.i64("xd_score").unwrap_or(0));
            r.insert("xd_tier".into(), json!(tier));
            r.insert("xd_label".into(), json!(label));
            xdtop.push(Value::Object(r));
        }
    }
    let disk = disk_size().read().clone();
    let data = json!({
        "list": list,
        "stats": stats,
        "hoster": hoster,
        "favotop": favotop,
        "xdtop": xdtop,
        "totalComments": total_comments,
        "totalFavs": total_favs,
        "totalUsers": total_users,
        "totalAnonUsers": total_anon,
        "enable_nsfl": cfg.enable_nsfl(),
        "diskSize": disk,
        "tmp": null,
        "session": member_session(&req),
        "page_meta": {
            "title": "ranking",
            "description": "User ranking and site statistics",
            "url": format!("https://{}/ranking", cfg.str_or("main.url.domain", "")),
        }
    });
    Ok(no_store(html(crate::tpl::render(&st, Some(&req), "ranking", data))))
}
