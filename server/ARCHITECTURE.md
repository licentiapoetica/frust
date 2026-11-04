# Server architecture and conventions

The f0ckm backend is a single Rust binary (`server/`). It replaced the earlier Node.js server; the
module comments still name the old file a part was ported from. Behaviour the browser frontend
(`public/s/js`, `views/`) relies on — URLs, status codes, JSON shapes, template data — is kept
stable; change the frontend alongside when changing it.

## Layout

| Module | Ported from (Node) |
|---|---|
| `server/src/main.rs` | boot sequence of `src/index.mjs` |
| `server/src/app.rs` | middleware chain of `src/index.mjs` (sessions, bans, CSRF, …) |
| `server/src/http/` | flummpress: `Req`, responses, router, guards, static files |
| `server/src/tpl.rs` | template engine + the patched `tpl.render` (globals) |
| `server/src/db.rs` | postgres.js (`Sql` builder, `sql!` macro, JSON rows) |
| `server/src/config.rs` | `src/inc/config.mjs` |
| `server/src/settings.rs` | `src/inc/settings.mjs` (+ motd, page texts) |
| `server/src/security.rs` | `src/inc/security.mjs` |
| `server/src/util.rs` | `src/inc/lib.mjs` (helpers) |
| `server/src/private_items.rs`, `hidden_items.rs` | same names in `src/inc/` |
| `server/src/proc.rs` | `queue.spawn` (external tools, no shell) |
| `server/src/net_guard.rs` | SSRF guard: URL validation and the egress proxy for curl/yt-dlp |
| `server/src/chan_http.rs` | `src/inc/chan_http.mjs` (4chan requests: host allow-list, curl-impersonate, media streaming) |
| `server/src/routes/<name>.rs` | `src/inc/routes/<name>.mjs` (apiv2 → `routes/apiv2/<name>.rs`) |
| `server/src/bot/` | `cuffeo` + `src/inc/events` + `src/inc/trigger` |

Already ported modules you should reuse (don't duplicate them):

| Module | Contents |
|---|---|
| `f0cklib/` | feeds and item pages: `getf0cks`, `getf0ck`, `get_item_page`, `get_random(_pool)`, `FeedOpts`, `compute_base_mode`, `resolve_mime_sql`, search scoring |
| `social/` | comments (`get_comments`, mentions/embeds), subscriptions, halls, user halls; `social::helpers` has audit log, word filter, anon activity log |
| `media/` | upload pipeline: `queue` (thumbnails, phash, repost checks, ffprobe), `delete` (`safe_delete_media_file`, `move_to_deleted`), `multipart` (streaming parser with limits), `mime`, `transcode`, `autotagger` |
| `auth.rs`, `anon_auth.rs`, `webauthn.rs` | session create/destroy, reCAPTCHA, anonymous identities, passkeys (table `webauthn_credentials`; the old `passkey_credentials` is unused) |
| `smtp.rs`, `gate.rs`, `bot/` | mail, private-society gate, chat bots (`st.bot`) |
| `wordfilter.rs` | `apply_word_filter` (comment word filter; `social::helpers` re-exports it) |
| `retention.rs` | personal-data retention job (started from `main.rs`) |

Reference ports: `routes/banned.rs`, `routes/theme.rs`, `routes/comments.rs`, `routes/index.rs`.
Login/logout/forgot/reset password (from `admin.mjs`) are already in `routes/register.rs`.

## Routes

```rust
pub fn register(r: &mut Router) {
    r.get(r"^\/banned\/?$", banned);                 // JS regex *source*, verbatim
    r.post_g(r"^\/api\/v2\/x$", Guard::UserAuth, x); // router.post(path, lib.userauth, cb)
    let mut g = Group::new(r, r"^\/api\/v2");        // router.group(...)
    g.get(r"\/tags\/(?<id>\d+)$", tags);
}
async fn banned(st: AppState, mut req: Req) -> Res { ... }
```

* JS string paths (`router.get('/api/chat', …)`) match the exact path only: register them with
  `r.exact(Method::GET, "/api/chat", None, h)` — `r.get` treats its argument as a regex.
* Copy the regex source **exactly** as written in the JS literal (keep the `\/` escapes). The router
  reproduces flummpress' route ordering from that string, so changing it changes which route wins.
* Register your module in `routes/mod.rs` (`pub mod x;` + `x::register(&mut r);`).
* Guards: `lib.auth`/`lib.adminAuth` → `Guard::Auth`/`AdminAuth`, `lib.modAuth` → `ModAuth`,
  `lib.userauth` → `UserAuth`, `lib.registeredUser` → `RegisteredUser`, `lib.loggedin` → `LoggedIn`,
  `lib.chanAuth` → `ChanAuth`, `lib.apiKeyAuth` → `ApiKeyAuth`.
* The old "bypass" middleware in index.mjs (upload handlers etc.) are registered with
  `r.pre(Method::POST, r"^\/api\/v2\/upload$", None, handler)` — checked before normal routes.

### Request (`Req`)

`req.path` (pathname, trailing slashes stripped), `req.method`, `req.param("id")` (named regex
groups), `req.query("x")` / `req.query_all` / `req.query_object()` (`req.url.qs`),
`req.cookie("x")`, `req.header("x")`, `req.session()` (`Option<&Session>`; `req.session` field),
`req.mode`, `req.theme`, `req.real_ip()`, `req.is_xhr()`, `req.is_onion()`.
Body: `req.post().await?` → `&Value` (JSON or urlencoded, like `req.post`), `req.post_str("k")`,
`req.body_bytes(limit)`, `req.take_body()` (stream for uploads).
Cookies to set: `req.set_cookie(format!("k=v; {}", cookie_options(&cfg, CookieExpiry::MaxAge(n), http_only)))`.

### Responses

`html(body)`, `html_code(body, code)`, `json_resp(&json!(..), code)`, `json_ok(json!(..))`,
`fail(code, "msg")` (`{success:false,msg}`), `redirect(url)` (307 like `res.redirect`),
`redirect_code(url, code)`, `reply(code, "text/plain", body)` (`res.reply`), `raw(code, body)`
(`res.writeHead(code).end(body)`, no content type), `json_raw(&v, code)` (`application/json`
without charset, for `writeHead(..{'Content-Type':'application/json'})`).
`res.reply({ body })` defaults to `200 text/html`. Errors: return `Err(..)` / use `?` → logged with
a reference id, client gets a generic 500.

### Session

`crate::session::Session` is typed (see the struct). Anonymous users: `is_anon`, `fingerprint`,
`anon_login`; `user == "anonymous"`. Helpers in `settings.rs`: `is_anon_session`,
`can_anon_do(&cfg, "comment")`, `session_owner_name`, `can_use_chan`, `check_anon_permission`, …
In templates the session is `req.session_value()` (or `false`).

## Database

```rust
use crate::{sql, db::{Sql, Row, RowExt}};
let rows: Vec<Row> = sql!("select id, tag from tags where id = ", id, " limit 1").fetch_all(&st.db).await?;
let row: Option<Row> = sql!(...).fetch_optional(&st.db).await?;
let n: Option<i64> = sql!("select count(*) from items").fetch_scalar(&st.db).await?;
let ids: Vec<i32> = sql!("select id from items where ...").fetch_scalar_list(&st.db).await?;
let affected: u64 = sql!("update ...").execute(&st.db).await?;
```

* In `sql!`, **string literals are SQL text, everything else is a bound parameter** (`$n`).
  Literal numbers/bools must be written inside the SQL text. Nested fragments: `@frag`
  (`sql!("select ... where ", @filter, " limit ", n)`), or build with
  `let mut q = Sql::new("..."); q.push("..").bind(v).append(&frag);`.
* `db.unsafe(x)` → `q.push(&x)` (only with SQL built from trusted code, never user input).
  `${db(list)}` for `IN (...)` → `q.bind_list(&ids)` or `= ANY(` + bind(vec) + `)`.
* Bound types are strict: bind `i32` for `integer` columns in comparisons that need it, `i64` for
  bigint; parse user input to numbers first (`+req.params.id` → `parse::<i32>()`, reject on error).
  `count(*)` is bigint → `fetch_scalar::<i64>`. `Option<T>` binds NULL.
* Rows are JSON objects with postgres.js conventions (int8/numeric → **string**, timestamps → ISO
  string, json → JSON). `RowExt`: `row.str("k")`, `row.string("k")`, `row.i64("k")` (parses strings),
  `row.bool("k")` (JS truthiness), `row.val("k")`. Pass rows to templates/JSON unchanged
  (`Value::Object(row)`), so output keeps the old shape.
* Transactions: `let mut tx = st.db.begin().await?; q.execute(&mut *tx).await?; tx.commit().await?;`
* `pg_notify`: `sql!("select pg_notify('chan', ", payload_string, ")")`.
* Typed structs are welcome for request input and internal logic (`serde::Deserialize` on
  `req.post()`), but keep DB rows that go to templates/JSON as `Row` unless there's a reason.

## Config & settings

`let cfg = st.cfg();` then `cfg.get("websrv.x")` (`&Value`), `cfg.truthy("a.b")` (`!!cfg.a.b`),
`cfg.enabled("a.b")` (`cfg.a.b !== false`), `cfg.is_false`, `cfg.str_or("a.b", "dflt")`,
`cfg.int("a.b")`, `cfg.str_list`, `cfg.nsfl_tag_id()`, `cfg.enable_nsfl()`, `cfg.mimes()`,
`cfg.paths.b` etc. `cfg.nsfp` → **`st.settings.nsfp_ids()`** (live-editable). DB-backed site
settings: `st.settings.manual_approval()`, `min_tags()`, `protect_files()`, `settings::save(&st, k, v)`
+ `st.settings.update(|s| s.x = ..)`. Caches: `st.caches.*`, `st.invalidate_session_cache(hash)`,
`st.invalidate_upload_count_cache(user)`, `st.update_halls_cache()`.

## Templates

Templates in `views/` are MiniJinja now (converted automatically from flummpress). Render with
`crate::tpl::render(&st, Some(&req), "item", json!({...}))` — same data keys as the old
`tpl.render("item", {...}, req)`. Includes rendered without `req` in the old code pass `None`.

* Autoescape is ON. Values that are trusted HTML must be passed pre-built and printed with
  `|safe` in the template; naming them `*_html` makes that obvious. Never mark user input safe.
* Inside `<script>`: quoted values use `{{ x|string|tojson }}` (the converter does this), JSON blobs
  are pre-serialized with `util::json_for_script` and printed `|safe`.
* Helpers: `t(key, data)`, `x|locale_string(lang)`, `|locale_date`, `|locale_time`,
  `|locale_number`, `|format_size`, `|fixed(n)`, `|encode_uri_component`, `|time_ago`, `|entries`,
  `mtime(path)`, `now()`, `floor()`, `parse_int()`, plus MiniJinja builtins and Python string
  methods (`x.startswith(..)`, `x.split(..)`).
* Converted templates may still contain `PORT_TODO` markers where the converter couldn't translate
  the JS (search for it). Fix those you touch.
* Verify syntax with `./server/target/debug/f0ckm check-templates` (from the worktree root).

## External tools, files, HTTP

* `crate::proc::spawn("ffmpeg", &[...]).await?` / `spawn_with(.., Opts { timeout, .. })` — never a shell.
* User-supplied URLs: check with `net_guard::validate_remote_url`, and pass `media::proxy_url(cfg)` as
  `--proxy` to curl/yt-dlp so every connection (incl. redirects) goes through the SSRF guard.
* Paths from URLs: `http::files::safe_join(dir, rel)` (refuses `..`).
* Outgoing HTTP: `st.http` (reqwest). Chat notifications: `st.bot.notify_matrix(&cfg, msg)`,
  `st.bot.send(network, target, msg)`.

## Tooling

* `f0ckm help` lists the CLI commands (`server/src/cli/`): setup (seed, create-admin, users, API keys),
  media maintenance (import, regen, rehash, clean, …) and build steps (build-css, check-templates).
* Browser libraries (Ruffle, Cap, marked, JSZip) are npm packages pinned in `package-lock.json`, not
  in git: `npm ci && npm run vendor` copies them into `public/s/` (the Docker build does this itself).
* Tests: `cargo test --offline` in `server/`; templates: `f0ckm check-templates`.
