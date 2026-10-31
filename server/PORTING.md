# Porting guide (Node → Rust)

The Node backend in `src/` is being replaced by the Rust server in `server/`. Port behaviour
faithfully: same URLs, same status codes, same JSON shapes, same template data. Where the old code
has an obvious bug or security hole, fix it and mention it in your report (don't silently change
behaviour otherwise).

There is no production deployment: stored data (password hashes, passkeys, file layouts) does not
need to stay compatible, so prefer best practice over mirroring old internals. What must keep
working is the browser frontend (`public/s/js`, `views/`) — keep endpoints, JSON shapes and
template data, or update the frontend alongside. Don't alter existing tables in the dev database
(the Node reference server still runs on it); add new tables/columns via a `migrations/*.sql` file
and `server/src/migrations.rs`.

## Layout

| Rust | Old |
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
| `server/src/chan_http.rs` | `src/inc/chan_http.mjs` (4chan requests: host allow-list, curl-impersonate, media streaming) |
| `server/src/routes/<name>.rs` | `src/inc/routes/<name>.mjs` (apiv2 → `routes/apiv2/<name>.rs`) |
| `server/src/bot/` | `cuffeo` + `src/inc/events` + `src/inc/trigger` |

Already ported modules you should reuse (don't duplicate them):

| Module | Contents |
|---|---|
| `f0cklib/` | feeds and item pages: `getf0cks`, `getf0ck`, `get_item_page`, `get_random(_pool)`, `FeedOpts`, `compute_base_mode`, `resolve_mime_sql`, search scoring |
| `social/` | comments (`get_comments`, mentions/embeds, xD score), subscriptions, halls, user halls; `social::helpers` has audit log, word filter, anon activity log |
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
* Paths from URLs: `http::files::safe_join(dir, rel)` (refuses `..`).
* Outgoing HTTP: `st.http` (reqwest). Chat notifications: `st.bot.notify_matrix(&cfg, msg)`,
  `st.bot.send(network, target, msg)`.

## Rules for parallel work

* Work only in your own worktree and branch. Commit there (small logical commits, message ending
  with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`). Don't push, don't touch `master`.
* Don't restructure foundation files. You may **add** functions/fields to shared files
  (`util.rs`, `state.rs`, `settings.rs`, `http/mod.rs`, `tpl.rs`) when needed — append, don't
  rewrite — and list every such change in your final report so merges stay easy.
* Build with `cargo check --offline` (fast) and `cargo build --offline` when you need to run.
  No network: all crates are already in `~/.cargo`. Keep `CARGO_BUILD_JOBS=4` (shared machine).
* Running for tests: from the worktree root,
  `STORAGE_DIR=/home/nx/Projects/f0ckm/f0ckm-data DB_HOST=localhost DB_PORT=5454 F0CKM_PORT=<your port> ./server/target/debug/f0ckm`.
  The old Node server (reference) runs at `http://localhost:1337` against the same database.
  The database is the owner's development data: read freely; only write rows you create for a test
  and remove them afterwards. Never delete or alter existing data, never run destructive SQL.
* Unported dependencies: if you need a function another agent is porting, write a minimal local
  version or a clearly marked `TODO(port): ...` stub and mention it in the report.

## Tooling and removing Node

The Node scripts are replaced by subcommands of the binary (`server/src/cli/`, `f0ckm help`); the
Docker image has no Node. What still has to go in the final cleanup step (nothing in the Rust server
reads these):

* `src/` (the Node server), `scripts/*.mjs`, `debug/`, `package.json`, `package-lock.json`,
  `node_modules/`, `.eslintrc.json`
* the fallbacks to them: `locales_dir()` in `main.rs` (`src/inc/locales`) and the
  `node_modules/@ruffle-rs/ruffle` fallback in `routes/static_files.rs`
