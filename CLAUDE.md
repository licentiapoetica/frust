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
- **Never edit `bundle.css` by hand.** It is built from `f0ckm.css` + `v0ck.css` by `f0ckm build-css` (also at server start). Edit the source files instead.

## Alternative frontends

Besides the default f0ckm templates in `views/`, the site can render an old frontend: `views/frontends/<name>/` (templates with the same view names, e.g. `index.html`, `item.html`) and `public/s/fe/<name>/` (their CSS/JS/assets). `websrv.frontend` picks the site default; visitors switch with `/frontend/<name>` (cookie) when `websrv.frontend_switch` is on; `?frontend=<name>` previews one page. A view a frontend has no template for falls back to the default one (see `tpl::frontend_for` / `tpl::render`). Current ones: `w0bm` (w0bm.com layout1), `w0bm2` (w0bm.com layout2, "njum") and `f0ckv2`.

- These are frontend-only ports: no backend changes for them; they use the same route data and API endpoints as the default frontend.
- They keep the original look 1:1 (their own CSS, rounded corners allowed there). All other rules above (no inline scripts, CSS in files, no-JS support) apply to them too.
