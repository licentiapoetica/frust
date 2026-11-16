// The custom emoji picker of the alternative frontends: the same as f0ckm's (comments.js,
// setupEmojiPicker), built on the frontend's own markup and styled by the frontend's CSS.
//
//   f0ckEmojiPicker.attach({ textarea, trigger, box, onChange }) → { open, close, toggle }
//     textarea  the comment field the emojis go into (:name: at the cursor)
//     trigger   the button that opens and closes the picker
//     box       the (hidden) element the picker is shown in
//     onChange  called with the textarea after an emoji went in (e.g. to grow the field)
//
// What it does, as in f0ckm:
//  - tabs: Favorites first, then one per sticker pack (its picture), else one flat grid
//  - favourites: right-click a sticker (or hold it and tap the star) to add / remove it; kept in
//    localStorage and, for logged-in users, in the account (/api/v2/user/emoji-favorites)
//  - right-click (or hold) the Favorites tab: open the picker on it by default
//  - hold a sticker: a big preview; sliding / moving over others previews them
//  - typing :na… in the field: a list of matching emojis (arrows, Enter / Tab, Escape)
//  - the field keeps the focus (the phone keyboard stays open) while picking
//  - the form's reset (Cancel) closes the picker
// No inline styles: everything is classes and the hidden attribute (see the f0ck-ep block in the
// frontend's CSS). Emoji list: f0ckLoadEmojiData (f0ck-core.js).
(() => {
  'use strict';
  if (window.f0ckEmojiPicker) return;

  const FAV_KEY = 'f0ck_emoji_favs';
  const DEFAULT_TAB_KEY = 'f0ck_emoji_default_tab';
  const FAVS = '__favs__';
  const HOLD_MS = 800;
  const TAB_HOLD_MS = 500;
  const AC_BATCH = 20;

  const store = {
    get: (k) => { try { return localStorage.getItem(k); } catch { return null; } },
    set: (k, v) => { try { localStorage.setItem(k, v); } catch { /* ignore */ } },
    del: (k) => { try { localStorage.removeItem(k); } catch { /* ignore */ } },
  };
  const isVideo = (url) => /\.webm(?:$|[?#])/i.test(url || '');
  const el = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  };
  const media = (url, name) => {
    let m;
    if (isVideo(url)) {
      m = el('video', 'f0ck-ep-media');
      m.autoplay = m.loop = m.muted = m.playsInline = true;
    } else {
      m = el('img', 'f0ck-ep-media');
      m.loading = 'lazy';
      m.alt = name ? `:${name}:` : '';
    }
    m.src = url;
    m.draggable = false;
    m.addEventListener('error', () => { m.hidden = true; }, { once: true });
    return m;
  };
  const toast = (msg) => {
    if (window.f0ckFrontend?.flash) return window.f0ckFrontend.flash(msg, 'info');
    let t = document.querySelector('.f0ck-ep-toast');
    if (!t) { t = el('div', 'f0ck-ep-toast'); t.setAttribute('role', 'status'); document.body.append(t); }
    t.textContent = msg;
    t.classList.remove('is-out');
    t.classList.add('is-in');
    clearTimeout(toast.timer);
    toast.timer = setTimeout(() => t.classList.replace('is-in', 'is-out'), 2000);
  };

  // ── the emojis (shared by every picker on the page; again after the live event emojis_updated) ──
  let data = null;
  const load = () => (window.f0ckLoadEmojiData ? window.f0ckLoadEmojiData() : Promise.resolve({ map: {}, packs: [] }))
    .then((d) => { data = d || { map: {}, packs: [] }; return data; });

  // the open pickers, rebuilt when favourites or the emojis change
  const open = new Set();
  const refreshAll = () => open.forEach((p) => (p.box.isConnected ? p.refresh() : open.delete(p)));
  window.addEventListener('f0ck:emojis_updated', () => { data = null; if (open.size) load().then(() => open.forEach((p) => p.rebuild())); });

  // ── favourites (the same storage and account sync as f0ckm) ──
  const favs = {
    synced: false,
    get() {
      try { const v = JSON.parse(store.get(FAV_KEY) || '[]'); return Array.isArray(v) ? v : []; } catch { return []; }
    },
    has(name) { return this.get().some((f) => f.name === name); },
    toggle(name, url) {
      const list = this.get();
      const i = list.findIndex((f) => f.name === name);
      const action = i >= 0 ? 'remove' : 'add';
      if (i >= 0) list.splice(i, 1); else list.push({ name, url });
      store.set(FAV_KEY, JSON.stringify(list));
      if (window.f0ckSession?.logged_in) {
        fetch('/api/v2/user/emoji-favorites/toggle', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.getCsrfToken?.() || '', 'X-Requested-With': 'XMLHttpRequest' },
          body: JSON.stringify({ name, url, action }),
        }).catch(() => {});
      }
      return action === 'add';
    },
    sync() {
      if (this.synced || !window.f0ckSession?.logged_in) return;
      this.synced = true;
      fetch('/api/v2/user/emoji-favorites', { credentials: 'same-origin', headers: { 'X-Requested-With': 'XMLHttpRequest' } })
        .then((r) => (r.ok ? r.json() : null))
        .then((d) => {
          if (d?.success && Array.isArray(d.favorites)) { store.set(FAV_KEY, JSON.stringify(d.favorites)); refreshAll(); }
        })
        .catch(() => {});
    },
  };
  const toggleFav = (name, url) => {
    toast(favs.toggle(name, url) ? 'Emoji added to Favorites' : 'Emoji removed from Favorites');
    refreshAll();
  };

  // ── hold-to-preview (one overlay for the page) ──
  const preview = {
    node: null,
    showing: false,
    byTouch: false,
    clearTimer: null,
    ensure() {
      if (this.node?.isConnected) return this.node;
      this.node = el('div', 'f0ck-ep-preview');
      this.node.addEventListener('click', () => this.hide());
      document.body.append(this.node);
      return this.node;
    },
    show(name, url, byTouch) {
      const node = this.ensure();
      clearTimeout(this.clearTimer);
      const fav = el('button', 'f0ck-ep-preview-fav' + (favs.has(name) ? ' is-fav' : ''), favs.has(name) ? '★' : '☆');
      fav.type = 'button';
      fav.title = favs.has(name) ? 'Remove from Favorites' : 'Add to Favorites';
      fav.addEventListener('click', (e) => { e.stopPropagation(); this.hide(); toggleFav(name, url); });
      fav.addEventListener('touchend', (e) => { e.preventDefault(); e.stopPropagation(); fav.click(); });
      const m = media(url, name);
      node.replaceChildren(m, fav);
      node.classList.add('is-visible');
      this.showing = true;
      if (byTouch != null) this.byTouch = byTouch;
      if (m.play) m.play().catch(() => {});
    },
    hide() {
      if (!this.node) return;
      this.showing = false;
      this.byTouch = false;
      this.node.classList.remove('is-visible');
      clearTimeout(this.clearTimer);
      this.clearTimer = setTimeout(() => this.node?.replaceChildren(), 200);
    },
  };
  // released outside the sticker (desktop): the preview goes
  document.addEventListener('mouseup', () => { if (preview.showing && !preview.byTouch) preview.hide(); });

  // the favourites menu of a right-clicked sticker: closed by any other click or Escape
  const closeMenus = () => document.querySelectorAll('.f0ck-ep-menu:not([hidden])').forEach((m) => { m.hidden = true; });
  document.addEventListener('click', (e) => { if (!e.target.closest?.('.f0ck-ep-menu')) closeMenus(); });
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    closeMenus();
    if (preview.showing) preview.hide();
  });

  const attach = ({ textarea, trigger, box, onChange }) => {
    if (!textarea || !box) return null;
    if (textarea._f0ckEmojiPicker) return textarea._f0ckEmojiPicker;
    const changed = () => { try { onChange?.(textarea); } catch { /* ignore */ } };
    const isAdmin = !!window.f0ckSession?.is_admin;

    // ── :name autocomplete: the field goes into a .f0ck-ep-field box with the list right after it
    //    (the frontend's CSS places the list against that box) ──
    const ac = el('div', 'f0ck-ep-ac');
    ac.hidden = true;
    ac.setAttribute('role', 'listbox');
    if (!textarea.parentElement?.classList.contains('f0ck-ep-field')) {
      const field = el('div', 'f0ck-ep-field');
      textarea.before(field);
      field.append(textarea);
    }
    textarea.after(ac);
    let acMatches = [];
    let acShown = 0;
    let acActive = -1;
    const colon = () => {
      const pos = textarea.selectionStart;
      if (pos == null || pos !== textarea.selectionEnd) return null;
      const m = textarea.value.slice(0, pos).match(/:([a-z0-9_-]*)$/i);
      return m ? { query: m[1].toLowerCase(), at: pos - m[0].length } : null;
    };
    const hideAc = () => { ac.hidden = true; ac.replaceChildren(); acMatches = []; acShown = 0; acActive = -1; };
    const acInsert = (name) => {
      const hit = colon();
      if (!hit) return;
      textarea.setRangeText(`:${name}:`, hit.at, textarea.selectionStart, 'end');
      textarea.focus();
      hideAc();
      changed();
    };
    const acMore = () => {
      acMatches.slice(acShown, acShown + AC_BATCH).forEach((name, i) => {
        const item = el('div', 'f0ck-ep-ac-item');
        item.setAttribute('role', 'option');
        item.dataset.idx = String(acShown + i);
        item.append(media(data.map[name], name), el('span', '', `:${name}:`));
        item.addEventListener('mousedown', (e) => { e.preventDefault(); acInsert(name); });
        ac.append(item);
      });
      acShown = Math.min(acMatches.length, acShown + AC_BATCH);
    };
    const renderAc = () => {
      const hit = colon();
      if (!hit) return hideAc();
      if (!data) {
        load().then(() => { if (document.activeElement === textarea) renderAc(); });
        return hideAc();
      }
      acMatches = Object.keys(data.map).filter((n) => n.includes(hit.query));
      if (!acMatches.length) return hideAc();
      ac.replaceChildren();
      acShown = 0;
      acActive = -1;
      acMore();
      ac.hidden = false;
    };
    const acSet = (i) => {
      const items = ac.querySelectorAll('.f0ck-ep-ac-item');
      items.forEach((n) => n.classList.remove('is-active'));
      if (items[i]) { items[i].classList.add('is-active'); items[i].scrollIntoView({ block: 'nearest' }); }
      acActive = i;
    };
    textarea.addEventListener('input', renderAc);
    textarea.addEventListener('click', () => { if (colon()) renderAc(); });
    textarea.addEventListener('blur', () => setTimeout(hideAc, 150));
    textarea.addEventListener('keydown', (e) => {
      if (ac.hidden) return;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (acActive === acShown - 1 && acShown < acMatches.length) acMore();
        acSet((acActive + 1) % acMatches.length);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        const i = acActive <= 0 ? acMatches.length - 1 : acActive - 1;
        while (i > acShown - 1 && acShown < acMatches.length) acMore();
        acSet(i);
      } else if ((e.key === 'Enter' || e.key === 'Tab') && acActive >= 0 && !e.ctrlKey && !e.metaKey) {
        e.preventDefault();
        acInsert(acMatches[acActive]);
      } else if (e.key === 'Escape') {
        hideAc();
      }
    });
    ac.addEventListener('scroll', () => {
      if (ac.scrollHeight - ac.scrollTop - ac.clientHeight < 50) acMore();
    });

    // ── the picker ──
    box.classList.add('f0ck-ep');
    let activeTab = null;
    let grid = null;
    let tabBar = null;
    let menu = null;

    const insert = (name) => {
      const s = textarea.selectionStart ?? textarea.value.length;
      const e = textarea.selectionEnd ?? s;
      textarea.setRangeText(`:${name}:`, s, e, 'end');
      textarea.focus();
      changed();
    };
    const sticker = (name, url) => {
      const b = el('button', 'f0ck-ep-sticker');
      b.type = 'button';
      b.title = `:${name}:`;
      b.dataset.name = name;
      b.dataset.url = url;
      b.append(media(url, name));
      if (favs.has(name)) b.append(el('span', 'f0ck-ep-badge', '★'));
      return b;
    };
    const notice = (text, link) => {
      box.replaceChildren();
      box.classList.remove('has-tabs');
      box.classList.add('is-empty');
      const p = el('div', 'f0ck-ep-notice', text);
      if (link) { const a = el('a', '', link.text); a.href = link.href; p.append(' ', a); }
      box.append(p);
    };
    const showMenu = (name, url) => {
      if (!menu) return;
      closeMenus();
      const fav = favs.has(name);
      const b = el('button', 'f0ck-ep-menu-item');
      b.type = 'button';
      b.append(el('span', 'f0ck-ep-menu-star', fav ? '★' : '☆'), media(url, name),
        el('span', '', `${fav ? 'Remove' : 'Add'} :${name}: ${fav ? 'from' : 'to'} Favorites`));
      b.addEventListener('click', (e) => { e.stopPropagation(); menu.hidden = true; toggleFav(name, url); });
      menu.replaceChildren(b);
      menu.hidden = false;
    };
    const showTab = (id) => {
      if (!grid) return;
      activeTab = id;
      tabBar?.querySelectorAll('.f0ck-ep-tab').forEach((t) => {
        const on = t.dataset.packId === String(id);
        t.classList.toggle('is-active', on);
        t.setAttribute('aria-selected', String(on));
      });
      grid.replaceChildren();
      grid.classList.toggle('is-favs', id === FAVS);
      if (id === FAVS) {
        const list = favs.get();
        if (!list.length) grid.append(el('div', 'f0ck-ep-grid-empty', 'No favorites yet. Right-click (or hold) any sticker to add it here.'));
        list.forEach((f) => grid.append(sticker(f.name, f.url)));
        return;
      }
      const packs = data?.packs || [];
      const pack = packs.find((p) => String(p.id ?? null) === String(id)) || packs[0];
      (pack?.emojis || []).forEach((e) => grid.append(sticker(e.name, e.url)));
    };
    const toggleDefault = (tab) => {
      if (store.get(DEFAULT_TAB_KEY) === FAVS) {
        store.del(DEFAULT_TAB_KEY);
        tab.classList.remove('is-default');
        toast('Favorites unset as default tab');
      } else {
        store.set(DEFAULT_TAB_KEY, FAVS);
        tab.classList.add('is-default');
        toast('Favorites set as default tab');
      }
    };
    const build = () => {
      grid = tabBar = menu = null;
      box.classList.remove('is-empty', 'has-tabs');
      if (!data) return notice('Loading emojis…');
      if (!Object.keys(data.map).length) {
        return notice('No custom emojis yet.', isAdmin ? { text: 'Add some in the admin panel', href: '/admin/emojis' } : null);
      }
      favs.sync();
      box.replaceChildren();
      grid = el('div', 'f0ck-ep-grid');
      menu = el('div', 'f0ck-ep-menu');
      menu.hidden = true;
      const packs = data.packs || [];
      if (!packs.length) {
        // no packs: one flat grid, as in f0ckm
        Object.entries(data.map).forEach(([name, url]) => grid.append(sticker(name, url)));
        box.append(grid, menu);
        return;
      }
      box.classList.add('has-tabs');
      tabBar = el('div', 'f0ck-ep-tabs');
      tabBar.setAttribute('role', 'tablist');
      const favTab = el('button', 'f0ck-ep-tab f0ck-ep-tab-favs');
      favTab.type = 'button';
      favTab.title = 'Favorites (right-click or hold: open the picker here)';
      favTab.dataset.packId = FAVS;
      favTab.setAttribute('role', 'tab');
      favTab.append(el('span', 'f0ck-ep-tab-star', '★'));
      if (store.get(DEFAULT_TAB_KEY) === FAVS) favTab.classList.add('is-default');
      tabBar.append(favTab);
      packs.forEach((pack) => {
        const tab = el('button', 'f0ck-ep-tab');
        tab.type = 'button';
        tab.title = pack.name || 'Emojis';
        tab.dataset.packId = String(pack.id ?? null);
        tab.setAttribute('role', 'tab');
        const list = pack.emojis || [];
        if (list.length) {
          // a still picture: the pack's thumbnail, else its first non-video emoji
          const icon = el('img', 'f0ck-ep-tab-icon');
          icon.src = pack.thumb_url || (list.find((e) => !isVideo(e.url)) || list[0]).url;
          icon.alt = pack.name || '';
          icon.loading = 'lazy';
          icon.draggable = false;
          tab.append(icon);
        } else {
          tab.textContent = '☺';
        }
        tabBar.append(tab);
      });
      box.append(tabBar, grid, menu);
      showTab(store.get(DEFAULT_TAB_KEY) === FAVS ? FAVS : String(packs[0]?.id ?? null));
    };

    // ── pointer handling: hold-to-preview, right-click menu, taps that keep the keyboard open ──
    let holdTimer = null;
    let holdFired = false;
    let suppressClickUntil = 0;
    let lastTabHold = 0;
    let lastTouch = 0;
    let touch = null;
    const stickerAt = (t) => t?.closest?.('.f0ck-ep-sticker');
    const startHold = (s, byTouch) => {
      clearTimeout(holdTimer);
      holdFired = false;
      holdTimer = setTimeout(() => {
        holdFired = true;
        preview.show(s.dataset.name, s.dataset.url, byTouch);
      }, HOLD_MS);
    };
    const cancelHold = () => clearTimeout(holdTimer);

    // the field keeps the focus
    box.addEventListener('mousedown', (e) => {
      e.preventDefault();
      if (e.button !== 0 || Date.now() - lastTouch < 800) return;
      const s = stickerAt(e.target);
      if (s) startHold(s, false);
    });
    box.addEventListener('mouseup', () => {
      cancelHold();
      if (holdFired) {
        holdFired = false;
        suppressClickUntil = Date.now() + 400;
        preview.hide();
      }
    });
    box.addEventListener('mouseout', (e) => {
      const s = stickerAt(e.target);
      if (s && !s.contains(e.relatedTarget)) cancelHold();
    });
    box.addEventListener('mouseover', (e) => {
      const s = stickerAt(e.target);
      if (s && preview.showing && !preview.byTouch) preview.show(s.dataset.name, s.dataset.url, false);
    });
    box.addEventListener('click', (e) => {
      const tab = e.target.closest('.f0ck-ep-tab');
      if (tab) { e.stopPropagation(); closeMenus(); showTab(tab.dataset.packId); return; }
      const s = stickerAt(e.target);
      if (!s) return;
      e.stopPropagation();
      closeMenus();
      if (Date.now() < suppressClickUntil) return;
      insert(s.dataset.name);
    });
    box.addEventListener('contextmenu', (e) => {
      const tab = e.target.closest('.f0ck-ep-tab-favs');
      const s = stickerAt(e.target);
      if (!tab && !s) return;
      e.preventDefault();
      e.stopPropagation();
      // on touch screens the hold does it (preview with its star / default tab)
      if (e.pointerType === 'touch' || Date.now() - lastTouch < 800) return;
      if (tab) { if (Date.now() - lastTabHold > 1000) toggleDefault(tab); return; }
      if (!holdFired) showMenu(s.dataset.name, s.dataset.url);
    });
    box.addEventListener('touchstart', (e) => {
      lastTouch = Date.now();
      const t = e.touches[0];
      touch = { x: t.clientX, y: t.clientY, moved: false };
      holdFired = false;
      const favTab = e.target.closest('.f0ck-ep-tab-favs');
      const s = stickerAt(e.target);
      if (s) startHold(s, true);
      else if (favTab) {
        clearTimeout(holdTimer);
        holdTimer = setTimeout(() => {
          holdFired = true;
          lastTabHold = Date.now();
          try { navigator.vibrate?.(40); } catch { /* ignore */ }
          toggleDefault(favTab);
        }, TAB_HOLD_MS);
      }
    }, { passive: true });
    box.addEventListener('touchmove', (e) => {
      lastTouch = Date.now();
      const t = e.touches[0];
      if (touch && (Math.abs(t.clientX - touch.x) > 10 || Math.abs(t.clientY - touch.y) > 10)) {
        touch.moved = true;
        if (!holdFired) cancelHold();
      }
      // sliding over the stickers while the preview is up shows each one
      if (preview.showing) {
        const s = stickerAt(document.elementFromPoint(t.clientX, t.clientY));
        if (s && box.contains(s)) preview.show(s.dataset.name, s.dataset.url, true);
      }
    }, { passive: true });
    box.addEventListener('touchend', (e) => {
      lastTouch = Date.now();
      cancelHold();
      const t = touch;
      touch = null;
      if (holdFired) {
        // the preview stays until it is tapped (its star adds to favourites)
        e.preventDefault();
        holdFired = false;
        return;
      }
      if (!t || t.moved) return; // a scroll
      // a tap: click it ourselves so the field keeps the focus (keyboard stays open)
      const target = e.target.closest('.f0ck-ep-tab, .f0ck-ep-sticker, .f0ck-ep-menu-item, a');
      if (!target) return;
      e.preventDefault();
      target.click();
    }, { passive: false });
    box.addEventListener('touchcancel', () => { cancelHold(); touch = null; });

    // ── open / close (the frontend's CSS animates .is-opening / .is-closing) ──
    let closing = 0;
    const isOpen = () => !box.hidden && !box.classList.contains('is-closing');
    const reveal = () => {
      let done = false;
      const run = () => {
        if (done || !isOpen()) return;
        done = true;
        box.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
      };
      box.addEventListener('animationend', run, { once: true });
      setTimeout(run, 300);
    };
    const ctl = {
      box,
      refresh() { if (grid && tabBar) showTab(activeTab); else if (grid) build(); },
      rebuild: () => build(),
      open() {
        closing++;
        box.classList.remove('is-closing', 'is-opening');
        build();
        if (!data) load().then(() => { if (isOpen()) build(); });
        box.hidden = false;
        void box.offsetWidth; // restart the animation
        box.classList.add('is-opening');
        trigger?.classList.add('is-active');
        trigger?.setAttribute('aria-expanded', 'true');
        open.add(ctl);
        reveal();
      },
      close() {
        if (box.hidden) return;
        const token = ++closing;
        trigger?.classList.remove('is-active');
        trigger?.setAttribute('aria-expanded', 'false');
        open.delete(ctl);
        closeMenus();
        box.classList.remove('is-opening');
        box.classList.add('is-closing');
        const finish = () => {
          if (token !== closing) return;
          closing++;
          box.hidden = true;
          box.classList.remove('is-closing');
        };
        box.addEventListener('animationend', finish, { once: true });
        setTimeout(finish, 300);
      },
      toggle() { if (isOpen()) ctl.close(); else ctl.open(); },
    };
    if (trigger) {
      trigger.addEventListener('mousedown', (e) => e.preventDefault());
      trigger.addEventListener('touchstart', (e) => { e.preventDefault(); ctl.toggle(); }, { passive: false });
      trigger.addEventListener('click', (e) => { e.preventDefault(); ctl.toggle(); });
    }
    // Cancel (the form's reset) closes it
    textarea.form?.addEventListener('reset', () => { hideAc(); ctl.close(); });
    textarea._f0ckEmojiPicker = ctl;
    return ctl;
  };

  window.f0ckEmojiPicker = { attach, load };
})();
