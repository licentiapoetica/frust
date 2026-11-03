/*
 * w0bm2: the w0bm.com layout2 ("njum") frontend for f0ckm.
 * Plain JavaScript rewrite of newscript.js, the Bootstrap 4 bits it used (dropdowns, modals,
 * alerts, popover), bootstrap-tagsinput, jquery.timeago and clipboard.js — no jQuery.
 * Pages work without this file; it adds the AJAX actions, comments and the video extras.
 */
(() => {
  'use strict';

  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

  let DATA = {};
  try { DATA = JSON.parse(($('#w2-data') || {}).textContent || '{}'); } catch (e) { DATA = {}; }
  const csrf = () => DATA.csrf || ($('meta[name="csrf-token"]') || {}).content || '';
  const store = {
    get(k) { try { return localStorage.getItem('w2_' + k); } catch (e) { return null; } },
    set(k, v) { try { localStorage.setItem('w2_' + k, v); } catch (e) { /* private mode */ } },
  };

  function escapeHTML(str) {
    return String(str ?? '').replace(/[&<>"'`]/g, (s) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;', '`': '&#x60;',
    })[s]);
  }

  /** fetch wrapper for the f0ckm APIs: sends the CSRF token, returns parsed JSON. */
  async function api(method, url, body) {
    const headers = { 'X-CSRF-Token': csrf(), 'X-Requested-With': 'XMLHttpRequest', Accept: 'application/json' };
    let payload;
    if (body instanceof FormData) {
      payload = body;
    } else if (body instanceof URLSearchParams) {
      payload = body;
      headers['Content-Type'] = 'application/x-www-form-urlencoded';
    } else if (body !== undefined) {
      payload = JSON.stringify(body);
      headers['Content-Type'] = 'application/json';
    }
    try {
      const res = await fetch(url, { method, headers, body: payload, credentials: 'same-origin' });
      const text = await res.text();
      let json;
      try { json = JSON.parse(text); } catch (e) { json = { success: res.ok, raw: text }; }
      if (!json || typeof json !== 'object') json = { success: res.ok, value: json };
      if (json.success === undefined) json.success = res.ok;
      json._status = res.status;
      return json;
    } catch (e) {
      return { success: false, msg: 'Network error' };
    }
  }
  /** The captcha answer of a form: Cap's token, or the image captcha when Cap isn't available. */
  function capFields(form, target) {
    if (!form) return;
    ['cap-token', 'cap-image-id', 'cap-image-answer'].forEach((n) => {
      const el = form.querySelector(`input[name="${n}"]`);
      if (!el || !el.value) return;
      if (target instanceof URLSearchParams) target.set(n, el.value); else target[n] = el.value;
    });
  }
  // Cap's widget script missing or blocked: fall back to the image captcha
  window.addEventListener('load', () => setTimeout(() => {
    if ($('cap-widget') && !(window.customElements && customElements.get('cap-widget'))) document.documentElement.classList.add('w2-nocap');
  }, 1000));
  const msgOf = (r, fallback) => (r && (r.msg || r.message || (typeof r.error === 'string' && r.error))) || fallback;

  // ── flash messages (partials/flash.blade.php + newscript.js flash()/alertrm()) ──
  function autoRemove(el, index) {
    setTimeout(() => {
      el.classList.add('w2-hide');
      setTimeout(() => el.remove(), 300);
    }, 3000 + index * 1000);
  }
  function flash(type, message) {
    const box = $('#w2-flash');
    if (!box) return;
    if (type === 'error') type = 'danger';
    const el = document.createElement('div');
    el.className = `alert alert-${type} alert-dismissable`;
    el.setAttribute('role', 'alert');
    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'close';
    close.setAttribute('aria-label', 'Close');
    close.innerHTML = '<span aria-hidden="true">&times;</span>';
    close.addEventListener('click', () => el.remove());
    const span = document.createElement('span');
    span.textContent = message;
    el.append(close, span);
    box.appendChild(el);
    autoRemove(el, box.children.length - 1);
  }
  $$('#w2-flash .alert').forEach((el, i) => autoRemove(el, i));

  // ── timeago (jquery.timeago with the Laravel diffForHumans strings) ──
  function timeAgo(date) {
    const secs = Math.round((Date.now() - date.getTime()) / 1000);
    const s = Math.abs(secs);
    const m = s / 60, h = m / 60, d = h / 24, y = d / 365;
    let w;
    if (s < 45) w = `${Math.max(1, s)} seconds`;
    else if (s < 90) w = '1 minute';
    else if (m < 45) w = `${Math.round(m)} minutes`;
    else if (m < 90) w = '1 hour';
    else if (h < 24) w = `${Math.round(h)} hours`;
    else if (h < 42) w = '1 day';
    else if (d < 30) w = `${Math.round(d)} days`;
    else if (d < 45) w = '1 month';
    else if (d < 365) w = `${Math.round(d / 30)} months`;
    else if (y < 1.5) w = '1 year';
    else w = `${Math.round(y)} years`;
    return secs < 0 ? `${w} from now` : `${w} ago`;
  }
  function refreshTimes(root = document) {
    $$('time.timeago[datetime]', root).forEach((t) => {
      const d = new Date(t.getAttribute('datetime'));
      if (isNaN(d)) return;
      t.textContent = timeAgo(d);
      t.title = d.toLocaleString();
    });
  }
  refreshTimes();
  setInterval(refreshTimes, 30000);

  // ── dropdowns (Bootstrap 4 dropdown.js) ──
  function closeDropdowns(except) {
    $$('.dropdown-menu.show').forEach((m) => {
      if (m === except) return;
      m.classList.remove('show');
      const p = m.parentElement;
      if (!p) return;
      p.classList.remove('show');
      const t = p.querySelector('[data-toggle="dropdown"]');
      if (t) t.setAttribute('aria-expanded', 'false');
    });
  }
  $$('[data-toggle="dropdown"]').forEach((toggle) => {
    toggle.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      const menu = toggle.parentElement.querySelector('.dropdown-menu');
      if (!menu) return;
      const open = !menu.classList.contains('show');
      closeDropdowns(menu);
      menu.classList.toggle('show', open);
      toggle.parentElement.classList.toggle('show', open);
      toggle.setAttribute('aria-expanded', open ? 'true' : 'false');
    });
  });
  document.addEventListener('click', (e) => {
    if (!e.target.closest('.dropdown-menu')) closeDropdowns();
    $$('.w2-pop-anchor > .w2-toggle:checked').forEach((cb) => {
      if (!e.target.closest('.w2-pop-anchor')) cb.checked = false;
    });
  });
  $$('.dropdown-menu a').forEach((a) => a.addEventListener('click', () => setTimeout(closeDropdowns, 0)));

  // ── modals: CSS :target opens them; JS adds Escape and closes the ?login=1 one ──
  const modalOpen = () => !!$('.w2-modal:target, .w2-modal.w2-open');
  function closeModal() {
    $$('.w2-modal.w2-open').forEach((m) => m.classList.remove('w2-open'));
    const t = location.hash.length > 1 && document.getElementById(location.hash.slice(1));
    if (t && t.classList.contains('w2-modal')) location.hash = '_';
  }
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    closeDropdowns();
    $$('.w2-pop-anchor > .w2-toggle:checked').forEach((cb) => { cb.checked = false; });
    if (modalOpen()) closeModal();
  });
  // the login modal opened by /login (?login=1): close it in place, links to other modals switch over
  $$('.w2-modal.w2-open .close, .w2-modal.w2-open .w2-modal-backdrop, .w2-modal.w2-open a[href^="#"]').forEach((a) => a.addEventListener('click', (e) => {
    const m = a.closest('.w2-modal');
    m.classList.remove('w2-open');
    if (a.classList.contains('close') || a.classList.contains('w2-modal-backdrop')) {
      e.preventDefault();
      history.replaceState(null, '', location.pathname);
    }
  }));
  // focus the first field of a modal when it opens
  const focusModal = () => {
    const m = $('.w2-modal:target');
    const f = m && m.querySelector('input:not([type="hidden"]):not(.w2-honeypot), textarea, select');
    if (f) setTimeout(() => f.focus({ preventScroll: true }), 50);
  };
  window.addEventListener('hashchange', focusModal);
  focusModal();

  // labels used as buttons (collapses, info popover): Enter/Space
  $$('label[role="button"][for]').forEach((l) => l.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    e.preventDefault();
    const cb = document.getElementById(l.htmlFor);
    if (cb) { cb.checked = !cb.checked; cb.dispatchEvent(new Event('change')); }
  }));

  // ── tags input (bootstrap-tagsinput markup, own code) ──
  class TagsInput {
    constructor(input) {
      this.input = input;
      this.items = [];
      this.listeners = {};
      const wrap = document.createElement('div');
      wrap.className = 'bootstrap-tagsinput';
      this.list = document.createElement('span');
      this.list.className = 'w2-tagsinput-list';
      this.field = document.createElement('input');
      this.field.type = 'text';
      this.field.placeholder = input.placeholder || 'Input tags...';
      this.field.setAttribute('autocomplete', 'off');
      this.field.setAttribute('aria-label', input.placeholder || 'Tags');
      if (input.id) { this.field.id = input.id; input.id += '-hidden'; }
      wrap.append(this.list, this.field);
      input.type = 'hidden';
      input.after(wrap);
      this.wrap = wrap;
      this.suggest = document.createElement('div');
      this.suggest.className = 'w2-suggest';
      this.suggest.hidden = true;
      wrap.after(this.suggest);
      wrap.addEventListener('click', () => this.field.focus());
      this.field.addEventListener('keydown', (e) => this.onKey(e));
      this.field.addEventListener('blur', () => setTimeout(() => { this.commit(); this.suggest.hidden = true; }, 150));
      this.field.addEventListener('input', () => this.loadSuggestions());
      (input.value || '').split(',').map((t) => t.trim()).filter(Boolean).forEach((t) => this.add(t, true));
    }
    on(ev, fn) { (this.listeners[ev] = this.listeners[ev] || []).push(fn); }
    emit(ev, item) {
      let ok = true;
      (this.listeners[ev] || []).forEach((fn) => { if (fn(item) === false) ok = false; });
      return ok;
    }
    sync() { this.input.value = this.items.join(','); }
    add(tag, silent = false) {
      tag = String(tag).trim().replace(/,/g, '');
      if (!tag || this.items.some((t) => t.toLowerCase() === tag.toLowerCase())) return;
      if (!silent && !this.emit('beforeItemAdd', tag)) return;
      this.items.push(tag);
      const span = document.createElement('span');
      span.className = 'tag label label-info';
      span.textContent = tag;
      span.dataset.tag = tag;
      const x = document.createElement('span');
      x.setAttribute('data-role', 'remove');
      x.setAttribute('role', 'button');
      x.setAttribute('aria-label', 'Remove ' + tag);
      x.addEventListener('click', (e) => { e.stopPropagation(); this.remove(tag); });
      span.appendChild(x);
      this.list.appendChild(span);
      this.sync();
      if (!silent) this.emit('itemAdded', tag);
    }
    remove(tag) {
      const i = this.items.findIndex((t) => t.toLowerCase() === String(tag).toLowerCase());
      if (i < 0) return;
      const real = this.items[i];
      this.items.splice(i, 1);
      $$('.tag', this.list).forEach((s) => { if (s.dataset.tag === real) s.remove(); });
      this.sync();
      this.emit('itemRemoved', real);
    }
    removeAll() { [...this.items].forEach((t) => this.remove(t)); }
    commit() {
      if (this.field.value.trim()) this.add(this.field.value);
      this.field.value = '';
    }
    onKey(e) {
      const links = $$('a', this.suggest);
      const active = links.find((l) => l.classList.contains('active'));
      if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && links.length && !this.suggest.hidden) {
        e.preventDefault();
        let i = links.indexOf(active);
        i = e.key === 'ArrowDown' ? Math.min(links.length - 1, i + 1) : Math.max(0, i - 1);
        links.forEach((l) => l.classList.remove('active'));
        links[i].classList.add('active');
      } else if (e.key === 'Enter' || e.key === ',' || (e.key === 'Tab' && this.field.value.trim())) {
        if (e.key === 'Enter' && !this.field.value.trim() && this.input.form) return; // submit the form
        e.preventDefault();
        if (active && !this.suggest.hidden) { this.add(active.dataset.tag); this.field.value = ''; } else this.commit();
        this.suggest.hidden = true;
      } else if (e.key === 'Backspace' && !this.field.value && this.items.length) {
        this.remove(this.items[this.items.length - 1]);
      } else if (e.key === 'Escape') {
        this.suggest.hidden = true;
      }
    }
    loadSuggestions() {
      const q = this.field.value.trim();
      clearTimeout(this.timer);
      if (q.length < 2) { this.suggest.hidden = true; return; }
      this.timer = setTimeout(async () => {
        const r = await api('GET', '/api/v2/tags/suggest?q=' + encodeURIComponent(q));
        const list = (r && (r.suggestions || r.tags || r.items || r.value)) || [];
        this.suggest.innerHTML = '';
        (Array.isArray(list) ? list : []).slice(0, 10).forEach((s) => {
          const name = typeof s === 'string' ? s : (s.tag || s.name || s.normalized);
          if (!name) return;
          const a = document.createElement('a');
          a.href = '#';
          a.dataset.tag = name;
          a.textContent = name + (s.count || s.tagged ? ` (${s.count || s.tagged})` : '');
          a.addEventListener('mousedown', (e) => {
            e.preventDefault();
            this.add(name);
            this.field.value = '';
            this.suggest.hidden = true;
          });
          this.suggest.appendChild(a);
        });
        this.suggest.hidden = !this.suggest.children.length;
      }, 200);
    }
  }
  const tagInputs = new Map();
  $$('input[data-role="tagsinput"]').forEach((el) => tagInputs.set(el.name + ':' + (el.form ? el.form.id : ''), new TagsInput(el)));

  // ── nav: unread messages / notifications (the green "|" in front of the name) ──
  if (DATA.user) {
    (async () => {
      let unread = 0;
      const n = await api('GET', '/api/notifications');
      if (n && Array.isArray(n.notifications)) {
        const c = n.notifications.filter((x) => !(x.is_read || x.read)).length;
        const el = $('#w2-notif-count');
        if (el) el.textContent = c;
        unread += c;
      }
      if (DATA.private_messages && !DATA.is_anon) {
        const d = await api('GET', '/api/dm/unread');
        if (d && d.success) {
          const el = $('#w2-dm-count');
          if (el) el.textContent = d.count || 0;
          unread += Number(d.count) || 0;
        }
      }
      const ind = $('#w2-msg-indicator');
      if (ind && unread > 0) { ind.classList.remove('nomsg'); ind.classList.add('yesmg'); }
    })();
  }

  // search: guests can't use /search, send them to the tag page
  const searchForm = $('#w2-search-form');
  if (searchForm && !DATA.user) {
    searchForm.addEventListener('submit', (e) => {
      e.preventDefault();
      const q = searchForm.querySelector('input[name="tag"]').value.trim();
      if (q) location.href = '/tag/' + encodeURIComponent(q);
    });
  }

  // ── filter modal (partials/filterselect.blade.php): excluded tags ──
  const filterForm = $('#w2-filter-form');
  const filterTi = tagInputs.get('tagname:w2-filter-form');
  if (filterForm && filterTi) {
    let saved = [];
    api('GET', '/api/v2/settings/excluded_tags').then((r) => {
      if (r && r.success && Array.isArray(r.tags)) {
        saved = r.tags.map((t) => t.tag || t.normalized).filter(Boolean);
        saved.forEach((t) => filterTi.add(t, true));
      }
    });
    filterForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      filterTi.commit();
      const now = [...filterTi.items];
      const has = (list, t) => list.some((x) => x.toLowerCase() === t.toLowerCase());
      let ok = true;
      for (const t of now.filter((t) => !has(saved, t))) {
        const r = await api('POST', '/api/v2/settings/excluded_tags', { tagname: t });
        if (!r.success) { ok = false; flash('error', `${t}: ${msgOf(r, 'Error updating filter')}`); filterTi.remove(t); }
      }
      for (const t of saved.filter((t) => !has(now, t))) {
        const r = await api('DELETE', '/api/v2/settings/excluded_tags/' + encodeURIComponent(t));
        if (!r.success) { ok = false; flash('error', `${t}: ${msgOf(r, 'Error updating filter')}`); }
      }
      saved = [...filterTi.items];
      if (ok) {
        flash('success', 'Filter successfully updated');
        closeModal();
      }
    });
  }

  // ── scroll helpers (commentform arrows) ──
  const toTop = $('.scrollToTop');
  if (toTop) {
    toTop.addEventListener('click', (e) => { e.preventDefault(); window.scrollTo({ top: 0, behavior: 'smooth' }); });
    const upd = () => toTop.classList.toggle('w2-visible', window.scrollY > 0);
    window.addEventListener('scroll', upd, { passive: true });
    upd();
  }
  $$('.w2-bottom-arrow').forEach((a) => a.addEventListener('click', (e) => {
    e.preventDefault();
    window.scrollTo({ top: document.body.scrollHeight, behavior: 'smooth' });
  }));

  // ── the item page ──
  const itemRoot = $('#w2-item');
  if (itemRoot) initItem(itemRoot);

  function initItem(root) {
    const itemId = root.dataset.itemId;
    const videoElem = $('#video');
    const image = $('#w2-image');
    const album = $('#w2-album');

    // volume memory (newscript.js)
    if (videoElem) {
      const v = parseFloat(store.get('volume'));
      videoElem.volume = isFinite(v) ? Math.min(1, Math.max(0, v)) : 0.3;
      videoElem.addEventListener('volumechange', () => store.set('volume', String(videoElem.volume)));
      if (videoElem.autoplay && videoElem.paused) {
        const p = videoElem.play();
        if (p && p.catch) p.catch(() => {});
      }
    }

    // flash: the sandboxed <f0ck-flash> player from flash-embed.js
    const flashStage = $('#w2-flash-stage');
    if (flashStage && window.customElements && customElements.get('f0ck-flash')) {
      const el = document.createElement('f0ck-flash');
      flashStage.appendChild(el);
      flashStage.classList.add('is-loaded');
      el.load({ url: flashStage.dataset.swf }).catch(() => flash('error', 'Could not load the Flash movie'));
    }

    // album: thumbnails switch the stage image
    if (album) {
      const stage = $('#w2-album-stage');
      $$('.w2-album-thumb', album).forEach((a) => a.addEventListener('click', (e) => {
        if (!(a.dataset.mime || '').startsWith('image')) return;
        e.preventDefault();
        $$('.w2-album-thumb', album).forEach((x) => x.classList.remove('active'));
        a.classList.add('active');
        image.src = a.dataset.src;
        stage.href = a.dataset.src;
      }));
    }

    // background canvas (#bg): the blurred copy of the video
    const canvas = $('#bg');
    let background = store.get('background') !== 'false' && root.dataset.showBg !== '0';
    let ctx = null, cw = 0, ch = 0;
    if (canvas && canvas.getContext) {
      ctx = canvas.getContext('2d');
      const resize = () => { cw = canvas.width = canvas.clientWidth | 0; ch = canvas.height = canvas.clientHeight | 0; };
      resize();
      window.addEventListener('resize', () => { resize(); drawOnce(); });
    }
    const isVideo = videoElem && videoElem.tagName === 'VIDEO';
    function drawOnce() {
      if (!ctx || !background) return;
      try {
        if (isVideo && videoElem.readyState >= 2) ctx.drawImage(videoElem, 0, 0, cw, ch);
        else if (image && image.complete && image.naturalWidth) ctx.drawImage(image, 0, 0, cw, ch);
      } catch (e) { /* media not drawable */ }
    }
    function animationLoop() {
      if (!ctx || !isVideo || videoElem.paused || videoElem.ended || !background) return;
      drawOnce();
      requestAnimationFrame(animationLoop);
    }
    if (isVideo) {
      videoElem.addEventListener('play', animationLoop);
      videoElem.addEventListener('loadeddata', drawOnce);
      videoElem.addEventListener('seeked', drawOnce);
      if (!videoElem.paused) animationLoop();
    }
    if (image) { if (image.complete) drawOnce(); image.addEventListener('load', drawOnce); }
    if (canvas && !background) canvas.classList.add('fader-out');
    const toggleBg = $('#togglebg');
    if (toggleBg && canvas) toggleBg.addEventListener('click', (e) => {
      e.preventDefault();
      background = !background;
      store.set('background', String(background));
      canvas.classList.toggle('fader-in', background);
      canvas.classList.toggle('fader-out', !background);
      if (background) { drawOnce(); animationLoop(); }
    });

    // floating video (newscript.js "Floating Video": .video gets .stuck below the fold)
    const wrap = $('.video-wrap');
    const floater = isVideo ? videoElem : null;
    if (wrap && floater) {
      const onScroll = () => {
        const below = wrap.getBoundingClientRect().bottom < 0;
        floater.classList.toggle('stuck', below && !floater.paused);
      };
      window.addEventListener('scroll', onScroll, { passive: true });
      floater.addEventListener('pause', onScroll);
      floater.addEventListener('play', onScroll);
    }

    // prev / next / random / fav keys (newscript.js key bindings)
    const go = (id) => {
      const a = document.getElementById(id);
      if (a && !a.classList.contains('w2-hidden-nav')) a.click();
    };
    document.addEventListener('keydown', (e) => {
      if (e.defaultPrevented || e.ctrlKey || e.altKey || e.shiftKey || e.metaKey) return;
      if (/^(input|textarea|select)$/i.test(e.target.nodeName) || e.target.isContentEditable || modalOpen()) return;
      const k = e.key.toLowerCase();
      if (k === 'arrowright' || k === 'd' || k === 'l') go('next');
      else if (k === 'arrowleft' || k === 'a' || k === 'h') go('prev');
      else if (k === 'r') { const r = $('#w2-context'); if (r) r.click(); }
      else if (k === 'f') { const f = $('#fav'); if (f) f.click(); }
    });
    // the mouse wheel over the side columns / the video flips through the items
    let wheelLock = 0;
    $$('.scrollable').forEach((el) => el.addEventListener('wheel', (e) => {
      if (e.ctrlKey || e.altKey || e.shiftKey || modalOpen() || el.classList.contains('stuck')) return;
      e.preventDefault();
      if (Math.abs(e.deltaY) < 4 || Date.now() < wheelLock) return;
      wheelLock = Date.now() + 600;
      go(e.deltaY > 0 ? 'next' : 'prev');
    }, { passive: false }));

    // favorite
    const favForm = $('#w2-fav-form');
    if (favForm) favForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      const action = favForm.elements.action.value;
      const r = await api('POST', '/api/v2/togglefav', new URLSearchParams({ postid: itemId, action }));
      if (!r.success) { flash('error', msgOf(r, 'Error saving favorite')); return; }
      const faved = action === 'add';
      favForm.elements.action.value = faved ? 'remove' : 'add';
      const btn = $('#fav');
      const icon = btn.querySelector('i');
      icon.classList.toggle('fa-heart', faved);
      icon.classList.toggle('fa-heart-o', !faved);
      btn.classList.toggle('bg-toggle', !faved);
      btn.setAttribute('aria-pressed', faved ? 'true' : 'false');
    });

    // comment subscription (f0ckm)
    const subForm = $('#w2-sub-form');
    if (subForm) subForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      const wanted = subForm.elements.subscribed.value;
      const r = await api('POST', `/api/subscribe/${itemId}`, { subscribed: wanted });
      if (!r.success) { flash('error', msgOf(r, 'Error')); return; }
      const on = r.subscribed !== undefined ? !!r.subscribed : wanted === '1';
      subForm.elements.subscribed.value = on ? '0' : '1';
      const btn = $('#w2-subscribe');
      const icon = btn.querySelector('i');
      icon.classList.toggle('fa-bell', on);
      icon.classList.toggle('fa-bell-o', !on);
      btn.title = on ? 'Unsubscribe from the comments' : 'Subscribe to the comments';
      btn.setAttribute('aria-pressed', on ? 'true' : 'false');
      flash('info', on ? 'Subscribed to the comments' : 'Unsubscribed');
    });

    // copy link (clipboard.js)
    $$('.copylink').forEach((a) => a.addEventListener('click', async (e) => {
      e.preventDefault();
      const text = a.dataset.clipboardText || location.href;
      try {
        await navigator.clipboard.writeText(text);
      } catch (err) {
        const ta = document.createElement('textarea');
        ta.value = text;
        document.body.appendChild(ta);
        ta.select();
        try { document.execCommand('copy'); } catch (x) { /* nothing left to try */ }
        ta.remove();
      }
      a.classList.add('w2-copied');
      setTimeout(() => a.classList.remove('w2-copied'), 1500);
    }));

    // report (partials/report.blade.php)
    const reportForm = $('#webmreport');
    if (reportForm) reportForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      const body = new URLSearchParams();
      body.set('item_id', itemId);
      const cats = Array.from($('#reportReasons').selectedOptions).map((o) => o.value);
      const reason = $('#reportText').value.trim();
      if (!reason && !cats.length) { flash('error', 'Please select at least one reason or provide a description.'); return; }
      if (cats.length) body.set('categories', cats.join(','));
      if (reason) body.set('reason', reason);
      capFields(reportForm, body);
      const r = await api('POST', '/api/v2/report', body);
      if (r.success) {
        flash('success', 'Report sent. Thanks!');
        reportForm.reset();
        closeModal();
      } else flash('error', msgOf(r, 'Error sending report'));
    });

    // title (partials/frontendedit.blade.php)
    const editForm = $('#webmedit');
    if (editForm) editForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      const title = $('#videotitleedit').value.trim();
      const r = await api('PATCH', `/api/v2/items/${itemId}/title`, { title });
      if (!r.success) { flash('error', msgOf(r, 'Error updating video')); return; }
      flash('success', 'Video successfully updated');
      const b = $('.video-titlebar b');
      if (b) b.innerHTML = title ? escapeHTML(title) : '<u>No Title</u>';
      closeModal();
    });

    // tags (partials/tags.blade.php)
    const tagForm = $('#w2-tag-form');
    const tagDisplay = $('#tag-display');
    const canDelete = tagDisplay && tagDisplay.dataset.canDelete === '1';
    const currentTags = () => (tagDisplay ? $$('[data-tag]', tagDisplay).map((s) => s.dataset.tag) : []);
    function renderTags(tags) {
      if (!tagDisplay) return;
      tagDisplay.innerHTML = '';
      if (!tags.length) { tagDisplay.textContent = 'No tags yet ...'; return; }
      tags.forEach((t) => {
        const name = t.tag || t.name || '';
        const norm = (t.normalized || name).toLowerCase();
        const bad = norm === 'nsfw' || norm === 'nsfl';
        const span = document.createElement('span');
        span.className = 'badge ' + (norm === 'sfw' ? 'badge-sfw' : bad ? 'badge-nsfw' : 'badge-secondary');
        span.dataset.tag = name;
        span.innerHTML = `<a href="/tag/${encodeURIComponent(norm)}" class="badge${bad ? ' nsfw-text' : ''}">${escapeHTML(name)}</a>` +
          (canDelete ? '<a class="delete-tag badge" href="#" title="Delete tag"><i class="fa fa-times"></i></a>' : '');
        tagDisplay.append(span, document.createTextNode(' '));
      });
    }
    if (tagForm) {
      const ti = tagInputs.get('tagname:w2-tag-form');
      if (ti) ti.on('beforeItemAdd', (tag) => {
        if (currentTags().some((t) => t.toLowerCase() === tag.toLowerCase())) { flash('info', 'Tag already exists'); return false; }
        return true;
      });
      tagForm.addEventListener('submit', async (e) => {
        e.preventDefault();
        if (ti) ti.commit();
        const tags = ti ? [...ti.items] : [];
        if (!tags.length) { flash('error', 'No tags specified'); return; }
        let last = null;
        for (const t of tags) {
          const r = await api('POST', `/api/v2/tags/${itemId}`, { tagname: t });
          if (!r.success) flash('error', `${t}: ${msgOf(r, 'Error saving tag(s)')}`);
          if (Array.isArray(r.tags)) last = r.tags;
        }
        if (last) renderTags(last);
        if (ti) ti.removeAll();
      });
    }
    if (tagDisplay) tagDisplay.addEventListener('click', async (e) => {
      const del = e.target.closest('.delete-tag');
      if (!del) return;
      e.preventDefault();
      const holder = del.closest('[data-tag]');
      if (!confirm('Do you really want to delete this tag?')) return;
      const r = await api('DELETE', `/api/v2/tags/${itemId}/${encodeURIComponent(holder.dataset.tag)}`);
      if (r.success) {
        flash('success', 'Tag successfully deleted');
        if (Array.isArray(r.tags)) renderTags(r.tags); else holder.remove();
      } else flash('error', msgOf(r, 'Error deleting tag'));
    });

    // delete the upload (mods)
    const delBtn = $('#w2-delete-video');
    if (delBtn) delBtn.addEventListener('click', async (e) => {
      e.preventDefault();
      let reason;
      do {
        reason = prompt(`Reason for deleting ${root.dataset.itemKey} by ${root.dataset.uploader}`);
        if (reason === null) return;
        reason = reason.trim();
      } while (!reason);
      const r = await api('POST', '/api/v2/admin/deletepost', { postid: itemId, reason });
      if (r.success) {
        flash('success', 'Video deleted. Redirect in 3 seconds...');
        setTimeout(() => { location.href = '/'; }, 3000);
      } else flash('error', msgOf(r, 'Error deleting video'));
    });

    const commentsBox = $('#w2-comments');
    if (DATA.enable_comments && commentsBox) initComments(commentsBox, itemId);
  }

  // ── comments (partials/comments.blade.php, comment.blade.php, commentform.blade.php) ──
  const emojiMap = {};
  function renderContent(raw) {
    const html = escapeHTML(raw || '').split('\n').map((line) => {
      let l = line
        .replace(/(https?:\/\/[^\s<]+[^\s<.,;:!?)\]'"])/g, (u) => `<a href="${u}" rel="noopener nofollow ugc" target="_blank">${u}</a>`)
        .replace(/(^|\s)(\/c\/[\w-]+\.(webp|png|jpe?g|gif|avif))(#\w+)?(?=\s|$)/gi, (m, p, src) => `${p}<a href="${src}" target="_blank" rel="noopener"><img class="comment_image" src="${src}" loading="lazy" alt=""></a>`)
        .replace(/(^|\s)(\/c\/[\w-]+\.(webm|mp4|mov|ogg|mp3|opus|m4a))(#\w+)?(?=\s|$)/gi, (m, p, src) => `${p}<video class="comment_image" src="${src}" controls loop preload="metadata"></video>`)
        .replace(/(^|[^\w;])&gt;&gt;(\d+)/g, (m, p, id) => `${p}<a href="#c${id}" class="comment-context-link">&gt;&gt;${id}</a>`)
        .replace(/:([a-z0-9_-]+):/gi, (m, name) => (emojiMap[name] ? `<img class="comment_emoji" src="${escapeHTML(emojiMap[name])}" alt=":${name}:" title=":${name}:">` : m));
      if (/^&gt;(?!&gt;\d)/.test(line)) l = `<span class="greentext">${l}</span>`;
      return l;
    }).join('\n');
    return html
      .replace(/\[spoiler\]([\s\S]*?)\[\/spoiler\]/gi, '<span class="spoiler">$1</span>')
      .replace(/\[rb\]([\s\S]*?)\[\/rb\]/gi, '<span class="rainbow">$1</span>')
      .replace(/\[krebs\]([\s\S]*?)\[\/krebs\]/gi, '<span class="krebs">$1</span>')
      .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
      .replace(/`([^`\n]+)`/g, '<code>$1</code>');
  }

  function initComments(box, itemId) {
    const isMod = box.dataset.mod === '1';
    const me = (box.dataset.user || '').toLowerCase();
    const count = $('#w2-comment-count');
    const form = $('#commentForm > form');
    const ta = $('#cinput');
    let comments = [];

    function insertText(before, after) {
      if (!ta) return;
      const s = ta.selectionStart, en = ta.selectionEnd, v = ta.value;
      const str = v.substring(0, s) + before + v.substring(s, en);
      ta.value = str + after + v.substring(en);
      ta.focus();
      ta.setSelectionRange(str.length, str.length);
    }

    api('GET', '/api/v2/emojis').then((r) => {
      const list = (r && Array.isArray(r.emojis)) ? r.emojis : [];
      const picker = $('#w2-emojis');
      list.forEach((em) => {
        const name = em.name || em.shortcode;
        const url = em.url || em.src;
        if (!name || !url) return;
        emojiMap[name] = url;
        if (!picker) return;
        const a = document.createElement('a');
        a.className = 'w0bm_emoji_anchor';
        a.href = '#';
        a.title = `:${name}:`;
        const img = document.createElement('img');
        img.className = 'comment_emoji_small';
        img.loading = 'lazy';
        img.src = url;
        img.alt = `:${name}:`;
        a.appendChild(img);
        a.addEventListener('click', (e) => { e.preventDefault(); insertText(`:${name}:`, ''); });
        picker.appendChild(a);
      });
      if (list.length && comments.length) render();
    });

    function commentHTML(c) {
      const name = c.is_anon
        ? (c.display_name || 'anonymous') + (c.anon_short_fingerprint ? ` (${c.anon_short_fingerprint})` : '')
        : (c.display_name || c.username || 'unknown');
      const own = me && (c.username || '').toLowerCase() === me;
      const okColor = /^#[0-9a-f]{3,8}$/i.test(c.username_color || '');
      const color = okColor ? ` style="--user-color: ${c.username_color}"` : '';
      const files = (c.files || []).map((f) => {
        const src = f.url || f.path || f.src || (f.dest ? '/c/' + f.dest : '');
        if (!src || (f.dest && (c.content || '').includes(f.dest))) return '';
        const mime = f.mime || '';
        const s = escapeHTML(src);
        if (mime.startsWith('video')) return `<video src="${s}" controls loop preload="metadata"></video>`;
        if (mime.startsWith('image')) return `<a href="${s}" target="_blank" rel="noopener"><img src="${s}" loading="lazy" alt=""></a>`;
        return `<a href="${s}" target="_blank" rel="noopener">${escapeHTML(f.original_filename || f.original_name || f.filename || 'file')}</a>`;
      }).join('');
      const tools = [];
      if (!c.is_deleted && DATA.user) tools.push('<a class="w2-reply" href="#" data-act="reply">[reply]</a>');
      if (!c.is_deleted && (isMod || own)) tools.push('<a class="delete_comment" href="#" data-act="delete">[del]</a>');
      if (!c.is_deleted && (isMod || own)) tools.push('<a class="edit_comment" href="#" data-act="edit">[edit]</a>');
      const author = c.is_anon
        ? `<span class="user_color">${escapeHTML(name)}</span>`
        : `<a class="user_color${okColor ? ' w2-colored' : ''}"${color} href="/user/${encodeURIComponent((c.username || '').toLowerCase())}">${escapeHTML(name)}</a>`;
      return `<div class="panel ${c.is_deleted ? 'panel-danger' : ''} video-comment-wrapper${c.parent_id ? ' w2-reply-comment' : ''}" id="c${c.id}" data-id="${c.id}">
    <div class="umrandung_name">
      <div class="comment_header">${author} <time class="timeago" datetime="${escapeHTML(c.created_at)}"></time> No. <a href="#c${c.id}">${c.id}</a>${c.is_pinned ? ' <i class="fa fa-thumb-tack" title="Pinned"></i>' : ''}${c.parent_id ? ` <a href="#c${c.parent_id}" class="comment-context-link">&gt;&gt;${c.parent_id}</a>` : ''}
        <div class="dingz">${tools.join(' ')}</div>
      </div>
    </div>
    <div class="comment panel-body video-comment-body">${c.is_deleted ? '<i>[deleted]</i>' : renderContent(c.content)}${files ? `<div class="comment-files">${files}</div>` : ''}</div>
</div>`;
    }

    function render() {
      if (count) count.textContent = comments.filter((c) => !c.is_deleted).length;
      if (!comments.length) {
        box.innerHTML = '<div class="panel panel-default nocomments"><div class="panel-body">No comments yet! Be the first one to comment!<br /></div></div>';
        return;
      }
      box.innerHTML = comments.map(commentHTML).join('');
      refreshTimes(box);
    }
    async function load() {
      const r = await api('GET', `/api/comments/${itemId}`);
      if (!r.success) return;
      comments = (r.comments || []).slice();
      // the API sends the newest first; w0bm listed them oldest first
      comments.sort((a, b) => (new Date(a.created_at) - new Date(b.created_at)) || (a.id - b.id));
      render();
    }
    load().then(() => {
      if (!/^#c\d+$/.test(location.hash)) return;
      const toggle = $('#w2-comments-toggle');
      if (toggle) toggle.checked = true;
      const t = document.getElementById(location.hash.slice(1));
      if (t) t.scrollIntoView();
    });

    box.addEventListener('click', async (e) => {
      const ctx = e.target.closest('.comment-context-link');
      if (ctx) { const t = document.getElementById(ctx.getAttribute('href').slice(1)); if (t) { e.preventDefault(); t.scrollIntoView({ behavior: 'smooth' }); history.replaceState(null, '', ctx.getAttribute('href')); } return; }
      const a = e.target.closest('[data-act]');
      if (!a) return;
      e.preventDefault();
      const wrap = a.closest('[data-id]');
      const id = wrap.dataset.id;
      const c = comments.find((x) => String(x.id) === id);
      if (a.dataset.act === 'reply') {
        if (form) { form.dataset.parent = id; insertText(`>>${id}\n`, ''); }
      } else if (a.dataset.act === 'delete') {
        if (!confirm(`Delete comment ${id}?`)) return;
        const r = await api('POST', `/api/comments/${id}/delete`, {});
        if (r.success) { flash('success', 'Comment deleted'); load(); } else flash('error', msgOf(r, 'Error deleting comment'));
      } else if (a.dataset.act === 'edit') {
        if (wrap.querySelector('.w2-comment-edit')) return;
        const bodyEl = wrap.querySelector('.video-comment-body');
        const area = document.createElement('textarea');
        area.className = 'form-control w2-comment-edit';
        area.value = (c && c.content) || '';
        const tools = wrap.querySelector('.dingz');
        const old = tools.innerHTML;
        tools.innerHTML = '<a href="#" class="saveCommentEdit">[save]</a> <a href="#" class="abortCommentEdit">[abort]</a>';
        bodyEl.replaceWith(area);
        area.focus();
        tools.querySelector('.abortCommentEdit').addEventListener('click', (ev) => {
          ev.preventDefault();
          ev.stopPropagation();
          area.replaceWith(bodyEl);
          tools.innerHTML = old;
        });
        tools.querySelector('.saveCommentEdit').addEventListener('click', async (ev) => {
          ev.preventDefault();
          ev.stopPropagation();
          const r = await api('POST', `/api/comments/${id}/edit`, { content: area.value });
          if (r.success) { flash('success', 'Comment edited successfully'); load(); } else flash('error', msgOf(r, 'Error editing comment'));
        });
      }
    });

    $$('.textModifiers [data-format]').forEach((a) => a.addEventListener('click', (e) => {
      e.preventDefault();
      insertText(`[${a.dataset.format}]`, `[/${a.dataset.format}]`);
    }));

    if (form && ta) {
      let last = '';
      ta.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); form.requestSubmit(); }
      });
      form.addEventListener('submit', async (e) => {
        e.preventDefault();
        const content = ta.value;
        if (!content.trim()) return;
        if (content === last) { flash('info', "nope. just don't. seriously... don't."); return; }
        const body = { item_id: itemId, content };
        if (form.dataset.parent && content.includes(`>>${form.dataset.parent}`)) body.parent_id = form.dataset.parent;
        capFields(form, body);
        const r = await api('POST', '/api/comments', body);
        if (!r.success) { flash('error', msgOf(r, 'Error saving comment')); return; }
        last = content;
        ta.value = '';
        delete form.dataset.parent;
        ta.blur();
        await load();
        window.scrollTo({ top: document.body.scrollHeight, behavior: 'smooth' });
      });
    }
  }

  // missing thumbnails get w0bm's small_404.gif (main.blade.php: img.nothumb)
  const noThumb = (img) => {
    if (img.dataset.w2Fallback) return;
    img.dataset.w2Fallback = '1';
    img.classList.add('nothumb');
    img.src = '/s/fe/w0bm2/img/small_404.gif';
  };
  $$('.main-item img, img.w2-thumb').forEach((img) => {
    if (img.complete && !img.naturalWidth) noThumb(img);
    else img.addEventListener('error', () => noThumb(img));
  });

  // subscriptions page: unsubscribe
  $$('.w2-unsub').forEach((f) => f.addEventListener('submit', async (e) => {
    e.preventDefault();
    const r = await api('POST', f.action, { subscribed: '0' });
    if (r.success && !r.subscribed) f.closest('.main-item').remove();
    else if (!r.success) flash('error', msgOf(r, 'Error'));
  }));

  // server-rendered comment text (user comment pages): render the markup like the item page does
  $$('.w2-raw').forEach((el) => { el.innerHTML = renderContent(el.textContent); });

  // notifications: "mark all as read"
  const readAll = $('#w2-readall');
  if (readAll) readAll.addEventListener('submit', async (e) => {
    e.preventDefault();
    const r = await api('POST', '/api/notifications/read', {});
    if (r.success) { flash('success', 'Marked all notifications as read'); $$('.notif-item.unread').forEach((n) => n.classList.remove('unread')); }
    else flash('error', msgOf(r, 'Failed to mark all notifications as read'));
  });

  window.W2 = { api, flash, escapeHTML, refreshTimes, closeModal, TagsInput, tagInputs, data: DATA };
})();
