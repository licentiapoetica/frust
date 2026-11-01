# CLAUDE.md

## Database changes MUST go into the schema (non-negotiable)

Every database change — new table, column, index, constraint, sequence, type, default, rename, drop — **must be written directly into [migrations/f0ckm_schema.sql](migrations/f0ckm_schema.sql) in the same change.** Nothing may exist only in a migration file, in `server/src/migrations.rs`, or in a live database.

Why: a fresh install builds its whole database from `f0ckm_schema.sql` (see `server/src/main.rs`). Anything missing from it is lost on every new setup.

How:
- Add the object to `f0ckm_schema.sql` in the matching pg_dump section (the `DROP ... IF EXISTS` block at the top, `CREATE TABLE`, sequences/defaults, primary keys, indexes, foreign keys), so the file stays a complete, valid dump.
- Existing databases still need the change too: add an idempotent statement (`IF NOT EXISTS` / `IF EXISTS`) to `STARTUP_MIGRATIONS` in [server/src/migrations.rs](server/src/migrations.rs), and a `migrations/*.sql` file if useful.
- Before finishing, check the schema and the startup migrations agree (same column types, defaults, nullability, constraints).

## Design rules (non-negotiable)

- **Never use rounded borders.** No `border-radius` anywhere: not in CSS files, not in JS, not in templates. Corners stay square.
- **Never use inline CSS.** No `style="..."` attributes in templates or generated HTML, no `<style>` blocks in views, and no setting styles from JS (`el.style.x = ...`, `cssText`). To change looks from JS, toggle a class.
- **Always put CSS in the CSS files** in [public/s/css/](public/s/css/): `f0ckm.css` / `v0ck.css` for site-wide styles, or the page's own file (`upload.css`, `dm.css`, `meme-creator.css`). Use the theme variables in `:root` where they fit.
- **Never edit `bundle.css` by hand.** It is built from `f0ckm.css` + `v0ck.css` by `f0ckm build-css` (also at server start). Edit the source files instead.
