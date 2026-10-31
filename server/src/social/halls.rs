//! Halls (curated collections) and user halls (port of the hall parts of f0cklib.mjs).

use crate::db::{Row, RowExt, Sql};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

/// `{ success: false, message }`
fn failure(message: impl Into<String>) -> Value {
    json!({ "success": false, "message": message.into() })
}

fn excluded_filter(excluded: &[i64]) -> Sql {
    if excluded.is_empty() {
        return Sql::empty();
    }
    let ids: Vec<i32> = excluded.iter().filter_map(|t| i32::try_from(*t).ok()).collect();
    sql!("AND NOT EXISTS (SELECT 1 FROM tags_assign ta_ex WHERE ta_ex.item_id = i.id AND ta_ex.tag_id = ANY(", ids, "::int[]))")
}

/// `f0cklib.getHalls()` — all halls with active item counts.
pub async fn get_halls(st: &AppState) -> Vec<Row> {
    sql!(
        r#"SELECT h.*,
               COUNT(DISTINCT CASE WHEN i.active = true THEN ha.item_id END)::int AS item_count
        FROM halls h
        LEFT JOIN halls_assign ha ON ha.hall_id = h.id
        LEFT JOIN items i ON i.id = ha.item_id
        GROUP BY h.id
        ORDER BY h.name ASC"#
    )
    .fetch_all(&st.db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[F0CKLIB] Error fetching halls: {e}");
        vec![]
    })
}

/// `f0cklib.getHallsOverview(mode, excludedTags)` — halls matching the mode's rating.
pub async fn get_halls_overview(st: &AppState, mode: i64, excluded: &[i64]) -> Vec<Row> {
    let rating = match mode {
        0 => Some("sfw"),
        1 => Some("nsfw"),
        4 => Some("nsfl"),
        _ => None,
    };
    let rating_filter = match rating {
        Some(r) => sql!("AND h.rating = ", r),
        None => Sql::empty(),
    };
    sql!(
        r#"SELECT
          h.id,
          h.name,
          h.slug,
          h.description,
          h.rating,
          h.custom_image,
          COALESCE(counts.total_items, 0) AS total_items,
          counts.latest_item_id
        FROM halls h
        LEFT JOIN (
          SELECT
            ha.hall_id,
            COUNT(DISTINCT ha.item_id) AS total_items,
            MAX(i.id) AS latest_item_id
          FROM halls_assign ha
          JOIN items i ON i.id = ha.item_id
          WHERE i.active = true
            "#,
        @excluded_filter(excluded),
        r#"
          GROUP BY ha.hall_id
        ) counts ON counts.hall_id = h.id
        WHERE true "#,
        @rating_filter,
        " ORDER BY h.name ASC"
    )
    .fetch_all(&st.db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[F0CKLIB] Error fetching halls overview: {e}");
        vec![]
    })
}

/// `f0cklib.addItemToHall(itemId, hallInput, userId, description)` → `{success}` / `{success:false, message}`.
pub async fn add_item_to_hall(st: &AppState, item_id: i64, hall_input: &str, user_id: Option<i64>, description: Option<&str>) -> Value {
    match add_item_to_hall_inner(st, item_id, hall_input, user_id, description.filter(|d| !d.is_empty())).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[F0CKLIB] Error adding item to hall: {e:#}");
            failure(e.to_string())
        }
    }
}

async fn add_item_to_hall_inner(st: &AppState, item_id: i64, hall_input: &str, user_id: Option<i64>, description: Option<&str>) -> anyhow::Result<Value> {
    let mut hall = sql!("SELECT id, slug FROM halls WHERE slug = ", hall_input, " LIMIT 1").fetch_optional(&st.db).await?;
    if hall.is_none() {
        let generated = crate::util::slugify(hall_input);
        hall = sql!("SELECT id FROM halls WHERE slug = ", &generated, " LIMIT 1").fetch_optional(&st.db).await?;
        match &hall {
            None => {
                let name = hall_input.trim();
                if name.is_empty() || generated.is_empty() {
                    anyhow::bail!("Invalid hall name");
                }
                // `description` is NOT NULL with default '' — the old code passed null here,
                // which failed for new halls without a description; keep the column default.
                let mut q = sql!("INSERT INTO halls (name, slug, description) VALUES (", name, ", ", &generated, ", ");
                match description {
                    Some(d) => q.bind(d),
                    None => q.push("DEFAULT"),
                };
                q.push(")");
                q.execute(&st.db).await?;
                hall = sql!("SELECT id FROM halls WHERE slug = ", &generated, " LIMIT 1").fetch_optional(&st.db).await?;
                st.update_halls_cache().await;
            }
            Some(h) => {
                if let Some(d) = description {
                    sql!("UPDATE halls SET description = ", d, " WHERE id = ", h.val("id").as_i64().unwrap_or(0) as i32).execute(&st.db).await?;
                }
            }
        }
    } else if let (Some(d), Some(h)) = (description, &hall) {
        sql!("UPDATE halls SET description = ", d, " WHERE id = ", h.val("id").as_i64().unwrap_or(0) as i32).execute(&st.db).await?;
    }
    let hall_id = hall.and_then(|h| h.i64("id")).ok_or_else(|| anyhow::anyhow!("Cannot read properties of undefined (reading 'id')"))?;
    let n = sql!(
        "INSERT INTO halls_assign (hall_id, item_id, user_id) VALUES (",
        hall_id as i32,
        ", ",
        item_id,
        ", ",
        user_id.map(|u| u as i32),
        ") ON CONFLICT (hall_id, item_id) DO NOTHING"
    )
    .execute(&st.db)
    .await?;
    if n == 0 {
        return Ok(failure("Item is already in this hall"));
    }
    Ok(json!({ "success": true }))
}

/// `f0cklib.updateHallMetadata(hallSlug, description)`
pub async fn update_hall_metadata(st: &AppState, hall_slug: &str, description: Option<&str>) -> Value {
    let r = sql!("UPDATE halls SET description = ", description, " WHERE slug = ", hall_slug).execute(&st.db).await;
    match r {
        Ok(0) => {
            tracing::error!("[F0CKLIB] Error updating hall metadata: Hall not found");
            failure("Hall not found")
        }
        Ok(_) => {
            st.update_halls_cache().await;
            json!({ "success": true })
        }
        Err(e) => {
            tracing::error!("[F0CKLIB] Error updating hall metadata: {e}");
            failure(e.to_string())
        }
    }
}

/// `f0cklib.removeItemFromHall(itemId, hallSlug)`
pub async fn remove_item_from_hall(st: &AppState, item_id: i64, hall_slug: &str) -> Value {
    let hall = match sql!("SELECT id FROM halls WHERE slug = ", hall_slug, " LIMIT 1").fetch_optional(&st.db).await {
        Ok(Some(h)) => h,
        Ok(None) => {
            tracing::error!("[F0CKLIB] Error removing item from hall: Hall not found");
            return failure("Hall not found");
        }
        Err(e) => return failure(e.to_string()),
    };
    let hall_id = hall.i64("id").unwrap_or(0) as i32;
    match sql!("DELETE FROM halls_assign WHERE hall_id = ", hall_id, " AND item_id = ", item_id).execute(&st.db).await {
        Ok(_) => json!({ "success": true }),
        Err(e) => {
            tracing::error!("[F0CKLIB] Error removing item from hall: {e}");
            failure(e.to_string())
        }
    }
}

// ── User halls ──────────────────────────────────────────────────────────────

/// `f0cklib.getUserHalls(userId, mode, excludedTags, viewerUserId)` — private halls only for the owner.
pub async fn get_user_halls(st: &AppState, user_id: i64, mode: i64, excluded: &[i64], viewer_user_id: Option<i64>) -> Vec<Row> {
    let mode_filter = match mode {
        1 => Sql::new("AND i.id IN (SELECT item_id FROM tags_assign WHERE tag_id = 2)"),
        2 => Sql::new("AND NOT EXISTS (SELECT 1 FROM tags_assign WHERE item_id = i.id)"),
        3 => Sql::empty(),
        _ => Sql::new("AND i.id IN (SELECT item_id FROM tags_assign WHERE tag_id = 1)"),
    };
    let private_filter = if viewer_user_id.is_some_and(|v| v == user_id) { Sql::empty() } else { Sql::new("AND uh.is_private = false") };
    sql!(
        r#"SELECT
          uh.id,
          uh.name,
          uh.slug,
          uh.description,
          uh.is_private,
          uh.custom_image,
          uh.created_at,
          COALESCE(counts.total_items, 0) AS total_items,
          counts.latest_item_id
        FROM user_halls uh
        LEFT JOIN (
          SELECT
            uha.hall_id,
            COUNT(DISTINCT uha.item_id) AS total_items,
            MAX(i.id) AS latest_item_id
          FROM user_halls_assign uha
          JOIN items i ON i.id = uha.item_id
          WHERE i.active = true
            "#,
        @mode_filter,
        " ",
        @excluded_filter(excluded),
        r#"
          GROUP BY uha.hall_id
        ) counts ON counts.hall_id = uh.id
        WHERE uh.user_id = "#,
        user_id as i32,
        " ",
        @private_filter,
        " ORDER BY uh.name ASC"
    )
    .fetch_all(&st.db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[F0CKLIB] Error fetching user halls: {e}");
        vec![]
    })
}

/// `f0cklib.getUserHall(userId, slug)`
pub async fn get_user_hall(st: &AppState, user_id: i64, slug: &str) -> Option<Row> {
    let Ok(uid) = i32::try_from(user_id) else { return None };
    sql!(
        r#"SELECT uh.*, u."user" as owner_name
        FROM user_halls uh
        JOIN "user" u ON u.id = uh.user_id
        WHERE uh.user_id = "#,
        uid,
        " AND uh.slug = ",
        slug,
        " LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[F0CKLIB] Error fetching user hall: {e}");
        None
    })
}

/// `f0cklib.getUserHallByOwnerName(ownerName, slug)` — owner matched case-insensitively.
pub async fn get_user_hall_by_owner_name(st: &AppState, owner_name: &str, slug: &str) -> Option<Row> {
    // The old code used `ILIKE ownerName`, so `_`/`%` in the URL acted as wildcards; match the
    // name literally instead (case-insensitive) so one user's URL can't resolve another's hall.
    sql!(
        r#"SELECT uh.*, u."user" as owner_name
        FROM user_halls uh
        JOIN "user" u ON u.id = uh.user_id
        WHERE u."user" ILIKE "#,
        crate::util::escape_like(owner_name),
        " AND uh.slug = ",
        slug,
        " LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[F0CKLIB] Error fetching user hall by owner name: {e}");
        None
    })
}

/// `f0cklib.createUserHall(userId, name, slug, description)`
pub async fn create_user_hall(st: &AppState, user_id: i64, name: &str, slug: &str, description: Option<&str>) -> Value {
    let r: anyhow::Result<Value> = async {
        if name.is_empty() || slug.is_empty() {
            anyhow::bail!("Missing name or slug");
        }
        let exists = sql!("SELECT id FROM user_halls WHERE user_id = ", user_id as i32, " AND slug = ", slug, " LIMIT 1").fetch_optional(&st.db).await?;
        if exists.is_some() {
            return Ok(failure("A hall with this slug already exists"));
        }
        let row = sql!(
            "INSERT INTO user_halls (user_id, name, slug, description) VALUES (",
            user_id as i32,
            ", ",
            name,
            ", ",
            slug,
            ", ",
            description.filter(|d| !d.is_empty()),
            ") RETURNING id, name, slug"
        )
        .fetch_optional(&st.db)
        .await?;
        Ok(json!({ "success": true, "hall": row.map(Value::Object).unwrap_or(Value::Null) }))
    }
    .await;
    r.unwrap_or_else(|e| {
        tracing::error!("[F0CKLIB] Error creating user hall: {e:#}");
        failure(e.to_string())
    })
}

/// Fields of `updateUserHall`. `description: None` keeps the column, `Some(None)` clears it.
pub struct UserHallUpdate {
    pub name: Option<String>,
    pub new_slug: Option<String>,
    pub description: Option<Option<String>>,
    pub is_private: Option<bool>,
}

/// `f0cklib.updateUserHall(userId, slug, { name, newSlug, description, is_private })`
pub async fn update_user_hall(st: &AppState, user_id: i64, slug: &str, u: UserHallUpdate) -> Value {
    let r: anyhow::Result<Value> = async {
        let Some(hall) = sql!("SELECT id FROM user_halls WHERE user_id = ", user_id as i32, " AND slug = ", slug, " LIMIT 1").fetch_optional(&st.db).await? else {
            return Ok(failure("Hall not found"));
        };
        let hall_id = hall.i64("id").unwrap_or(0) as i32;
        if let Some(ns) = u.new_slug.as_deref().filter(|s| !s.is_empty() && *s != slug) {
            let conflict = sql!("SELECT id FROM user_halls WHERE user_id = ", user_id as i32, " AND slug = ", ns, " AND id != ", hall_id, " LIMIT 1")
                .fetch_optional(&st.db)
                .await?;
            if conflict.is_some() {
                return Ok(failure("Slug already taken"));
            }
        }
        let mut q = sql!("UPDATE user_halls SET name = COALESCE(", u.name.clone(), ", name), slug = COALESCE(", u.new_slug.clone(), ", slug), description = ");
        match &u.description {
            Some(d) => q.bind(d.clone().filter(|d| !d.is_empty())),
            None => q.push("description"),
        };
        q.push(", is_private = COALESCE(").bind(u.is_private).push(", is_private) WHERE id = ").bind(hall_id);
        q.execute(&st.db).await?;
        let new_slug = u.new_slug.filter(|s| !s.is_empty()).unwrap_or_else(|| slug.to_string());
        Ok(json!({ "success": true, "newSlug": new_slug }))
    }
    .await;
    r.unwrap_or_else(|e| {
        tracing::error!("[F0CKLIB] Error updating user hall: {e:#}");
        failure(e.to_string())
    })
}

/// `f0cklib.deleteUserHall(userId, slug)`
pub async fn delete_user_hall(st: &AppState, user_id: i64, slug: &str) -> Value {
    let Ok(uid) = i32::try_from(user_id) else { return failure("Hall not found") };
    match sql!("DELETE FROM user_halls WHERE user_id = ", uid, " AND slug = ", slug).execute(&st.db).await {
        Ok(0) => failure("Hall not found"),
        Ok(_) => json!({ "success": true }),
        Err(e) => {
            tracing::error!("[F0CKLIB] Error deleting user hall: {e}");
            failure(e.to_string())
        }
    }
}

/// `f0cklib.addItemToUserHall(hallId, itemId, addedByUserId)`
pub async fn add_item_to_user_hall(st: &AppState, hall_id: i64, item_id: i64, added_by: i64) -> Value {
    let r: anyhow::Result<Value> = async {
        let item = sql!("SELECT id FROM items WHERE id = ", item_id, " AND active = true LIMIT 1").fetch_optional(&st.db).await?;
        if item.is_none() {
            return Ok(failure("Item not found or not active"));
        }
        let n = sql!(
            "INSERT INTO user_halls_assign (hall_id, item_id, user_id) VALUES (",
            hall_id as i32,
            ", ",
            item_id as i32,
            ", ",
            added_by as i32,
            ") ON CONFLICT (hall_id, item_id) DO NOTHING"
        )
        .execute(&st.db)
        .await?;
        if n == 0 {
            return Ok(failure("Item is already in this hall"));
        }
        Ok(json!({ "success": true }))
    }
    .await;
    r.unwrap_or_else(|e| {
        tracing::error!("[F0CKLIB] Error adding item to user hall: {e:#}");
        failure(e.to_string())
    })
}

/// `f0cklib.removeItemFromUserHall(hallId, itemId)`
pub async fn remove_item_from_user_hall(st: &AppState, hall_id: i64, item_id: i64) -> Value {
    match sql!("DELETE FROM user_halls_assign WHERE hall_id = ", hall_id as i32, " AND item_id = ", item_id).execute(&st.db).await {
        Ok(_) => json!({ "success": true }),
        Err(e) => {
            tracing::error!("[F0CKLIB] Error removing item from user hall: {e}");
            failure(e.to_string())
        }
    }
}
