// f0ckgle frontend: the extras on top of pages that work without JavaScript.
//  - live actions: the plain forms marked .fg-ajax (comments, replies, favs, follow, rating, tags,
//    report, delete, halls) are sent with fetch instead; the parts of the page they change
//    (data-fg-refresh → [data-fg-live] regions) are swapped in from a fresh render of the page,
//    so nothing reloads and the markup stays the server's. Live comments use the same refresh.
//  - the f0ckFrontend hooks for the shared live events (views/frontends/README.md)
//  - menus (<details>): one open at a time, closed by an outside click or Escape
//  - keys: "/" focuses the search box; on an item ← → for previous / next, r for random
//  - custom emojis :name: in comments as pictures, and the emoji picker
//  - videos: f0ckm's player (v0ck); Flash items: starts the Ruffle player
//  - moderators: the × on a tag removes it
//  - comment polls: voting and live redraws are f0ck-core.js' (f0ckPoll, a capture listener that
//    stops the submit before the one here); a new comment's poll fields are sent with it
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

  // ── custom emojis :name: → pictures in comment texts (the server sends them as text). Only text
  //    nodes are touched, so links and attributes stay as they are (f0ckLoadEmojis: f0ck-core.js) ──
  const EMOJI = /:([a-z0-9_-]+):/g;
  const HAS_EMOJI = /:[a-z0-9_-]+:/; // .test() on the /g one would keep its lastIndex
  const emojify = async (root) => {
    if (!root || !window.f0ckLoadEmojis) return;
    const bodies = root.matches?.('.fg-comment-body') ? [root] : [...root.querySelectorAll('.fg-comment-body')];
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
          const img = document.createElement('img');
          img.className = 'f0ck-emoji';
          img.src = map[name];
          img.alt = img.title = m;
          img.loading = 'lazy';
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
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', () => emojify(document.body));
  else emojify(document.body);
  window.addEventListener('f0ck:emojis_updated', () => {
    document.querySelectorAll('.fg-emoji-box').forEach(box => box.replaceChildren());
    emojify(document.body);
  });

  // the emoji picker under a comment field (the comment form, a comment being edited): inserts
  // :name: at the cursor of the field of its form
  document.addEventListener('click', async e => {
    const btn = e.target.closest('.fg-emoji-btn');
    const box = btn && document.getElementById(btn.getAttribute('aria-controls'));
    if (!btn || !box) return;
    const ta = btn.closest('form')?.querySelector('textarea');
    if (!box.childElementCount) {
      const map = (await window.f0ckLoadEmojis?.()) || {};
      const names = Object.keys(map).sort();
      if (!names.length) {
        const p = document.createElement('span');
        p.className = 'fg-emoji-empty';
        p.textContent = 'No emojis yet';
        box.append(p);
      }
      names.forEach(name => {
        const b = document.createElement('button');
        b.type = 'button';
        b.title = `:${name}:`;
        const img = document.createElement('img');
        img.src = map[name];
        img.alt = `:${name}:`;
        img.loading = 'lazy';
        b.append(img);
        b.addEventListener('click', () => {
          if (!ta) return;
          const before = ta.value.slice(0, ta.selectionStart);
          const pad = before && !/\s$/.test(before) ? ' ' : '';
          ta.setRangeText(`${pad}:${name}: `, ta.selectionStart, ta.selectionEnd, 'end');
          ta.focus();
        });
        box.append(b);
      });
    }
    box.hidden = !box.hidden;
    btn.setAttribute('aria-expanded', String(!box.hidden));
  });

  // ── mods edit a comment in place, as in f0ckm (snippets/comments.html): Edit swaps the text for
  //    the field with the emoji picker, Cancel brings the text back, Save (the .fg-ajax form) sends
  //    it and the list is swapped in. The links work without JavaScript too (#edit-c<id>). ──
  const endEdit = (form, restore = true) => {
    const ta = form.elements.content;
    if (restore) ta.value = ta.defaultValue;
    const box = form.querySelector('.fg-emoji-box');
    if (box) box.hidden = true;
    form.querySelector('.fg-emoji-btn')?.setAttribute('aria-expanded', 'false');
    form.closest('.fg-comment')?.classList.remove('fg-editing');
  };
  document.addEventListener('click', e => {
    const open = e.target.closest('.fg-edit-link');
    const cancel = !open && e.target.closest('.fg-edit-cancel');
    if (!open && !cancel) return;
    e.preventDefault();
    if (cancel) {
      endEdit(cancel.closest('form'));
      return;
    }
    const form = document.getElementById(`edit-c${open.dataset.edit}`);
    if (!form) return;
    // one at a time
    document.querySelectorAll('.fg-comment.fg-editing .fg-comment-edit').forEach(f => { if (f !== form) endEdit(f); });
    form.closest('.fg-comment').classList.add('fg-editing');
    const ta = form.elements.content;
    ta.focus({ preventScroll: true });
    ta.setSelectionRange(ta.value.length, ta.value.length);
  });

  // ── live regions: swap [data-fg-live="name"] in from a fresh render of this page ──
  const pageUrl = () => {
    const u = new URL(location.href);
    u.searchParams.delete('reply');
    // ?meta=1 reads the file's metadata (the tag suggestions): not for every refresh
    u.searchParams.delete('meta');
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
        if (cur && next) {
          const el = document.importNode(next, true);
          // a comment being edited stays in edit: its field (text, cursor, emoji picker) goes along
          cur.querySelectorAll('.fg-comment.fg-editing').forEach(c => {
            const form = c.querySelector(':scope > .fg-comment-edit');
            const twin = el.querySelector(`#${c.id} > .fg-comment-edit`);
            if (!form || !twin) return;
            twin.replaceWith(form);
            form.closest('.fg-comment').classList.add('fg-editing');
          });
          cur.replaceWith(el);
          emojify(el);
        }
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
  // body: an object (sent as JSON) or FormData (a file upload, sent as multipart)
  const send = async (url, body, method = 'POST') => {
    const multipart = body instanceof FormData;
    const res = await fetch(url, {
      method,
      credentials: 'same-origin',
      headers: multipart
        ? { Accept: 'application/json', 'X-CSRF-Token': csrf() }
        : { 'Content-Type': 'application/json', Accept: 'application/json', 'X-CSRF-Token': csrf() },
      body: multipart ? body : JSON.stringify({ ...body, csrf_token: body.csrf_token || csrf() }),
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
    let body = formBody(form);
    // a file upload (a hall's picture): the form as it is, without the no-JS return address
    if (form.enctype === 'multipart/form-data') {
      body = new FormData(form);
      body.delete('back');
    }
    if (isComment) {
      body.content = String(body.content || '').trim();
      // a comment may be just a poll (its fields: snippets/comment-poll-builder.html, sent as they
      // are; an invalid poll comes back as an error and the comment is not posted)
      if (!body.content && !String(body.poll_question || '').trim()) return;
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
        // the poll fields: empty again (anonymous checked, as rendered) and folded away
        form.querySelectorAll('.poll-builder-nojs').forEach(d => {
          d.querySelectorAll('input, textarea').forEach(f => {
            if (f.type === 'checkbox') f.checked = f.defaultChecked;
            else f.value = '';
          });
          d.open = false;
        });
      } else if (form.elements.tagname) {
        form.elements.tagname.value = '';
      }
      if ('fgClose' in form.dataset) {
        form.reset();
        if (location.hash) location.hash = '_';
      }
      if ('fgClear' in form.dataset) form.reset();
      // a tag suggestion that is a tag now: checked, no second click
      if ('fgAdded' in form.dataset) form.classList.add('is-added');
      if (form.dataset.fgOk) flash(form.dataset.fgOk, 'success');
      // an edited comment: back to its (new) text
      if ('fgEdit' in form.dataset) endEdit(form, false);
      await refresh((form.dataset.fgRefresh || '').split(/\s+/).filter(Boolean));
      // a hall's new picture has the same address: load it again
      if (form.dataset.fgBust) {
        const v = Date.now();
        document.querySelectorAll('img[data-fg-hall-img]').forEach(img => {
          if (img.dataset.fgHallImg !== form.dataset.fgBust) return;
          const u = new URL(img.src, location.href);
          u.searchParams.set('v', v);
          img.src = u.href;
        });
      }
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
      buttons.forEach(b => { if (b.isConnected && !form.classList.contains('is-added')) b.disabled = false; });
    }
  });

  // a picked file is uploaded at once (without JavaScript the form's Upload button sends it)
  document.addEventListener('change', e => {
    const input = e.target;
    if (input instanceof HTMLInputElement && input.type === 'file' && 'fgAutosubmit' in input.dataset && input.files?.length) {
      input.form?.requestSubmit();
    }
  });

  // Tag suggestions (Edit dialog → #meta): the server reads the file's metadata for the page's
  // ?meta=1 render (without JavaScript the link loads it); here its dialog is opened at once and
  // filled from that render. Its contents are swapped, not the dialog, so it stays :target.
  document.addEventListener('click', async e => {
    const a = e.target.closest?.('a[data-fg-meta]');
    if (!a || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const box = document.querySelector('[data-fg-live="meta"]');
    if (!box) return;
    e.preventDefault();
    location.hash = 'meta';
    box.classList.add('fg-meta-busy');
    try {
      const u = new URL(a.href, location.href);
      u.hash = '';
      // X-Purpose: prefetch: not a view of the f0ck (routes/index.rs)
      const res = await fetch(u.href, { credentials: 'same-origin', cache: 'no-store', headers: { Accept: 'text/html', 'X-Purpose': 'prefetch' } });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const next = new DOMParser().parseFromString(await res.text(), 'text/html').querySelector('[data-fg-live="meta"]');
      const cur = document.querySelector('[data-fg-live="meta"]');
      if (next && cur) {
        cur.replaceChildren(...[...next.childNodes].map(n => document.importNode(n, true)));
      }
    } catch (err) {
      flash(`Couldn't read the metadata: ${err.message}`, 'error');
    } finally {
      document.querySelector('[data-fg-live="meta"]')?.classList.remove('fg-meta-busy');
    }
  }, true);

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
    // plain text; `[label](#bad-apple)` becomes the link that starts Bad Apple!! (f0ck-core.js)
    motd(text) {
      document.querySelectorAll('.fg-motd').forEach(el => {
        const t = el.querySelector('.fg-motd-text');
        t.textContent = text || '';
        window.f0ckMotdLinks?.(t);
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
  document.querySelectorAll('.fg-motd-text').forEach(el => window.f0ckMotdLinks?.(el));

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
      if (location.hash === '#meta') location.hash = 'edit';
      // a confirmation goes back to the dialog it came from (a hall's delete → its Edit dialog)
      else if (/^#hall-.+-delete$/.test(location.hash)) location.hash = location.hash.slice(0, -'-delete'.length);
      else if (document.querySelector('.fg-modal:target')) location.hash = '_';
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

  // ── videos play in f0ckm's player (v0ck.js, loaded on a video's page): its controls replace the
  //    native ones (kept in the page for visitors without JavaScript), it keeps the volume and
  //    starts the video unless autoplay is off. Autonext (its settings menu, as on f0ckm): at the
  //    end go to the next f0ck instead of looping (v0ck turns `loop` off while it is on) ──
  const video = document.querySelector('.fg-player > video#my-video');
  if (video && window.v0ck) {
    const autonext = () => { try { return localStorage.getItem('autoplay') === 'true'; } catch { return false; } };
    if (!window.toggleAutoplay) {
      window.toggleAutoplay = () => {
        try { localStorage.setItem('autoplay', autonext() ? 'false' : 'true'); } catch { /* storage blocked */ }
        document.querySelectorAll('#toggleautoplay').forEach(el => el.classList.toggle('active', autonext()));
      };
    }
    new window.v0ck(video);
    video.addEventListener('ended', () => {
      if (autonext()) location.href = page.next || '/random';
    });
  }

  // ── Flash: the SWF plays in f0ckm's sandboxed Ruffle frame (<f0ck-flash>, /s/js/flash-embed.js,
  //    loaded by the item page); this only starts it in #ruffle-container ──
  const ruffle = document.getElementById('ruffle-container');
  if (ruffle && ruffle.dataset.swf && window.customElements?.get('f0ck-flash')) {
    const player = document.createElement('f0ck-flash');
    ruffle.appendChild(player);
    player.addEventListener('loadedmetadata', () => {
      let vol = 0.5;
      try { vol = +(localStorage.getItem('volume') ?? 0.5); } catch { /* storage blocked */ }
      player.volume = Number.isFinite(vol) ? vol : 0.5;
    });
    player.load({ url: ruffle.dataset.swf, autoplay: true, background: true }).catch(() => {
      flash('Flash playback failed', 'error');
    });
  }

  // ── /tags: "More tags" appends the next page from /api/tags (cards as snippets/tag-cards.html) ──
  document.addEventListener('click', async e => {
    const btn = e.target.closest('[data-tags-more]');
    const grid = document.getElementById('fg-tag-grid');
    if (!btn || !grid) return;
    const page = (parseInt(btn.dataset.page, 10) || 1) + 1;
    btn.disabled = true;
    try {
      const res = await (await fetch(`/api/tags?page=${page}`, { credentials: 'same-origin', headers: { Accept: 'application/json' } })).json();
      const mode = grid.dataset.mode || '0';
      (res.tags || []).forEach(tt => {
        const a = document.createElement('a');
        a.className = 'fg-card';
        a.href = `/tag/${tt.safe_tag}`;
        const img = document.createElement('span');
        img.className = 'fg-card-img';
        img.innerHTML = `<img src="/tag_image/${tt.encoded_tag}?m=${encodeURIComponent(mode)}" alt="" loading="lazy">`;
        const src = document.createElement('span');
        src.className = 'fg-card-src';
        src.innerHTML = '<span class="fg-fav fg-fav-letter">#</span>';
        src.append(`${tt.total_items || 0} f0cks`);
        const title = document.createElement('span');
        title.className = 'fg-card-title';
        title.textContent = `#${tt.tag}`;
        a.append(img, src, title);
        grid.appendChild(a);
      });
      btn.dataset.page = String(page);
      if (!res.hasMore) btn.closest('.fg-more-wrap')?.remove();
    } catch {
      flash('Could not load more tags', 'error');
    } finally {
      btn.disabled = false;
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
