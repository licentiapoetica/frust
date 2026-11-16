// f0ckgle frontend: the extras on top of pages that work without JavaScript.
//  - live actions: the plain forms marked .fg-ajax (comments, replies, favs, follow, rating, tags,
//    report, delete) are sent with fetch instead; the parts of the page they change
//    (data-fg-refresh → [data-fg-live] regions) are swapped in from a fresh render of the page,
//    so nothing reloads and the markup stays the server's. Live comments use the same refresh.
//  - the f0ckFrontend hooks for the shared live events (views/frontends/README.md)
//  - menus (<details>): one open at a time, closed by an outside click or Escape
//  - keys: "/" focuses the search box; on an item ← → for previous / next, r for random
//  - moderators: the × on a tag removes it
(() => {
  'use strict';
  let page = {};
  try { page = JSON.parse(document.getElementById('fg-data')?.textContent || '{}') || {}; } catch { page = {}; }
  const csrf = () => document.querySelector('meta[name="csrf-token"]')?.content || '';

  // ── snackbar ──
  const flashBox = () => document.getElementById('fg-flash');
  const flash = (message, type) => {
    const box = flashBox();
    if (!box) return;
    const el = document.createElement('div');
    el.className = `fg-toast fg-toast-${type || 'info'}`;
    el.textContent = String(message ?? '');
    box.appendChild(el);
    setTimeout(() => el.remove(), 5000);
  };
  // the message of a form post without JavaScript (snippets/flash.html) goes too
  flashBox()?.querySelectorAll('.fg-toast').forEach(el => setTimeout(() => el.remove(), 6000));

  const shown = new Set();
  const once = (key, message, type) => {
    if (shown.has(key)) return;
    shown.add(key);
    flash(message, type);
  };

  // ── live regions: swap [data-fg-live="name"] in from a fresh render of this page ──
  const pageUrl = () => {
    const u = new URL(location.href);
    u.searchParams.delete('reply');
    u.hash = '';
    return u.toString();
  };
  let queue = Promise.resolve();
  const refresh = (names) => {
    const list = [...new Set(names)].filter(n => document.querySelector(`[data-fg-live="${n}"]`));
    if (!list.length) return queue;
    queue = queue.then(async () => {
      const res = await fetch(pageUrl(), { credentials: 'same-origin', cache: 'no-store', headers: { Accept: 'text/html' } });
      if (!res.ok) return;
      const doc = new DOMParser().parseFromString(await res.text(), 'text/html');
      list.forEach(n => {
        const cur = document.querySelector(`[data-fg-live="${n}"]`);
        const next = doc.querySelector(`[data-fg-live="${n}"]`);
        if (cur && next) cur.replaceWith(document.importNode(next, true));
      });
    }).catch(err => console.error('[f0ckgle] refresh', err));
    return queue;
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

  const replyBar = (form) => form.querySelector('.fg-reply-to');
  const setReply = (form, id, name) => {
    form.elements.parent_id.value = id || '';
    const bar = replyBar(form);
    if (!bar) return;
    bar.hidden = !id;
    bar.querySelector('span').textContent = id ? `${name ? name + ' ' : ''}#${id}` : '';
  };

  document.addEventListener('submit', async e => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement) || !form.classList.contains('fg-ajax')) return;
    e.preventDefault();
    if (form.dataset.fgConfirm && !confirm(form.dataset.fgConfirm)) return;
    const isComment = 'fgComment' in form.dataset;
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
      // getAttribute: an input named "action" (the fav form) shadows form.action
      const data = await send(form.getAttribute('action'), body);
      if ('fgGo' in form.dataset) {
        location.href = form.elements.back?.value || '/';
        return;
      }
      if (isComment) {
        form.elements.content.value = '';
        setReply(form, '', '');
      } else if (form.elements.tagname) {
        form.elements.tagname.value = '';
      }
      if ('fgClose' in form.dataset) {
        form.reset();
        if (location.hash) location.hash = '_';
      }
      if (form.dataset.fgOk) flash(form.dataset.fgOk, 'success');
      await refresh((form.dataset.fgRefresh || '').split(/\s+/).filter(Boolean));
      const id = data?.comment?.id;
      const el = id && document.getElementById(`c${id}`);
      if (el) {
        el.classList.add('fg-comment-target');
        el.scrollIntoView({ block: 'center', behavior: 'smooth' });
      }
    } catch (err) {
      flash(err.message || 'Something went wrong', 'error');
      if ('fgClose' in form.dataset) window.f0ckCapReset?.();
    } finally {
      buttons.forEach(b => { if (b.isConnected) b.disabled = false; });
    }
  });

  // reply: fill the form's parent_id in place (without JavaScript the link reloads with ?reply=<id>)
  document.addEventListener('click', e => {
    const reply = e.target.closest('[data-reply]');
    const cancel = e.target.closest('.fg-reply-cancel');
    const form = document.getElementById('comment-form');
    if (!form || !(reply || cancel)) return;
    e.preventDefault();
    if (cancel) return setReply(form, '', '');
    setReply(form, reply.dataset.reply, reply.dataset.name);
    form.scrollIntoView({ block: 'center', behavior: 'smooth' });
    form.elements.content.focus();
  });
  // Ctrl/Cmd+Enter posts a comment
  document.addEventListener('keydown', e => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && e.target.closest?.('#comment-form textarea')) {
      e.preventDefault();
      document.getElementById('comment-form').requestSubmit();
    }
  });

  window.f0ckFrontend = {
    itemId: page.itemId || null,
    flash,
    motd(text) {
      document.querySelectorAll('.fg-motd').forEach(el => {
        el.textContent = text || '';
        el.hidden = !String(text || '').trim();
      });
    },
    comments(data) {
      if (!page.itemId || String(data?.item_id) !== String(page.itemId)) return;
      refresh(['comments', 'comments-head']);
    },
    notify() {
      flash('You have a new notification', 'info');
    },
    newItem() {
      // only on the first page of a listing, where it would show up
      if (!document.querySelector('.fg-grid') || document.querySelector('.fg-pages [rel="prev"]')) return;
      once('new', 'New results are available. Reload to see them.', 'info');
    },
    dmBadge() {
      const badge = document.getElementById('dm-badge');
      if (!badge) return;
      badge.textContent = String((parseInt(badge.textContent, 10) || 0) + 1);
      badge.classList.add('fg-on');
    },
    reconnected() {
      if (page.itemId) refresh(['comments', 'comments-head', 'actions', 'kp']);
    },
  };

  // ── menus ──
  const menus = () => [...document.querySelectorAll('details.fg-menu')];
  menus().forEach(d => d.addEventListener('toggle', () => {
    if (d.open) menus().forEach(o => { if (o !== d) o.open = false; });
  }));
  document.addEventListener('click', e => {
    menus().forEach(d => { if (d.open && !d.contains(e.target)) d.open = false; });
  });

  // ── keys ──
  const typing = t => t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));
  document.addEventListener('keydown', e => {
    if (e.key === 'Escape') {
      menus().forEach(d => { d.open = false; });
      if (/^#(report|delete)$/.test(location.hash)) location.hash = '_';
      return;
    }
    if (typing(e.target) || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === '/') {
      const q = document.querySelector('.fg-search input[name="tag"]');
      if (q) { e.preventDefault(); q.focus(); q.select(); }
    } else if (page.itemId && e.key === 'ArrowLeft' && page.prev) {
      location.href = page.prev;
    } else if (page.itemId && e.key === 'ArrowRight' && page.next) {
      location.href = page.next;
    } else if (page.itemId && e.key === 'r') {
      location.href = '/random';
    }
  });

  // ── moderators: remove a tag (DELETE has no form; the panel is refreshed in place) ──
  document.addEventListener('click', async e => {
    const btn = e.target.closest('#tags .fg-tag-del');
    if (!btn) return;
    const tag = btn.closest('[data-tag]')?.dataset.tag;
    const id = document.getElementById('tags')?.dataset.itemId;
    if (!tag || !id || !confirm(`Remove the tag "${tag}"?`)) return;
    btn.disabled = true;
    try {
      await send(`/api/v2/tags/${id}/${encodeURIComponent(tag)}`, {}, 'DELETE');
      await refresh(['kp']);
    } catch (err) {
      btn.disabled = false;
      flash(err.message || 'could not remove the tag', 'error');
    }
  });
})();
