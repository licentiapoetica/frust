/*
 * w0bm frontend for f0ckm: a jQuery-free rewrite of w0bm.com's w0bmscript.js plus the bits of
 * Bootstrap 3 / video.js / bootstrap-tagsinput / timeago / clipboard.js it used, talking to the
 * f0ckm API.
 *
 * Page data comes from <script type="application/json"> blocks: #w0bm-data (every page) and
 * #w0bm-item-data (item page). Styles are never set from here, only classes, except CSS custom
 * properties for live numbers (playback progress, volume).
 *
 * Pages change without reloading, like in f0ckm (navigation at the end of this file): the next
 * page is fetched, the page part of <body> (everything before #w0bm-flash) is swapped and boot()
 * sets it up again. So everything in boot() lives as long as its page: listeners on document /
 * window go through on() (removed on leaving), intervals through every(), other clean-ups
 * through onLeave(). What stays across pages: the flash box, the shared scripts and their data
 * (snippets/frontend-scripts.html), the live socket.
 */
(() => {
  'use strict';

  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const readJSON = (id, doc = document) => {
    const el = doc.getElementById(id);
    if (!el) return null;
    try { return JSON.parse(el.textContent); } catch { return null; }
  };
  // Shared by every page: Cap loads once, the emoji list once
  let capLoading = null;
  let emojis = null; // name -> url

  // ── Page lifetime ──────────────────────────────────────────────────────────────────────
  let pageAC = new AbortController();
  let pageCleanups = [];
  const on = (target, type, fn, opts) => target.addEventListener(type, fn,
    Object.assign(typeof opts === 'boolean' ? { capture: opts } : { ...(opts || {}) }, { signal: pageAC.signal }));
  const every = (fn, ms) => { const id = setInterval(fn, ms); pageCleanups.push(() => clearInterval(id)); return id; };
  const onLeave = (fn) => { pageCleanups.push(fn); };
  function leavePage() {
    pageAC.abort();
    pageCleanups.splice(0).forEach((fn) => { try { fn(); } catch (err) { console.error('[w0bm] leave', err); } });
    pageAC = new AbortController();
  }

  // Fullscreen goes on this element, not on the player: it sits after the swapped part of <body>,
  // so it stays when → / ← / random change the page, and fullscreen with it. The player's media
  // box moves in while fullscreen (buildPlayer).
  let fsHost = null;
  const fullscreenHost = () => {
    if (!fsHost) {
      fsHost = document.createElement('div');
      fsHost.className = 'w0bm-fs-host';
      document.body.appendChild(fsHost);
    }
    return fsHost;
  };

  function boot() {
  const DATA = readJSON('w0bm-data') || {};
  // the footer data (window.f0ckSession) stays from the first page; the rating mode can change
  if (window.f0ckSession && DATA.mode !== undefined) window.f0ckSession.mode = DATA.mode;
  // Stand-ins for visitors without JavaScript (image captcha, plain forms): not needed now
  $$('.w0bm-nojs-only').forEach((el) => el.remove());
  const ITEM = readJSON('w0bm-item-data');
  const csrf = () => DATA.csrf || $('meta[name="csrf-token"]')?.content || '';
  const store = {
    get(k) { try { return localStorage.getItem(k); } catch { return null; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } },
  };
  const isMobile = matchMedia('(max-width: 767px)').matches || /Mobi|Android/i.test(navigator.userAgent);

  function escapeHTML(str) {
    return String(str ?? '').replace(/[&<>"'`=\/]/g, (s) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;', '/': '&#x2F;', '`': '&#x60;', '=': '&#x3D;',
    })[s]);
  }

  // ── API ────────────────────────────────────────────────────────────────────────────────
  // body: plain object / FormData / URLSearchParams (sent urlencoded) or {json: …}
  async function api(url, { method = 'POST', body, json } = {}) {
    const headers = { 'X-CSRF-Token': csrf(), 'X-Requested-With': 'XMLHttpRequest', Accept: 'application/json' };
    let payload;
    if (json !== undefined) {
      headers['Content-Type'] = 'application/json';
      payload = JSON.stringify(json);
    } else if (body !== undefined) {
      payload = body instanceof URLSearchParams ? body : new URLSearchParams(body instanceof FormData ? body : Object.entries(body));
    }
    if (method !== 'GET') prefetched.clear();
    let res, data = null;
    try {
      res = await fetch(url, { method, headers, body: payload, credentials: 'same-origin' });
    } catch {
      return { ok: false, status: 0, data: null, msg: 'Network error' };
    }
    try { data = await res.json(); } catch { /* not JSON */ }
    const ok = res.ok && !(data && data.success === false);
    // redirected/url: a successful POST /login answers with a redirect (fetch follows it)
    return { ok, status: res.status, data, redirected: res.redirected, url: res.url, msg: (data && (data.msg || data.message)) || (ok ? '' : 'Error ' + res.status) };
  }

  // ── Flash messages (partials/flash.blade.php) ─────────────────────────────────────────
  // `key`: a message with the same key is updated in place instead of stacking a new one
  function flash(type, message, key) {
    if (type === 'error') type = 'danger';
    const box = $('#sidebar .flashcontainer > .flash-inner') || $('#w0bm-flash > .flash-inner');
    if (!box) return;
    let div = key ? Array.from(box.children).find((c) => c.dataset.flashKey === key) : null;
    if (div) {
      clearTimeout(div._w0bmTimer);
      div.className = 'alert alert-' + type + ' alert-dismissable';
      div.lastChild.textContent = message;
    } else {
      div = document.createElement('div');
      div.className = 'alert alert-' + type + ' alert-dismissable';
      div.setAttribute('role', 'alert');
      if (key) div.dataset.flashKey = key;
      div.innerHTML = '<button type="button" class="close" data-dismiss="alert" aria-label="Close"><span aria-hidden="true">&times;</span></button>';
      div.append(document.createTextNode(message));
      box.appendChild(div);
    }
    const n = box.children.length;
    div._w0bmTimer = setTimeout(() => {
      div.classList.add('w0bm-alert-out');
      setTimeout(() => div.remove(), 300);
    }, 3000 + n * 1000);
  }
  // Server-rendered alerts fade out like alertrm()
  $$('.flash-inner > .alert').filter((a) => !a._w0bmTimer && !a.dataset.w0bmFade).forEach((a, i) => (a.dataset.w0bmFade = '1') && setTimeout(() => {
    a.classList.add('w0bm-alert-out');
    setTimeout(() => a.remove(), 300);
  }, 3000 + i * 1000));

  // ── Modals (Bootstrap 3 modal, :target without JS) ────────────────────────────────────
  function openModal(m) {
    if (!m) return;
    $$('.modal.w0bm-modal-open').forEach((o) => o !== m && closeModal(o));
    m.classList.add('w0bm-modal-open', 'in');
    document.body.classList.add('modal-open');
    const t = m.querySelector('.w0bm-random-title');
    if (t) {
      try {
        const titles = JSON.parse(t.dataset.titles);
        t.textContent = titles[Math.floor(Math.random() * titles.length)];
      } catch { /* keep */ }
    }
    m.dispatchEvent(new CustomEvent('w0bm:open'));
    const input = m.querySelector('input[type="text"]:not([type="hidden"]), textarea');
    if (input) setTimeout(() => input.focus(), 50);
  }
  function closeModal(m) {
    if (!m) return;
    const wasOpen = m.classList.contains('w0bm-modal-open');
    m.classList.remove('w0bm-modal-open', 'in');
    if (wasOpen) m.dispatchEvent(new CustomEvent('w0bm:close'));
    if (!$('.modal.w0bm-modal-open')) document.body.classList.remove('modal-open');
    if (location.hash === '#' + m.id) history.replaceState(null, '', location.pathname + location.search);
    // /?login=1 opened the login modal: drop the parameter once it is closed
    // /?login=1 or a no-JS form answer (?login_error=…) opened it: drop those parameters
    if (/[?&](login|login_error|login_done|login_captcha|register_error|register_done)=/.test(location.search)) {
      const u = new URL(location.href);
      ['login', 'login_error', 'login_done', 'login_captcha', 'register_error', 'register_done'].forEach((k) => u.searchParams.delete(k));
      history.replaceState(null, '', u.pathname + u.search + u.hash);
    }
  }
  // Confirmations and short questions in a w0bm modal (template #w0bm-dialog-tpl in body-end.html)
  // instead of the browser's confirm()/prompt(). Resolves to true (confirmed), the typed text
  // (with `input`) or null (cancelled).
  function dialog({ title, text = '', input = null, ok = 'OK', danger = true } = {}) {
    const tpl = $('#w0bm-dialog-tpl');
    if (!tpl) return Promise.resolve(input ? window.prompt(text || title, '') : (window.confirm(text || title) || null));
    const m = tpl.content.firstElementChild.cloneNode(true);
    m.querySelector('.modal-title').textContent = title || '';
    const p = m.querySelector('.w0bm-dialog-text');
    p.textContent = text;
    p.hidden = !text;
    const inp = m.querySelector('.w0bm-dialog-input');
    const lab = m.querySelector('.w0bm-dialog-label');
    if (input) {
      inp.hidden = false;
      inp.placeholder = input.placeholder || '';
      inp.required = !!input.required;
      if (input.label) { lab.textContent = input.label; lab.hidden = false; }
    }
    const okBtn = m.querySelector('.w0bm-dialog-ok');
    okBtn.textContent = ok;
    okBtn.classList.toggle('btn-danger', danger);
    okBtn.classList.toggle('btn-primary', !danger);
    document.body.appendChild(m);
    onLeave(() => closeModal(m));
    return new Promise((resolve) => {
      let result = null;
      m.querySelector('form').addEventListener('submit', (e) => {
        e.preventDefault();
        result = input ? inp.value.trim() : true;
        closeModal(m);
      });
      m.addEventListener('w0bm:close', () => { m.remove(); resolve(result); }, { once: true });
      openModal(m);
      setTimeout(() => (input ? inp : okBtn).focus(), 60);
    });
  }

  // /register redirects to /#register, /#register:<token> or /#register:error|success:<message>
  const regHash = decodeURIComponent(location.hash.slice(1));
  if (/^register(-modal)?(:|$)/.test(regHash) && $('#register-modal')) {
    const m = $('#register-modal');
    const parts = regHash.split(':');
    const box = m.querySelector('.w0bm-register-msg');
    if ((parts[1] === 'error' || parts[1] === 'success') && box) {
      box.textContent = parts.slice(2).join(':');
      box.className = 'w0bm-register-msg alert alert-' + (parts[1] === 'error' ? 'danger' : 'success');
      box.hidden = false;
    } else if (parts[1]) {
      const tok = m.querySelector('input[name="token"]');
      if (tok) tok.value = parts.slice(1).join(':');
    }
    history.replaceState(null, '', location.pathname + location.search);
    openModal(m);
  } else if (location.hash && location.hash.length > 1) {
    const m = document.getElementById(location.hash.slice(1));
    if (m && m.classList.contains('modal')) {
      history.replaceState(null, '', location.pathname + location.search);
      openModal(m);
    }
  }

  // ── Dropdowns, alerts, collapse, modal triggers (one delegated click handler) ─────────
  on(document, 'click', (e) => {
    const t = e.target;
    const modalTrigger = t.closest('[data-toggle="modal"]');
    if (modalTrigger) {
      e.preventDefault();
      const sel = modalTrigger.dataset.target || modalTrigger.getAttribute('href');
      closeDropdowns();
      openModal(sel ? $(sel) : null);
      return;
    }
    const dismissModal = t.closest('[data-dismiss="modal"], .w0bm-modal-shade');
    if (dismissModal) {
      e.preventDefault();
      closeModal(dismissModal.closest('.modal'));
      return;
    }
    const dismissAlert = t.closest('[data-dismiss="alert"]');
    if (dismissAlert) {
      e.preventDefault();
      dismissAlert.closest('.alert')?.remove();
      return;
    }
    const dd = t.closest('[data-toggle="dropdown"]');
    if (dd) {
      e.preventDefault();
      const li = dd.parentElement;
      const wasOpen = li.classList.contains('open');
      closeDropdowns();
      if (!wasOpen) {
        li.classList.add('open');
        dd.setAttribute('aria-expanded', 'true');
      }
      return;
    }
    if (!t.closest('.dropdown-menu')) closeDropdowns();
  });
  function closeDropdowns() {
    $$('.w0bm-dropdown.open').forEach((li) => {
      li.classList.remove('open');
      li.querySelector('[data-toggle="dropdown"]')?.setAttribute('aria-expanded', 'false');
    });
  }
  on(document, 'keydown', (e) => {
    if (e.key === 'Escape') {
      $$('.modal.w0bm-modal-open').forEach(closeModal);
      closeDropdowns();
      $$('.w0bm-popover-host > .popover').forEach((p) => p.remove());
    }
  });

  // Theme switcher (themechanger.blade.php)
  $$('#switcher button[data-style]').forEach((b) => b.addEventListener('click', async (e) => {
    e.preventDefault();
    if (b.dataset.confirm && !(await dialog({ title: b.dataset.style, text: b.dataset.confirm, ok: 'Use it' }))) return;
    window.w0bmStyle?.set(b.dataset.style);
  }));
  // "t" cycles the themes, like f0ckm. Themes that ask for confirmation (Epilepsie: flashing)
  // are left out: they stay an explicit choice in the menu.
  on(document, 'keydown', (e) => {
    if (e.key !== 't' || e.defaultPrevented || e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    if (/^(input|textarea|select)$/i.test(e.target.nodeName) || e.target.isContentEditable) return;
    if (!window.w0bmStyle) return;
    const themes = $$('#switcher button[data-style]').filter((b) => !b.dataset.confirm).map((b) => b.dataset.style);
    if (!themes.length) return;
    const current = document.documentElement.dataset.w0bmTheme || 'w0bmcustom';
    const next = themes[(themes.indexOf(current) + 1) % themes.length];
    window.w0bmStyle.set(next);
  });

  // ── Tooltips & popovers (Bootstrap 3 look, positioned by CSS) ─────────────────────────
  function initTooltip(el) {
    if (el.dataset.w0bmTip) return;
    el.dataset.w0bmTip = '1';
    const title = el.getAttribute('title');
    if (!title) return;
    el.dataset.originalTitle = title;
    el.removeAttribute('title');
    el.classList.add('w0bm-tip-host');
    el.addEventListener('mouseenter', () => {
      if (el.querySelector(':scope > .tooltip')) return;
      const tip = document.createElement('div');
      tip.className = 'tooltip top in';
      tip.setAttribute('role', 'tooltip');
      tip.innerHTML = '<div class="tooltip-arrow"></div><div class="tooltip-inner"></div>';
      tip.lastChild.textContent = el.dataset.originalTitle;
      el.appendChild(tip);
    });
    el.addEventListener('mouseleave', () => el.querySelector(':scope > .tooltip')?.remove());
  }
  const initTooltips = (root = document) => $$('[data-toggle="tooltip"]', root).forEach(initTooltip);

  function initPopover(btn) {
    const tpl = $(btn.dataset.popover);
    if (!tpl) return;
    const title = btn.getAttribute('title') || '';
    btn.removeAttribute('title');
    const host = document.createElement('span');
    host.className = 'w0bm-popover-host';
    btn.parentNode.insertBefore(host, btn);
    host.appendChild(btn);
    let hideTimer;
    const show = () => {
      clearTimeout(hideTimer);
      if (host.querySelector(':scope > .popover')) return;
      const pop = document.createElement('div');
      pop.className = 'popover top in';
      pop.setAttribute('role', 'tooltip');
      pop.innerHTML = '<div class="arrow"></div><h3 class="popover-title"></h3><div class="popover-content"></div>';
      pop.querySelector('.popover-title').textContent = title;
      pop.querySelector('.popover-content').appendChild(tpl.content.cloneNode(true));
      host.appendChild(pop);
    };
    const hide = () => {
      clearTimeout(hideTimer);
      hideTimer = setTimeout(() => host.querySelector(':scope > .popover')?.remove(), 100);
    };
    host.addEventListener('mouseenter', show);
    host.addEventListener('mouseleave', hide);
    btn.addEventListener('click', (e) => {
      // touch: tap toggles; the download button still downloads on desktop
      if (btn.id === 'dlbutton' && !isMobile) return;
      e.preventDefault();
      if (host.querySelector(':scope > .popover')) host.querySelector(':scope > .popover').remove();
      else show();
    });
  }
  $$('[data-toggle="popover"][data-popover]').forEach(initPopover);

  // ── timeago (jquery.timeago with w0bm's Laravel-like strings) ─────────────────────────
  function timeago(date) {
    const d = date instanceof Date ? date : new Date(date);
    if (isNaN(d)) return '';
    let distance = Date.now() - d.getTime();
    const future = distance < 0;
    distance = Math.abs(distance);
    const seconds = distance / 1000, minutes = seconds / 60, hours = minutes / 60, days = hours / 24, years = days / 365;
    const r = (s, n) => s.replace(/%d/i, Math.round(n));
    const words = seconds < 45 && r('%d seconds', seconds) ||
      seconds < 90 && '1 minute' ||
      minutes < 45 && r('%d minutes', minutes) ||
      minutes < 90 && '1 hour' ||
      hours < 24 && r('%d hours', hours) ||
      hours < 42 && '1 day' ||
      days < 30 && r('%d days', days) ||
      days < 45 && '1 month' ||
      days < 365 && r('%d months', days / 30) ||
      years < 1.5 && '1 year' ||
      r('%d years', years);
    return words + (future ? ' from now' : ' ago');
  }
  function refreshTimeago(root = document) {
    $$('time.timeago[datetime]', root).forEach((t) => {
      const d = new Date(t.getAttribute('datetime'));
      if (isNaN(d)) return;
      if (!t.dataset.w0bmTitled) {
        t.dataset.w0bmTitled = '1';
        if (t.dataset.originalTitle !== undefined) t.dataset.originalTitle = d.toLocaleString();
        else t.setAttribute('title', d.toLocaleString());
      }
      // only the text node: an open tooltip lives inside the element and must survive the refresh
      const s = timeago(d);
      let label = t.firstChild;
      if (!label || label.nodeType !== Node.TEXT_NODE) {
        label = document.createTextNode('');
        t.prepend(label);
      }
      if (label.nodeValue !== s) label.nodeValue = s;
    });
  }
  refreshTimeago();
  initTooltips();
  every(refreshTimeago, 1000);

  // Missing thumbnails get w0bm's 404 thumb (main.blade.php)
  const thumbFallback = (img) => {
    if (img.tagName !== 'IMG' || img.dataset.w0bmFallback) return;
    if (!img.closest('.main-item, .w0bm-rowthumb, .w0bm-thumbs')) return;
    img.dataset.w0bmFallback = '1';
    img.classList.add('nothumb');
    img.src = '/s/fe/w0bm/img/small_404.gif';
  };
  on(document, 'error', (e) => thumbFallback(e.target), true);

  // ── Endless grid (index-main.html: front page, tags, users, halls) ─────────────────────
  // Near the end of the grid the next page is fetched and its thumbnails appended (skipping ones
  // already shown, e.g. live uploads); the page links and the address follow. Without JavaScript
  // the page links stay as they are.
  const endlessGrid = $('.w0bm-index-page .items');
  const pagerNext = (root) => root.querySelector('.page-header .pagination a[rel="next"]');
  if (endlessGrid && pagerNext(document) && 'IntersectionObserver' in window) {
    let next = pagerNext(document).href;
    let loading = false;
    const sentinel = document.createElement('div');
    sentinel.className = 'w0bm-more';
    endlessGrid.after(sentinel);
    const io = new IntersectionObserver(async (entries) => {
      if (!entries.some((en) => en.isIntersecting) || loading || !next) return;
      loading = true;
      sentinel.classList.add('w0bm-more-loading');
      const url = next;
      try {
        const res = await fetch(url, { credentials: 'same-origin' });
        if (!res.ok) throw new Error(res.status);
        const doc = new DOMParser().parseFromString(await res.text(), 'text/html');
        const seen = new Set($$('.main-item > a', endlessGrid).map((x) => x.getAttribute('href')));
        doc.querySelectorAll('.items > .main-item').forEach((item) => {
          const href = item.querySelector('a')?.getAttribute('href');
          if (href && seen.has(href)) return;
          endlessGrid.appendChild(document.importNode(item, true));
        });
        const pager = doc.querySelector('.page-header .pagination');
        const curPager = $('.page-header .pagination');
        if (pager && curPager) curPager.replaceWith(document.importNode(pager, true));
        next = pagerNext(doc)?.href || null;
        history.replaceState(history.state, '', url);
        shownPath = pathKey(location.href);
      } catch (err) {
        next = null;
      }
      loading = false;
      sentinel.classList.toggle('w0bm-more-loading', false);
      if (!next) {
        io.disconnect();
        sentinel.hidden = true;
      }
    }, { rootMargin: '0px 0px 600px 0px' });
    io.observe(sentinel);
    onLeave(() => io.disconnect());
  }
  $$('.main-item img, .w0bm-thumbs img').forEach((img) => { if (img.complete && !img.naturalWidth) thumbFallback(img); });
  on(document, 'error', (e) => {
    const img = e.target;
    if (img.tagName === 'IMG' && img.classList.contains('w0bm-avatar')) img.remove();
  }, true);

  // ── Copy link (clipboard.js) ──────────────────────────────────────────────────────────
  $$('.copylink[data-clipboard-text]').forEach((b) => b.addEventListener('click', async (e) => {
    e.preventDefault();
    b.focus();
    const text = b.dataset.clipboardText;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.className = 'sr-only';
      document.body.appendChild(ta);
      ta.select();
      try { document.execCommand('copy'); } catch { /* nothing left to try */ }
      ta.remove();
      b.focus();
    }
  }));

  // ── bootstrap-tagsinput ───────────────────────────────────────────────────────────────
  // Replaces <input data-role="tagsinput"> with the plugin's markup; the original input keeps
  // the comma-joined value. el.w0bmTags = { items(), add(), remove(), removeAll() }.
  function tagsinput(input, { beforeAdd } = {}) {
    if (input.w0bmTags) return input.w0bmTags;
    const items = [];
    const box = document.createElement('div');
    box.className = 'bootstrap-tagsinput';
    const typer = document.createElement('input');
    typer.type = 'text';
    typer.placeholder = input.placeholder || '';
    typer.setAttribute('autocomplete', 'off');
    typer.size = Math.max(typer.placeholder.length, 1);
    box.appendChild(typer);
    input.parentNode.insertBefore(box, input);
    input.type = 'hidden';
    const sync = () => {
      input.value = items.join(',');
      typer.placeholder = items.length ? '' : (input.getAttribute('placeholder') || '');
    };
    const api_ = {
      items: () => items.slice(),
      add(tag) {
        tag = String(tag).trim();
        if (!tag) return;
        if (items.some((t) => t.toLowerCase() === tag.toLowerCase())) return;
        if (beforeAdd && beforeAdd(tag) === false) return;
        items.push(tag);
        const span = document.createElement('span');
        span.className = 'tag label label-info';
        span.textContent = tag;
        const rm = document.createElement('span');
        rm.dataset.role = 'remove';
        rm.addEventListener('click', () => api_.remove(tag));
        span.appendChild(rm);
        box.insertBefore(span, typer);
        sync();
        input.dispatchEvent(new CustomEvent('itemAdded', { detail: tag }));
      },
      remove(tag) {
        const i = items.findIndex((t) => t === tag);
        if (i < 0) return;
        items.splice(i, 1);
        box.querySelectorAll('.tag')[i]?.remove();
        sync();
        input.dispatchEvent(new CustomEvent('itemRemoved', { detail: tag }));
      },
      removeAll() { items.slice().forEach((t) => api_.remove(t)); },
      flush() {
        if (typer.value.trim()) {
          typer.value.split(',').forEach((t) => api_.add(t));
          typer.value = '';
        }
      },
      input: typer,
    };
    // Suggestions while typing (GET /api/v2/tags/suggest, like f0ckm's tag autocomplete)
    const sug = document.createElement('ul');
    sug.className = 'dropdown-menu w0bm-tag-suggest';
    sug.setAttribute('role', 'listbox');
    box.appendChild(sug);
    let sugTimer = null;
    let sugSeq = 0;
    let sugIndex = -1;
    const sugItems = () => Array.from(sug.children);
    const closeSug = () => { sug.classList.remove('w0bm-open'); sug.textContent = ''; sugIndex = -1; };
    const markSug = (i) => {
      const list = sugItems();
      sugIndex = list.length ? (i + list.length) % list.length : -1;
      list.forEach((li, n) => li.classList.toggle('active', n === sugIndex));
    };
    const pickSug = (tag) => { api_.add(tag); typer.value = ''; closeSug(); typer.focus(); };
    typer.addEventListener('input', () => {
      clearTimeout(sugTimer);
      const q = typer.value.trim();
      if (!q || q.includes(',')) { closeSug(); return; }
      sugTimer = setTimeout(async () => {
        const seq = ++sugSeq;
        const r = await api('/api/v2/tags/suggest?q=' + encodeURIComponent(q), { method: 'GET' });
        if (seq !== sugSeq || typer.value.trim() !== q) return; // a newer query is on its way
        const have = items.map((t) => t.toLowerCase());
        const list = (r.data?.suggestions || []).map((s) => s.tag).filter((t) => t && !have.includes(String(t).toLowerCase())).slice(0, 8);
        sug.textContent = '';
        list.forEach((tag) => {
          const li = document.createElement('li');
          li.setAttribute('role', 'option');
          const a = document.createElement('a');
          a.href = '#';
          a.textContent = tag;
          // mousedown, so the field doesn't lose focus (and flush half a tag) before the pick
          a.addEventListener('mousedown', (ev) => { ev.preventDefault(); pickSug(tag); });
          li.appendChild(a);
          sug.appendChild(li);
        });
        sugIndex = -1;
        sug.classList.toggle('w0bm-open', list.length > 0);
      }, 150);
    });
    typer.addEventListener('keydown', (e) => {
      if (sug.classList.contains('w0bm-open')) {
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault();
          markSug(sugIndex + (e.key === 'ArrowDown' ? 1 : -1));
          return;
        }
        if ((e.key === 'Enter' || e.key === 'Tab') && sugIndex >= 0) {
          e.preventDefault();
          pickSug(sugItems()[sugIndex].textContent);
          return;
        }
        if (e.key === 'Escape') {
          e.stopPropagation();
          closeSug();
          return;
        }
      }
      if (e.key === 'Enter' || e.key === ',') {
        closeSug();
        if (e.key === 'Enter' && !typer.value.trim()) return; // submit the form
        e.preventDefault();
        api_.flush();
      } else if (e.key === 'Backspace' && !typer.value && items.length) {
        api_.remove(items[items.length - 1]);
      }
    });
    typer.addEventListener('blur', () => { closeSug(); api_.flush(); });
    box.addEventListener('click', () => typer.focus());
    (input.defaultValue || '').split(',').forEach((t) => api_.add(t));
    input.w0bmTags = api_;
    return api_;
  }

  // ── Search forms: members use /search?tag=, guests the tag page ───────────────────────
  $$('form.w0bm-searchform').forEach((f) => f.addEventListener('submit', (e) => {
    const q = (f.querySelector('input[name="tag"]')?.value || '').trim();
    if (!q) { e.preventDefault(); return; }
    if (f.dataset.guest) {
      e.preventDefault();
      navigate('/tag/' + encodeURIComponent(q.split(',')[0].trim()));
    }
  }));

  // ── Captcha (Cap) on demand ───────────────────────────────────────────────────────────
  function loadCap() {
    if (capLoading) return capLoading;
    const add = (src) => new Promise((res) => {
      const s = document.createElement('script');
      s.src = src;
      s.onload = res;
      s.onerror = res;
      document.head.appendChild(s);
    });
    capLoading = add('/s/cap/cap-config.js').then(() => add('/s/cap/cap.min.js'));
    return capLoading;
  }
  // Widgets in the page load Cap right away, those in modals when the modal opens
  if ($$('cap-widget').some((w) => !w.closest('.modal'))) loadCap();
  on(document, 'w0bm:open', (e) => { if (e.target.querySelector('cap-widget')) loadCap(); }, true);
  // a modal already open on load (from the URL, e.g. #register-modal after a failed form post, or
  // rendered open by the server) opened before this listener existed
  if ($$('.modal.w0bm-modal-open cap-widget, .modal:target cap-widget').length) loadCap();
  function showCap(form) {
    const tpl = form.querySelector('template.w0bm-cap-template');
    if (tpl && !form.querySelector('cap-widget')) {
      tpl.parentNode.insertBefore(tpl.content.cloneNode(true), tpl);
      loadCap();
    }
  }

  // ── Login (modal + page) ──────────────────────────────────────────────────────────────
  $$('form.w0bm-loginform').forEach((f) => f.addEventListener('submit', async (e) => {
    e.preventDefault();
    const err = f.querySelector('.w0bm-login-error');
    const btn = f.querySelector('button[type="submit"]');
    btn.disabled = true;
    const fd = new FormData(f);
    if (!f.querySelector('input[name="kmsi"]')?.checked) fd.delete('kmsi');
    const r = await api('/login', { body: fd });
    btn.disabled = false;
    // Logged in: the server redirects (with the session cookie) to the page we came from, like
    // f0ckm.js handles it. Without the login parameters, so the modal doesn't open again.
    if (r.redirected && r.url) {
      const u = new URL(r.url, location.href);
      ['login', 'login_error', 'login_done', 'login_captcha'].forEach((k) => u.searchParams.delete(k));
      location.href = u.pathname + u.search;
      return;
    }
    if (r.data && r.data.success) {
      const u = new URL(location.href);
      u.searchParams.delete('login');
      location.href = r.data.redirect && r.data.redirect !== '/' ? r.data.redirect : (u.pathname + u.search);
      return;
    }
    if (r.data && r.data.redirect) { location.href = r.data.redirect; return; }
    if (r.data && r.data.captcha) showCap(f);
    if (err) {
      err.textContent = r.msg || 'Login failed';
      err.hidden = false;
    } else {
      flash('error', r.msg || 'Login failed');
    }
  }));

  // ── Navbar badges (unread notifications / DMs) ────────────────────────────────────────
  function setBadge(el, n) {
    if (!el) return;
    el.textContent = n > 99 ? '99+' : String(n);
    el.classList.toggle('w0bm-has-count', n > 0);
    el.hidden = n <= 0;
  }
  // Notifications page: the count on each tab (the server renders the first one, see
  // routes/notifications.rs unread_by_tab; the type lists are its USER/SYSTEM/ADMIN_TYPES)
  const NOTIF_TAB = {};
  ['comment_reply', 'subscription', 'mention', 'upload_comment'].forEach((t) => { NOTIF_TAB[t] = 'user'; });
  ['approve', 'deny', 'item_deleted', 'upload_success', 'upload_error', 'admin_pending', 'report', 'warning', 'invite_approved', 'invite_denied'].forEach((t) => { NOTIF_TAB[t] = 'system'; });
  NOTIF_TAB.invite_request = 'admin';
  const tabCount = (tab) => {
    const li = $('.w0bm-tabs > li[data-tab="' + tab + '"]');
    return li ? +(li.querySelector('.w0bm-tab-count')?.textContent || 0) || 0 : 0;
  };
  function setTabCount(tab, n) {
    const li = $('.w0bm-tabs > li[data-tab="' + tab + '"]');
    if (!li) return;
    n = Math.max(0, n);
    li.classList.toggle('w0bm-tab-unread', n > 0);
    const b = li.querySelector('.w0bm-tab-count');
    if (b) {
      b.textContent = n > 99 ? '99+' : String(n);
      b.hidden = n <= 0;
    }
  }
  let pollBadges = () => {};
  if (DATA.user) {
    const poll = async () => {
      const n = await api('/api/notifications', { method: 'GET' });
      if (n.ok && Array.isArray(n.data?.notifications)) {
        const unread = n.data.notifications.filter((x) => !x.is_read);
        setBadge($('#w0bm-notif-badge'), unread.length);
        if ($('.w0bm-tabs > li[data-tab]')) {
          const per = { user: 0, system: 0, admin: 0 };
          unread.forEach((x) => { if (NOTIF_TAB[x.type]) per[NOTIF_TAB[x.type]]++; });
          Object.keys(per).forEach((t) => setTabCount(t, per[t]));
        }
      }
      // #dm-badge: messages.js keeps it current too (it sets its display)
      if ($('#dm-badge')) {
        const d = await api('/api/dm/unread', { method: 'GET' });
        if (d.ok) {
          const b = $('#dm-badge');
          b.style.removeProperty('display');
          setBadge(b, +d.data?.count || 0);
          setTabCount('dm', +d.data?.count || 0);
        }
      }
    };
    pollBadges = poll;
    poll();
    every(() => { if (!document.hidden) poll(); }, 60000);
  }

  // ── Filter modal (filterselect.blade.php): excluded tags ──────────────────────────────
  const filterInput = $('#filter');
  if (filterInput) {
    const tags = tagsinput(filterInput);
    let initial = [];
    const load = async () => {
      const r = await api('/api/v2/settings/excluded_tags', { method: 'GET' });
      if (!r.ok) return;
      tags.removeAll();
      initial = (r.data.tags || []).map((t) => ({ name: t.tag, key: t.normalized || t.tag, id: t.id }));
      initial.forEach((t) => tags.add(t.name));
    };
    load();
    $('#submitfilter')?.addEventListener('click', async (e) => {
      e.preventDefault();
      tags.flush();
      const now = tags.items();
      const lower = now.map((t) => t.toLowerCase());
      const removed = initial.filter((t) => !lower.includes(t.name.toLowerCase()));
      const added = now.filter((t) => !initial.some((i) => i.name.toLowerCase() === t.toLowerCase()));
      let failed = [];
      for (const t of removed) {
        const r = await api('/api/v2/settings/excluded_tags/' + encodeURIComponent(t.key), { method: 'DELETE' });
        if (!r.ok) failed.push(t.name + ': ' + r.msg);
      }
      for (const t of added) {
        const r = await api('/api/v2/settings/excluded_tags', { body: { tagname: t } });
        if (!r.ok) failed.push(t + ': ' + r.msg);
      }
      if (failed.length) {
        flash('error', 'Error updating filter (' + failed.join(', ') + ')');
        load();
      } else {
        flash('success', 'Filter successfully updated');
        closeModal($('#filterselectmodal'));
        if (removed.length || added.length) setTimeout(() => reloadPage(), 600);
      }
    });
  }

  // ── Media filter (filter dialog, partials/filterselect.html) ─────────────────────────────
  // The same "mime" cookie as f0ckm's mime picker; a change applies at once (page reload). Without
  // JavaScript the form's "Apply" goes through /mime.
  const mimeForm = $('#w0bm-mime-form');
  // Ticked = shown. All ticked is no filter (cookie cleared); the last ticked type stays ticked.
  mimeForm?.addEventListener('change', (e) => {
    const boxes = $$('input[type="checkbox"]:not(:disabled)', mimeForm);
    const picked = boxes.filter((c) => c.checked).map((c) => c.name);
    if (!picked.length) { e.target.checked = true; return; }
    document.cookie = picked.length < boxes.length
      ? 'mime=' + picked.join(',') + '; path=/; max-age=31536000; SameSite=Lax'
      : 'mime=; path=/; max-age=0; SameSite=Lax';
    closeModal($('#filterselectmodal'));
    reloadPage();
  });

  // ── Generic JSON forms (report …) ─────────────────────────────────────────────────────
  // Report a comment: the item's report modal (partials/report.html, f0ckm's reasons and the
  // captcha), sending comment_id instead of item_id while it is open for the comment
  function reportComment(id) {
    const form = $('#webmreport');
    const modal = $('#webmreportmodal');
    if (!form || !modal) return;
    const item = form.querySelector('input[name="item_id"]');
    const comment = form.querySelector('input[name="comment_id"]');
    item.disabled = true;
    comment.disabled = false;
    comment.value = id;
    modal.addEventListener('w0bm:close', () => {
      item.disabled = false;
      comment.disabled = true;
      comment.value = '';
    }, { once: true });
    openModal(modal);
  }

  $$('form.w0bm-ajaxform').forEach((f) => f.addEventListener('submit', async (e) => {
    e.preventDefault();
    const r = await api(f.action, { body: new FormData(f) });
    if (r.ok) {
      flash('success', f.dataset.success || 'Done');
      f.reset();
      closeModal(f.querySelector('.modal'));
    } else {
      flash('error', r.msg || 'Error');
    }
  }));

  // ════════════════════════════════════════════════════════════════════════════════════
  // Item page
  // ════════════════════════════════════════════════════════════════════════════════════
  const media = $('#video');
  let player = null;

  const fmtTime = (s) => {
    if (!isFinite(s) || s < 0) s = 0;
    s = Math.floor(s);
    const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
    return (h ? h + ':' + String(m).padStart(2, '0') : m) + ':' + String(sec).padStart(2, '0');
  };

  // video.js 5 skin for a native <video>/<audio> (same DOM, so video-js.min.css + vjsnew.css apply)
  function buildPlayer(el) {
    const isAudio = el.tagName === 'AUDIO';
    const wrap = document.createElement('div');
    wrap.className = 'video-js vjs-default-skin vjs-paused vjs-controls-enabled vjs-workinghover vjs-user-active' + (isAudio ? ' w0bm-vjs-audio vjs-has-started' : '');
    el.parentNode.insertBefore(wrap, el);
    wrap.appendChild(el);
    el.removeAttribute('id');
    wrap.id = 'video';
    el.id = 'video_html5_api';
    el.controls = false;
    el.classList.add('vjs-tech');
    const rates = [2, 1.5, 1, 0.5, 0.25];
    wrap.insertAdjacentHTML('beforeend',
      '<button class="vjs-big-play-button" type="button" aria-live="polite" title="Play Video"><span class="vjs-control-text">Play Video</span></button>' +
      '<div class="vjs-control-bar">' +
        '<div class="vjs-progress-control vjs-control"><div tabindex="0" class="vjs-progress-holder vjs-slider vjs-slider-horizontal" role="slider" aria-label="progress bar"><div class="vjs-load-progress"></div><div class="vjs-play-progress vjs-slider-bar" data-current-time="0:00"><span class="vjs-control-text"><span>Progress</span>: 0%</span></div></div></div>' +
        '<button class="vjs-play-control vjs-control vjs-button vjs-paused" type="button" title="Play"><span class="vjs-control-text">Play</span></button>' +
        '<button class="vjs-mute-control vjs-control vjs-button vjs-vol-3" type="button" title="Mute"><span class="vjs-control-text">Mute</span></button>' +
        '<div class="vjs-volume-control vjs-control"><div tabindex="0" class="vjs-volume-bar vjs-slider-bar vjs-slider vjs-slider-horizontal" role="slider" aria-label="volume level"><div class="vjs-volume-level"><span class="vjs-control-text"></span></div></div></div>' +
        '<div class="vjs-current-time vjs-time-control vjs-control"><div class="vjs-current-time-display" aria-live="off"><span class="vjs-control-text">Current Time </span><span class="w0bm-t">0:00</span></div></div>' +
        '<div class="vjs-time-control vjs-time-divider"><div><span>/</span></div></div>' +
        '<div class="vjs-duration vjs-time-control vjs-control"><div class="vjs-duration-display" aria-live="off"><span class="vjs-control-text">Duration Time </span><span class="w0bm-t">0:00</span></div></div>' +
        '<div class="vjs-custom-control-spacer vjs-spacer">&nbsp;</div>' +
        '<div class="vjs-playback-rate vjs-menu-button vjs-menu-button-popup vjs-control vjs-button" tabindex="0" role="menuitem" aria-live="polite" title="Playback Rate"><div class="vjs-menu" role="presentation"><ul class="vjs-menu-content" role="menu">' +
          rates.map((r) => '<li class="vjs-menu-item' + (r === 1 ? ' vjs-selected' : '') + '" tabindex="-1" role="menuitemcheckbox" data-rate="' + r + '">' + r + 'x</li>').join('') +
        '</ul></div><div class="vjs-playback-rate-value">1x</div></div>' +
        (isAudio ? '' : '<button class="vjs-fullscreen-control vjs-control vjs-button" type="button" title="Fullscreen"><span class="vjs-control-text">Fullscreen</span></button>') +
      '</div>');

    const q = (s) => wrap.querySelector(s);
    const playBtn = q('.vjs-play-control'), muteBtn = q('.vjs-mute-control');
    const holder = q('.vjs-progress-holder'), volBar = q('.vjs-volume-bar');
    const cur = q('.vjs-current-time-display .w0bm-t'), dur = q('.vjs-duration-display .w0bm-t');
    const playProg = q('.vjs-play-progress');
    const rateBtn = q('.vjs-playback-rate'), rateVal = q('.vjs-playback-rate-value');
    const box = wrap.parentElement;
    const setVar = (k, v) => box.style.setProperty(k, String(v));

    const p = {
      el, wrap,
      paused: () => el.paused,
      play: () => el.play().catch(() => {}),
      pause: () => el.pause(),
      toggle: () => (el.paused ? p.play() : p.pause()),
      volume(v) {
        if (v === undefined) return el.volume;
        el.volume = Math.min(1, Math.max(0, v));
        if (el.volume > 0) el.muted = false;
      },
      duration: () => el.duration,
      currentTime(t) {
        if (t === undefined) return el.currentTime;
        el.currentTime = t;
      },
    };

    const updState = () => {
      const paused = el.paused;
      wrap.classList.toggle('vjs-paused', paused);
      wrap.classList.toggle('vjs-playing', !paused);
      playBtn.classList.toggle('vjs-paused', paused);
      playBtn.classList.toggle('vjs-playing', !paused);
      playBtn.title = paused ? 'Play' : 'Pause';
      playBtn.firstChild.textContent = playBtn.title;
      if (!paused) wrap.classList.add('vjs-has-started');
      wrap.classList.toggle('vjs-ended', el.ended);
    };
    const updTime = () => {
      const d = el.duration;
      const frac = d > 0 && isFinite(d) ? el.currentTime / d : 0;
      setVar('--progress', frac.toFixed(5));
      cur.textContent = fmtTime(el.currentTime);
      playProg.dataset.currentTime = fmtTime(el.currentTime);
      if (el.buffered.length && d > 0 && isFinite(d)) setVar('--buffered', (el.buffered.end(el.buffered.length - 1) / d).toFixed(4));
    };
    const updVol = () => {
      const v = el.muted ? 0 : el.volume;
      setVar('--volume', v.toFixed(3));
      const lvl = v === 0 ? 0 : v < 0.33 ? 1 : v < 0.67 ? 2 : 3;
      muteBtn.className = 'vjs-mute-control vjs-control vjs-button vjs-vol-' + lvl;
      muteBtn.title = el.muted ? 'Unmute' : 'Mute';
      // 1/x keeps the sign of -0: muted with volume 0 (w0bmscript.js)
      store.set('volume', String(el.volume * (el.muted ? -1 : 1)));
    };

    // saved volume (default 0.3)
    const saved = parseFloat(store.get('volume') ?? '0.3');
    el.volume = Math.abs(isNaN(saved) ? 0.3 : saved);
    el.muted = 0 > 1 / (isNaN(saved) ? 0.3 : saved);

    ['play', 'pause', 'ended'].forEach((ev) => el.addEventListener(ev, updState));
    el.addEventListener('timeupdate', updTime);
    el.addEventListener('progress', updTime);
    el.addEventListener('durationchange', () => { dur.textContent = fmtTime(el.duration); updTime(); });
    el.addEventListener('loadedmetadata', () => { dur.textContent = fmtTime(el.duration); });
    el.addEventListener('volumechange', updVol);
    el.addEventListener('waiting', () => wrap.classList.add('vjs-waiting'));
    el.addEventListener('playing', () => wrap.classList.remove('vjs-waiting'));
    el.addEventListener('seeking', () => wrap.classList.add('vjs-seeking'));
    el.addEventListener('seeked', () => wrap.classList.remove('vjs-seeking'));

    playBtn.addEventListener('click', p.toggle);
    q('.vjs-big-play-button').addEventListener('click', p.play);
    el.addEventListener('click', p.toggle);
    muteBtn.addEventListener('click', () => {
      if (el.muted || el.volume === 0) {
        el.muted = false;
        if (el.volume === 0) el.volume = 0.3;
      } else {
        el.muted = true;
      }
    });

    // sliders
    const drag = (bar, onFrac) => {
      const at = (e) => {
        const r = bar.getBoundingClientRect();
        return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
      };
      bar.addEventListener('pointerdown', (e) => {
        e.preventDefault();
        bar.setPointerCapture(e.pointerId);
        wrap.classList.add('vjs-scrubbing');
        onFrac(at(e));
        const move = (ev) => onFrac(at(ev));
        const up = () => {
          wrap.classList.remove('vjs-scrubbing');
          active(); // restart the hide timer that skipped while scrubbing
          bar.removeEventListener('pointermove', move);
          bar.removeEventListener('pointerup', up);
          bar.removeEventListener('pointercancel', up);
        };
        bar.addEventListener('pointermove', move);
        bar.addEventListener('pointerup', up);
        bar.addEventListener('pointercancel', up);
      });
    };
    drag(holder, (f) => { if (el.duration && isFinite(el.duration)) { el.currentTime = f * el.duration; updTime(); } });
    drag(volBar, (f) => { el.volume = f; el.muted = f === 0; });
    holder.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowRight') { el.currentTime += 5; e.preventDefault(); e.stopPropagation(); }
      if (e.key === 'ArrowLeft') { el.currentTime -= 5; e.preventDefault(); e.stopPropagation(); }
    });

    // playback rate menu
    const setRate = (r) => {
      el.playbackRate = r;
      rateVal.textContent = r + 'x';
      rateBtn.querySelectorAll('.vjs-menu-item').forEach((li) => li.classList.toggle('vjs-selected', +li.dataset.rate === r));
    };
    rateBtn.addEventListener('click', (e) => {
      const li = e.target.closest('.vjs-menu-item');
      if (li) { setRate(+li.dataset.rate); rateBtn.querySelector('.vjs-menu').classList.remove('vjs-lock-showing'); return; }
      // clicking the button cycles like video.js
      const i = rates.indexOf(el.playbackRate);
      setRate(rates[(i - 1 + rates.length) % rates.length]);
    });

    // fullscreen: the media box moves into the host (fullscreenHost) and back on leaving it
    const fsBtn = q('.vjs-fullscreen-control');
    const host = fullscreenHost();
    const mark = document.createComment('w0bm-media');
    const intoHost = () => { if (box.parentNode !== host) { box.before(mark); host.appendChild(box); } };
    const outOfHost = () => { if (box.parentNode === host) { if (mark.parentNode) mark.replaceWith(box); else box.remove(); } };
    const showFs = () => {
      const fs = document.fullscreenElement === host && box.parentNode === host;
      wrap.classList.toggle('vjs-fullscreen', fs);
      if (fsBtn) fsBtn.title = fs ? 'Non-Fullscreen' : 'Fullscreen';
    };
    const toggleFs = () => {
      if (document.fullscreenElement) { document.exitFullscreen?.(); return; }
      if (!host.requestFullscreen) return;
      intoHost();
      host.requestFullscreen().catch(() => { outOfHost(); showFs(); });
    };
    fsBtn?.addEventListener('click', toggleFs);
    if (!isAudio) el.addEventListener('dblclick', toggleFs);
    on(document, 'fullscreenchange', () => {
      if (document.fullscreenElement !== host) outOfHost();
      showFs();
    });
    // a page change in fullscreen: this video stops and goes, the next one moves in (below)
    onLeave(() => {
      if (box.parentNode !== host) return;
      try { el.pause(); el.removeAttribute('src'); el.load(); } catch { /* gone */ }
      box.remove();
    });
    if (!isAudio && document.fullscreenElement === host) { intoHost(); showFs(); }
    if (!isAudio) p.toggleFullscreen = toggleFs;

    // user activity (inactivityTimeout: 1000); a mouse resting on the control bar keeps it shown
    let idle, overBar = false;
    const bar = q('.vjs-control-bar');
    bar.addEventListener('pointerenter', (e) => { if (e.pointerType === 'mouse') overBar = true; });
    bar.addEventListener('pointerleave', (e) => { if (e.pointerType === 'mouse') { overBar = false; active(); } });
    const active = () => {
      wrap.classList.add('vjs-user-active');
      wrap.classList.remove('vjs-user-inactive');
      clearTimeout(idle);
      idle = setTimeout(() => {
        if (!el.paused && !isAudio && !overBar && !wrap.classList.contains('vjs-scrubbing')) {
          wrap.classList.remove('vjs-user-active');
          wrap.classList.add('vjs-user-inactive');
        }
      }, 1000);
    };
    wrap.addEventListener('mousemove', active);
    wrap.addEventListener('touchstart', active, { passive: true });
    wrap.addEventListener('mouseleave', () => { if (!el.paused && !isAudio) { wrap.classList.remove('vjs-user-active'); wrap.classList.add('vjs-user-inactive'); } });
    // playback starting counts as activity too: the controls hide a second later without the mouse
    // touching the player (an item reached with the keys, a swipe or the prev/next links)
    el.addEventListener('play', active);

    updState();
    updVol();
    updTime();
    return p;
  }

  if (media && (media.tagName === 'VIDEO' || media.tagName === 'AUDIO')) {
    player = buildPlayer(media);
    if (ITEM && ITEM.autoplay && player.paused()) player.play();
  }
  // came here in fullscreen, but this page has no video to show in it
  if (fsHost && document.fullscreenElement === fsHost && !fsHost.firstChild) document.exitFullscreen?.().catch(() => {});

  // Album: thumbnails swap the stage
  $$('.w0bm-album').forEach((album) => {
    const stage = album.querySelector('.w0bm-album-stage');
    album.querySelectorAll('.w0bm-album-thumb').forEach((a) => a.addEventListener('click', (e) => {
      e.preventDefault();
      album.querySelectorAll('.w0bm-album-thumb').forEach((x) => x.classList.toggle('active', x === a));
      const src = a.dataset.src, mime = a.dataset.mime || '';
      stage.textContent = '';
      if (mime.startsWith('video')) {
        const v = document.createElement('video');
        v.src = src; v.controls = true; v.loop = true; v.autoplay = true; v.playsInline = true;
        stage.appendChild(v);
      } else if (mime.startsWith('audio')) {
        const v = document.createElement('audio');
        v.src = src; v.controls = true; v.loop = true; v.autoplay = true;
        stage.appendChild(v);
      } else {
        const l = document.createElement('a');
        l.href = src; l.target = '_blank';
        const img = document.createElement('img');
        img.src = src;
        l.appendChild(img);
        stage.appendChild(l);
      }
    }));
  });

  // Flash (sandboxed Ruffle frame from /s/js/flash-embed.js)
  const swf = $('#w0bm-swf[data-swf]');
  let flashPlayer = null;
  // resolves once the movie is loaded (true) or failed (false): the backdrop waits for it
  let flashLoaded = Promise.resolve(false);
  const savedVolume = () => {
    const v = parseFloat(store.get('volume'));
    return Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : 0.5;
  };
  if (swf && window.customElements?.get('f0ck-flash')) {
    const el = document.createElement('f0ck-flash');
    // The download link is the no-JS / failed-load stand-in: hidden while the player loads
    swf.classList.add('w0bm-flash-js');
    swf.appendChild(el);
    flashPlayer = el;
    flashLoaded = el.load({ url: swf.dataset.swf, volume: savedVolume() }).then(() => true, () => {
      el.remove();
      flashPlayer = null;
      swf.classList.remove('w0bm-flash-js');
      return false;
    });

    // Volume: drag up/down on the right edge of the stage (a strip over the frame, which swallows
    // mouse events itself); touches arrive from the frame as flash-touch. The level shows in a
    // small HUD (a <meter>) and is saved like the video player's ("volume").
    const hud = document.createElement('div');
    hud.className = 'w0bm-volhud';
    hud.hidden = true;
    hud.innerHTML = '<i class="fa fa-volume-up" aria-hidden="true"></i><meter min="0" max="1" value="0"></meter>';
    const strip = document.createElement('div');
    strip.className = 'w0bm-flash-volstrip';
    strip.title = 'Drag up/down: volume';
    swf.append(strip, hud);
    let hudTimer = null;
    const showVolume = (v) => {
      hud.querySelector('meter').value = v;
      const icon = hud.querySelector('i');
      icon.className = 'fa ' + (v === 0 ? 'fa-volume-off' : v <= 0.5 ? 'fa-volume-down' : 'fa-volume-up');
      hud.hidden = false;
      clearTimeout(hudTimer);
      hudTimer = setTimeout(() => { hud.hidden = true; }, 1000);
    };
    onLeave(() => { delete window.w0bmFlashVolume; });
    window.w0bmFlashVolume = (v) => {
      v = Math.max(0, Math.min(1, Math.round(v * 100) / 100));
      el.volume = v;
      store.set('volume', String(v));
      showVolume(v);
    };
    let drag = null;
    const start = (x, y) => {
      const r = swf.getBoundingClientRect();
      if (x < r.left + r.width / 2) return false;
      drag = { y, vol: el.volume, moved: false };
      return true;
    };
    const move = (y) => {
      if (!drag) return;
      const dy = drag.y - y;
      if (!drag.moved && Math.abs(dy) < 4) return;
      drag.moved = true;
      window.w0bmFlashVolume(drag.vol + dy / 200);
    };
    const end = () => { drag = null; strip.classList.remove('w0bm-dragging'); };
    strip.addEventListener('mousedown', (e) => {
      if (e.button !== 0 || !start(e.clientX, e.clientY)) return;
      e.preventDefault();
      strip.classList.add('w0bm-dragging');
    });
    on(document, 'mousemove', (e) => { if (drag) move(e.clientY); });
    on(document, 'mouseup', end);
    el.addEventListener('flash-touch', (e) => {
      const d = e.detail || {};
      if (d.phase === 'start') start(d.clientX, d.clientY);
      else if (d.phase === 'move') move(d.clientY);
      else end();
    });
  }

  // ── The blurred background (#bg) ──────────────────────────────────────────────────────
  const canvas = $('#bg');
  if (canvas) {
    const img = $('#w0bm-image') || $('.w0bm-album-image');
    // Flash: the player's ambient canvas (a small copy of the movie, kept up to date by the frame)
    const source = player ? (player.el.tagName === 'VIDEO' ? player.el : null) : (flashPlayer ? flashPlayer.ambientCanvas : img);
    if (!source) {
      canvas.remove();
      $('#togglebg')?.remove();
    } else {
      if (store.get('background') == null) store.set('background', isMobile ? 'false' : 'true');
      let background = store.get('background') === 'true';
      const ctx = canvas.getContext('2d');
      let cw = 0, ch = 0;
      const size = () => {
        cw = canvas.width = canvas.clientWidth | 0;
        ch = canvas.height = canvas.clientHeight | 0;
      };
      size();
      canvas.classList.toggle('w0bm-bg-off', !background);
      const drawOnce = () => {
        try { ctx.drawImage(source, 0, 0, cw, ch); } catch { /* not ready / tainted */ }
      };
      const loop = () => {
        if (!background) return;
        if (source.tagName === 'VIDEO') {
          if (source.paused || source.ended) return;
          drawOnce();
          requestAnimationFrame(loop);
        } else if (source.tagName === 'CANVAS') {
          // Flash: ~12 redraws a second are plenty for a blurred backdrop
          if (!canvas.isConnected) return;
          if (!document.hidden) drawOnce();
          setTimeout(loop, 80);
        } else if (source.complete) {
          drawOnce();
        } else {
          source.addEventListener('load', drawOnce, { once: true });
        }
      };
      if (source.tagName === 'VIDEO') source.addEventListener('play', loop);
      on(window, 'resize', () => { size(); if (source.tagName === 'IMG' || source.paused) drawOnce(); });
      if (source.tagName === 'CANVAS') {
        // Flash: while Ruffle loads, its picture is blank white; the backdrop fades in with the
        // movie (a moment after it is loaded, once real frames have arrived)
        canvas.classList.add('w0bm-bg-wait');
        flashLoaded.then((ok) => {
          if (!ok || !canvas.isConnected) return;
          setTimeout(() => {
            if (!canvas.isConnected) return;
            canvas.classList.remove('w0bm-bg-wait');
            loop();
          }, 400);
        });
      } else loop();
      $('#togglebg')?.addEventListener('click', (e) => {
        e.preventDefault();
        background = !background;
        store.set('background', String(background));
        canvas.classList.toggle('w0bm-bg-off', !background);
        if (background && !canvas.classList.contains('w0bm-bg-wait')) { size(); drawOnce(); loop(); }
      });
    }
  }

  // ── Tables (index, profile, search): the row's thumbnail follows the mouse ─────────────
  // One floating box (#w0bm-hoverthumb) next to the cursor; its position goes through two custom
  // properties. Without JavaScript the CSS preview in the row (.w0bm-rowthumb) shows instead.
  {
    let box = null;
    onLeave(() => box?.remove());
    const place = (e) => {
      if (!box) return;
      const w = box.offsetWidth || 130, h = box.offsetHeight || 130;
      const x = e.clientX + 15 + w > window.innerWidth - 10 ? e.clientX - 15 - w : e.clientX + 15;
      const y = e.clientY + 15 + h > window.innerHeight - 10 ? e.clientY - 15 - h : e.clientY + 15;
      box.style.setProperty('--thumb-x', Math.max(0, x) + 'px');
      box.style.setProperty('--thumb-y', Math.max(0, y) + 'px');
    };
    $$('tr[data-thumb]').forEach((row) => {
      row.addEventListener('mouseenter', (e) => {
        if (!box) {
          box = document.createElement('div');
          box.id = 'w0bm-hoverthumb';
          box.innerHTML = '<img alt="">';
          document.body.appendChild(box);
        }
        const img = box.firstChild;
        img.onerror = () => { img.onerror = null; img.src = '/s/fe/w0bm/img/small_404.gif'; };
        img.src = row.dataset.thumb;
        box.hidden = false;
        place(e);
      });
      row.addEventListener('mousemove', place);
      row.addEventListener('mouseleave', () => { if (box) box.hidden = true; });
    });
  }

  // ── Comments sidebar toggle (C) ───────────────────────────────────────────────────────
  const aside = $('.aside');
  if (aside && $('#toggle')) {
    if (store.get('comments') == null) store.set('comments', 'true');
    // the class on <html> is also set by early.js before the page is drawn (no flash)
    const apply = () => document.documentElement.classList.toggle('w0bm-comments-off', store.get('comments') !== 'true');
    apply();
    var toggleComments = () => { store.set('comments', String(store.get('comments') !== 'true')); apply(); };
    $('#toggle').addEventListener('click', (e) => { e.preventDefault(); toggleComments(); });
  }

  // ── Keyboard / mouse wheel navigation ─────────────────────────────────────────────────
  const visible = (el) => el && !el.classList.contains('w0bm-invisible');
  const goNext = () => { const n = $('#next'); if (visible(n)) n.click(); };
  const goPrev = () => { const p = $('#prev'); if (visible(p)) p.click(); };
  // "F is for Fullscreen" (settings page, localStorage like f0ckm): F toggles fullscreen of the video
  // or Flash movie, Shift+F favorites. Images and audio have no fullscreen: F favorites there.
  const fForFullscreen = () => store.get('fForFullscreen') === 'true';
  const toggleItemFullscreen = () => {
    if (player?.toggleFullscreen) { player.toggleFullscreen(); return true; }
    // Flash: the stage (frame, volume strip and HUD) goes fullscreen
    const stage = flashPlayer?.closest('#w0bm-swf');
    if (stage?.requestFullscreen) {
      if (document.fullscreenElement) document.exitFullscreen?.().catch(() => {});
      else stage.requestFullscreen().catch(() => {});
      return true;
    }
    return false;
  };
  if (ITEM) {
    on(document, 'keydown', (e) => {
      if (e.defaultPrevented || /^(input|textarea|select)$/i.test(e.target.nodeName) || e.target.isContentEditable || e.ctrlKey || e.altKey || e.metaKey) return;
      if ($('.modal.w0bm-modal-open')) return;
      if (e.shiftKey) {
        if (e.keyCode === 70 && fForFullscreen() && !e.repeat) { e.preventDefault(); $('#fav')?.click(); } // Shift+F
        return;
      }
      // 0-9 (top row or numpad): jump to 0 %, 10 %, ... 90 % of the video / audio
      if (/^[0-9]$/.test(e.key)) {
        const d = player && player.duration();
        if (d && isFinite(d)) { e.preventDefault(); player.currentTime(d * Number(e.key) / 10); }
        return;
      }
      switch (e.keyCode) {
        case 39: case 68: case 76: goNext(); break; // → d l
        case 37: case 65: case 72: goPrev(); break; // ← a h
        case 82: $('#w0bm-context')?.click(); break; // r
        case 70: if (e.repeat) break; if (!(fForFullscreen() && toggleItemFullscreen())) $('#fav')?.click(); break; // f
        case 80: cycleRating(); break; // p
        case 73: if (openTagInput()) e.preventDefault(); break; // i (no "i" typed into the field)
        case 67: if (typeof toggleComments === 'function') toggleComments(); break; // c
        case 88: if ($('#w0bm-delmodal')) { e.preventDefault(); openModal($('#w0bm-delmodal')); } break; // x (mods: delete)
        case 87: case 38: if (player) { e.preventDefault(); player.volume(player.volume() + 0.1); } else if (flashPlayer && window.w0bmFlashVolume) { e.preventDefault(); window.w0bmFlashVolume(flashPlayer.volume + 0.1); } break; // w ↑
        case 83: case 40: if (player) { e.preventDefault(); player.volume(player.volume() - 0.1); } else if (flashPlayer && window.w0bmFlashVolume) { e.preventDefault(); window.w0bmFlashVolume(flashPlayer.volume - 0.1); } break; // s ↓
        case 32: if (player) { e.preventDefault(); player.toggle(); } break; // space
        default: break;
      }
    });
    const main = $('.wrapper > .w0bm-main');
    const wheelNav = (e) => {
      if (e.ctrlKey || e.altKey || e.shiftKey) return;
      if (e.target.closest('.modal, .popover, .vjs-volume-control')) return;
      // pages taller than the window keep scrolling normally (not in fullscreen: nothing scrolls)
      const se = document.scrollingElement;
      if (!document.fullscreenElement && se && se.scrollHeight > window.innerHeight + 5) return;
      e.preventDefault();
      if (e.deltaY > 0) goNext(); else if (e.deltaY < 0) goPrev();
    };
    main?.addEventListener('wheel', wheelNav, { passive: false });
    // the fullscreen video sits in the host, outside main
    if (fsHost) on(fsHost, 'wheel', wheelNav, { passive: false });
  }

  // ── Favorite ──────────────────────────────────────────────────────────────────────────
  // The heart (and you in "Favorited by") changes at once; the request runs in the background.
  // Clicks while one is on the way only change what is wanted: when it is back, one more request
  // sends the latest state if it differs (explicit add / remove, so a repeat changes nothing).
  // A failed request puts the heart back to what the server has.
  const favForm = $('#w0bm-fav-form');
  if (favForm) {
    const action = favForm.querySelector('input[name="action"]');
    const icon = favForm.querySelector('i');
    const favBtn = favForm.querySelector('#fav');
    const list = $('#w0bm-favlist');
    const selfHref = DATA.user ? '/user/' + encodeURIComponent(String(DATA.user).toLowerCase()) : null;
    let confirmed = action.value === 'remove'; // faved on the server
    let wanted = confirmed;
    let busy = false;
    const showList = () => { if (list) list.closest('.w0bm-favpop').hidden = !list.textContent.trim(); };
    // "Favorited by" popover on the heart (partials/favlist.html)
    const renderList = (favs) => {
      if (!list) return;
      list.textContent = '';
      favs.forEach((f, i) => {
        if (i) list.append(', ');
        const self = DATA.user && f.user === DATA.user;
        if (f.is_anon || (f.hide_fav_badge && !self)) { list.append('?'); return; }
        const a = document.createElement('a');
        a.href = '/user/' + encodeURIComponent(String(f.user || '').toLowerCase());
        a.textContent = f.display_name || f.user;
        list.append(a);
      });
      showList();
    };
    // until the server's list arrives: add or take out the own name
    const selfInList = (faved) => {
      if (!list || !selfHref) return;
      const own = [...list.querySelectorAll('a')].find((a) => a.getAttribute('href') === selfHref);
      if (faved && !own) {
        if (list.textContent.trim()) list.append(', ');
        const a = document.createElement('a');
        a.href = selfHref;
        a.textContent = DATA.display_name || DATA.user;
        list.append(a);
      } else if (!faved && own) {
        const sep = own.previousSibling?.nodeType === 3 ? own.previousSibling : own.nextSibling?.nodeType === 3 ? own.nextSibling : null;
        sep?.remove();
        own.remove();
      }
      showList();
    };
    const show = (faved) => {
      action.value = faved ? 'remove' : 'add';
      icon.classList.toggle('fa-heart', faved);
      icon.classList.toggle('fa-heart-o', !faved);
      if (favBtn) favBtn.dataset.faved = faved ? '1' : '0';
      selfInList(faved);
    };
    const sync = async () => {
      if (busy) return;
      busy = true;
      while (wanted !== confirmed) {
        const target = wanted;
        const body = new FormData(favForm);
        body.set('action', target ? 'add' : 'remove');
        const r = await api('/api/v2/togglefav', { body });
        if (!r.ok) {
          flash('error', r.msg || 'Error');
          wanted = confirmed;
          show(confirmed);
          break;
        }
        confirmed = target;
        if (wanted === confirmed && Array.isArray(r.data?.favs)) renderList(r.data.favs);
      }
      busy = false;
    };
    favForm.addEventListener('submit', (e) => {
      e.preventDefault();
      wanted = !wanted;
      show(wanted);
      sync();
    });
  }

  // ── Subscribe to comments ─────────────────────────────────────────────────────────────
  const subForm = $('#w0bm-sub-form');
  subForm?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const want = subForm.querySelector('input[name="subscribed"]');
    const r = await api(subForm.action, { body: new FormData(subForm) });
    if (!r.ok) { flash('error', r.msg || 'Error'); return; }
    const on = !!r.data.subscribed;
    want.value = on ? '0' : '1';
    const btn = subForm.querySelector('button');
    btn.querySelector('i').className = 'fa ' + (on ? 'fa-bell' : 'fa-bell-o');
    btn.title = on ? 'Unsubscribe' : 'Subscribe to comments';
    flash('success', on ? 'Subscribed to comments' : 'Unsubscribed');
  });

  // ── Edit / delete the item ────────────────────────────────────────────────────────────
  const editForm = $('#webmedit');
  editForm?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const id = editForm.dataset.itemId;
    const errors = [];
    const title = editForm.querySelector('input[name="title"]');
    if (title && title.value !== title.defaultValue) {
      const r = await api('/api/v2/items/' + id + '/title', { method: 'PATCH', json: { title: title.value } });
      if (r.ok) {
        title.defaultValue = title.value;
        const h = $('#w0bm-item-title');
        if (h) h.textContent = r.data.title || '';
      } else errors.push(r.msg);
    }
    const rating = editForm.querySelector('input[name="rating"]:checked');
    if (rating && !rating.defaultChecked) {
      const r = await api('/api/v2/tags/' + id + '/cycle-rating', { method: 'PUT', body: { rating: rating.value } });
      if (r.ok) {
        editForm.querySelectorAll('input[name="rating"]').forEach((o) => { o.defaultChecked = o.checked; });
        reloadTags();
      } else errors.push(r.msg);
    }
    if (errors.length) flash('error', 'Error updating upload: ' + errors.join(', '));
    else flash('success', 'Upload successfully updated');
    closeModal($('#webmeditmodal'));
  });
  // Edit modal tools (partials/frontendedit.html), sent with fetch; plain forms without JavaScript
  const editItemId = editForm?.dataset.itemId;
  const rethumbForm = $('#w0bm-rethumb-form');
  rethumbForm?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const btn = rethumbForm.querySelector('button');
    btn.disabled = true;
    const r = await api(rethumbForm.action, { body: new FormData(rethumbForm) });
    btn.disabled = false;
    if (!r.ok) { flash('error', r.msg || 'Thumbnail regeneration failed'); return; }
    flash('success', 'Thumbnail regenerated');
    bustThumbs();
  });
  function bustThumbs() {
    const bust = Date.now();
    $$('img[src*="/t/' + editItemId + '.webp"]').forEach((img) => {
      const u = new URL(img.src, location.origin);
      u.searchParams.set('t', bust);
      img.src = u.toString();
    });
  }
  // Flash: capture the player's current frame, preview it, then upload it as the thumbnail
  const snapBtn = $('#w0bm-snap-btn');
  const snapBox = $('#w0bm-snap-preview');
  if (snapBtn && snapBox) {
    const snapImg = snapBox.querySelector('img');
    let snapBlob = null;
    const clearSnap = () => {
      if (snapImg.src.startsWith('blob:')) URL.revokeObjectURL(snapImg.src);
      snapImg.removeAttribute('src');
      snapBlob = null;
      snapBox.hidden = true;
    };
    const capture = async () => {
      const blob = flashPlayer ? await flashPlayer.snapshot({ max: 640, quality: 0.92 }).catch(() => null) : null;
      if (!blob) { flash('error', 'Flash player not ready, play the Flash first'); return; }
      clearSnap();
      snapBlob = blob;
      snapImg.src = URL.createObjectURL(blob);
      snapBox.hidden = false;
    };
    snapBtn.addEventListener('click', capture);
    snapBox.addEventListener('click', async (e) => {
      const b = e.target.closest('button[data-snap]');
      if (!b) return;
      if (b.dataset.snap === 'again') { capture(); return; }
      if (b.dataset.snap === 'cancel' || !snapBlob) { clearSnap(); return; }
      b.disabled = true;
      const fd = new FormData();
      fd.append('file', snapBlob, 'thumbnail.jpg');
      let data = null;
      try {
        const res = await fetch('/api/v2/items/' + editItemId + '/thumbnail', {
          method: 'POST',
          headers: { 'X-CSRF-Token': csrf(), 'X-Requested-With': 'XMLHttpRequest', Accept: 'application/json' },
          body: fd,
          credentials: 'same-origin',
        });
        data = await res.json().catch(() => null);
        if (!res.ok && data) data.success = false;
      } catch { /* network error */ }
      b.disabled = false;
      if (!data || data.success === false) { flash('error', (data && data.msg) || 'Thumbnail upload failed'); return; }
      flash('success', data.msg || 'Thumbnail updated');
      clearSnap();
      bustThumbs();
    });
    onLeave(clearSnap);
  }
  const metaBtn = $('#w0bm-meta-btn');
  const metaBox = $('#w0bm-meta-results');
  metaBtn?.addEventListener('click', async () => {
    metaBtn.disabled = true;
    metaBox.hidden = false;
    metaBox.textContent = 'reading the file …';
    const r = await api('/api/v2/meta/extract/item/' + editItemId, { method: 'GET' });
    metaBtn.disabled = false;
    const fields = (r.ok && r.data && Array.isArray(r.data.fields)) ? r.data.fields : [];
    metaBox.textContent = '';
    if (!fields.length) {
      metaBox.textContent = r.ok ? 'No metadata found in this file.' : (r.msg || 'Could not read the metadata');
      return;
    }
    const have = new Set($$('#tag-display .label').map((l) => String(l.dataset.tag || l.textContent).trim().toLowerCase()));
    fields.forEach((field) => {
      const chip = document.createElement('button');
      chip.type = 'button';
      chip.className = 'w0bm-meta-chip';
      chip.textContent = field;
      if (have.has(String(field).toLowerCase())) chip.classList.add('w0bm-meta-added');
      chip.addEventListener('click', async () => {
        if (chip.classList.contains('w0bm-meta-added')) return;
        chip.disabled = true;
        const t = await api('/api/v2/tags/' + editItemId, { json: { tagname: field } });
        chip.disabled = false;
        if (!t.ok) { flash('error', t.msg || 'Error adding tag'); return; }
        chip.classList.add('w0bm-meta-added');
        if (t.data && t.data.tags) renderTags(t.data.tags); else reloadTags();
        flash('success', 'Tag "' + field + '" added');
      });
      metaBox.appendChild(chip);
    });
  });
  const unavailForm = $('#w0bm-451-form');
  unavailForm?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const btn = unavailForm.querySelector('button');
    btn.disabled = true;
    const r = await api(unavailForm.action, { body: new FormData(unavailForm) });
    btn.disabled = false;
    if (!r.ok) { flash('error', r.msg || 'Failed to change visibility'); return; }
    // flip the form in place: the next click does the opposite
    const off = +r.data.visibility === 3;
    flash('success', off ? 'Item is unavailable now (451)' : 'Item is available again');
    unavailForm.querySelector('input[name="visibility"]').value = off ? '0' : '3';
    btn.classList.toggle('btn-primary', off);
    btn.classList.toggle('btn-danger', !off);
    btn.innerHTML = '<i class="fa fa-ban"></i> ' + (off ? 'Make available again' : 'Make unavailable (451)');
    const hint = unavailForm.querySelector('.w0bm-edit-hint');
    if (hint) hint.textContent = off ? 'Unavailable for legal reasons (451) right now.' : 'Hide it behind a 451 Unavailable For Legal Reasons page.';
  });
  // [del] / "x": the delete modal (partials/deletemodal.html), sent with fetch
  const delForm = $('#w0bm-delform');
  delForm?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const btn = delForm.querySelector('[type="submit"]');
    btn.disabled = true;
    const r = await api(delForm.action, { body: new FormData(delForm) });
    btn.disabled = false;
    if (r.ok) {
      closeModal(delForm.querySelector('.modal'));
      flash('success', 'Deleted');
      const n = $('#prev:not(.w0bm-invisible)') || $('#next:not(.w0bm-invisible)');
      setTimeout(() => navigate(n ? n.href : '/'), 500);
    } else flash('error', r.msg || 'Error deleting');
  });

  // ── Halls ("+" in the item bar, partials/hallmodal.html) ───────────────────────────────
  // Plain forms; sent with fetch here. Options carry data-in="1" (✓) when the item is in that hall;
  // Remove only shows for those.
  $$('#w0bm-hallmodal form.w0bm-hallform').forEach((form) => {
    const select = form.querySelector('select[name="hall"]');
    const addBtn = form.querySelector('[data-hall-add]');
    const removeBtn = form.querySelector('[data-hall-remove]');
    const newName = form.querySelector('input[name="new_name"]');
    const label = (o) => (o.dataset.name || o.value) + (o.textContent.includes('(private)') ? ' (private)' : '') + (o.dataset.in === '1' ? ' ✓' : '');
    const sync = () => {
      const o = select.selectedOptions[0];
      removeBtn.hidden = !(o && o.value && o.dataset.in === '1');
      addBtn.hidden = !!(o && o.value && o.dataset.in === '1');
    };
    // Typing a new name means "new hall": drop the picked one
    newName?.addEventListener('input', () => { if (newName.value.trim()) { select.value = ''; sync(); } });
    select.addEventListener('change', () => { if (newName && select.value) newName.value = ''; sync(); });
    sync();
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      const btn = e.submitter || addBtn;
      const remove = btn === removeBtn;
      const fd = new FormData(form);
      if (btn.name) fd.set(btn.name, btn.value);
      btn.disabled = true;
      const r = await api(btn.getAttribute('formaction') || form.action, { body: fd });
      btn.disabled = false;
      if (!r.ok) { flash('error', r.msg || 'Error'); return; }
      let opt = select.selectedOptions[0];
      const created = r.data && r.data.created;
      if (created) {
        opt = document.createElement('option');
        opt.value = created.slug;
        opt.dataset.name = created.name;
        select.appendChild(opt);
        select.value = created.slug;
        if (newName) newName.value = '';
      }
      if (opt && opt.value) {
        opt.dataset.in = remove ? '0' : '1';
        opt.textContent = label(opt);
      }
      sync();
      const name = opt ? (opt.dataset.name || opt.value) : '';
      flash('success', (remove ? 'Removed from ' : 'Added to ') + (name ? '"' + name + '"' : 'hall'));
    });
  });

  // ── Tags ──────────────────────────────────────────────────────────────────────────────
  const tagDisplay = $('#tag-display');
  function renderTags(tags) {
    if (!tagDisplay) return;
    const canDelete = tagDisplay.dataset.canDelete === '1';
    tagDisplay.textContent = '';
    const ratings = ['sfw', 'nsfw', 'nsfl'];
    // unrated: "?" in place of the rating tag (click or "p" to rate)
    if (!(tags || []).some((t) => ratings.includes(t.normalized || String(t.tag).toLowerCase()))) {
      const q = document.createElement('span');
      q.className = 'label label-default w0bm-rating-tag w0bm-rating-unset';
      q.dataset.rating = 'untagged';
      q.title = 'unrated';
      q.textContent = '?';
      tagDisplay.append(q, ' ');
    }
    if (!tags || !tags.length) { tagDisplay.append('No tags yet ...'); markRatingTags(); return; }
    tags.forEach((t, i) => {
      const norm = t.normalized || String(t.tag).toLowerCase();
      const span = document.createElement('span');
      span.className = 'label label-default' + (ratings.includes(norm) ? ' w0bm-rating-tag' : '');
      span.dataset.tag = t.tag;
      const a = document.createElement('a');
      a.href = '/tag/' + encodeURIComponent(norm);
      a.className = 'default-link' + (ratings.includes(norm) ? ' w0bm-tag-' + norm : '');
      a.textContent = t.tag;
      span.appendChild(a);
      if (canDelete && !ratings.includes(norm)) {
        span.append(' ');
        const d = document.createElement('a');
        d.className = 'delete-tag default-link';
        d.href = '#';
        d.innerHTML = '<i class="fa fa-times"></i>';
        span.appendChild(d);
      }
      tagDisplay.appendChild(span);
      if (i < tags.length - 1) tagDisplay.append(' ');
    });
    markRatingTags();
  }
  async function reloadTags() {
    if (!ITEM) return;
    const r = await api('/api/v2/tags/' + ITEM.id, { method: 'GET' });
    if (r.ok) renderTags(r.data.tags);
  }
  function markRatingTags() {
    if (!tagDisplay || tagDisplay.dataset.canRate !== '1') return;
    tagDisplay.querySelectorAll('.w0bm-rating-tag').forEach((s) => {
      s.classList.add('w0bm-can-rate');
      s.title = 'Click to change the rating';
    });
  }
  markRatingTags();
  tagDisplay?.addEventListener('click', async (e) => {
    const del = e.target.closest('.delete-tag');
    if (del) {
      e.preventDefault();
      const tag = del.closest('.label').dataset.tag;
      if (!(await dialog({ title: 'Delete tag', text: 'Do you really want to delete the tag "' + tag + '"?', ok: 'Delete' }))) return;
      const r = await api('/api/v2/tags/' + ITEM.id + '/' + encodeURIComponent(tag), { method: 'DELETE' });
      if (r.ok) {
        flash('success', 'Tag successfully deleted');
        renderTags(r.data.tags);
      } else flash('error', r.msg || 'Error deleting tag');
      return;
    }
    const rating = e.target.closest('.w0bm-rating-tag.w0bm-can-rate');
    if (rating) {
      e.preventDefault();
      cycleRating();
    }
  });
  // Rating: unrated -> sfw -> nsfw -> nsfl (if enabled) -> sfw; click on the rating tag or "p".
  // Like f0ckm: every press shows the new rating at once, only the last one is saved (after a short
  // pause), as an explicit rating (POST /api/v2/item/:id/rating).
  let wantedRating = null;
  let ratingTimer = null;
  let ratingSaving = Promise.resolve();
  // the tags as the server has them, kept from the first press until a save answers: a failed
  // save puts them back at once (then reloads them)
  let savedTags = null;
  const shownRating = () => {
    const tag = tagDisplay && Array.from(tagDisplay.querySelectorAll('.w0bm-rating-tag')).find((s) => s.dataset.rating !== 'untagged');
    return tag ? String(tag.dataset.tag).toLowerCase() : 'untagged';
  };
  function showRating(rating) {
    tagDisplay.querySelectorAll('.w0bm-rating-tag').forEach((s) => s.remove());
    const span = document.createElement('span');
    span.className = 'label label-default w0bm-rating-tag';
    span.dataset.tag = rating;
    const a = document.createElement('a');
    a.href = '/tag/' + rating;
    a.className = 'default-link w0bm-tag-' + rating;
    a.textContent = rating;
    span.appendChild(a);
    tagDisplay.prepend(span, ' ');
    markRatingTags();
    // keep the edit modal's rating toggles on the current rating
    $$('#webmedit input[name="rating"]').forEach((o) => { o.checked = o.defaultChecked = (o.value === rating); });
  }
  // "i": open [+Add Tag] and focus its field, like f0ckm. Escape in the field closes it again.
  function openTagInput() {
    const toggle = $('#tag-add-toggle');
    const field = $('#tag-add .bootstrap-tagsinput input') || $('#tag-add input[name="tagname"]');
    if (!toggle || !field) return false;
    // the tags live in the sidebar: show it for this page if C hid it (the saved choice stays)
    document.documentElement.classList.remove('w0bm-comments-off');
    toggle.checked = true;
    field.focus();
    return true;
  }
  $('#tag-add')?.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    const toggle = $('#tag-add-toggle');
    if (toggle) toggle.checked = false;
    e.target.blur();
  });
  function cycleRating() {
    if (!ITEM || !tagDisplay || tagDisplay.dataset.canRate !== '1') return;
    const cur = wantedRating || shownRating();
    if (!savedTags) savedTags = [...tagDisplay.childNodes].map((n) => n.cloneNode(true));
    const next = cur === 'sfw' ? 'nsfw' : (cur === 'nsfw' && DATA.nsfl) ? 'nsfl' : 'sfw';
    wantedRating = next;
    showRating(next);
    flash('info', 'Rating: ' + next.toUpperCase(), 'rating');
    clearTimeout(ratingTimer);
    ratingTimer = setTimeout(saveRating, 300);
  }
  function saveRating() {
    const rating = wantedRating;
    // one request at a time; a newer choice waits for the running one
    ratingSaving = ratingSaving.then(async () => {
      const r = await api('/api/v2/item/' + ITEM.id + '/rating', { json: { rating, csrf_token: DATA.csrf || '' } });
      if (wantedRating !== rating) return; // pressed again meanwhile: the next save follows
      wantedRating = null;
      const before = savedTags;
      savedTags = null;
      if (r.ok) {
        // same message and colour as while pressing; only a failure turns it red
        flash('info', 'Rating: ' + String(r.data.rating || rating).toUpperCase(), 'rating');
        if (r.data.tags) renderTags(r.data.tags); else reloadTags();
      } else {
        flash('error', r.msg || 'Could not change the rating', 'rating');
        if (before) { tagDisplay.replaceChildren(...before); markRatingTags(); }
        reloadTags();
      }
    });
  }
  const tagForm = $('form.w0bm-tagform');
  const tagInput = $('#tags');
  if (tagForm && tagInput) {
    const ti = tagsinput(tagInput, {
      beforeAdd(tag) {
        const existing = $$('#tag-display .label').map((s) => (s.dataset.tag || '').toLowerCase());
        if (existing.includes(tag.toLowerCase())) { flash('info', 'Tag already exists'); return false; }
        return true;
      },
    });
    tagForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      ti.flush();
      const list = ti.items();
      if (!list.length) { flash('error', 'No tags specified'); return; }
      let last = null, errors = [];
      for (const tag of list) {
        const r = await api('/api/v2/tags/' + ITEM.id, { body: { tagname: tag } });
        if (r.ok) last = r.data.tags;
        else errors.push(tag + ': ' + r.msg);
        if (r.data && r.data.tags) last = r.data.tags;
      }
      if (last) renderTags(last);
      ti.removeAll();
      if (errors.length) flash('error', 'Error saving tag(s): ' + errors.join(', '));
      else {
        const t = $('#tag-add-toggle');
        if (t) t.checked = false;
      }
    });
  }

  // ── Comments ──────────────────────────────────────────────────────────────────────────
  async function loadEmojis() {
    if (emojis) return emojis;
    emojis = {};
    const r = await api('/api/v2/emojis', { method: 'GET' });
    if (r.ok) (r.data.emojis || []).forEach((e) => { if (e.name && e.url) emojis[e.name] = e.url; });
    return emojis;
  }
  const allowedImgHosts = (() => {
    try { return JSON.parse($('#w0bm-images')?.textContent || '[]'); } catch { return []; }
  })();
  const isAllowedMedia = (url) => {
    try {
      const u = new URL(url, location.href);
      if (u.origin === location.origin) return true;
      return allowedImgHosts.some((h) => u.hostname === h || u.hostname.endsWith('.' + h));
    } catch { return false; }
  };

  // w0bm's simplemd for f0ckm comments: greentext, links, media, mentions, emojis, timestamps,
  // [spoiler] [blur] [rb] [reich] [krebs] [glitch], **bold**, *italic*, `code`
  function renderComment(text) {
    const codes = [];
    let s = escapeHTML(text || '').replace(/&#x2F;/g, '/').replace(/&#x3D;/g, '=').replace(/&#x60;/g, '`');
    s = s.replace(/```([\s\S]*?)```/g, (m, c) => { codes.push('<pre>' + c.replace(/^\n/, '') + '</pre>'); return '\u0000' + (codes.length - 1) + '\u0000'; });
    s = s.replace(/`([^`\n]+)`/g, (m, c) => { codes.push('<code>' + c + '</code>'); return '\u0000' + (codes.length - 1) + '\u0000'; });
    s = s.split('\n').map((line) => (/^&gt;(?!&gt;\d)/.test(line) ? '<span class="greentext">' + line + '</span>' : line)).join('\n');
    s = s.replace(/(^|[\s(])((?:https?:\/\/|\/(?:b|c)\/)[^\s<>"']+)/g, (m, pre, url) => {
      const clean = url.replace(/&amp;/g, '&');
      const path = clean.split(/[?#]/)[0].toLowerCase();
      if (isAllowedMedia(clean)) {
        if (/\.(png|jpe?g|gif|webp|avif)$/.test(path)) return pre + '<a href="' + url + '" target="_blank" rel="noopener noreferrer"><img class="comment_image" src="' + url + '" alt="" loading="lazy"></a>';
        if (/\.(mp4|webm|mov|ogv)$/.test(path)) return pre + '<video src="' + url + '" controls loop muted playsinline preload="metadata"></video>';
        if (/\.(mp3|ogg|wav|flac|opus|m4a|aac)$/.test(path)) return pre + '<audio src="' + url + '" controls preload="metadata"></audio>';
      }
      const ext = !clean.startsWith('/') && !clean.startsWith(location.origin);
      return pre + '<a href="' + url + '"' + (ext ? ' rel="extern noopener noreferrer" target="_blank"' : '') + '>' + url + '</a>';
    });
    s = s.replace(/(^|[\s(])@([A-Za-z0-9_\-]{1,32})/g, (m, pre, u) => pre + '<a class="w0bm-mention" href="/user/' + u.toLowerCase() + '">@' + u + '</a>');
    s = s.replace(/(^|\s)&gt;&gt;(\d+)/g, (m, pre, id) => pre + '<a class="w0bm-quote" href="#c' + id + '">&gt;&gt;' + id + '</a>');
    s = s.replace(/\b(\d{1,2}:\d{2})\b(?![^<]*>)/g, '<a class="comment_clickable_timestamp" href="#">$1</a>');
    s = s.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>').replace(/(^|[^*])\*([^*\n]+)\*(?!\*)/g, '$1<em>$2</em>');
    const wrapTag = (tag, cls, attrs = '') => {
      const re = new RegExp('\\[' + tag + '\\]((?:(?!\\[' + tag + '\\])[\\s\\S])*?)\\[\\/' + tag + '\\]', 'gi');
      for (let i = 0; i < 10; i++) {
        const n = s.replace(re, '<span class="' + cls + '"' + attrs + '>$1</span>');
        if (n === s) break;
        s = n;
      }
    };
    wrapTag('spoiler', 'spoiler');
    wrapTag('blur', 'blur-text');
    wrapTag('rb', 'rainbow');
    wrapTag('reich', 'reich');
    wrapTag('krebs', 'anim');
    // the same as in the other frontends (f0ck-core.js f0ckCommentFx, /s/css/comment-fx.css)
    wrapTag('glitch', 'cfx-glitch', ' data-glitch="0.5"');
    if (emojis) s = s.replace(/:([a-z0-9_\-]+):/g, (m, n) => (emojis[n] ? '<img class="comment_emoji" src="' + escapeHTML(emojis[n]) + '" alt=":' + n + ':" title=":' + n + ':">' : m));
    s = s.replace(/\u0000(\d+)\u0000/g, (m, i) => codes[+i]);
    return s.replace(/\n/g, '<br>');
  }

  const commentBox = $('#w0bm-comments');
  let comments = [];
  // everything that answers each comment: direct replies (parent_id) and >>id quotes, listed on
  // the answered comment as ↳ >>36 >>37 (like the page's static list, routes/comments.rs replied_by)
  function repliedByMap(list) {
    const ids = new Set(list.map((c) => Number(c.id)));
    const map = new Map();
    const add = (target, id) => {
      if (target === id || !ids.has(target)) return;
      if (!map.has(target)) map.set(target, []);
      if (!map.get(target).includes(id)) map.get(target).push(id);
    };
    list.forEach((c) => {
      const id = Number(c.id);
      if (c.parent_id) add(Number(c.parent_id), id);
      if (!c.is_deleted) for (const m of String(c.content || '').matchAll(/>>(\d+)/g)) add(+m[1], id);
    });
    map.forEach((v) => v.sort((a, b) => a - b));
    return map;
  }
  function commentPanel(c, repliedBy) {
    const mine = DATA.user && c.username && c.username.toLowerCase() === String(DATA.user).toLowerCase() && !c.is_anon;
    const div = document.createElement('div');
    div.className = 'panel ' + (c.is_deleted ? 'panel-danger' : 'panel-default') + (c.is_pinned ? ' w0bm-pinned' : '');
    div.dataset.id = c.id;
    div.id = 'c' + c.id;
    const body = document.createElement('div');
    body.className = 'panel-body';
    // the comment's id, top right: what >>id quotes refer to
    body.insertAdjacentHTML('beforeend', '<a class="w0bm-cid" href="#c' + c.id + '" title="Link to this comment">#' + c.id + '</a>');
    const answers = (repliedBy && repliedBy.get(Number(c.id))) || [];
    if (answers.length) body.insertAdjacentHTML('beforeend', '<div class="w0bm-replies">↳ ' + answers.map((r) => '<a href="#c' + r + '">&gt;&gt;' + r + '</a>').join(' ') + '</div>');
    body.insertAdjacentHTML('beforeend', c.is_deleted && !DATA.is_mod ? '<i>[deleted]</i>' : renderComment(c.content));
    (c.files || []).filter((f) => f.dest && !String(c.content || '').includes(f.dest)).forEach((f) => {
      const url = '/c/' + String(f.dest || '').replace(/^\/?c\//, '');
      const m = f.mime || '';
      if (m.startsWith('image')) body.insertAdjacentHTML('beforeend', '<br><a href="' + escapeHTML(url) + '" target="_blank"><img class="comment_image" src="' + escapeHTML(url) + '" alt="" loading="lazy"></a>');
      else if (m.startsWith('video')) body.insertAdjacentHTML('beforeend', '<br><video src="' + escapeHTML(url) + '" controls loop playsinline preload="metadata"></video>');
      else if (m.startsWith('audio')) body.insertAdjacentHTML('beforeend', '<br><audio src="' + escapeHTML(url) + '" controls preload="metadata"></audio>');
      else body.insertAdjacentHTML('beforeend', '<br><a href="' + escapeHTML(url) + '" download>' + escapeHTML(f.original_filename || url) + '</a>');
    });
    const foot = document.createElement('div');
    foot.className = 'panel-footer';
    const name = c.is_anon ? 'Anonymous' + (c.anon_short_fingerprint ? ' (' + c.anon_short_fingerprint + ')' : '') : (c.display_name || c.username);
    foot.innerHTML = 'by ' + (c.is_anon || !c.username
      ? '<span>' + escapeHTML(name) + '</span>'
      : '<a href="/user/' + encodeURIComponent(String(c.username).toLowerCase()) + '"' + (c.username_color ? ' class="w0bm-usercolor"' : '') + '>' + escapeHTML(name) + '</a>') +
      ' <small><time class="timeago" data-toggle="tooltip" data-placement="top" datetime="' + escapeHTML(c.created_at) + '" title="' + escapeHTML(c.created_at) + '"></time></small>' +
      (c.is_pinned ? ' <i class="fa fa-thumb-tack" title="pinned"></i>' : '') +
      // actions: one row of icon buttons on the right (labels as tooltips)
      '<span class="w0bm-c-actions">' +
        (DATA.user && !c.is_deleted && !(ITEM && ITEM.locked && !DATA.is_mod) && $('#cinput') ? '<a class="reply_comment w0bm-cact" href="#" title="Reply" aria-label="Reply"><i class="fa fa-reply"></i></a>' : '') +
        (!c.is_deleted && DATA.is_mod ? '<a class="pin_comment w0bm-cact' + (c.is_pinned ? ' w0bm-cact-on' : '') + '" href="#" title="' + (c.is_pinned ? 'Unpin' : 'Pin') + '" aria-label="' + (c.is_pinned ? 'Unpin' : 'Pin') + '"><i class="fa fa-thumb-tack"></i></a>' : '') +
        (!c.is_deleted && DATA.is_mod ? '<a class="edit_comment w0bm-cact" href="#" title="Edit" aria-label="Edit"><i class="fa fa-pencil"></i></a>' : '') +
        (DATA.user ? '<a class="report_comment w0bm-cact" href="#" title="Report" aria-label="Report"><i class="fa fa-flag"></i></a>' : '') +
        (!c.is_deleted && (DATA.is_mod || (mine && DATA.allow_comment_deletion)) ? '<a class="delete_comment w0bm-cact w0bm-cact-danger" href="#" title="Delete" aria-label="Delete"><i class="fa fa-trash"></i></a>' : '') +
      '</span>';
    if (c.username_color) foot.querySelector('a.w0bm-usercolor')?.style.setProperty('--user-color', c.username_color);
    div.append(body, foot);
    return div;
  }
  function renderComments() {
    if (!commentBox) return;
    commentBox.textContent = '';
    const list = comments.filter((c) => DATA.is_mod || !c.is_deleted || comments.some((x) => x.parent_id === c.id));
    if (!list.length) {
      commentBox.innerHTML = '<div class="panel panel-default nocomments"><div class="panel-body">No comments yet …<br /></div></div>';
    } else {
      const repliedBy = repliedByMap(list);
      list.forEach((c) => commentBox.appendChild(commentPanel(c, repliedBy)));
    }
    const amount = $('#commentAmount');
    if (amount) amount.textContent = String(comments.filter((c) => !c.is_deleted).length);
    refreshTimeago(commentBox);
    initTooltips(commentBox);
  }
  async function loadComments(scrollTo) {
    if (!commentBox || !ITEM) return;
    await loadEmojis();
    const r = await api('/api/comments/' + ITEM.id + '?sort=old', { method: 'GET' });
    if (!r.ok) return;
    comments = (r.data.comments || []).slice().sort((a, b) => (b.is_pinned - a.is_pinned) || (new Date(a.created_at) - new Date(b.created_at)));
    renderComments();
    const target = scrollTo || (location.hash.match(/^#c(\d+)$/) || [])[1];
    if (target) {
      const el = document.getElementById('c' + target);
      if (el) {
        el.classList.add('w0bm-highlight');
        el.scrollIntoView({ block: 'center' });
      }
    } else if (scrollTo === undefined && commentBox.parentElement) {
      // newest at the bottom, like w0bm
    }
  }
  loadComments();

  commentBox?.addEventListener('click', async (e) => {
    const ts = e.target.closest('.comment_clickable_timestamp');
    if (ts) {
      e.preventDefault();
      const m = ts.textContent.match(/(\d{1,2}):(\d{2})/);
      if (player && m) {
        const seek = +m[1] * 60 + +m[2];
        if (!player.duration() || seek <= player.duration()) player.currentTime(seek);
      }
      return;
    }
    const panel = e.target.closest('.panel[data-id]');
    if (!panel) return;
    const id = panel.dataset.id;
    const c = comments.find((x) => String(x.id) === id);
    if (e.target.closest('.reply_comment')) {
      e.preventDefault();
      const ta = $('#cinput');
      const form = ta.form;
      let parent = form.querySelector('input[name="parent_id"]');
      if (!parent) {
        parent = document.createElement('input');
        parent.type = 'hidden';
        parent.name = 'parent_id';
        form.appendChild(parent);
      }
      parent.value = id;
      // like f0ckm: the reply quotes the comment as >>id (a link to it, not an @mention)
      const quote = '>>' + id;
      if (!new RegExp('(^|\\s)' + quote + '(?!\\d)').test(ta.value)) ta.value = quote + ' ' + ta.value;
      ta.focus();
      ta.setSelectionRange(ta.value.length, ta.value.length);
      return;
    }
    if (e.target.closest('.delete_comment')) {
      e.preventDefault();
      let reason = '';
      if (DATA.is_mod && !(c && DATA.user && c.username === DATA.user)) {
        reason = await dialog({ title: 'Delete comment', text: 'Comment ' + id + ' by ' + (c ? c.username : ''), input: { label: 'Reason', placeholder: 'Reason' }, ok: 'Delete' });
        if (reason === null) return;
      } else if (!(await dialog({ title: 'Delete comment', text: 'Delete this comment?', ok: 'Delete' }))) return;
      const r = await api('/api/comments/' + id + '/delete', { body: { reason: reason.trim() } });
      if (r.ok) { flash('success', 'Comment deleted'); loadComments(null); }
      else flash('error', r.msg || 'Error deleting comment');
      return;
    }
    if (e.target.closest('.pin_comment')) {
      e.preventDefault();
      const r = await api('/api/comments/' + id + '/pin', { body: {} });
      if (r.ok) loadComments(null);
      else flash('error', r.msg || 'Error');
      return;
    }
    if (e.target.closest('.report_comment')) {
      e.preventDefault();
      reportComment(id);
      return;
    }
    const edit = e.target.closest('.edit_comment');
    if (edit && c) {
      e.preventDefault();
      const body = panel.querySelector('.panel-body');
      const ta = document.createElement('textarea');
      ta.className = 'form-control';
      ta.value = c.content || '';
      body.replaceWith(ta);
      const actions = panel.querySelector('.w0bm-c-actions');
      const old = actions.innerHTML;
      actions.innerHTML = '<a href="#" class="saveCommentEdit w0bm-cact w0bm-cact-text" title="Save">save</a><a href="#" class="abortCommentEdit w0bm-cact w0bm-cact-text" title="Cancel">cancel</a>';
      actions.querySelector('.abortCommentEdit').addEventListener('click', (ev) => {
        ev.preventDefault();
        ta.replaceWith(body);
        actions.innerHTML = old;
      });
      actions.querySelector('.saveCommentEdit').addEventListener('click', async (ev) => {
        ev.preventDefault();
        const r = await api('/api/comments/' + id + '/edit', { body: { content: ta.value } });
        if (r.ok) { flash('success', 'Comment edited successfully'); loadComments(null); }
        else { flash('error', r.msg || 'Error editing comment'); ta.replaceWith(body); actions.innerHTML = old; }
      });
    }
  });

  // Comment tools of a form: [rb] [reich] [krebs] [glitch] [spoiler] (.w0bm-fmt, formatText) and the
  // emoji box (.emoji-shit opens .emojis-box). Used by the item's comment form and the upload's
  // first comment.
  function commentTools(root, ta) {
    $$('.w0bm-fmt', root).forEach((a) => a.addEventListener('click', (e) => {
      e.preventDefault();
      const tag = a.dataset.fmt;
      const start = ta.selectionStart, end = ta.selectionEnd;
      const sel = ta.value.slice(start, end);
      ta.setRangeText('[' + tag + ']' + sel + '[/' + tag + ']', start, end, 'end');
      if (!sel) ta.setSelectionRange(start + tag.length + 2, start + tag.length + 2);
      ta.focus();
    }));
    const emojiBtn = $('.emoji-shit', root);
    const emojiBox = $('.emojis-box', root);
    if (!emojiBtn || !emojiBox) return;
    emojiBtn.addEventListener('click', async (e) => {
      e.preventDefault();
      const list = await loadEmojis();
      const child = $('.emojis', emojiBox);
      if (!child.childElementCount) {
        const names = Object.keys(list);
        if (!names.length) child.textContent = 'No emojis yet';
        names.forEach((n) => {
          const a = document.createElement('a');
          a.className = 'w0bm_emoji_anchor';
          a.title = ':' + n + ':';
          a.href = '#';
          a.innerHTML = '<img class="comment_emoji_small" loading="lazy" alt="">';
          a.firstChild.src = list[n];
          a.addEventListener('click', (ev) => {
            ev.preventDefault();
            ta.setRangeText(':' + n + ': ', ta.selectionStart, ta.selectionEnd, 'end');
            ta.focus();
          });
          child.appendChild(a);
        });
      }
      const open = !emojiBox.classList.toggle('w0bm-hidden');
      emojiBtn.setAttribute('aria-expanded', String(open));
    });
  }

  const commentForm = $('#commentForm > form');
  if (commentForm) {
    let lastComment = '';
    let capTries = 0;
    const ta = commentForm.querySelector('textarea');
    ta.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); commentForm.requestSubmit(); }
    });
    commentForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      const fd = new FormData(commentForm);
      const sig = fd.get('content') + '|' + (fd.get('parent_id') || '');
      if (!String(fd.get('content') || '').trim()) return;
      if (sig === lastComment) { flash('error', "nope. just don't. seriously... don't."); return; }
      if (player && player.el.tagName === 'VIDEO' && /\b\d{1,2}:\d{2}\b/.test(fd.get('content')) === false && player.currentTime() > 0) {
        fd.set('video_time', String(player.currentTime()));
      }
      const btn = commentForm.querySelector('button[type="submit"]');
      btn.disabled = true;
      const r = await api('/api/comments', { body: fd });
      btn.disabled = false;
      if (r.ok) {
        capTries = 0;
        lastComment = sig;
        ta.value = '';
        ta.blur();
        commentForm.querySelector('input[name="parent_id"]')?.remove();
        commentForm.querySelector('cap-widget')?.remove();
        await loadComments(r.data.comment ? r.data.comment.id : null);
        const sc = $('#comment_container_scrollable');
        if (sc && !(r.data.comment)) sc.scrollTop = sc.scrollHeight;
      } else {
        if (r.data && r.data.captcha_required && window.f0ckCaptchaGate && capTries < 1) {
          capTries++;
          try {
            await window.f0ckCaptchaGate(r.data.captcha_required);
            commentForm.requestSubmit();
            return;
          } catch { /* cancelled */ }
        }
        capTries = 0;
        flash('error', r.msg || 'Error saving comment');
      }
    });
    commentTools(commentForm, ta);
  }

  // ════════════════════════════════════════════════════════════════════════════════════
  // Other pages
  // ════════════════════════════════════════════════════════════════════════════════════

  // Notifications page: read all / one
  $('#w0bm-readall')?.addEventListener('click', async (e) => {
    e.preventDefault();
    const tab = e.currentTarget.dataset.tab || '';
    const r = await api('/api/notifications/read' + (tab ? '?tab=' + encodeURIComponent(tab) : ''), { body: {} });
    if (r.ok) {
      flash('success', 'Marked all messages as read');
      $$('.list-group-item-info').forEach((x) => x.classList.remove('list-group-item-info'));
      // only this tab's messages were marked: the bell keeps the other tabs' count
      setTabCount(tab, 0);
      pollBadges();
    } else flash('error', 'Failed to mark all messages as read');
  });
  const markNotifRead = (el) => {
    if (!el.classList.contains('list-group-item-info')) return;
    el.classList.remove('list-group-item-info');
    navigator.sendBeacon?.('/api/notifications/' + el.dataset.id + '/read?csrf_token=' + encodeURIComponent(csrf()));
    const tab = $('#w0bm-readall')?.dataset.tab;
    if (tab) setTabCount(tab, tabCount(tab) - 1);
    const bell = $('#w0bm-notif-badge');
    if (bell) setBadge(bell, (+bell.textContent || 0) - 1);
  };
  $$('.w0bm-notif a.list-group-item[data-id]').forEach((a) => a.addEventListener('click', () => markNotifRead(a)));
  // Comment messages open in place (<details>): the comment in w0bm's formatting, read once opened.
  // Delegated, so the rows of "older messages" work too.
  $('.w0bm-notif')?.addEventListener('toggle', (e) => {
    const d = e.target;
    if (!d.matches?.('details.w0bm-msg') || !d.open) return;
    markNotifRead(d);
    const text = d.querySelector('.w0bm-msg-text[data-raw]');
    if (text && !text.dataset.done) {
      loadEmojis().then(() => {
        text.innerHTML = renderComment(text.dataset.raw);
        text.dataset.done = '1';
      });
    }
  }, true);
  const moreNotifs = $('#w0bm-more-notifs');
  moreNotifs?.addEventListener('click', async (e) => {
    e.preventDefault();
    const page = +(moreNotifs.dataset.next || 2);
    const tab = moreNotifs.dataset.tab || '';
    const r = await api('/ajax/notifications?page=' + page + (tab ? '&tab=' + encodeURIComponent(tab) : ''), { method: 'GET' });
    if (!r.ok) return;
    $('.w0bm-notif .list-group')?.insertAdjacentHTML('beforeend', r.data.html || '');
    refreshTimeago();
    if (r.data.hasMore) moreNotifs.dataset.next = String(r.data.nextPage || page + 1);
    else moreNotifs.remove();
  });

  // User comments page: server-rendered raw text -> comment markup
  (async () => {
    const list = $$('.w0bm-cmd');
    if (!list.length) return;
    await loadEmojis();
    list.forEach((el) => { el.innerHTML = renderComment(el.textContent); el.classList.add('w0bm-md-done'); });
  })();

  // Tags page: "Load more" (/api/tags renders tag-cards.html of this frontend)
  // /tags: the next tags load by themselves once the end of the list comes into view
  const moreTags = $('#w0bm-more-tags');
  if (moreTags && 'IntersectionObserver' in window) {
    let busy = false;
    const io = new IntersectionObserver(async (entries) => {
      if (!entries.some((en) => en.isIntersecting) || busy) return;
      busy = true;
      moreTags.classList.add('w0bm-more-loading');
      const page = +moreTags.dataset.page || 2;
      const r = await api('/api/tags?ajax=1&page=' + page, { method: 'GET' });
      moreTags.classList.toggle('w0bm-more-loading', false);
      busy = false;
      if (r.ok) $('#categories')?.insertAdjacentHTML('beforeend', r.data.html || '');
      if (r.ok && r.data.hasMore) {
        moreTags.dataset.page = String(page + 1);
        // still in view (short pages): keep going
        io.unobserve(moreTags);
        io.observe(moreTags);
      } else {
        io.disconnect();
        moreTags.hidden = true;
      }
    }, { rootMargin: '0px 0px 600px 0px' });
    io.observe(moreTags);
    onLeave(() => io.disconnect());
  }

  // Markdown of rules/about/terms (admin-written; rendered escaped, then a small Markdown subset)
  function renderMarkdown(src) {
    const esc = (t) => escapeHTML(t).replace(/&#x2F;/g, '/').replace(/&#x3D;/g, '=').replace(/&#x60;/g, '`');
    const inline = (t) => esc(t)
      .replace(/`([^`]+)`/g, '<code>$1</code>')
      .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
      .replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>')
      .replace(/\[([^\]]+)\]\(((?:https?:\/\/|\/|#|mailto:)[^)\s]+)\)/g, '<a href="$2">$1</a>')
      .replace(/(^|[\s(])(https?:\/\/[^\s<]+)/g, '$1<a href="$2" rel="extern noopener noreferrer" target="_blank">$2</a>');
    const out = [];
    let list = null, para = [], code = null;
    const flushPara = () => { if (para.length) { out.push('<p>' + para.map(inline).join('<br>') + '</p>'); para = []; } };
    const flushList = () => { if (list) { out.push('<' + list.tag + '>' + list.items.map((i) => '<li>' + inline(i) + '</li>').join('') + '</' + list.tag + '>'); list = null; } };
    for (const line of String(src).split(/\r?\n/)) {
      if (code !== null) {
        if (/^```/.test(line)) { out.push('<pre>' + esc(code.join('\n')) + '</pre>'); code = null; } else code.push(line);
        continue;
      }
      if (/^```/.test(line)) { flushPara(); flushList(); code = []; continue; }
      let m;
      if ((m = line.match(/^(#{1,6})\s+(.*)$/))) { flushPara(); flushList(); const n = Math.min(6, m[1].length + 3); out.push('<h' + n + '>' + inline(m[2]) + '</h' + n + '>'); continue; }
      if ((m = line.match(/^\s*[-*+]\s+(.*)$/)) || (m = line.match(/^\s*\d+[.)]\s+(.*)$/))) {
        flushPara();
        const tag = /^\s*\d/.test(line) ? 'ol' : 'ul';
        if (!list || list.tag !== tag) { flushList(); list = { tag, items: [] }; }
        list.items.push(m[1]);
        continue;
      }
      if (/^\s*(---|\*\*\*)\s*$/.test(line)) { flushPara(); flushList(); out.push('<hr>'); continue; }
      if ((m = line.match(/^>\s?(.*)$/))) { flushPara(); flushList(); out.push('<blockquote>' + inline(m[1]) + '</blockquote>'); continue; }
      if (!line.trim()) { flushPara(); flushList(); continue; }
      flushList();
      para.push(line);
    }
    if (code !== null) out.push('<pre>' + esc(code.join('\n')) + '</pre>');
    flushPara();
    flushList();
    return out.join('');
  }
  $$('.w0bm-md').forEach((el) => {
    el.innerHTML = renderMarkdown(el.textContent);
    el.classList.add('w0bm-md-done');
  });

  // Subscriptions page: unsubscribe without reload
  $$('form.w0bm-unsub').forEach((f) => f.addEventListener('submit', async (e) => {
    e.preventDefault();
    const r = await api(f.action, { body: new FormData(f) });
    if (r.ok) f.closest('.w0bm-subrow')?.remove();
    else flash('error', r.msg || 'Error');
  }));

  // ── Upload (upload.blade.php) ─────────────────────────────────────────────────────────
  const ulForm = $('#w0bm-upload');
  if (ulForm) {
    const tagsUl = tagsinput($('#tags_upload'));
    const ulComment = $('#ulcomment');
    if (ulComment) commentTools(ulForm, ulComment);
    const fileInput = $('#w0bm-file');
    const dnd = $('#dragndrop');
    const dndText = $('#dragndrop-text');
    const defaultPreview = dndText.innerHTML;
    const progress = $('#w0bm-progress');
    const stats = $('#upload-stats');
    let currentFile = null;
    let xhr = null;
    const humanFileSize = (size) => {
      if (!size) return '0 B';
      const i = Math.floor(Math.log(size) / Math.log(1024));
      return (size / Math.pow(1024, i)).toFixed(2) + ' ' + ['B', 'kB', 'MB', 'GB', 'TB'][i];
    };
    const maxBytes = +ulForm.dataset.maxBytes || 0;
    const reset = () => {
      currentFile = null;
      fileInput.value = '';
      dndText.innerHTML = defaultPreview;
      dnd.classList.remove('w0bm-dnd-has-file');
      if (xhr) { xhr.abort(); xhr = null; }
    };
    const preview = (file) => {
      if (maxBytes && file.size > maxBytes) {
        flash('error', 'File too big. Max ' + humanFileSize(maxBytes));
        return;
      }
      currentFile = file;
      dnd.classList.add('w0bm-dnd-has-file');
      dndText.textContent = '';
      const url = URL.createObjectURL(file);
      let el;
      if (file.type.startsWith('video')) {
        el = document.createElement('video');
        el.id = 'video_preview';
        Object.assign(el, { src: url, autoplay: true, controls: true, loop: true, muted: true });
      } else if (file.type.startsWith('image')) {
        el = document.createElement('img');
        el.id = 'video_preview';
        el.src = url;
      } else if (file.type.startsWith('audio')) {
        el = document.createElement('audio');
        Object.assign(el, { src: url, controls: true });
      }
      if (el) dndText.appendChild(el);
      const info = document.createElement('span');
      info.className = 'upload-info';
      info.append(file.name + ' — ' + humanFileSize(file.size) + ' — ');
      const clear = document.createElement('a');
      clear.id = 'dragndrop-clear';
      clear.className = 'fa fa-times';
      clear.href = '#';
      clear.addEventListener('click', (e) => { e.preventDefault(); e.stopPropagation(); reset(); });
      info.appendChild(clear);
      dndText.appendChild(info);
    };
    fileInput.addEventListener('change', () => { if (fileInput.files[0]) preview(fileInput.files[0]); });
    $('#dragndrop-link').addEventListener('click', (e) => { if (currentFile) e.preventDefault(); });
    ['dragenter', 'dragover'].forEach((ev) => dnd.addEventListener(ev, (e) => { e.preventDefault(); dnd.classList.add('w0bm-dnd-over'); }));
    ['dragleave', 'drop'].forEach((ev) => dnd.addEventListener(ev, (e) => { e.preventDefault(); dnd.classList.remove('w0bm-dnd-over'); }));
    dnd.addEventListener('drop', (e) => { const f = e.dataTransfer?.files?.[0]; if (f) preview(f); });

    const lazor = $('#btn-upload');
    const send = (fd) => new Promise((resolve) => {
      xhr = new XMLHttpRequest();
      xhr.open('POST', '/api/v2/upload');
      xhr.setRequestHeader('X-CSRF-Token', csrf());
      xhr.setRequestHeader('X-Requested-With', 'XMLHttpRequest');
      const started = Date.now();
      // Sent completely: full bar while the server processes the file
      xhr.upload.addEventListener('load', () => {
        progress.style.setProperty('--progress', '1');
        lazor?.style.setProperty('--progress', '1');
        progress.firstElementChild?.setAttribute('aria-valuenow', '100');
        stats.textContent += ' – processing…';
      });
      xhr.upload.addEventListener('progress', (e) => {
        if (!e.lengthComputable) return;
        const frac = Math.min(1, e.loaded / e.total);
        progress.style.setProperty('--progress', frac.toFixed(4));
        // the shoop's laz0r is the visible progress bar (w0bm-fe.css: #btn-upload.w0bm-firing)
        lazor?.style.setProperty('--progress', frac.toFixed(4));
        progress.firstElementChild?.setAttribute('aria-valuenow', String(Math.round(frac * 100)));
        const secs = Math.max(0.001, (Date.now() - started) / 1000);
        stats.textContent = 'Speed: ' + humanFileSize(e.loaded / secs) + '/s Uploaded: ' + humanFileSize(e.loaded) + ' / ' + humanFileSize(e.total);
      });
      xhr.addEventListener('load', () => {
        let data = null;
        try { data = JSON.parse(xhr.responseText); } catch { /* not JSON */ }
        resolve({ status: xhr.status, data });
      });
      xhr.addEventListener('error', () => resolve({ status: 0, data: null }));
      xhr.addEventListener('abort', () => resolve({ status: -1, data: null }));
      xhr.send(fd);
    });

    ulForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      tagsUl.flush();
      const fd = new FormData(ulForm);
      fd.delete('file');
      const url = String(fd.get('url') || '').trim();
      if (!currentFile && !url) { flash('error', 'Select a file first'); return; }
      if (!fd.get('rating')) { flash('error', 'Choose a rating (SFW/NSFW)'); return; }
      const minTags = +ulForm.dataset.minTags || 0;
      if (tagsUl.items().length < minTags) { flash('error', 'Add at least ' + minTags + (minTags === 1 ? ' tag' : ' tags')); return; }
      fd.set('tags', tagsUl.items().join(','));
      if (currentFile && !url) fd.set('file', currentFile, currentFile.name);
      const btn = $('#btn-upload');
      btn.disabled = true;
      btn.style.setProperty('--progress', '0');
      btn.classList.add('w0bm-firing');
      progress.classList.remove('w0bm-hidden');
      let r = await send(fd);
      if (r.status === 403 && r.data && r.data.captcha_required && window.f0ckCaptchaGate) {
        try {
          await window.f0ckCaptchaGate(r.data.captcha_required);
          r = await send(fd);
        } catch { /* cancelled */ }
      }
      btn.disabled = false;
      xhr = null;
      if (r.data && r.data.success) {
        flash('success', r.data.msg || 'Upload successful');
        setTimeout(() => navigate(r.data.redirect || '/pending'), 800);
        return;
      }
      progress.classList.add('w0bm-hidden');
      btn.classList.remove('w0bm-firing');
      if (r.status !== -1) flash('error', (r.data && (r.data.msg || r.data.message)) || (r.status === 413 ? 'File too large' : 'Upload failed'));
    });
  }

  // Settings page: small JSON forms (data-method, JSON or urlencoded)
  $$('form.w0bm-setting').forEach((f) => f.addEventListener('submit', async (e) => {
    e.preventDefault();
    const fd = new FormData(f);
    const out = {};
    const ints = (f.dataset.int || '').split(',').filter(Boolean);
    for (const [k, v] of fd.entries()) if (k !== 'csrf_token') out[k] = ints.includes(k) && /^-?\d+$/.test(v) ? +v : v;
    $$('input[type="checkbox"][name]', f).forEach((c) => { out[c.name] = c.checked; });
    const method = f.dataset.method || 'POST';
    const r = f.dataset.json ? await api(f.action, { method, json: out }) : await api(f.action, { method, body: fd });
    const st = f.querySelector('.w0bm-status');
    if (r.ok) {
      if (st) st.textContent = '✔ saved';
      else flash('success', 'Saved');
      if (f.dataset.reload) setTimeout(() => location.reload(), 400);
    } else {
      flash('error', r.msg || 'Error');
    }
  }));
  $$('form.w0bm-setting input[type="checkbox"][data-autosave], form.w0bm-setting select[data-autosave]').forEach((c) => c.addEventListener('change', () => c.form.requestSubmit()));

  window.w0bm = { api, flash, openModal, closeModal, tagsinput, escapeHTML, timeago, player: () => player, pollBadges: () => pollBadges(), reloadComments: () => loadComments(null) };

  // Hooks for the shared client code (f0ck-core.js / f0ck-frontend.js, loaded after this file by
  // snippets/frontend-scripts.html): its messages show in w0bm's flash box, live events refresh
  // the badges and the comment list. Contract: views/frontends/README.md
  const flashTypes = { error: 'error', success: 'success', info: 'info', warning: 'warning' };
  window.f0ckFrontend = Object.assign(window.f0ckFrontend || {}, {
    itemId: ITEM ? ITEM.id : null,
    flash: (message, type) => flash(flashTypes[type] || 'info', String(message ?? '')),
    notify: () => { pollBadges(); refreshNotifList(); },
    comments: (data) => {
      if (ITEM && data && String(data.item_id) === String(ITEM.id)) loadComments(null);
    },
    dmBadge: () => pollBadges(),
    reconnected: () => pollBadges(),
    motd: (text) => renderMotd(text),
    newItem: (data) => addNewItem(data),
    // passkey.js closes the login dialog when it opens "Login as anonymous"
    closeLogin: () => closeModal($('#login-modal')),
  });

  // On /notifications a new notification appears at the top of the list right away (rows already
  // shown, e.g. loaded "older messages", stay)
  async function refreshNotifList() {
    const list = $('.w0bm-notif #listitems');
    if (!list) return;
    const tab = $('#w0bm-readall')?.dataset.tab || '';
    const r = await api('/ajax/notifications?page=1' + (tab ? '&tab=' + encodeURIComponent(tab) : ''), { method: 'GET' });
    if (!r.ok || !r.data || !r.data.html) return;
    const tmp = document.createElement('div');
    tmp.innerHTML = r.data.html;
    const have = new Set($$('[data-id]', list).map((el) => el.dataset.id));
    const fresh = $$(':scope > [data-id]', tmp).filter((el) => !have.has(el.dataset.id));
    if (!fresh.length) return;
    list.querySelectorAll(':scope > .list-group-item:not([data-id])').forEach((el) => {
      if (/no messages/i.test(el.textContent)) el.hidden = true;
    });
    fresh.reverse().forEach((el) => list.prepend(el));
    refreshTimeago();
  }

  // New uploads appear at the top of the main grid, like in f0ckm: only on the unfiltered first page,
  // only in the visitor's rating mode, after the pinned items, never twice
  function addNewItem(data) {
    const grid = $('.items');
    if (!grid || !data || !data.id) return;
    const path = location.pathname;
    if (!/^\/(p\/1\/?)?$/.test(path) || new URLSearchParams(location.search).get('view') === 'index') return;
    const nsfl = window.f0ckSession?.nsfl_tag_id || 3;
    const rating = data.tag_id === 1 ? 'sfw' : data.tag_id === 2 ? 'nsfw' : (data.tag_id && data.tag_id == nsfl) ? 'nsfl' : 'untagged';
    const mode = Number(DATA.mode ?? 0);
    const wanted = { 0: 'sfw', 1: 'nsfw', 2: 'untagged', 4: 'nsfl' }[mode];
    if (mode !== 3 && wanted && rating !== wanted) return;
    const key = String(data.slug || data.id);
    if (grid.querySelector(`a[href$="/${CSS.escape(key)}"], a[href$="/${data.id}"]`)) return;
    const tile = document.createElement('div');
    tile.className = 'main-item w0bm-mode-' + rating + ' w0bm-new-item';
    const a = document.createElement('a');
    a.href = '/' + key;
    a.title = (data.title || key) + (data.display_name || data.username ? ' – ' + (data.display_name || data.username) : '');
    const img = document.createElement('img');
    img.src = '/t/' + data.id + '.webp';
    img.alt = String(data.id);
    img.loading = 'lazy';
    img.addEventListener('error', () => { img.src = '/s/fe/w0bm/img/small_404.gif'; img.classList.add('nothumb'); });
    a.appendChild(img);
    tile.appendChild(a);
    const pinned = grid.querySelectorAll('.w0bm-pinned');
    if (pinned.length) pinned[pinned.length - 1].after(tile);
    else grid.prepend(tile);
    $('.w0bm-noresults')?.remove();
    requestAnimationFrame(() => requestAnimationFrame(() => tile.classList.remove('w0bm-new-item')));
  }

  // Message of the day (above the comment form): escaped, inline Markdown, sanitized, as in f0ckm
  const motdBox = $('#w0bm-motd');
  function renderMotd(text) {
    if (!motdBox) return;
    const body = motdBox.querySelector('.w0bm-motd-text');
    const motd = String(text ?? '');
    if (window.marked && window.Sanitizer && window.escHTML) body.innerHTML = window.Sanitizer.clean(window.marked.parseInline(window.escHTML(motd)));
    else body.textContent = motd;
    motdBox.hidden = !motd.trim() || motdBox.dataset.show === '0';
  }
  // marked (deferred) and the sanitizer load after this file: render once they're there
  if (motdBox) {
    const first = () => renderMotd(motdBox.querySelector('.w0bm-motd-text').textContent);
    if (document.readyState === 'loading') on(document, 'DOMContentLoaded', first);
    else first();
  }

  // Passkey login (partials/loginform.html): the WebAuthn round trip is f0ck-core.js's
  // f0ckPasskeyLogin, loaded after this file, so it is looked up on click
  $$('.w0bm-passkey-btn').forEach((btn) => btn.addEventListener('click', async () => {
    const box = btn.closest('.w0bm-passkey');
    const err = box.querySelector('.w0bm-passkey-error');
    const label = btn.innerHTML;
    err.hidden = true;
    btn.disabled = true;
    btn.innerHTML = '<i class="fa fa-spinner fa-spin"></i> Waiting for passkey…';
    try {
      await window.f0ckPasskeyLogin();
      location.reload();
    } catch (e) {
      err.textContent = (e && e.message) || 'Passkey login failed.';
      err.hidden = false;
      btn.disabled = false;
      btn.innerHTML = label;
    }
  }));

  // Navbar brand (image + text from the admin panel): window.updateBrandImage / updateBrandText
  // in f0ck-core.js work on the [data-brand-img] / [data-brand-text] marks in partials/navigation.html.

  // item page: the neighbours load in the background, like f0ckm (600 ms after the page shows)
  if (ITEM) {
    const t = setTimeout(() => ['#next', '#prev'].forEach((sel) => {
      const a = $(sel);
      if (visible(a) && navigable(a)) prefetch(a.href);
    }), 600);
    onLeave(() => clearTimeout(t));
  }
  } // boot()

  // ════════════════════════════════════════════════════════════════════════════════════
  // Navigation without reloading (like f0ckm)
  // ════════════════════════════════════════════════════════════════════════════════════
  // Links and GET forms fetch the page and swap the page part of <body> (everything before
  // #w0bm-flash), then boot() sets it up. That covers w0bm's own pages and the shared pages in the
  // w0bm shell (settings, messages, admin, mod, ...): their stylesheets, page scripts and footer
  // extras come and go with them (F0ckPageSwap, f0ck-core.js). A full load instead when: another
  // frontend, someone else logged in (login, logout: the footer data changes), not HTML, or the
  // request fails. Without JavaScript all of this is plain links and forms.
  // Pages that never load in place: session changes, frontend switch, files, the upload page (its
  // own form instead of the quick upload)
  const FULL_LOAD = /^\/(?:logout|login|register|frontend\/|api\/|ajax\/|s\/|b\/|t\/|c\/|a\/|upload\/?$|mime(?:\/|$))/;
  const pathKey = (href) => { const u = new URL(href, location.href); return u.pathname + u.search; };
  let shownPath = pathKey(location.href);

  function navigable(a) {
    if (!a || !a.href) return false;
    if (a.target && a.target !== '_self') return false;
    if (a.hasAttribute('download') || a.closest('[data-no-ajax]')) return false;
    const raw = a.getAttribute('href') || '';
    if (!raw || raw.startsWith('#') || /^(javascript|mailto|tel):/i.test(raw)) return false;
    let u;
    try { u = new URL(a.href, location.href); } catch { return false; }
    if (u.origin !== location.origin) return false;
    // a jump within this page
    if (u.hash && pathKey(u.href) === pathKey(location.href)) return false;
    if (FULL_LOAD.test(u.pathname)) return false;
    // files (media, downloads)
    if (/\.[a-z0-9]{2,5}$/i.test(u.pathname)) return false;
    return true;
  }

  // ── Fetched pages, kept briefly: prefetched neighbours, hovered links ──────────────────
  // Keyed by URL and the page it is requested from (/random picks its tag/user/hall from the
  // Referer). Any change sent to the server (api() with POST/PUT/...) empties it.
  const prefetched = new Map();
  const PREFETCH_TTL = 60000;
  const cacheKey = (url) => new URL(url, location.href).href + ' ' + location.href;
  function fetchPage(url, { signal, prefetch: pf = false } = {}) {
    const headers = { Accept: 'text/html' };
    if (pf) Object.assign(headers, { 'X-Purpose': 'prefetch', Purpose: 'prefetch' });
    return fetch(url, { credentials: 'same-origin', headers, signal, priority: pf ? 'low' : 'high' }).then(async (res) => {
      const type = res.headers.get('content-type') || '';
      if (!/text\/html/i.test(type)) return null;
      return { url: res.url || new URL(url, location.href).href, status: res.status, html: await res.text(), prefetched: pf };
    });
  }
  function prefetch(url) {
    const key = cacheKey(url);
    const hit = prefetched.get(key);
    if (hit && Date.now() - hit.at < PREFETCH_TTL) return;
    const entry = { at: Date.now(), page: fetchPage(url, { prefetch: true }).catch(() => null) };
    prefetched.set(key, entry);
    if (prefetched.size > 12) prefetched.delete(prefetched.keys().next().value);
  }
  function takePrefetched(url) {
    const key = cacheKey(url);
    const hit = prefetched.get(key);
    prefetched.delete(key);
    return hit && Date.now() - hit.at < PREFETCH_TTL ? hit.page : null;
  }

  // Hovering a link for a moment loads its page already (a click follows in most cases)
  let hoverTimer = null;
  window.addEventListener('mouseover', (e) => {
    const a = e.target.closest?.('a[href]');
    if (!a || a === e.relatedTarget?.closest?.('a[href]')) return;
    clearTimeout(hoverTimer);
    if (!navigable(a)) return;
    hoverTimer = setTimeout(() => prefetch(a.href), 120);
  });
  window.addEventListener('mouseout', (e) => {
    const a = e.target.closest?.('a[href]');
    if (a && !a.contains(e.relatedTarget)) clearTimeout(hoverTimer);
  });
  window.addEventListener('touchstart', (e) => {
    const a = e.target.closest?.('a[href]');
    if (navigable(a)) prefetch(a.href);
  }, { passive: true });

  // ── Loading bar (only when a page takes a moment) ──────────────────────────────────────
  const bar = document.createElement('div');
  bar.className = 'w0bm-navbar-loading';
  bar.hidden = true;
  document.body.appendChild(bar);
  let barTimer = null;
  const loading = (onOff) => {
    clearTimeout(barTimer);
    if (onOff) barTimer = setTimeout(() => { bar.hidden = false; }, 150);
    else bar.hidden = true;
  };

  // one of w0bm's pages (its own or a shared one in its shell), for the same visitor
  function isOwnPage(doc) {
    const next = readJSON('w0bm-data', doc);
    const cur = readJSON('w0bm-data') || {};
    return !!(next && doc.getElementById('w0bm-flash') && next.frontend === cur.frontend && (next.user || null) === (cur.user || null));
  }

  // The page part of <body>: the nodes before #w0bm-flash
  function swapBody(doc) {
    const anchor = document.getElementById('w0bm-flash');
    const nextAnchor = doc.getElementById('w0bm-flash');
    const old = [];
    for (const n of document.body.childNodes) { if (n === anchor) break; old.push(n); }
    // media of the page we leave stop (and stop downloading)
    old.forEach((n) => n.querySelectorAll?.('video, audio').forEach((m) => {
      try { m.pause(); m.removeAttribute('src'); m.querySelectorAll('source').forEach((x) => x.remove()); m.load(); } catch { /* gone */ }
    }));
    const frag = document.createDocumentFragment();
    for (const n of Array.from(doc.body.childNodes)) { if (n === nextAnchor) break; frag.appendChild(document.adoptNode(n)); }
    const scripts = Array.from(frag.querySelectorAll('script'));
    old.forEach((n) => n.remove());
    anchor.before(frag);
    document.documentElement.classList.toggle('f0ck-shell', doc.documentElement.classList.contains('f0ck-shell'));
    // flash messages of the new page join the box
    const box = anchor.querySelector('.flash-inner');
    nextAnchor.querySelectorAll('.flash-inner > .alert').forEach((a) => box?.appendChild(document.adoptNode(a)));
    document.body.className = doc.body.className;
    document.title = doc.title;
    const desc = doc.querySelector('meta[name="description"]');
    if (desc) document.querySelector('meta[name="description"]')?.setAttribute('content', desc.getAttribute('content') || '');
    return scripts;
  }

  let navSeq = 0;
  let navAbort = null;
  // opts: replace (no new history entry), pop (back/forward: no entry, scroll restored),
  // keepScroll (the same page again)
  async function navigate(url, opts = {}) {
    const swap = window.F0ckPageSwap;
    if (!swap) { location.assign(url); return; }
    const target = new URL(url, location.href);
    const seq = ++navSeq;
    navAbort?.abort();
    navAbort = new AbortController();
    loading(true);
    let page = null;
    try {
      page = await (takePrefetched(target.href) || null);
      if (!page) page = await fetchPage(target.href, { signal: navAbort.signal });
    } catch (err) {
      if (err && err.name === 'AbortError') return;
      page = null;
    }
    if (seq !== navSeq) return;
    loading(false);
    const doc = page && new DOMParser().parseFromString(page.html, 'text/html');
    if (!doc || !isOwnPage(doc)) { location.assign(page ? page.url : target.href); return; }
    // the new page's stylesheets first (a shared page: f0ck-shell.css, w0bm-shell.css)
    const dropStyles = await swap.styles(doc, 'w0bm-flash');
    if (seq !== navSeq) return;

    // where the page we leave was scrolled to, for "back"
    if (!opts.pop) {
      try { history.replaceState({ ...(history.state || {}), w0bmScroll: window.scrollY }, ''); } catch { /* ignore */ }
    }
    if ('scrollRestoration' in history) history.scrollRestoration = 'manual';
    const final = new URL(page.url);
    if (!final.hash && target.hash) final.hash = target.hash;
    leavePage();
    const scripts = swapBody(doc);
    dropStyles();
    if (opts.replace) history.replaceState({ w0bm: 1 }, '', final.href);
    else if (!opts.pop) history.pushState({ w0bm: 1 }, '', final.href);
    shownPath = pathKey(location.href);
    // the same order as a full load: the page's own scripts, then w0bm.js, then the footer
    await swap.scripts(scripts);
    boot();
    await swap.tail(doc, 'w0bm-flash');
    swap.shown();
    // a prefetched item page didn't mark its notifications read (the server skips that for prefetches)
    const item = readJSON('w0bm-item-data');
    if (page.prefetched && item && item.id && (readJSON('w0bm-data') || {}).user) {
      fetch('/api/notifications/item/' + item.id + '/read', { method: 'POST', credentials: 'same-origin', headers: { 'X-CSRF-Token': (readJSON('w0bm-data') || {}).csrf || '' } })
        .then(() => window.w0bm?.pollBadges()).catch(() => {});
    }
    const saved = opts.pop ? history.state?.w0bmScroll : null;
    if (typeof saved === 'number') window.scrollTo(0, saved);
    else if (final.hash.length > 1 && document.getElementById(decodeURIComponent(final.hash.slice(1)))) document.getElementById(decodeURIComponent(final.hash.slice(1))).scrollIntoView();
    else if (!opts.keepScroll) window.scrollTo(0, 0);
    document.dispatchEvent(new CustomEvent('w0bm:page', { detail: { url: location.href } }));
  }
  // the same page again (a filter changed)
  const reloadPage = () => navigate(location.href, { replace: true, keepScroll: true });

  window.addEventListener('click', (e) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = e.target.closest?.('a[href]');
    if (!navigable(a)) return;
    e.preventDefault();
    navigate(a.href);
  });
  window.addEventListener('submit', (e) => {
    const f = e.target;
    if (e.defaultPrevented || !(f instanceof HTMLFormElement)) return;
    if ((f.getAttribute('method') || 'get').toLowerCase() !== 'get' || (f.target && f.target !== '_self')) return;
    const u = new URL(f.action || location.href, location.href);
    if (u.origin !== location.origin || FULL_LOAD.test(u.pathname)) return;
    let fd;
    try { fd = new FormData(f, e.submitter || undefined); } catch { fd = new FormData(f); }
    u.search = new URLSearchParams(fd).toString();
    e.preventDefault();
    navigate(u.href);
  });
  window.addEventListener('popstate', () => {
    // only the #hash changed: the page stays
    if (pathKey(location.href) === shownPath) return;
    navigate(location.href, { pop: true });
  });

  boot();
})();
