// f0ckyou frontend: the extras on top of pages that work without JavaScript.
//  - live actions: the plain forms marked .fy-ajax (comments, replies, likes, subscribe, rating,
//    tags, report, delete, mark as read) are sent with fetch instead; the parts of the page they
//    change (data-fy-refresh → [data-fy-live] regions) are swapped in from a fresh render of the
//    page, so nothing reloads and the markup stays the server's. Live comments use the same refresh.
//  - lists keep going: the next page is loaded into the list ([data-fy-feed]) when the page links
//    scroll into view
//  - the f0ckFrontend hooks for the shared live events (views/frontends/README.md), the
//    notification badge
//  - menus (<details>): one open at a time, closed by an outside click or Escape
//  - keys: "/" search; on a f0ck ← → (or Shift+P / Shift+N) previous / next, r random, k play /
//    pause, j / l back / forward 10 s, m mute, f full screen
//  - custom emojis :name: in comments as pictures, and the emoji picker
//  - Flash items: starts the Ruffle player; the player volume is remembered
//  - moderators: the × on a tag removes it
//  - pages change in place: a click on a link of the site (or a search) fetches the page and swaps
//    in its column, masthead and guide, with history (back / forward) and a progress bar; the parts
//    of a page are set up again for every page shown (setupPage) and taken down when it goes
//    (onLeave). Pages that need their own scripts or the shared pages' shell load as usual.
(() => {
  'use strict';
  let page = {};
  const readPage = () => {
    try { page = JSON.parse(document.getElementById('fy-data')?.textContent || '{}') || {}; } catch { page = {}; }
  };
  readPage();
  // what a page sets up and has to take down when the next one is shown
  let leaving = [];
  const onLeave = (fn) => { leaving.push(fn); };
  const csrf = () => document.querySelector('meta[name="csrf-token"]')?.content || '';
  const parse = html => new DOMParser().parseFromString(html, 'text/html');

  // ── snackbar ──
  const flashBox = () => document.getElementById('fy-flash');
  const flash = (message, type) => {
    const box = flashBox();
    if (!box) return;
    const el = document.createElement('div');
    el.className = `fy-toast fy-toast-${type || 'info'}`;
    el.textContent = String(message ?? '');
    box.appendChild(el);
    setTimeout(() => el.remove(), 5000);
  };
  // the message of a form post without JavaScript (snippets/flash.html) goes too
  flashBox()?.querySelectorAll('.fy-toast').forEach(el => setTimeout(() => el.remove(), 6000));

  // ── custom emojis :name: → pictures in comment texts (the server sends them as text). Only text
  //    nodes are touched, so links and attributes stay as they are (f0ckLoadEmojis: f0ck-core.js) ──
  const EMOJI = /:([a-z0-9_-]+):/g;
  const HAS_EMOJI = /:[a-z0-9_-]+:/; // .test() on the /g one would keep its lastIndex
  const emojify = async (root) => {
    if (!root || !window.f0ckLoadEmojis) return;
    const bodies = root.matches?.('.fy-comment-body') ? [root] : [...root.querySelectorAll('.fy-comment-body')];
    if (!bodies.length) return;
    const map = await window.f0ckLoadEmojis();
    if (!map || !Object.keys(map).length) return;
    bodies.forEach(body => {
      const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
      const nodes = [];
      while (walker.nextNode()) if (HAS_EMOJI.test(walker.currentNode.nodeValue)) nodes.push(walker.currentNode);
      nodes.forEach(node => {
        const text = node.nodeValue;
        const frag = document.createDocumentFragment();
        let last = 0;
        text.replace(EMOJI, (m, name, at) => {
          if (!map[name]) return m;
          frag.append(text.slice(last, at));
          // sticker packs bring video stickers (.webm): those play as a muted loop, as in f0ckm
          let img;
          if (/\.webm(?:$|[?#])/i.test(map[name])) {
            img = document.createElement('video');
            img.autoplay = img.loop = img.muted = img.playsInline = true;
            img.title = m;
          } else {
            img = document.createElement('img');
            img.alt = img.title = m;
            img.loading = 'lazy';
          }
          img.className = 'f0ck-emoji';
          img.src = map[name];
          frag.append(img);
          last = at + m.length;
          return m;
        });
        if (!last) return;
        frag.append(text.slice(last));
        node.replaceWith(frag);
      });
    });
  };
  // ── play times in comment texts (2:20, 1:02:20) as links that jump the player there, as f0ckm
  //    (comments.js: the same pattern, .comment-timestamp, data-time); text nodes only ──
  const TIMECODE = /\b(?<![./:])(?:(\d{1,2}):)?(\d{1,3}):([0-5]\d)\b/g;
  const linkTimes = (root) => {
    if (!root) return;
    const bodies = root.matches?.('.fy-comment-body') ? [root] : [...root.querySelectorAll('.fy-comment-body')];
    bodies.forEach(body => {
      const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT, {
        acceptNode: n => (n.parentElement?.closest('a, code, pre') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
      });
      const nodes = [];
      while (walker.nextNode()) {
        TIMECODE.lastIndex = 0;
        if (TIMECODE.test(walker.currentNode.nodeValue)) nodes.push(walker.currentNode);
      }
      nodes.forEach(node => {
        const text = node.nodeValue;
        const frag = document.createDocumentFragment();
        let last = 0;
        TIMECODE.lastIndex = 0;
        text.replace(TIMECODE, (m, hrs, mins, secs, at) => {
          frag.append(text.slice(last, at));
          const a = document.createElement('a');
          a.href = '#';
          a.className = 'comment-timestamp';
          a.dataset.time = String((hrs ? parseInt(hrs, 10) : 0) * 3600 + parseInt(mins, 10) * 60 + parseInt(secs, 10));
          a.textContent = m;
          frag.append(a);
          last = at + m.length;
          return m;
        });
        if (!last) return;
        frag.append(text.slice(last));
        node.replaceWith(frag);
      });
    });
  };
  const decorate = (root) => {
    linkTimes(root);
    emojify(root);
  };
  // a click on one: the player goes there, plays, and comes into view
  document.addEventListener('click', e => {
    const link = e.target.closest?.('.comment-timestamp');
    if (!link) return;
    e.preventDefault();
    e.stopPropagation();
    const secs = parseFloat(link.dataset.time);
    const media = document.getElementById('my-video');
    if (!media || Number.isNaN(secs) || !('currentTime' in media)) return;
    media.currentTime = secs;
    if (media.paused) media.play().catch(() => {});
    (media.closest('.fy-player') || media).scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, true);

  // ── >>id links between comments (the replies in a comment's head, the quotes in its text): the
  //    page goes to that comment and lights it up for a moment, as f0ckm (comments.js) ──
  document.addEventListener('click', e => {
    const link = e.target.closest?.('a.comment-context-link, a.fy-quote');
    if (!link || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
    const id = (link.dataset.id || (link.getAttribute('href') || '').replace(/^#c/, '')).replace(/\D/g, '');
    const target = id && document.getElementById(`c${id}`);
    e.preventDefault();
    e.stopPropagation();
    if (!target) {
      flash(`Comment #${id} isn't on this page (deleted?)`, 'info');
      return;
    }
    // a reply in a closed thread: open it first
    const thread = target.closest('details');
    if (thread && !thread.open) thread.open = true;
    target.scrollIntoView({ behavior: 'smooth', block: 'center' });
    target.classList.remove('fy-comment-flash');
    void target.offsetWidth; // restart the animation for a second click
    target.classList.add('fy-comment-flash');
    setTimeout(() => target.classList.remove('fy-comment-flash'), 2200);
    try { history.replaceState(history.state, '', `#c${id}`); } catch { /* ignore */ }
  }, true);

  window.addEventListener('f0ck:emojis_updated', () => emojify(document.body));

  // the emoji picker under the comment field: f0ckm's, with its tabs, favourites and previews
  // (/s/js/emoji-picker.js); set up for every page shown
  const setupEmojiPicker = () => {
    const form = document.getElementById('comment-form');
    if (!form || !window.f0ckEmojiPicker) return;
    window.f0ckEmojiPicker.attach({
      textarea: form.elements.content,
      trigger: form.querySelector('.fy-emoji-btn'),
      box: document.getElementById('fy-emoji-box'),
      onChange: growField,
    });
  };

  // ── the comment field grows with its lines (rows, not an inline height) ──
  const growField = ta => {
    if (!ta) return;
    ta.rows = Math.min(12, Math.max(1, ta.value.split('\n').length));
  };
  document.addEventListener('input', e => {
    if (e.target.matches?.('#comment-form textarea')) growField(e.target);
  });

  // ── live regions: swap [data-fy-live="name"] in from a fresh render of this page ──
  const pageUrl = () => {
    const u = new URL(location.href);
    u.searchParams.delete('reply');
    u.hash = '';
    return u.toString();
  };
  let queue = Promise.resolve();
  const refresh = (names) => {
    const list = [...new Set(names)].filter(n => document.querySelector(`[data-fy-live="${n}"]`));
    if (!list.length) return queue;
    queue = queue.then(async () => {
      // X-Purpose: prefetch: this reload of the page for its parts isn't a view (routes/index.rs)
      const res = await fetch(pageUrl(), { credentials: 'same-origin', cache: 'no-store', headers: { Accept: 'text/html', 'X-Purpose': 'prefetch' } });
      if (!res.ok) return;
      const doc = parse(await res.text());
      list.forEach(n => {
        const cur = document.querySelector(`[data-fy-live="${n}"]`);
        const next = doc.querySelector(`[data-fy-live="${n}"]`);
        if (cur && next) {
          const el = document.importNode(next, true);
          // an opened box (the description) stays open
          if (cur.tagName === 'DETAILS' && cur.open) el.open = true;
          cur.replaceWith(el);
          decorate(el);
        }
      });
    }).catch(err => console.error('[f0ckyou] refresh', err));
    return queue;
  };

  // ── the notification badge in the masthead (unread count from /api/notifications) ──
  const notifBadge = async () => {
    const badge = document.getElementById('fy-notif-badge');
    if (!badge) return;
    try {
      const res = await fetch('/api/notifications', { credentials: 'same-origin', headers: { Accept: 'application/json' } });
      const data = await res.json();
      if (!data || data.success === false) return;
      const n = (data.notifications || []).filter(x => !x.is_read).length;
      badge.textContent = n > 99 ? '99+' : String(n);
      badge.hidden = !n;
    } catch { /* offline: keep the badge as it is */ }
  };

  // ── forms sent with fetch: the same endpoints and fields, as JSON (they answer JSON then) ──
  const formBody = (form) => {
    const body = {};
    for (const [k, v] of new FormData(form)) {
      if (k === 'back' || v instanceof File) continue;
      if (k.endsWith('[]')) (body[k.slice(0, -2)] ||= []).push(v);
      else body[k] = v;
    }
    return body;
  };
  const send = async (url, body, method = 'POST') => {
    const res = await fetch(url, {
      method,
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json', 'X-CSRF-Token': csrf() },
      body: JSON.stringify({ ...body, csrf_token: body.csrf_token || csrf() }),
    });
    let data = {};
    try { data = await res.json(); } catch { data = { success: false, msg: `HTTP ${res.status}` }; }
    if (data.success === false || (!res.ok && data.success !== true)) throw new Error(data.msg || data.message || data.error || `HTTP ${res.status}`);
    return data;
  };

  const setReply = (form, id, name) => {
    form.elements.parent_id.value = id || '';
    const bar = form.querySelector('.fy-reply-to');
    if (!bar) return;
    bar.hidden = !id;
    bar.querySelector('span').textContent = id ? `${name ? '@' + name + ' ' : ''}#${id}` : '';
  };

  // a confirmation inside the page (the viewer may block confirm())
  const ask = (question) => new Promise(resolve => {
    let ok = false;
    try { ok = window.confirm(question); } catch { ok = false; }
    resolve(ok);
  });

  document.addEventListener('submit', async e => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement) || !form.classList.contains('fy-ajax')) return;
    e.preventDefault();
    if (form.dataset.fyConfirm && !(await ask(form.dataset.fyConfirm))) return;
    const isComment = 'fyComment' in form.dataset;
    const body = formBody(form);
    if (isComment) {
      body.content = String(body.content || '').trim();
      if (!body.content) return;
      body.parent_id = body.parent_id || null;
      if (window.f0ckCaptchaGate) {
        try { await window.f0ckCaptchaGate('comment'); } catch { return; }
      }
    }
    const buttons = [...form.querySelectorAll('button[type="submit"]'), ...(form.id ? document.querySelectorAll(`button[form="${form.id}"]`) : [])];
    buttons.forEach(b => { b.disabled = true; });
    try {
      // getAttribute: an input named "action" (the like form) shadows form.action
      const action = form.getAttribute('action');
      const data = await send(action, body);
      if ('fyGo' in form.dataset) {
        location.href = form.elements.back?.value || '/';
        return;
      }
      if (isComment) {
        form.elements.content.value = '';
        growField(form.elements.content);
        setReply(form, '', '');
      } else if (form.elements.tagname) {
        form.elements.tagname.value = '';
      }
      if ('fyClose' in form.dataset) {
        form.reset();
        if (location.hash) location.hash = '_';
      }
      if (form.dataset.fyOk) flash(form.dataset.fyOk, 'success');
      if (action.startsWith('/api/notifications')) notifBadge();
      await refresh((form.dataset.fyRefresh || '').split(/\s+/).filter(Boolean));
      const id = data?.comment?.id;
      const el = id && document.getElementById(`c${id}`);
      if (el) {
        // the new comment lights up for a moment (the frame fades out: fy-comment-posted)
        el.classList.add('fy-comment-posted');
        el.addEventListener('animationend', () => el.classList.remove('fy-comment-posted'), { once: true });
        el.scrollIntoView({ block: 'center', behavior: 'smooth' });
      }
    } catch (err) {
      flash(err.message || 'Something went wrong', 'error');
      if ('fyClose' in form.dataset) window.f0ckCapReset?.();
    } finally {
      buttons.forEach(b => { if (b.isConnected) b.disabled = false; });
    }
  });

  // reply: fill the form's parent_id in place (without JavaScript the link reloads with ?reply=<id>)
  document.addEventListener('click', e => {
    const reply = e.target.closest('[data-reply]');
    const cancel = e.target.closest('.fy-reply-cancel');
    const form = document.getElementById('comment-form');
    if (!form || !(reply || cancel)) return;
    e.preventDefault();
    if (cancel) return setReply(form, '', '');
    setReply(form, reply.dataset.reply, reply.dataset.name);
    form.scrollIntoView({ block: 'center', behavior: 'smooth' });
    form.elements.content.focus();
  });
  // "Cancel" under the comment field: empty it and drop the reply (after the reset has run)
  document.addEventListener('reset', e => {
    if (e.target.id !== 'comment-form') return;
    const form = e.target;
    setTimeout(() => {
      setReply(form, '', '');
      growField(form.elements.content);
      form.elements.content.blur();
    });
  });
  // Ctrl/Cmd+Enter posts a comment
  document.addEventListener('keydown', e => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && e.target.closest?.('#comment-form textarea')) {
      e.preventDefault();
      document.getElementById('comment-form').requestSubmit();
    }
  });

  // ── share: copy the link ──
  document.addEventListener('click', async e => {
    const btn = e.target.closest('[data-fy-copy]');
    if (!btn) return;
    const field = document.querySelector(btn.dataset.fyCopy);
    if (!field) return;
    try {
      await navigator.clipboard.writeText(field.value);
      flash('Link copied', 'success');
    } catch {
      field.select();
      flash('Press Ctrl+C to copy', 'info');
    }
  });

  // ── lists keep going: the next page into the list when the page links come into view ──
  const setupFeed = () => {
  const feedNav = document.querySelector('.fy-pages');
  const feed = feedNav?.previousElementSibling?.matches('[data-fy-feed]') ? feedNav.previousElementSibling : null;
  if (feedNav && feed && 'IntersectionObserver' in window) {
    let nav = feedNav;
    let loading = false;
    const io = new IntersectionObserver(async entries => {
      if (loading || !entries.some(en => en.isIntersecting)) return;
      const href = nav.querySelector('a[rel="next"]')?.getAttribute('href');
      if (!href) { io.disconnect(); return; }
      loading = true;
      nav.classList.add('fy-loading');
      try {
        const res = await fetch(new URL(href, location.href), { credentials: 'same-origin', headers: { Accept: 'text/html' } });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const doc = parse(await res.text());
        const nextNav = doc.querySelector('.fy-pages');
        const nextFeed = nextNav?.previousElementSibling?.matches('[data-fy-feed]') ? nextNav.previousElementSibling : doc.querySelector('[data-fy-feed]');
        [...(nextFeed?.children || [])].forEach(el => {
          const id = el.dataset.id || el.id;
          if (id && feed.querySelector(`[data-id="${CSS.escape(id)}"], #${CSS.escape(id)}`)) return;
          const node = document.importNode(el, true);
          feed.appendChild(node);
          decorate(node);
        });
        io.unobserve(nav);
        if (nextNav) {
          const el = document.importNode(nextNav, true);
          nav.replaceWith(el);
          nav = el;
          if (el.querySelector('a[rel="next"]')) io.observe(el);
        } else {
          nav.remove();
        }
      } catch {
        nav.classList.remove('fy-loading');
        io.disconnect();
        flash('Could not load more', 'error');
      } finally {
        loading = false;
      }
    }, { rootMargin: '800px 0px' });
    io.observe(nav);
    onLeave(() => io.disconnect());
  }
  };

  // ── new uploads: a pill at the top of the home feed that swaps in the fresh first page ──
  const newPill = () => {
    let pill = document.getElementById('fy-newpill');
    if (pill) return;
    pill = document.createElement('button');
    pill.type = 'button';
    pill.id = 'fy-newpill';
    pill.className = 'fy-newpill';
    pill.textContent = 'New f0cks';
    pill.addEventListener('click', async () => {
      pill.disabled = true;
      try {
        const res = await fetch(pageUrl(), { credentials: 'same-origin', cache: 'no-store', headers: { Accept: 'text/html' } });
        const doc = parse(await res.text());
        const cur = document.querySelector('[data-fy-feed]');
        const next = doc.querySelector('[data-fy-feed]');
        if (cur && next) cur.replaceWith(document.importNode(next, true));
        pill.remove();
        window.scrollTo({ top: 0, behavior: 'smooth' });
      } catch {
        pill.disabled = false;
        flash('Could not load the new f0cks', 'error');
      }
    });
    document.body.appendChild(pill);
  };

  window.f0ckFrontend = {
    itemId: page.itemId || null,
    flash,
    // the admin changed the site's brand (f0ck-core.js has updated the picture and the text): in a
    // logo showing it (f0ckYou brand → "use the site's brand"), the play mark only without one
    brand() {
      document.querySelectorAll('.fy-logo.navbar-brand').forEach(logo => {
        const img = logo.querySelector('[data-brand-img]');
        const txt = logo.querySelector('[data-brand-text]');
        logo.classList.toggle('fy-logo-custom', !!((img && !img.hidden) || (txt && txt.dataset.text)));
      });
    },
    comments(data) {
      if (!page.itemId || String(data?.item_id) !== String(page.itemId)) return;
      refresh(['comments', 'comments-head']);
    },
    notify() {
      notifBadge();
      if (document.querySelector('[data-fy-live="notifs"]')) refresh(['notifs']);
    },
    newItem() {
      // only on the first page of the home feed, where it would show up
      if (!document.body.classList.contains('fy-page-home') || document.querySelector('.fy-pages [rel="prev"]')) return;
      newPill();
    },
    dmBadge() {
      const badge = document.getElementById('dm-badge');
      if (!badge) return;
      badge.textContent = String((parseInt(badge.textContent, 10) || 0) + 1);
      badge.classList.add('fy-on');
    },
    reconnected() {
      if (page.session) notifBadge();
      if (page.itemId) refresh(['comments', 'comments-head', 'actions', 'desc']);
    },
    live(msg) {
      if (msg && msg.type === 'fy_brand') refreshLogo();
    },
  };

  // ── the logo: f0ckYou's own brand changed (admin panel → f0ckYou brand, live event fy_brand):
  //    every logo and the brand font's stylesheet come from a fresh render of this page ──
  const refreshLogo = () => {
    queue = queue.then(async () => {
      const res = await fetch(pageUrl(), { credentials: 'same-origin', cache: 'no-store', headers: { Accept: 'text/html', 'X-Purpose': 'prefetch' } });
      if (!res.ok) return;
      const doc = parse(await res.text());
      const fresh = [...doc.querySelectorAll('[data-fy-logo]')];
      document.querySelectorAll('[data-fy-logo]').forEach((cur, i) => {
        if (fresh[i]) cur.replaceWith(document.importNode(fresh[i], true));
      });
      const link = document.getElementById('fy-brand-font-css');
      const next = doc.getElementById('fy-brand-font-css');
      if (next && link) link.href = next.getAttribute('href');
      else if (next) document.head.appendChild(document.importNode(next, true));
      else if (link) link.remove();
    }).catch(err => console.error('[f0ckyou] logo', err));
  };

  // ── menus ──
  const menus = () => [...document.querySelectorAll('details.fy-menu')];
  const setupMenus = () => menus().forEach(d => {
    if (d.dataset.fyMenu) return;
    d.dataset.fyMenu = '1';
    d.addEventListener('toggle', () => {
      if (d.open) menus().forEach(o => { if (o !== d) o.open = false; });
    });
  });
  document.addEventListener('click', e => {
    menus().forEach(d => { if (d.open && !d.contains(e.target)) d.open = false; });
  });

  // ── the player: f0ckm's v0ck for videos (its controls, settings menu, keys ↑ ↓ . ,), the
  //    native one with a remembered volume for audio; keys; autonext; the blurred background ──
  const getMedia = () => document.getElementById('my-video');
  const setupPlayer = () => {
  const media = getMedia();
  const hasV0ck = !!(media instanceof HTMLVideoElement && window.v0ck);
  if (hasV0ck) {
    // v0ck replaces the native controls (kept in the page for visitors without JavaScript), keeps
    // the volume in localStorage and starts the video unless autoplay is off
    new window.v0ck(media);
  } else if (media && 'volume' in media) {
    try {
      const vol = parseFloat(localStorage.getItem('volume'));
      if (Number.isFinite(vol)) media.volume = Math.min(1, Math.max(0, vol));
    } catch { /* storage blocked */ }
    media.addEventListener('volumechange', () => {
      try { localStorage.setItem('volume', String(media.volume)); } catch { /* storage blocked */ }
    });
  }
  if (media instanceof HTMLVideoElement) {
    media.addEventListener('ended', () => {
      if (!autonext()) return;
      const up = activeList()?.querySelector('a[data-rec]');
      go(page.next || up?.getAttribute('href') || '/random');
    });
  }

  // the blurred background (f0ck-bg.js): the video's frames, the picture, the cover of a song or
  // the Flash movie, the thumbnail until there is a frame
  if (window.f0ckBackground && page.itemId) {
    const image = document.getElementById('f0ck-image');
    const cover = document.querySelector('.fy-audio-cover');
    const ruffleBox = document.getElementById('ruffle-container');
    window.f0ckBackground.start({
      media: media instanceof HTMLVideoElement ? media
        : image || cover || (ruffleBox ? () => ruffleBox.querySelector('f0ck-flash')?.ambientCanvas || null : null),
      thumb: `/t/${page.itemId}.webp`,
    });
    onLeave(() => window.f0ckBackground.stop());
  }
  };
  const fullscreen = () => {
    const player = document.querySelector('.fy-player');
    const media = getMedia();
    const btn = player?.querySelector('.v0ck_fs_btn');
    if (btn) return btn.click();
    if (document.fullscreenElement) document.exitFullscreen?.();
    else (player || media)?.requestFullscreen?.().catch(() => {});
  };
  const playPause = () => {
    const media = getMedia();
    if (!media || !('paused' in media)) return;
    if (media.paused) media.play().catch(() => {}); else media.pause();
  };

  // autonext (the player's settings menu, as f0ckm): at the end of a video go to the next f0ck
  // instead of looping (v0ck turns `loop` off while it is on)
  const autonext = () => { try { return localStorage.getItem('autoplay') === 'true'; } catch { return false; } };
  if (!window.toggleAutoplay) {
    window.toggleAutoplay = () => {
      try { localStorage.setItem('autoplay', autonext() ? 'false' : 'true'); } catch { /* storage blocked */ }
      document.querySelectorAll('#toggleautoplay').forEach(el => el.classList.toggle('active', autonext()));
    };
  }

  // ── keys ──
  const typing = t => t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));
  const guideToggle = () => document.getElementById('fy-guide-toggle');
  const drawerOpen = () => {
    const scrim = document.querySelector('.fy-scrim');
    return guideToggle()?.checked && scrim && getComputedStyle(scrim).display !== 'none';
  };
  document.addEventListener('keydown', e => {
    if (e.key === 'Escape') {
      menus().forEach(d => { d.open = false; });
      if (/^#(report|delete|share)$/.test(location.hash)) location.hash = '_';
      if (drawerOpen()) guideToggle().checked = false;
      return;
    }
    if (typing(e.target) || e.ctrlKey || e.metaKey || e.altKey) return;
    const key = e.key;
    if (key === '/') {
      const q = document.querySelector('#fy-search input[name="tag"]');
      if (q) { e.preventDefault(); q.focus(); q.select(); }
      return;
    }
    if (!page.itemId) return;
    const media = getMedia();
    if ((key === 'ArrowLeft' || key === 'P') && page.prev) {
      go(page.prev);
    } else if ((key === 'ArrowRight' || key === 'N') && page.next) {
      go(page.next);
    } else if (key === 'r') {
      go('/random');
    } else if (media && 'paused' in media && (key === 'k' || (key === ' ' && !e.target.closest?.('button, a, summary, label')))) {
      e.preventDefault();
      playPause();
    } else if (media && 'currentTime' in media && (key === 'j' || key === 'l')) {
      media.currentTime = Math.max(0, media.currentTime + (key === 'l' ? 10 : -10));
    } else if (media && 'muted' in media && key === 'm') {
      media.muted = !media.muted;
    } else if (key === 'f') {
      fullscreen();
    }
  });

  // ── Flash: the SWF plays in f0ckm's sandboxed Ruffle frame (<f0ck-flash>, /s/js/flash-embed.js,
  //    loaded by the watch page); this only starts it in #ruffle-container ──
  const setupFlash = () => {
  const ruffle = document.getElementById('ruffle-container');
  if (ruffle && ruffle.dataset.swf && window.customElements?.get('f0ck-flash')) {
    const fl = document.createElement('f0ck-flash');
    ruffle.appendChild(fl);
    fl.addEventListener('loadedmetadata', () => {
      let vol = 0.5;
      try { vol = +(localStorage.getItem('volume') ?? 0.5); } catch { /* storage blocked */ }
      fl.volume = Number.isFinite(vol) ? vol : 0.5;
    });
    // the background draws the movie (its ambientCanvas) once it runs
    fl.load({ url: ruffle.dataset.swf, autoplay: true, background: true }).then(() => window.f0ckBackground?.redraw()).catch(() => {
      flash('Flash playback failed', 'error');
    });
  }
  };

  // ── /tags: "Show more" appends the next page from /api/tags (cards as snippets/tag-cards.html) ──
  const tagCard = (tt, mode) => {
    const href = `/tag/${tt.safe_tag}`;
    const count = Number(tt.total_items || 0).toLocaleString('en-US');
    const art = document.createElement('article');
    art.className = 'fy-card fy-card-tag';
    const thumb = document.createElement('a');
    thumb.className = 'fy-thumb';
    thumb.href = href;
    const img = document.createElement('img');
    img.src = `/tag_image/${tt.encoded_tag}?m=${encodeURIComponent(mode)}`;
    img.alt = '';
    img.loading = 'lazy';
    const badge = document.createElement('span');
    badge.className = 'fy-badge-br';
    badge.textContent = count;
    thumb.append(img, badge);
    const meta = document.createElement('div');
    meta.className = 'fy-card-meta';
    const ava = document.createElement('span');
    ava.className = 'fy-card-ava';
    ava.innerHTML = '<span class="fy-ava fy-ava-letter" aria-hidden="true">#</span>';
    const text = document.createElement('div');
    text.className = 'fy-card-text';
    const h = document.createElement('h3');
    h.className = 'fy-card-title';
    const a = document.createElement('a');
    a.href = href;
    a.textContent = `#${tt.tag}`;
    h.append(a);
    const sub = document.createElement('div');
    sub.className = 'fy-card-sub';
    sub.textContent = `${count} f0cks`;
    text.append(h, sub);
    meta.append(ava, text);
    art.append(thumb, meta);
    return art;
  };
  document.addEventListener('click', async e => {
    const btn = e.target.closest('[data-tags-more]');
    const grid = document.getElementById('fy-tag-grid');
    if (!btn || !grid) return;
    const next = (parseInt(btn.dataset.page, 10) || 1) + 1;
    btn.disabled = true;
    try {
      const res = await (await fetch(`/api/tags?page=${next}`, { credentials: 'same-origin', headers: { Accept: 'application/json' } })).json();
      (res.tags || []).forEach(tt => grid.appendChild(tagCard(tt, grid.dataset.mode || '0')));
      btn.dataset.page = String(next);
      if (!res.hasMore) btn.closest('.fy-more-wrap')?.remove();
    } catch {
      flash('Could not load more tags', 'error');
    } finally {
      btn.disabled = false;
    }
  });

  // ── interests: the same learning as f0ckm (f0ckInterestEngine, f0ck-core.js): the f0ck on screen
  //    (how long it is watched), clicks on recommendations, searches ──
  const engine = window.f0ckInterestEngine;
  const setupTracking = () => {
    const watch = document.querySelector('.fy-watch[data-item-id]');
    if (engine && watch) {
      engine.startTrackingItem({
        itemId: watch.dataset.itemId,
        tags: [...document.querySelectorAll('#tags .fy-tag:not(.fy-tag-rating)')].map(el => el.dataset.tag).filter(Boolean),
        creator: watch.dataset.creator || '',
      });
      onLeave(() => engine.flushCurrentTracking());
    }
  };
  document.addEventListener('submit', e => {
    if (!engine || !e.target.matches?.('form.fy-search')) return;
    const q = e.target.elements.tag?.value || '';
    if (q.trim()) engine.recordTagSearch(q);
  });

  // ── "Up next": the recommendations of f0ckm's recommendations tab, the same calls as
  //    sidebar-activity.js. The server rendered the first page (snippets/recommendations.html);
  //    with the visitor's interests the list is asked for again, and more come at its end. ──
  const affinityParams = () => {
    try {
      const aff = engine ? engine.getTopAffinities() : null;
      if (!aff) return '';
      let q = '';
      if (aff.topTags && aff.topTags.length) q += `&session_tags=${encodeURIComponent(aff.topTags.join(','))}`;
      if (aff.topCreators && aff.topCreators.length) q += `&session_creators=${encodeURIComponent(aff.topCreators.join(','))}`;
      return q;
    } catch {
      return '';
    }
  };
  // at most one personal pick among the first three (enforceSidebarRecommendationDistribution)
  const spreadPicks = (items) => {
    if (!Array.isArray(items) || items.length <= 3) return items;
    const picks = [];
    for (let i = 0; i < 3; i++) if (items[i] && items[i].personalized) picks.push(i);
    if (picks.length > 1) {
      let other = 3;
      for (let r = 1; r < picks.length; r++) {
        while (other < items.length && items[other] && items[other].personalized) other++;
        if (other < items.length) {
          [items[picks[r]], items[other]] = [items[other], items[picks[r]]];
          other++;
        }
      }
    }
    return items;
  };
  const EXT = m => (m.includes('/') ? m.split('/')[1] : '')
    .replace('jpeg', 'jpg').replace('x-shockwave-flash', 'flash').replace('x-flac', 'flac').replace('mpeg', 'mp3').toUpperCase();
  // where the files are (websrv.paths.images, as on the thumbnails the server rendered)
  const MEDIA = (document.querySelector('[data-preview]')?.dataset.preview || '/b/x').replace(/\/[^/]*$/, '');
  const ICONS = document.querySelector('use[href*="/icons.svg"]')?.getAttribute('href')?.split('#')[0] || '/s/fe/f0ckyou/img/icons.svg';
  const el = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  };
  // a play time as macros.html timecode(): 0:13, 4:05, 1:02:05
  const timecode = (secs) => {
    const t = Math.floor(Number(secs) || 0);
    const h = Math.floor(t / 3600);
    const m = Math.floor((t % 3600) / 60);
    const sec = String(t % 60).padStart(2, '0');
    return h ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
  };
  // a view count as macros.html views(): cut, not rounded
  const viewCount = (raw) => {
    const n = Math.max(0, parseInt(raw, 10) || 0);
    const cut = (x) => String(Math.floor(x * 10) / 10);
    const s = n >= 1e6 ? `${cut(n / 1e6)}M` : n >= 1e4 ? `${Math.floor(n / 1e3)}K` : n >= 1e3 ? `${cut(n / 1e3)}K` : String(n);
    return `${s} view${n === 1 ? '' : 's'}`;
  };
  // the card of snippets/recommendations.html
  const recCard = (v) => {
    const anon = !!window.f0ckSession?.is_anonymized;
    const key = v.slug || v.id;
    const tags = Array.isArray(v.tags) ? v.tags.map(t => (typeof t === 'object' && t ? (t.tag || t.name || '') : String(t))).filter(Boolean) : [];
    const title = (v.title && v.title.trim()) || (tags.length ? '#' + tags.join(' #') : String(key));
    const who = anon ? 'anonymous' : (v.display_name || v.username || '');
    const mime = v.mime || '';
    const li = el('li');
    li.dataset.id = String(v.id);
    const href = v.from_tag ? `/tag/${encodeURIComponent(v.from_tag)}/${key}` : `/${key}`;
    li.dataset.personalized = v.personalized ? 'true' : 'false';
    const a = el('a');
    a.href = href;
    a.dataset.rec = String(v.id);
    a.dataset.tags = tags.join(',');
    a.dataset.creator = anon ? '' : (v.username || '');
    const thumb = el('span', 'fy-upnext-thumb');
    if (mime.startsWith('video/') && mime !== 'video/youtube' && v.dest && !/\.gif$/i.test(v.dest)) thumb.dataset.preview = `${MEDIA}/${v.dest}`;
    if (mime.startsWith('audio/') && !v.has_coverart) {
      const ph = el('span', 'fy-thumb-audio');
      const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      svg.setAttribute('class', 'fy-ico');
      svg.setAttribute('aria-hidden', 'true');
      const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
      use.setAttribute('href', `${ICONS}#music`);
      svg.append(use);
      ph.append(svg);
      thumb.append(ph);
    } else {
      const img = el('img');
      img.src = `/t/${v.id}.webp`;
      img.alt = '';
      img.loading = 'lazy';
      thumb.append(img);
    }
    // bottom right: the rating (not SFW), then the play time or the file type
    const badges = el('span', 'fy-badges');
    if (v.rating_class === 'nsfw' || v.rating_class === 'nsfl') badges.append(el('span', `fy-tbadge fy-rate-${v.rating_class}`, v.rating_class.toUpperCase()));
    const ext = EXT(mime);
    if (Number(v.duration) >= 1) badges.append(el('span', 'fy-tbadge fy-time', timecode(v.duration)));
    else if (ext) badges.append(el('span', 'fy-tbadge', ext));
    thumb.append(badges);
    const text = el('span', 'fy-upnext-text');
    const name = el('span', 'fy-upnext-name', title);
    name.title = title;
    text.append(name);
    if (who) text.append(el('span', 'fy-upnext-chan', who));
    const meta = el('span', 'fy-dim', viewCount(v.views));
    if (v.stamp) {
      const iso = new Date(v.stamp * 1000).toISOString();
      const time = el('time', null, window.f0ckTimeAgo ? window.f0ckTimeAgo(iso) : '');
      time.title = iso;
      meta.append(' · ', time);
    }
    if (v.personalized) meta.append(' · ', el('span', 'fy-foryou', 'For you'));
    text.append(meta);
    a.append(thumb, text);
    li.append(a);
    return li;
  };

  const getRecs = async (query) => {
    const res = await fetch(`/api/v2/recommendations?${query}${affinityParams()}`, {
      credentials: 'same-origin',
      headers: { 'X-Requested-With': 'XMLHttpRequest', Accept: 'application/json' },
    });
    const data = await res.json();
    return data && data.success ? (data.items || data.videos || []) : null;
  };
  // what a list has shown (kept with it when it moves on to the next f0ck)
  const recSeen = new WeakMap();
  const seenOf = (list) => {
    let seen = recSeen.get(list);
    if (!seen) {
      seen = new Set([...list.children].map(li => li.dataset.id));
      recSeen.set(list, seen);
    }
    return seen;
  };

  // the end of the tag's list: everything of the tag is shown, the other recommendations are a tab away
  const tagEnd = (list) => {
    const pane = list.parentElement;
    if (pane.querySelector('.fy-recs-end')) return;
    const p = el('p', 'fy-dim fy-recs-end', `That's all in #${(list.dataset.tag || '').toLowerCase()}. `);
    const label = el('label', 'fy-linkbtn', 'Show recommended');
    label.htmlFor = 'fy-upnext-recs';
    p.append(label);
    list.after(p);
  };
  const setupList = (list) => {
    const itemId = list.dataset.itemId;
    const source = list.dataset.source || 'recs';
    const tag = list.dataset.tag || '';
    const seen = seenOf(list);
    // the list came along from the f0ck before (go()): it stays as it is
    const kept = list.dataset.kept === '1';
    delete list.dataset.kept;
    const pane = list.parentElement;
    const empty = () => pane.querySelector('.fy-recs-empty');
    let loading = false;
    let done = false;
    const spinner = el('div', 'fy-recs-more');
    spinner.hidden = true;
    list.after(spinner);

    // with the visitor's interests: the list f0ckm's tab would show (without them the server's is it);
    // the tag's list is the tag's
    // the server's tag list was all the tag has
    if (source === 'tag' && list.dataset.tagDone) done = true;
    if (source === 'recs' && !kept && affinityParams()) {
      loading = true;
      getRecs(`limit=20&exclude_ids=${encodeURIComponent(itemId)}`).then(items => {
        if (!items || !items.length) return;
        spreadPicks(items);
        seen.clear();
        list.replaceChildren(...items.map(v => { seen.add(String(v.id)); return recCard(v); }));
        empty()?.remove();
      }).catch(() => {}).finally(() => { loading = false; });
    }

    const loadMore = async () => {
      if (loading || done) return;
      loading = true;
      spinner.hidden = false;
      try {
        const exclude = encodeURIComponent([itemId, ...[...seen].slice(-100)].join(','));
        let items = [];
        if (source === 'tag') {
          // the tag's tab shows the tag only; the rest is the Recommended tab's
          items = (await getRecs(`tag=${encodeURIComponent(tag)}&limit=15&exclude_ids=${exclude}`)) || [];
          if (items.length < 15) {
            list.dataset.tagDone = '1';
            done = true;
          }
        } else {
          items = (await getRecs(`limit=15&exclude_ids=${exclude}&continuation=1`)) || [];
        }
        const fresh = items.filter(v => !seen.has(String(v.id)) && String(v.id) !== String(itemId));
        fresh.forEach(v => { seen.add(String(v.id)); list.append(recCard(v)); });
        if (fresh.length) empty()?.remove();
        else done = true;
      } catch {
        done = true;
      } finally {
        if (done && source === 'tag' && list.children.length) tagEnd(list);
        spinner.hidden = true;
        loading = false;
      }
    };
    if ('IntersectionObserver' in window) {
      const sentinel = el('div', 'fy-recs-sentinel');
      spinner.after(sentinel);
      const io = new IntersectionObserver(entries => {
        if (entries.some(en => en.isIntersecting)) loadMore();
      }, { rootMargin: '400px 0px' });
      io.observe(sentinel);
      onLeave(() => io.disconnect());
    }
  };
  const setupRecs = () => document.querySelectorAll('.fy-upnext-list').forEach(setupList);
  // the list on screen: the tag's when its tab is on (or it is the only one)
  const activeList = () => (document.getElementById('fy-upnext-recs')?.checked ? null : document.getElementById('fy-recs-tag')) || document.getElementById('fy-recs');
  // a clicked recommendation makes room for a new one, the rest of the list stays (f0ckm's
  // sidebar: replaceCardWithNewRandom): at most one personal pick among the first three, none
  // next to another further down
  const replaceCard = async (li) => {
    const list = li?.parentElement;
    if (!list || li.dataset.swapping) return;
    li.dataset.swapping = '1';
    li.classList.add('fy-rec-swapping');
    const cards = [...list.children];
    const at = cards.indexOf(li);
    const seen = seenOf(list);
    const exclude = [...new Set([...seen, ...cards.map(c => c.dataset.id), li.dataset.id, list.dataset.itemId].filter(Boolean))].slice(-120);
    let prefer = '';
    if (at >= 0 && at < 3) {
      prefer = `&prefer_personalized=${li.dataset.personalized === 'true' ? 'true' : 'false'}`;
    } else if (at >= 3) {
      const near = [cards[at - 1], cards[at + 1]].some(c => c && c.dataset.personalized === 'true');
      if (near) prefer = '&prefer_personalized=false';
    }
    try {
      const ex = encodeURIComponent(exclude.join(','));
      // the tag's list: another item of the tag, or none (the rest is the Recommended tab's)
      const items = list.dataset.source === 'tag'
        ? await getRecs(`tag=${encodeURIComponent(list.dataset.tag || '')}&limit=1&exclude_ids=${ex}`)
        : await getRecs(`limit=1&exclude_ids=${ex}${prefer}`);
      const fresh = (items || []).find(v => !seen.has(String(v.id)));
      if (!fresh) throw new Error('none');
      seen.add(String(fresh.id));
      if (li.isConnected) li.replaceWith(recCard(fresh));
    } catch {
      // nothing new to show: the clicked one goes, the list moves up
      if (li.isConnected) li.remove();
    }
  };
  // a click on a recommendation teaches the interests (recordSuggestionClick, as f0ckm) and swaps
  // the card for a new one
  const picked = (e) => {
    if (e.button !== 0 && e.button !== 1) return;
    const a = e.target.closest?.('.fy-upnext-list a[data-rec]');
    if (!a) return;
    engine?.recordSuggestionClick(parseInt(a.dataset.rec, 10), (a.dataset.tags || '').split(',').filter(Boolean), a.dataset.creator || '');
    replaceCard(a.closest('li'));
  };
  document.addEventListener('click', picked);
  document.addEventListener('auxclick', picked);

  // ── moderators: remove a tag (DELETE has no form; the description is refreshed in place) ──
  document.addEventListener('click', async e => {
    const btn = e.target.closest('#tags .fy-tag-del');
    if (!btn) return;
    const tag = btn.closest('[data-tag]')?.dataset.tag;
    const id = document.getElementById('tags')?.dataset.itemId;
    if (!tag || !id || !(await ask(`Remove the tag "${tag}"?`))) return;
    btn.disabled = true;
    try {
      await send(`/api/v2/tags/${id}/${encodeURIComponent(tag)}`, {}, 'DELETE');
      await refresh(['desc']);
    } catch (err) {
      btn.disabled = false;
      flash(err.message || 'could not remove the tag', 'error');
    }
  });

  // ── a video's thumbnail plays it, muted, while the pointer is on it (as f0ckm's grid): after
  //    150 ms, so passing over a thumbnail doesn't fetch anything; only the start of the file is
  //    asked for (preload metadata) until it plays ──
  let preview = null; // { thumb, video, timer }
  const stopPreview = () => {
    if (!preview) return;
    clearTimeout(preview.timer);
    const { thumb, video } = preview;
    preview = null;
    thumb.classList.remove('fy-previewing');
    if (video) {
      video.pause();
      video.removeAttribute('src');
      video.load();
      video.remove();
    }
  };
  const startPreview = (thumb) => {
    stopPreview();
    const p = { thumb, video: null, timer: 0 };
    preview = p;
    p.timer = setTimeout(() => {
      if (preview !== p || !thumb.isConnected) return;
      const video = document.createElement('video');
      video.className = 'fy-preview-video';
      video.muted = true;
      video.loop = true;
      video.playsInline = true;
      video.preload = 'metadata';
      video.setAttribute('aria-hidden', 'true');
      video.oncanplay = () => {
        if (preview !== p) return;
        video.play().catch(() => {});
        video.classList.add('playing');
      };
      video.src = thumb.dataset.preview;
      thumb.classList.add('fy-previewing');
      thumb.appendChild(video);
      p.video = video;
    }, 150);
  };
  document.addEventListener('mouseover', e => {
    const thumb = e.target.closest?.('[data-preview]');
    if (thumb && (!preview || preview.thumb !== thumb)) startPreview(thumb);
  });
  document.addEventListener('mouseout', e => {
    const thumb = e.target.closest?.('[data-preview]');
    if (thumb && !(e.relatedTarget && thumb.contains(e.relatedTarget))) stopPreview();
  });

  // ── the view just counted (f0ck-core.js, after 3 seconds seen): the count on the page goes up ──
  document.addEventListener('f0ck:view-counted', e => {
    if (String(e.detail?.itemId) !== String(page.itemId)) return;
    const box = document.querySelector('.fy-views-count[data-views]');
    if (!box) return;
    const n = (parseInt(box.dataset.views, 10) || 0) + 1;
    box.dataset.views = String(n);
    box.textContent = `${n.toLocaleString('en-US')} view${n === 1 ? '' : 's'}`;
  });

  // ── the page on screen: set up its parts (also for every page shown in place) ──
  const setupPage = () => {
    readPage();
    window.f0ckFrontend.itemId = page.itemId || null;
    setupMenus();
    decorate(document.querySelector('.fy-content') || document.body);
    if (page.session) notifBadge();
    setupFeed();
    setupPlayer();
    setupFlash();
    setupTracking();
    setupRecs();
    setupEmojiPicker();
  };

  // ── pages change in place ──
  // Paths that load as usual: files and the API, the shared pages (they render in the shell with
  // their own scripts), sign-in / sign-out, settings that change the whole site, the upload page.
  const FULL = /^\/(?:api|b|t|a|s|ca|tag_image|hall_image|logout|login|register|forgot-password|frontend|settings|messages|admin|mod|pending|privacy|banned|upload|scroller|chan|meme|activity|flash-frame|maintenance)(?:\/|$)|^\/user\/[^/]+\/halls?(?:\/|$)/;
  // scripts a page may bring that only need to be there once (custom elements, settings)
  const ONCE = /\/(?:flash-embed|cap-config|cap\.min)\.js$/;
  const loaded = new Set([...document.scripts].map(sc => sc.src && new URL(sc.src, location.href).pathname).filter(Boolean));
  const loadOnce = (src) => new Promise(resolve => {
    const path = new URL(src, location.href).pathname;
    if (loaded.has(path)) return resolve();
    loaded.add(path);
    const sc = document.createElement('script');
    sc.src = src;
    sc.onload = sc.onerror = () => resolve();
    document.body.appendChild(sc);
  });
  const pathKey = (href) => { const u = new URL(href, location.href); return u.pathname + u.search; };
  let shown = pathKey(location.href);

  const leave = () => {
    const fns = leaving;
    leaving = [];
    fns.forEach(fn => { try { fn(); } catch (err) { console.error('[f0ckyou] leave', err); } });
    // the media of the page going: stop it and let go of the file
    document.querySelectorAll('.fy-content video, .fy-content audio').forEach(m => {
      try { m.pause(); m.removeAttribute('src'); m.load(); } catch { /* gone already */ }
    });
    document.getElementById('fy-newpill')?.remove();
    stopPreview();
    menus().forEach(d => { d.open = false; });
    const toggle = guideToggle();
    if (toggle) toggle.checked = false;
  };

  const swapOne = (doc, sel) => {
    const cur = document.querySelector(sel);
    const next = doc.querySelector(sel);
    if (cur && next) cur.replaceWith(document.importNode(next, true));
  };

  let navigation = null;
  async function go(href, { push = true, scroll = null } = {}) {
    const target = new URL(href, location.href);
    if (target.origin !== location.origin || FULL.test(target.pathname)) {
      location.href = target.href;
      return;
    }
    if (navigation) navigation.abort();
    const ctrl = navigation = new AbortController();
    document.documentElement.classList.add('fy-navigating');
    const full = (url) => { location.href = url || target.href; };
    let res;
    let doc;
    try {
      res = await fetch(target.href, { credentials: 'same-origin', headers: { Accept: 'text/html' }, signal: ctrl.signal });
      if (!(res.headers.get('content-type') || '').includes('text/html')) return full();
      doc = parse(await res.text());
    } catch {
      if (!ctrl.signal.aborted) full();
      return;
    }
    if (ctrl !== navigation) return;
    navigation = null;
    const content = doc.querySelector('.fy-content');
    const finalUrl = new URL(res.url || target.href);
    if (target.hash) finalUrl.hash = target.hash;
    // not one of f0ckyou's own pages (a shared page in the shell, sign-in, ...), or one with scripts
    // of its own: load it as usual
    const scripts = content ? [...content.querySelectorAll('script[src]')] : [];
    // f0ckyou's own CSS or JS changed since this page loaded (an update): load the new page as usual,
    // or it would show in the old styles and scripts
    const version = (d, sel, attr) => d.querySelector(sel)?.getAttribute(attr) || '';
    const stale = ['link[href*="/f0ckyou.css"]', 'href', 'script[src*="/f0ckyou.js"]', 'src'];
    const outdated = version(doc, stale[0], stale[1]) !== version(document, stale[0], stale[1])
      || version(doc, stale[2], stale[3]) !== version(document, stale[2], stale[3]);
    if (!content || outdated || doc.documentElement.classList.contains('f0ck-shell') || FULL.test(finalUrl.pathname)
      || scripts.some(sc => !ONCE.test(new URL(sc.getAttribute('src'), location.href).pathname))) {
      return full(finalUrl.href);
    }

    // where this page was scrolled to, for coming back
    try { history.replaceState({ ...(history.state || {}), fy: 1, fyScroll: window.scrollY }, ''); } catch { /* ignore */ }
    leave();

    // the blurred background stays from one f0ck to the next (no flash), the new one draws over it
    const next = document.importNode(content, true);
    const oldBg = document.getElementById('bg');
    const newBg = next.querySelector('#bg');
    if (oldBg && newBg) newBg.replaceWith(oldBg);
    // Up next stays too (as f0ckm's sidebar): the new f0ck takes the lists over as they are (the
    // tag's list for the same tag only), and the tab that was on
    ['fy-recs', 'fy-recs-tag'].forEach(id => {
      const oldList = document.getElementById(id);
      const newList = next.querySelector(`#${id}`);
      if (!oldList || !newList || !oldList.children.length || (oldList.dataset.tag || '') !== (newList.dataset.tag || '')) return;
      oldList.dataset.itemId = newList.dataset.itemId;
      oldList.dataset.kept = '1';
      newList.parentElement.querySelector('.fy-recs-empty')?.remove();
      newList.replaceWith(oldList);
    });
    const tab = document.querySelector('.fy-upnext-radio:checked')?.id;
    if (tab && next.querySelector(`#${tab}`)) next.querySelector(`#${tab}`).checked = true;
    document.querySelector('.fy-content').replaceWith(next);
    ['.fy-mast', '.fy-guide', '.fy-mini'].forEach(sel => swapOne(doc, sel));
    const data = document.getElementById('fy-data');
    const nextData = doc.getElementById('fy-data');
    if (data && nextData) data.textContent = nextData.textContent;
    // the item's view beacon (f0ck-core.js counts the view from it)
    document.querySelectorAll('img.f0ck-view-beacon').forEach(b => b.remove());
    const beacon = doc.querySelector('img.f0ck-view-beacon');
    if (beacon) document.body.appendChild(document.importNode(beacon, true));
    document.title = doc.title;
    document.body.className = doc.body.className;
    // stylesheets the new page uses that this one didn't load
    const haveCss = new Set([...document.querySelectorAll('link[rel="stylesheet"][href]')].map(l => new URL(l.getAttribute('href'), location.href).pathname));
    doc.querySelectorAll('link[rel="stylesheet"][href]').forEach(l => {
      const path = new URL(l.getAttribute('href'), location.href).pathname;
      if (!haveCss.has(path)) document.head.appendChild(document.importNode(l, true));
    });
    for (const sc of scripts) await loadOnce(sc.getAttribute('src'));

    if (push) history.pushState({ fy: 1 }, '', finalUrl.href);
    else history.replaceState({ ...(history.state || {}), fy: 1 }, '', finalUrl.href);
    shown = pathKey(finalUrl.href);
    const hashEl = finalUrl.hash.length > 1 ? document.getElementById(decodeURIComponent(finalUrl.hash.slice(1))) : null;
    if (typeof scroll === 'number') window.scrollTo(0, scroll);
    else if (hashEl) hashEl.scrollIntoView();
    else window.scrollTo(0, 0);

    setupPage();
    window.f0ckCountViewFromPage?.();
    document.documentElement.classList.remove('fy-navigating');
  }

  if ('scrollRestoration' in history) history.scrollRestoration = 'manual';
  try { history.replaceState({ ...(history.state || {}), fy: 1 }, ''); } catch { /* ignore */ }

  // links of the site: in place (a new tab, a download, a dialog link or another site as usual)
  document.addEventListener('click', e => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = e.target.closest?.('a[href]');
    if (!a || (a.target && a.target !== '_self') || a.hasAttribute('download')) return;
    const url = new URL(a.href, location.href);
    if (url.origin !== location.origin || FULL.test(url.pathname)) return;
    // the same page with a #part: a dialog (:target) or a jump
    if (pathKey(url.href) === pathKey(location.href) && url.hash) return;
    e.preventDefault();
    go(url.href);
  });
  // the search box (GET /search): in place too
  document.addEventListener('submit', e => {
    const form = e.target;
    if (e.defaultPrevented || !(form instanceof HTMLFormElement) || !form.matches('form.fy-search')) return;
    e.preventDefault();
    const url = new URL(form.getAttribute('action') || '/search', location.href);
    for (const [k, v] of new FormData(form)) if (typeof v === 'string' && v !== '') url.searchParams.append(k, v);
    go(url.href);
  });
  // back / forward
  window.addEventListener('popstate', e => {
    // only a #part changed (a dialog opened or closed): nothing to load
    if (pathKey(location.href) === shown) return;
    go(location.href, { push: false, scroll: e.state && typeof e.state.fyScroll === 'number' ? e.state.fyScroll : null });
  });

  setupPage();
})();
