# CLAUDE.md

## Database changes MUST go into the schema (non-negotiable)

Every database change — new table, column, index, constraint, sequence, type, default, rename, drop — **must be written directly into [migrations/f0ckm_schema.sql](migrations/f0ckm_schema.sql) in the same change.** Nothing may exist only in a migration file, in `server/src/migrations.rs`, or in a live database.

Why: a fresh install builds its whole database from `f0ckm_schema.sql` (see `server/src/main.rs`). Anything missing from it is lost on every new setup.

How:
- Add the object to `f0ckm_schema.sql` in the matching pg_dump section (the `DROP ... IF EXISTS` block at the top, `CREATE TABLE`, sequences/defaults, primary keys, indexes, foreign keys), so the file stays a complete, valid dump.
- Existing databases still need the change too: add an idempotent statement (`IF NOT EXISTS` / `IF EXISTS`) to `STARTUP_MIGRATIONS` in [server/src/migrations.rs](server/src/migrations.rs), and a `migrations/*.sql` file if useful.
- Before finishing, check the schema and the startup migrations agree (same column types, defaults, nullability, constraints).

## Design rules (non-negotiable)

- **Never use rounded borders.** No `border-radius` anywhere: not in CSS files, not in JS, not in templates. Corners stay square. (Exception: the ported old frontends below keep their original CSS 1:1, rounded corners included.)
- **Never use inline CSS.** No `style="..."` attributes in templates or generated HTML, no `<style>` blocks in views, and no setting styles from JS (`el.style.x = ...`, `cssText`). To change looks from JS, toggle a class.
- **Always put CSS in the CSS files** in [public/s/css/](public/s/css/): `f0ckm.css` / `v0ck.css` for site-wide styles, or the page's own file (`upload.css`, `dm.css`, `meme-creator.css`). Use the theme variables in `:root` where they fit. Pages that don't load the site CSS have their own file: `gate.css` (private-society gate pages), `scroller.css` (standalone scroller), `flash-frame.css` (Flash iframe), `cap-gate.css` (captcha popup, used on both kinds of page).
- **Only allowed inline styles:** (1) a value that comes from the database/config at render time (user colour, banner URL), kept to that one value, ideally a custom property (`style="--user-color: {{ … }}"`) used by a class; (2) the dynamic `<style>` in `snippets/header.html` (fonts/config variables + the anti-flash `html` background); (3) the initial `display:none` on modals in navbar/footer/page-modal that also render on pages with no site CSS (private-society gate), where JS reads or resets the inline value. Everything else goes in a CSS file.
- **Never use inline scripts.** No executable `<script>…</script>` blocks in templates. JS goes in a `.js` file: page/snippet scripts live under `public/s/js/views/`, mirroring the template path (`views/admin/users.html` → `public/s/js/views/admin/users.js`), loaded with `<script src="/s/js/views/….js?v={{ mtime("/public/s/js/views/….js") }}"></script>` at the spot the code needs to run. Pass template values as `data-*` attributes or a `<script type="application/json" id="…-data">{{ {…}|tojson }}</script>` data block (not executed) that the .js file reads.
- **Everything must keep working without JavaScript.** Every feature, page and control has to stay usable for visitors with JavaScript disabled, and look like the JS version (no "please enable JavaScript" notices). How it's done here:
  - `<html>` is rendered with class `no-js`; `header.early.js` removes it before any CSS loads. Style no-JS-only things with `html.no-js …`, hide no-JS stand-ins with `html:not(.no-js) …` (see the "No JavaScript" block in `f0ckm.css`).
  - Modals open with CSS `:target` (link to `#modal-id`, close via a link to `#_`), like the login/register/info/report modals.
  - Actions are real `<form method="post">`s to the existing API endpoints with a `back` field: `server/src/nojs.rs` turns the JSON reply into a redirect back, with failure messages as a one-time flash. Send explicit states (add/remove), not toggles.
  - Captchas: Cap needs JavaScript, so forms also accept the image captcha (`snippets/cap-image.html`, `server/src/cap_image.rs`).
  - Don't rely on `<noscript>` wrappers (script blockers rewrite them); use plain elements hidden by the CSS above.
  - Check new UI in a browser with JavaScript off.
- **With JavaScript, everything is live (AJAX).** In every frontend, actions happen in place with `fetch`, never by a page reload: posting, replying to and deleting comments, favs, subscriptions, ratings, adding/removing/excluding tags, reports, mod actions, settings. The plain forms above are only the no-JS fallback; the frontend's JS intercepts them and updates the page itself (only leaving the page when it is gone, e.g. after deleting the item). Live events (new comments, uploads, notifications, ...) update the page in place too. Check new UI with JavaScript on as well: nothing may reload.
- **Never edit `bundle.css` by hand.** It is built from `f0ckm.css` + `privacy.css` + `v0ck.css` by `f0ckm build-css` (also at server start). Edit the source files instead.

## Alternative frontends (keep them working)

Besides the default templates in `views/`, the site can render old layouts (`w0bm`, `f0ckv2`) and two joke ones (`f0ckgle`, a search engine homepage parody; `f0ckyou`, a video site parody): templates in `views/frontends/<name>/`, assets in `public/s/fe/<name>/`. `websrv.frontend` picks the site default; visitors switch with `/frontend/<name>` or `?frontend=<name>` (cookie). **The contract is [views/frontends/README.md](views/frontends/README.md); read it before touching views, routes' template data or shared scripts.**

- **`f0ckm check-frontends` must report 0 problems** (it also runs in `cargo test` as `tpl::tests::frontends_follow_contract`).
- Shared pages (settings, messages, admin, mod, mod_reports, pending, banned, password-change, privacy) exist once in `views/` and render inside every frontend's shell (`shell_header` / `shell_footer`). Never copy them into a frontend.
- Shared client code lives in `public/s/js/f0ck-core.js` (helpers, `ModAction`, the one live socket `F0ckLive`); f0ckm.js uses it too. When f0ckm.js gets a helper that shared page scripts call, put it in `f0ck-core.js` (or a fallback there) so the shared pages keep working in every frontend.
- **A frontend covers every page.** A new frontend gets its own template for every own page (`tpl::FRONTEND_OWN_PAGES`: index, item, user, search, tags, tags_user, halls, user-halls, comments_user, notifications, subscriptions, ranking, upload, about, rules, terms, error, login, register), in its own look. No page may fall back to f0ckm's layout; `f0ckm check-frontends` fails on a missing one. A new visitor-facing page goes into that list and into every frontend.
- Halls are called **lists** for visitors (f0ckyou: playlists), at `/lists`, `/list/<slug>`, `/user/<name>/lists`, `/user/<name>/list/<slug>`; the API is `/api/v2/me/lists`, `/api/v2/admin/lists`, `/mod/lists/*`, `/admin/lists`, pictures `/list_image/`, `/user_list_image/`. Only the code (identifiers, templates' `hall*` data), the database and the config keys (`halls_enabled`) keep `hall`. New text says list, new links use those paths (the old page paths only redirect).
- Changing the data a route passes to a view: grep `views/frontends/*/<view>.html` and update the frontends' own pages too.
- A new user-facing feature on an own page (item page, grid, comments, upload, ...) has to be added to each frontend's version of that page, in its style.
- Old frontends keep their original CSS 1:1, rounded corners included (exception to the rule above). Everything else in this file applies to them.

### Building a new frontend (checklist, learned building f0ckgle)

A new frontend is new design: the design rules above apply in full (square corners, no inline CSS/JS, works without JS, live with JS). Use `views/frontends/f0ckgle/` + `public/s/fe/f0ckgle/` as the most complete example (one base `snippets/head.html`, `topbar.html`, `footer.html`; every page included).

**Pages and data**
- Every page in `tpl::FRONTEND_OWN_PAGES` plus `shell/header.html` / `shell/footer.html` (README). Look at an existing frontend's version of each page for the data it gets (`grep -rn 'tpl::render(' server/src/routes`, then the route's `json!`).
- `index.html` also handles `private_society` without session (include the default `index.html`), `?login=1` and `?register=1` (own `login.html` / `register.html`; the login route renders `login` again on errors with `error`).
- 4chan proxy items (`item.is_chan`) keep f0ckm's item page: `{% include "item.html" %}`.
- Optional: `ajax-item.html` (Onara viewer), `settings-prefs.html` (only the preferences the frontend uses).

**Template gotchas (minijinja)**
- An undefined variable passes `is sequence`: test `x is defined and x is sequence`.
- Build lists in one expression (`([..] if a else []) + ([..] if b else [])`) rather than `{% set %}` inside `{% if %}` blocks.
- Give the `<body>` class a prefix (`fg-page-item`): a body class equal to a layout class (`fg-item`) styled the whole page.

**JavaScript**
- Load order at the end of `<body>`: page data, `snippets/frontend-scripts.html` (f0ck-core.js, live events, quick upload), then the frontend's own script, so `f0ck-core.js` helpers exist when it runs.
- Set the `window.f0ckFrontend` hooks (flash, comments, notify, newItem, dmBadge, reconnected, motd); without them live events do nothing visible.
- Live actions: intercept the plain forms with one delegated `submit` listener, send JSON (`Accept: application/json` makes the endpoints answer JSON instead of the no-JS redirect), update the page in place. Swapping server-rendered regions from a fresh `fetch` of the page (f0ckgle: `.fg-ajax` forms, `data-fg-live` regions) keeps the markup in one place. Use delegated listeners so swapped-in content keeps working.
- Read a form's URL with `form.getAttribute('action')`: an input named `action` (the fav form) shadows `form.action`.
- Comments: call `window.f0ckCaptchaGate('comment')` before posting with fetch; the no-JS form takes the image captcha (`snippets/cap-image.html` or the frontend's copy of it).
- Custom emojis are client-side in every frontend: the server sends `:name:` as text. Use `f0ckLoadEmojis()` / `f0ckEmojify()` (f0ck-core.js) on comment texts (text nodes only), again after every swap and on `f0ck:emojis_updated`, and give the comment form an emoji picker. Keep `.f0ck-emoji` small (comment-fx.css) against the frontend's own `img` rules.
- Flash: `/s/js/flash-embed.js` only defines `<f0ck-flash>`; the frontend's script creates it in `#ruffle-container` and calls `load({ url, autoplay: true })`.
- The exclude-tag forms (`.f0ck-exclude-form`) are handled by f0ck-core.js; don't intercept them twice.
- messages.js shows `#dm-badge` with an inline `display`: hide the badge by default in CSS.

**CSS**
- Media type filters: the `mime` cookie (f0ckm's picker) wins over `/video`-style paths, so type tabs go through `/mime?video=1&back=<listing>` (`/mime?back=…` clears it). Offer only the site's types: `scroller_mime_cats` (allowedMimes), Flash with `enable_swf`, anonymous users `anon_permissions.allowed_mimes`; no tabs for a single type. The homepage must not depend on `tmp.mime`.
- Light frontends: `upload.css` and `upload-modal.css` hard-code dark colours (white text, white-alpha borders and fields). Override them scoped to the upload page and `#upload-drag-modal` (see the upload block in f0ckgle.css), and give both f0ckm's colour variables (`--accent`, `--accent-rgb`, `--bg`, `--white`, `--upload-opt-color`, ...).
- Shell pages: `f0ck-shell.css` resets the chrome with `all: revert`, which also wipes inline SVG geometry: use icon files (`<img>` or `<use href="sprite.svg#id">`). It loads after the frontend's shell CSS, so rules for `.f0ck-shell-page` need `html.f0ck-shell .f0ck-shell-page` to win. A light frontend renders the shell with `theme="light"` (f0ckm.css' light theme) and sets `--white` / `--black` again on `html.f0ck-shell[theme="light"]` (f0ck-shell.css forces the dark values).
- Text size: a `html.user-fs-<n>` rule for every size in `crate::fonts::SIZES`; user font: `user_font_css` with `id="user-font-css"` and `data-user-font-class` on `<html>`.

**Tests and checks**
- `f0ckm check-frontends` → 0 problems.
- Add the frontend to the per-frontend tests in `server/src/tpl.rs`: `text_sizes_have_css`, `register_forms_take_applications` (register form + tos-consent), and a render test of every own page with sample data (f0ckgle: `f0ckgle_pages_render`, `F0CKM_DUMP_F0CKGLE=<dir>` writes the pages out).
- Look at the pages in a browser with JavaScript on and off, including the shared pages in the shell and the upload form after picking a file. Headless Chromium works for this: serve the dumped pages with `public/` on a scratch port (never the user's :1337 server) and drive it over the DevTools protocol.
