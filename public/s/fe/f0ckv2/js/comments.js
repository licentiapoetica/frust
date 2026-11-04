// f0ckv2 frontend: comments under an item (f0ckm's /api/comments), rendered in f0ckv2's style.
(() => {
  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  // escaped text -> links, >>quotes, >greentext, @mentions, line breaks, and the comment effects
  // [rb] [reich] [krebs] [glitch] shared with every frontend (f0ck-core.js, /s/css/comment-fx.css)
  // custom emojis :name: (shared loader in f0ck-core.js; loaded before the first render)
  let emojis = null;
  const emojiReady = () => (window.f0ckLoadEmojis ? window.f0ckLoadEmojis().then(m => { emojis = m; }) : Promise.resolve());
  const fx = html => {
    if(window.f0ckCommentFx) html = window.f0ckCommentFx(html);
    return window.f0ckEmojify ? window.f0ckEmojify(html, emojis) : html;
  };
  const format = raw => fx(esc(raw).split('\n').map(line => {
    let l = line
      .replace(/(^|\s)(\/c\/[\w.-]+\.(?:png|jpe?g|gif|webp|avif))(#gif)?(?=\s|$)/g, (m, pre, u) => `${pre}<a href="${u}" target="_blank"><img class="v2-comment-img" src="${u}" alt="" loading="lazy"></a>`)
      // [text](/local/path): f0ckm writes @mentions like that
      .replace(/\[([^\]\n]+)\]\((\/(?![\/\\])[^\s)"<]*)\)/g, (m, t, u) => `<a href="${u}">${t}</a>`)
      .replace(/(https?:\/\/[^\s<]+)/g, u => `<a href="${u}" target="_blank" rel="noopener noreferrer nofollow">${u}</a>`)
      .replace(/&gt;&gt;(\d+)/g, (m, id) => `<a href="#c${id}" class="v2-quote" data-quote="${id}">&gt;&gt;${id}</a>`)
      .replace(/(^|\s)@([A-Za-z0-9_-]{2,32})/g, (m, pre, u) => `${pre}<a href="/user/${encodeURIComponent(u.toLowerCase())}">@${u}</a>`);
    if(/^&gt;(?!&gt;)/.test(line)) l = `<span class="v2-greentext">${l}</span>`;
    return l;
  }).join('<br>'));

  // server-rendered comment texts (user comment pages): format them the same way
  // (once the page is parsed: f0ck-core.js, with the comment effects, loads after this file)
  const formatRaw = () => emojiReady().then(() => document.querySelectorAll('.v2-comment-body[data-raw]').forEach(el => {
    el.innerHTML = format(el.dataset.raw);
  }));
  if(document.readyState === 'loading') document.addEventListener('DOMContentLoaded', formatRaw);
  else formatRaw();

  const box = document.getElementById('comments');
  if(!box) return;
  const itemId = box.dataset.itemId;
  const me = (box.dataset.me || '').toLowerCase();
  const isMod = box.dataset.mod === '1';
  const canDelete = box.dataset.canDelete === '1';
  const list = document.getElementById('comment-list');
  const countEl = document.getElementById('comment-count');
  const form = document.getElementById('comment-form');
  const csrf = () => document.querySelector('meta[name="csrf-token"]')?.content || '';
  let sort = 'new';

  const ago = iso => {
    const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
    const units = [['year', 31536000], ['month', 2592000], ['week', 604800], ['day', 86400], ['hour', 3600], ['minute', 60]];
    for(const [n, sec] of units) {
      const v = Math.floor(s / sec);
      if(v >= 1) return `${v} ${n}${v === 1 ? '' : 's'} ago`;
    }
    return 'just now';
  };

  const avatar = c => (c.avatar_file && c.avatar_file !== 'default.png') ? `/a/${encodeURIComponent(c.avatar_file)}` : (c.avatar ? `/t/${+c.avatar}.webp` : '/s/fe/f0ckv2/img/default.png');

  const render = comments => {
    const byParent = new Map();
    comments.forEach(c => {
      const k = c.parent_id || 0;
      if(!byParent.has(k)) byParent.set(k, []);
      byParent.get(k).push(c);
    });
    const ids = new Set(comments.map(c => c.id));
    const roots = comments.filter(c => !c.parent_id || !ids.has(c.parent_id));
    const one = (c, depth) => {
      const name = c.is_anon ? `anonymous${c.anon_short_fingerprint ? ' #' + c.anon_short_fingerprint : ''}` : (c.display_name || c.username);
      const own = me && (c.username || '').toLowerCase() === me;
      const files = (c.files || []).filter(f => f.dest && !String(c.content || '').includes(f.dest)).map(f => {
        const url = `/c/${encodeURIComponent(f.dest)}`;
        return /^image\//.test(f.mime || '')
          ? `<a href="${url}" target="_blank"><img class="v2-comment-img" src="${url}" alt="${esc(f.original_filename || '')}" loading="lazy"></a>`
          : `<a class="badge badge-dark" href="${url}" target="_blank">${esc(f.original_filename || f.dest)}</a>`;
      }).join(' ');
      const kids = (byParent.get(c.id) || []).map(k => one(k, depth + 1)).join('');
      return `<div class="v2-comment${c.is_pinned ? ' pinned' : ''}${depth ? ' reply' : ''}" id="c${c.id}">
        <div class="v2-comment-head">
          ${c.is_anon ? `<img class="v2-comment-ava" src="/s/fe/f0ckv2/img/default.png" alt="">` : `<a href="/user/${encodeURIComponent((c.username || '').toLowerCase())}"><img class="v2-comment-ava" src="${avatar(c)}" alt=""></a>`}
          ${c.is_anon ? `<span class="v2-comment-name">${esc(name)}</span>` : `<a class="v2-comment-name" href="/user/${encodeURIComponent((c.username || '').toLowerCase())}">${esc(name)}</a>`}
          <a href="#c${c.id}" class="v2-comment-id"${form ? ` data-reply="${c.id}" data-name="${esc(name)}" title="reply"` : ''}>#${c.id}</a>
          <time class="v2-comment-time" tooltip="${esc(new Date(c.created_at).toLocaleString())}">${ago(c.created_at)}</time>
          ${c.is_pinned ? '<span class="badge badge-light">pinned</span>' : ''}
          <span class="v2-comment-actions">
            ${form ? `<a href="#" data-reply="${c.id}" data-name="${esc(name)}" title="reply">[re]</a>` : ''}
            ${!c.is_deleted && (isMod || (own && canDelete)) ? `<a href="#" data-delete="${c.id}" title="delete">[del]</a>` : ''}
          </span>
        </div>
        <div class="v2-comment-body">${c.is_deleted ? '<i>[deleted]</i>' : format(c.content)}${files ? `<div class="v2-comment-files">${files}</div>` : ''}${c.poll ? '<div class="badge badge-light">poll</div>' : ''}</div>
        ${kids ? `<div class="v2-comment-replies">${kids}</div>` : ''}
      </div>`;
    };
    list.innerHTML = roots.length ? roots.map(c => one(c, 0)).join('') : '<p class="v2-empty">no comments yet</p>';
    countEl.textContent = `(${comments.filter(c => !c.is_deleted).length})`;
  };

  // <danmaku> comments flying over the video (f0ckm's /s/js/danmaku.js on f0ckm's v0ck player; on/off
  // in the player's settings menu). Comments that arrive after the first load fly in right away.
  let seenIds = null;
  const playerMedia = () => document.querySelector('.v0ck video, .v0ck audio');
  const danmaku = comments => {
    if(typeof window.Danmaku === 'undefined') return;
    const player = document.querySelector('.v0ck');
    const media = playerMedia();
    if(!player || !media) {
      // the player is started by f0ck.js at the end of the page: try again once it ran
      if(document.readyState === 'loading') document.addEventListener('DOMContentLoaded', () => danmaku(comments), { once: true });
      return;
    }
    let d = window.danmakuInstance;
    if(d && (d.player !== player || !document.contains(d.player))) { d.destroy(); d = window.danmakuInstance = null; }
    if(!d) d = window.danmakuInstance = new window.Danmaku(player, media);
    const live = comments.filter(c => !c.is_deleted);
    const fresh = seenIds ? live.filter(c => !seenIds.has(c.id)) : [];
    seenIds = new Set(live.map(c => c.id));
    const feed = () => {
      d.load(live);
      fresh.forEach(c => d.fire(c.content, c.display_name || c.username || '?', c.username_color || null));
    };
    if(isFinite(media.duration) && media.duration > 0) feed();
    else media.addEventListener('loadedmetadata', feed, { once: true });
  };
  // </danmaku>

  const load = async () => {
    try {
      const res = await (await fetch(`/api/comments/${encodeURIComponent(itemId)}?sort=${sort}`, { credentials: 'same-origin' })).json();
      if(!res.success) return;
      await emojiReady();
      let comments = res.comments || [];
      comments.sort((a, b) => (b.is_pinned - a.is_pinned) || (sort === 'old' ? 1 : -1) * (new Date(a.created_at) - new Date(b.created_at)));
      render(comments);
      danmaku(comments);
      if(/^#c\d+$/.test(location.hash)) {
        box.open = true;
        document.getElementById(location.hash.slice(1))?.scrollIntoView();
      }
    } catch(err) {
      console.error(err);
    }
  };

  // collapsed by default (<details>): a link to a comment opens it
  window.addEventListener('hashchange', () => { if(/^#c\d+$/.test(location.hash)) box.open = true; });

  box.querySelectorAll('[data-sort]').forEach(a => a.addEventListener('click', e => {
    e.preventDefault();
    sort = a.dataset.sort;
    box.querySelectorAll('[data-sort]').forEach(x => x.classList.toggle('active', x === a));
    load();
  }));

  list.addEventListener('click', async e => {
    const reply = e.target.closest('[data-reply]');
    const del = e.target.closest('[data-delete]');
    const quote = e.target.closest('[data-quote]');
    if(reply && form) {
      e.preventDefault();
      form.parent_id.value = reply.dataset.reply;
      const r = form.querySelector('.v2-reply-to');
      r.hidden = false;
      r.querySelector('span').textContent = `${reply.dataset.name} (#${reply.dataset.reply})`;
      form.content.focus();
    }
    else if(del) {
      e.preventDefault();
      if(!confirm('delete this comment?')) return;
      const res = await (await fetch(`/api/comments/${del.dataset.delete}/delete`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf() },
        body: JSON.stringify({ csrf_token: csrf() })
      })).json().catch(() => ({}));
      if(!res.success) alert(res.message || res.msg || 'could not delete');
      load();
    }
    else if(quote) {
      const target = document.getElementById('c' + quote.dataset.quote);
      if(target) {
        e.preventDefault();
        target.scrollIntoView({ block: 'center' });
        target.classList.add('v2-flash');
        setTimeout(() => target.classList.remove('v2-flash'), 1200);
      }
    }
  });

  if(form) {
    form.querySelector('.v2-reply-cancel')?.addEventListener('click', e => {
      e.preventDefault();
      form.parent_id.value = '';
      form.querySelector('.v2-reply-to').hidden = true;
    });
    form.content.addEventListener('keydown', e => {
      if(e.key === 'Enter' && (e.ctrlKey || e.metaKey)) form.requestSubmit();
    });
    form.addEventListener('submit', async e => {
      e.preventDefault();
      const content = form.content.value.trim();
      if(!content) return;
      if(window.f0ckCaptchaGate) {
        try { await window.f0ckCaptchaGate('comment'); } catch { return; }
      }
      const btn = form.querySelector('button[type="submit"]');
      btn.disabled = true;
      try {
        const res = await (await fetch('/api/comments', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf() },
          body: JSON.stringify({
            item_id: itemId,
            parent_id: form.parent_id.value || null,
            content,
            // where in the video it was written: danmaku shows it there again
            video_time: (m => (m && m.currentTime > 0 ? m.currentTime : undefined))(playerMedia()),
            csrf_token: csrf()
          })
        })).json();
        if(!res.success) {
          alert(res.message || res.msg || 'could not post comment');
        }
        else {
          form.content.value = '';
          form.parent_id.value = '';
          form.querySelector('.v2-reply-to').hidden = true;
          await load();
        }
      } catch(err) {
        alert('could not post comment');
      }
      btn.disabled = false;
    });
  }

  // live event (f0ck-frontend.js): a comment was posted, edited or deleted somewhere; reload if it's
  // this item's (not while a reply is being typed)
  const fe = window.f0ckFrontend = window.f0ckFrontend || {};
  fe.comments = (data) => {
    if(String(data?.item_id) !== String(itemId)) return;
    if(form && form.content === document.activeElement && form.content.value.trim()) return;
    load();
  };

  load();
})();
