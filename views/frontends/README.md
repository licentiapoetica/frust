# Alternative frontends

Besides the default templates in `views/` (the "f0ckm" frontend), the site can render old layouts:
`views/frontends/<name>/` (templates) plus `public/s/fe/<name>/` (their CSS, JS and assets).
`websrv.frontend` picks the site default; visitors switch with `/frontend/<name>` or
`?frontend=<name>` (cookie) when `websrv.frontend_switch` is on.

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
};
```

The DM badge: `messages.js` updates `#dm-badge` when the frontend has one.

The navbar brand (image and text from the admin panel, templates get `custom_brand_image` and
`custom_navbar_brand_text`): mark them `<img data-brand-img>` and
`<span data-brand-text data-text="{{ custom_navbar_brand_text }}" data-fallback="{{ domain }}">`
inside `.navbar-brand`, and `f0ck-core.js` keeps them up to date. The text sits next to the image;
with neither, the site name shows.

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
  - everything works without JavaScript.

  Old frontends keep their original CSS, rounded corners included.
