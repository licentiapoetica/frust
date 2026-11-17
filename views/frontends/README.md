# Alternative frontends

Besides the default templates in `views/` (the "f0ckm" frontend), the site can render old layouts:
`views/frontends/<name>/` (templates) plus `public/s/fe/<name>/` (their CSS, JS and assets).
`websrv.frontend` picks the site default; visitors switch with `/frontend/<name>` or
`?frontend=<name>` (cookie) when `websrv.frontend_switch` is on. `websrv.frontends` (a list of
names) limits which installed frontends can be picked; empty or missing means all of them, and the
site default is always available (`tpl::frontend_names`).

**Check:** `f0ckm check-frontends` (also run by `cargo test`: `tpl::tests::frontends_follow_contract`).
It must report 0 problems.

## How a page is picked

For a view `X` (e.g. `index`, `item`, `user`), `tpl::render` uses `views/frontends/<fe>/X.html`
if it exists, else the default `views/X.html`.

There are two kinds of pages:

1. **Own pages.** These are the frontend's own templates in its own look: index, item, user,
   comments, upload, search and the like. The frontend gets the same data as the default view; see
   `grep -rn 'tpl::render(' server/src/routes`.
2. **Shared pages.** These are default views that include `shell_header` / `shell_footer`
   (settings, messages, admin, mod, mod_reports, pending, banned, user halls, privacy). They exist once.
   - A frontend must **not** override them.
   - `tpl::render` passes `shell_header` / `shell_footer` = the frontend's
     `shell/header.html` / `shell/footer.html`, so they show inside the frontend's chrome.
   - `f0ckm check-frontends` lists them.

## What a frontend must provide

**Every page.** A frontend has its own template for every own page, in its own look; none may fall
back to f0ckm's layout: `about`, `comments_user`, `error`, `halls`, `index`, `item`, `login`,
`notifications`, `ranking`, `register`, `rules`, `search`, `subscriptions`, `tags`, `tags_user`,
`terms`, `upload`, `user` (`tpl::FRONTEND_OWN_PAGES`; `f0ckm check-frontends` reports a missing one).
The shared pages come with the shell (below). Only standalone pages with their own look keep
f0ckm's: the scroller, the maintenance page, forgot-password, the 4chan proxy and the meme creator.
A new page the routes render goes into that list and into every frontend.

**`shell/header.html`** opens a shared page:
- the doctype;
- `<html class="… f0ck-shell">` with the palette variables (below);
- `<head>` with the frontend's own CSS, then `{% include "snippets/shell-head.html" %}`;
- the frontend's navbar and other chrome, each chrome root marked `class="f0ck-shell-chrome"`;
- then `<div class="f0ck-shell-page">` (the page content follows).

**`shell/footer.html`** closes it:
- `</div>` (closing `.f0ck-shell-page`);
- the frontend's footer chrome (also marked `f0ck-shell-chrome`);
- the frontend's own scripts;
- `{% include "snippets/frontend-scripts.html" %}`, then `{% include "snippets/shell-foot.html" %}`;
- `</body></html>`.

**Own pages** include `{% include "snippets/frontend-scripts.html" %}` at the end of `<body>`.
Usually that goes in the frontend's base layout. It provides:
- the page data (`window.f0ckSession`, `f0ckI18n`);
- `f0ck-core.js`;
- for logged-in visitors: `messages.js` (so DM keys exist on every page) and the live events
  (`f0ck-frontend.js`).

**Palette.** Set these on `html.f0ck-shell`; they style the shared pages through `/s/css/f0ck-shell.css`:
- `--fe-accent`, `--fe-accent-rgb`;
- `--fe-bg`, `--fe-surface`, `--fe-surface-2`, `--fe-border`;
- `--fe-text`, `--fe-muted`, `--fe-link` (plain links; component links keep f0ckm's colours);
- `--fe-font`, `--fe-heading-font`;
- `--fe-input-bg`, `--fe-radius`;
- `--fe-navbar-h`: the height of a sticky navbar, so sticky parts of the shared pages (the settings
  quick-nav) stay below it.

Frontend-specific fixes for the shared pages go in the frontend's own CSS, scoped to
`.f0ck-shell-page`.

**f0ckm.css `!important` rules.** f0ckm.css sets a few generic class names with `!important`:
`.navbar`, `.navbar-brand` (a sticky flex bar, a 40px brand), the dropdown arrows
(`.nav-link::after`), `.pagination`, and the page background and hidden scrollbars on
`html`/`body`. Variables like `--nav-bg` also come from f0ckm.css. Unlayered frontend CSS cannot
override layered `!important` declarations. So on shell pages, a frontend whose chrome uses those
class names restores its own values with `!important` in its shell CSS, inside
`@layer f0ck-shell-important { … }`. That layer is declared before f0ckm, so its important
declarations win. Examples: `public/s/fe/w0bm/css/w0bm-shell.css` and `public/s/fe/f0ckv2/css/f0ckv2-shell.css`.

**Onara viewer (optional).** With Onara on (settings, `f0ck_onara` cookie, `onara` config), an item
URL renders the grid page that contains the item, with the item over it (`routes/index.rs`
`onara_page`). A frontend opts in by having its own `ajax-item.html`, the item box on its own:
- `index.html` gets `is_onara_item`, `onara_item_html` (that box, rendered with the item's data) and
  the grid of the page it is on (`is_onara_active` on the item's thumbnail);
- it shows the box over the grid, with a close link to the grid page; prev / next / random stay the
  item's own links (f0ckv2: `index.html`, `snippets/item-body.html`, `.v2-onara` in f0ckv2.css).

Without `ajax-item.html` (w0bm), items always open on their own page and the settings page hides the
Onara toggle (`frontend_onara`, `tpl::frontend_has_onara`).

**Recommendations (optional).** A frontend that lists recommendations on its item page (f0ckm's
recommendations tab, `/api/v2/recommendations`) has `snippets/recommendations.html`. The item routes
then render the first page into the page as `recommendations` (the API's items, without the item
itself; `routes/index.rs` `add_recommendations`, `tpl::frontend_has_recommendations`), so the list
works without JavaScript. With JavaScript the frontend asks the API again with the visitor's
interests (`f0ckInterestEngine.getTopAffinities()`, f0ck-core.js: `session_tags` /
`session_creators`) and loads more with `continuation=1`, as sidebar-activity.js does; it starts the
interest tracking on its item page with `f0ckInterestEngine.startTrackingItem({ itemId, tags,
creator })`. f0ckyou: `snippets/recommendations.html`, the "Up next" part of f0ckyou.js.

**Item tools (the uploader and staff).** f0ckm's info box (`snippets/info-modal.html`) is what an
item's owner and the staff can do with it; a frontend's item page offers all of it, as plain forms
(f0ckgle / f0ckyou: the `#edit` dialog): title (`/api/v2/items/:id/title`), visibility
(`/api/v2/item/visibility`: public / unlisted / private, 451 for staff only, `can_manage_item`,
`is_mod_or_admin`), expiration (`/api/v2/items/:id/expiry`, presets and `custom` with
`expiry_amount` + `expiry_unit`, if `enable_expiring_uploads`), the file's details and similar uploads
(`item.show_repost_row`, `item.reposts`), a new thumbnail (`/api/v2/items/:id/rethumb`) and tag
suggestions from the file's metadata (`can_extract_meta`). For those the item page with `?meta=1`
gets `meta_fields` (the texts, each one an add-tag form) or `meta_error` (`routes/index.rs`,
`apiv2/index/meta.rs` `item_fields`), so they work without JavaScript; with it the frontend fetches
that render and shows its dialog in place (f0ckyou.js / f0ckgle.js `data-*-meta`).

**Player and blurred background (optional).** f0ckm's video player is `/s/js/v0ck.js` +
`/s/css/v0ck.css` (`new v0ck(video)`; f0ckv2, f0ckyou). Its settings menu calls
`window.toggleBackground` / `window.toggleAutoplay` when they exist. f0ckm's blurred item background
for a frontend is `/s/js/f0ck-bg.js`: a `<canvas id="bg">` (the admin's tuning in
`data-tuner="{{ audio_tuner_json }}"`) placed by the frontend's CSS behind a see-through page and faded
with `fader-in` / `fader-out`; `f0ckBackground.start({ media, thumb })` draws the video's frames,
a picture or Flash's `ambientCanvas` with f0ckm's tuning and switch (`show_background`), and defines
`window.toggleBackground` (f0ckyou: item.html, f0ckyou.js).

**Settings preferences (optional).** A frontend that only uses some of the preferences lists them in
`settings-prefs.html` (`{% set prefs = ["language", "show_motd", …] %}`, keys: the
`"…" in fe_prefs` tests in `views/settings.html`). The settings page then shows only those
(`frontend_settings_prefs`, tpl.rs); a group left empty is hidden. Without the file every
preference shows. w0bm: `views/frontends/w0bm/settings-prefs.html`.

**Hooks.** Set them in the frontend's main script; all are optional:

```js
window.f0ckFrontend = {
  itemId,            // id of the item on screen, if any
  flash(message, type),  // show a message ('success' | 'error' | 'info'); shared scripts call showFlash/flashMessage
  notify(data),      // a notification arrived: refresh the notification badge/list
  comments(data),    // comments of data.item_id changed: reload the list if it's on screen
  newItem(data),     // a new upload went live ({id, slug, tag_id, username, mime, …}): add it to the grid
  dmBadge(),         // a DM arrived and messages.js isn't loaded
  reconnected(),     // the live socket came back: events may have been missed
  motd(text),        // the MOTD changed
  brand(url, text),  // the navbar brand changed: url (image, null = removed) or text, the other undefined
  live(msg),         // any other live event ({type, data}), e.g. a frontend's own (f0ckyou: fy_brand)
};
```

The DM badge: `messages.js` updates `#dm-badge` when the frontend has one.

The navbar brand (image and text from the admin panel, templates get `custom_brand_image` and
`custom_navbar_brand_text`): mark them `<img data-brand-img>` and
`<span data-brand-text data-text="{{ custom_navbar_brand_text }}" data-fallback="{{ domain }}">`
inside `.navbar-brand`, and `f0ck-core.js` keeps them up to date. The text sits next to the image;
with neither, the site name shows.

The visitor's website font (settings, key `font` in `settings-prefs.html`): templates get
`user_font_css`, a stylesheet declaring the chosen family as `'CustomUserFont'` (empty without a
choice). Link it in the head with `id="user-font-css"`, put a class on `<html>` when it is set, and
give that class a `font-family: 'CustomUserFont'` rule that spares the frontend's icon fonts (w0bm:
`.w0bm-user-font` in w0bm-fe.css). Name the class in `<html data-user-font-class="…">`: settings.js
swaps the stylesheet and sets that class when the font or its weight is changed live. Without the
attribute nothing is applied live (never add f0ckm's `header-font-user`: f0ckm.css is layered into
the shell and its rule would force Font Awesome 6 onto the frontend's icons). The chosen weight is
declared as the normal weight (crate::fonts::Family::faces_at), so no CSS weight changes are needed.
The text size (`user_font_size`, a percent) is chosen per frontend (`user_options.font_sizes`, keyed
by frontend name; the settings page saves it for the frontend it is shown in; a frontend without
an entry has the default size, never another frontend's). It is a
`user-fs-<percent>` class on `<html>`: give each
size in crate::fonts::SIZES a rule (f0ckm, f0ckgle and f0ckyou scale the root font size, w0bm and f0ckv2 zoom the
body; `tpl::tests::text_sizes_have_css` checks them all).

## Rules

- **Shared client code.** It lives in `public/s/js/f0ck-core.js`: CSRF token, `escHTML`,
  time-ago, `ModAction`, and `F0ckLive`, the one live WebSocket. f0ckm.js uses it too.
  - Frontends never copy it and never open their own socket.
  - When f0ckm.js gets a helper that shared scripts call, move it to `f0ck-core.js` or give it a
    fallback there.
- **Comment text effects** (`[rb]` `[reich]` `[krebs]` `[glitch]`) look the same everywhere.
  - Every comment renderer runs its escaped HTML through `window.f0ckCommentFx` (f0ck-core.js);
    the no-JS comments get the server copy (`comment_fx`, server/src/routes/comments.rs).
  - Pages with comments load `/s/css/comment-fx.css`, plus `/s/css/glitchfx.css` and
    `/s/js/glitchfx.js` for `[glitch]`. GlitchFX picks up comments added later by itself.
  - A new effect goes into all three places: `F0CK_COMMENT_FX`, `COMMENT_FX` and comment-fx.css.
  - w0bm keeps its own classes for the first three (its theme CSS); `[glitch]` is shared.
- **Exclude-tag button** next to each tag of an item (not the rating tags): a plain form
  `.f0ck-exclude-form` posting to `/api/v2/settings/excluded_tags` (`remove=1` when the tag is
  excluded already), so it works without JavaScript. f0ck-core.js sends it with fetch and updates
  every such form on the page; tag lists a script rebuilds get it from `f0ckAddExcludeButton`.
  Markup in f0ck-core.js; the frontend's CSS shows `.f0ck-excl-off` / `.f0ck-excl-on`
  (w0bm: partials/tags.html, f0ckv2: snippets/item-body.html).
- **Quick upload** ("u", a file dropped anywhere) is f0ckm's upload dropdown in every frontend:
  `snippets/upload-modal-frontend.html` (from frontend-scripts.html) with upload.css,
  upload-modal.css and Font Awesome 6 in a layer (fa-layered.css). The frontend's CSS gives
  `#upload-drag-modal, #drop-overlay` f0ckm's colour variables (`--accent`, `--accent-rgb`, `--bg`,
  `--bg-form`, `--white`, `--text`, `--navbar-h`, ...), see w0bm-fe.css / f0ckv2.css.
- **Changing a shared page** (`views/settings.html`, `views/admin/*`, …) needs nothing per
  frontend: it renders in every shell.
  - Keep its markup working without f0ckm.js: no f0ckm.js-only globals unless `f0ck-core.js`
    has them.
  - Look at it in one alternative frontend.
- **Changing what a route passes to a view.** Own pages of every frontend use that data, so
  grep `views/frontends/*/<view>.html` and update them. The fallback only covers whole pages.
- **New user-facing features on own pages** (item page, grid, comments) need adding to each
  frontend's own page, in its style. Otherwise the feature doesn't exist there.
- The usual rules (CLAUDE.md) apply:
  - no inline scripts;
  - CSS in files;
  - everything works without JavaScript;
  - with JavaScript everything is live: actions (comments, replies, favs, tags, ratings, reports, ...)
    go through `fetch` and update the page in place, never by reloading it. f0ckgle does it generically:
    its forms are marked `.fg-ajax` and the regions they change `data-fg-live`, which f0ckgle.js swaps
    in from a fresh render of the page.

  Old frontends keep their original CSS, rounded corners included.
