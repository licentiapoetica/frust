// 4chan thread overview (views/chan/thread.html). Without JavaScript the filters are links (?show=)
// and rehost / favourite are plain forms (nojs.rs brings them back to the post). Here everything
// happens in place: filters switch at once, the forms post with fetch and the post's box changes to
// its new state.
//
// The page is loaded in place (f0ckm.js), so this file runs again for every thread: the listeners
// live on the document and are bound once; only the jump to the active post runs per page.
(() => {
  'use strict';

  const page = () => document.querySelector('#main.chan-thread-page');

  const flash = (msg, type) => {
    if (typeof window.showFlash === 'function') window.showFlash(msg, type || 'success');
    else if (typeof window.flashMessage === 'function') window.flashMessage(msg, 3000, type);
  };

  const csrf = (form) => (window.f0ckSession && window.f0ckSession.csrf_token)
    || (form && form.querySelector('input[name="csrf_token"]') ? form.querySelector('input[name="csrf_token"]').value : '');

  const icon = (cls) => {
    const i = document.createElement('i');
    i.className = cls;
    return i;
  };

  // A form posted with fetch; the reply as JSON (`back` is only for the no-JS redirect)
  async function post(form) {
    const body = new URLSearchParams(new FormData(form));
    body.delete('back');
    const res = await fetch(form.getAttribute('action'), {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
        'Accept': 'application/json',
        'X-CSRF-Token': csrf(form)
      },
      body: body.toString(),
      credentials: 'same-origin'
    });
    const data = await res.json().catch(() => null);
    if (!data) throw new Error('Server returned ' + res.status);
    if (!data.success) throw new Error(data.msg || data.message || 'Something went wrong');
    return data;
  }

  // ── Filters: all posts / media only / rehosted ───────────────────────────
  function setFilter(show) {
    const main = page();
    if (!main) return;
    main.querySelectorAll('.chan-toggle-btn[data-show]').forEach((b) => b.classList.toggle('active', b.dataset.show === show));
    main.querySelectorAll('.chan-post-card').forEach((card) => {
      const hide = (show === 'media' && card.dataset.hasMedia !== '1') || (show === 'rehosted' && card.dataset.isRehosted !== '1');
      card.classList.toggle('is-hidden', hide);
    });
    const qs = show === 'all' ? '' : '?show=' + show;
    history.replaceState(history.state, '', location.pathname + qs + location.hash);
  }

  // ── A post's rehost box after it went onto the site ──────────────────────
  function showRehosted(box, itemId, itemPath, faved) {
    const old = box.querySelector('form');
    const token = old ? (old.querySelector('input[name="csrf_token"]') || {}).value || '' : '';
    const back = old ? (old.querySelector('input[name="back"]') || {}).value || '' : '';
    const path = String(itemPath || itemId);

    const wrap = document.createElement('span');
    wrap.className = 'chan-rehosted-wrap';
    const link = document.createElement('a');
    link.href = '/' + path;
    link.className = 'chan-rehost-btn rehosted';
    link.target = '_blank';
    link.append(icon('fa-solid fa-check'), ' Rehosted /' + path);
    const edit = document.createElement('a');
    edit.href = '/' + path;
    edit.className = 'chan-rehost-edit-btn';
    edit.dataset.itemId = itemId;
    edit.title = 'Edit rating';
    edit.setAttribute('aria-label', 'Edit rating');
    edit.append(icon('fa-solid fa-pen'));
    wrap.append(link, edit);

    const form = document.createElement('form');
    form.method = 'post';
    form.action = '/api/v2/togglefav';
    form.className = 'chan-fav-form';
    [['csrf_token', token], ['back', back], ['postid', itemId], ['action', faved ? 'delete' : 'add']].forEach(([name, value]) => {
      const input = document.createElement('input');
      input.type = 'hidden';
      input.name = name;
      input.value = value;
      form.append(input);
    });
    const btn = document.createElement('button');
    btn.type = 'submit';
    btn.className = 'chan-card-fav-btn';
    form.append(btn);
    setFavButton(btn, faved);

    box.replaceChildren(wrap, form);

    const card = box.closest('.chan-post-card');
    if (card) {
      card.classList.add('is-rehosted');
      card.dataset.isRehosted = '1';
      const thumb = card.querySelector('.chan-thumb-link');
      if (thumb && !thumb.querySelector('.chan-thumb-rehost-badge')) {
        const badge = document.createElement('span');
        badge.className = 'chan-thumb-rehost-badge';
        badge.title = 'Rehosted on f0ckm as /' + path;
        badge.append(icon('fa-solid fa-check'), ' /' + path);
        thumb.append(badge);
      }
    }
    return edit;
  }

  function setFavButton(btn, faved) {
    btn.classList.toggle('favorited', faved);
    btn.replaceChildren(icon('fa-' + (faved ? 'solid' : 'regular') + ' fa-heart'));
    const label = faved ? 'Remove from favorites' : 'Favorite';
    btn.title = label;
    btn.setAttribute('aria-label', label);
  }

  async function rehost(form) {
    const btn = form.querySelector('button');
    if (!btn || btn.classList.contains('is-busy')) return;
    const before = Array.from(btn.childNodes, (n) => n.cloneNode(true));
    btn.classList.add('is-busy');
    // Same size while busy: only the icon turns into a spinner
    btn.replaceChildren(icon('fa-solid fa-circle-notch fa-spin'), ' Rehost');
    btn.setAttribute('aria-busy', 'true');
    try {
      const data = await post(form);
      const box = form.closest('.chan-rehost-container');
      const faved = !!(box && box.querySelector('.chan-card-fav-btn.favorited'));
      const edit = showRehosted(box, data.item_id, data.item_path, faved);
      flash(data.repost ? 'Already on the site: linked to the existing post' : 'Rehosted');
      if (typeof window.openChanRehostModal === 'function') {
        window.openChanRehostModal({ itemId: Number(data.item_id), anchorEl: edit });
      }
    } catch (err) {
      btn.classList.remove('is-busy');
      btn.removeAttribute('aria-busy');
      btn.replaceChildren(...before);
      flash(err.message || 'Rehosting failed', 'error');
    }
  }

  async function favourite(form) {
    const btn = form.querySelector('.chan-card-fav-btn');
    if (!btn || btn.classList.contains('is-busy')) return;
    const wasFaved = btn.classList.contains('favorited');
    const rehostFirst = !!form.querySelector('input[name="fav"]');
    // Shown at once; undone if the server says no
    setFavButton(btn, !wasFaved);
    btn.classList.add('is-busy');
    if (navigator.vibrate) navigator.vibrate(50);
    try {
      const data = await post(form);
      if (rehostFirst) {
        // Rehosted and added in one request (the rehost endpoint with fav=add)
        showRehosted(form.closest('.chan-rehost-container'), data.item_id, data.item_path, data.favorited !== false);
      } else {
        const faved = data.favorited !== undefined ? !!data.favorited : !wasFaved;
        setFavButton(btn, faved);
        const action = form.querySelector('input[name="action"]');
        if (action) action.value = faved ? 'delete' : 'add';
      }
      flash(data.msg || (wasFaved ? 'Removed from favorites' : 'Added to favorites'));
    } catch (err) {
      setFavButton(btn, wasFaved);
      flash(err.message || 'Could not update the favorite', 'error');
    } finally {
      btn.classList.remove('is-busy');
    }
  }

  // ── Media in the post: a thumbnail click plays the video / shows the image right there ──
  function expandMedia(thumb) {
    const box = thumb.closest('.chan-thumb-box');
    const content = thumb.closest('.chan-post-content');
    if (!box || box.querySelector('.chan-inline-media')) return;
    const src = thumb.dataset.inline;
    const wrap = document.createElement('div');
    wrap.className = 'chan-inline-media';
    let media;
    if (thumb.dataset.kind === 'video') {
      media = document.createElement('video');
      media.src = src;
      media.controls = true;
      media.autoplay = true;
      media.loop = true;
      media.playsInline = true;
      media.preload = 'auto';
    } else {
      media = document.createElement('img');
      media.src = src;
      media.alt = thumb.querySelector('img') ? thumb.querySelector('img').alt : '';
      media.className = 'chan-inline-collapse';
      media.title = 'Click to collapse';
    }
    const bar = document.createElement('div');
    bar.className = 'chan-inline-bar';
    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'chan-inline-btn chan-inline-collapse';
    close.append(icon('fa-solid fa-compress'), ' Collapse');
    const player = document.createElement('a');
    player.href = thumb.getAttribute('href');
    player.className = 'chan-inline-btn';
    player.append(icon('fa-solid fa-play'), ' Player view');
    bar.append(close, player);
    wrap.append(media, bar);
    thumb.classList.add('is-collapsed');
    box.classList.add('is-expanded');
    if (content) content.classList.add('has-inline-media');
    box.append(wrap);
  }

  function collapseMedia(box) {
    const wrap = box.querySelector('.chan-inline-media');
    if (!wrap) return;
    const video = wrap.querySelector('video');
    if (video) {
      // Stop it and its download
      video.pause();
      video.removeAttribute('src');
      video.load();
    }
    wrap.remove();
    box.classList.remove('is-expanded');
    const thumb = box.querySelector('.chan-thumb-link');
    if (thumb) thumb.classList.remove('is-collapsed');
    const content = box.closest('.chan-post-content');
    if (content) content.classList.remove('has-inline-media');
    const card = box.closest('.chan-post-card');
    // Keep the post in view when a tall video shrinks back to a thumbnail
    if (card && card.getBoundingClientRect().top < 0) scrollToPost(card, false);
  }

  // ── Quotes: jump with a flash, preview on hover ──────────────────────────
  const highlightPost = (el) => {
    if (!el) return;
    el.classList.remove('chan-flash-highlight');
    void el.offsetWidth;
    el.classList.add('chan-flash-highlight');
    setTimeout(() => el.classList.remove('chan-flash-highlight'), 2000);
  };

  const scrollToPost = (el, smooth) => {
    try {
      el.scrollIntoView({ behavior: smooth ? 'smooth' : 'instant', block: 'center' });
    } catch (_) {
      el.scrollIntoView();
    }
  };

  const quotedCard = (ref) => {
    const no = ref.dataset.post || (ref.getAttribute('href') || '').replace(/^#p?/, '');
    return no ? document.getElementById('p' + no) : null;
  };

  // The preview stays while the mouse is on the link or on the preview itself: leaving one starts
  // a short timer, arriving on the other stops it (the gap between them is crossed in time).
  let preview = null;
  let hideTimer = null;
  const hidePreview = () => {
    clearTimeout(hideTimer);
    if (preview) preview.classList.remove('is-shown');
  };
  const hideSoon = () => {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(hidePreview, 250);
  };
  const keepPreview = () => clearTimeout(hideTimer);

  function showPreview(ref, card) {
    if (!preview) {
      preview = document.createElement('div');
      preview.className = 'chan-quote-preview-popup';
      preview.setAttribute('aria-hidden', 'true');
      document.body.appendChild(preview);
    }
    preview.innerHTML = card.innerHTML;
    // A post shown expanded appears as its thumbnail (no second player in the preview)
    preview.querySelectorAll('.chan-inline-media').forEach((el) => el.remove());
    preview.querySelectorAll('.is-collapsed, .is-expanded, .has-inline-media').forEach((el) => el.classList.remove('is-collapsed', 'is-expanded', 'has-inline-media'));
    preview.classList.add('is-shown');
    const rect = ref.getBoundingClientRect();
    const h = preview.offsetHeight || 150;
    const w = preview.offsetWidth || 380;
    let top = rect.bottom + 8;
    if (top + h > window.innerHeight - 10) top = rect.top - h - 8;
    if (top < 10) top = 10;
    let left = rect.left;
    if (left + w > window.innerWidth - 10) left = window.innerWidth - w - 10;
    if (left < 10) left = 10;
    preview.style.setProperty('--chan-pop-x', left + 'px');
    preview.style.setProperty('--chan-pop-y', top + 'px');
  }

  function jumpTo(card, smooth) {
    if (card.classList.contains('is-hidden')) setFilter('all');
    scrollToPost(card, smooth);
    highlightPost(card);
  }

  // ── Bound once per document ──────────────────────────────────────────────
  if (!window.__f0ckChanThreadBound) {
    window.__f0ckChanThreadBound = true;

    document.addEventListener('submit', (e) => {
      const form = e.target.closest('#main.chan-thread-page form.chan-rehost-form, #main.chan-thread-page form.chan-fav-form');
      if (!form) return;
      e.preventDefault();
      if (form.classList.contains('chan-fav-form')) favourite(form);
      else rehost(form);
    });

    // Capture phase: these are links, which f0ckm.js' link handler would otherwise load as pages
    document.addEventListener('click', (e) => {
      if (!page() || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
      const handled = () => {
        e.preventDefault();
        e.stopPropagation();
      };

      const filter = e.target.closest('#main.chan-thread-page .chan-toggle-btn[data-show]');
      if (filter) {
        handled();
        setFilter(filter.dataset.show);
        return;
      }

      const thumb = e.target.closest('#main.chan-thread-page .chan-thumb-link[data-inline][data-kind]');
      if (thumb) {
        handled();
        expandMedia(thumb);
        return;
      }

      const collapse = e.target.closest('#main.chan-thread-page .chan-inline-collapse');
      if (collapse) {
        handled();
        collapseMedia(collapse.closest('.chan-thumb-box'));
        return;
      }

      const edit = e.target.closest('#main.chan-thread-page .chan-rehost-edit-btn[data-item-id]');
      if (edit && typeof window.openChanRehostModal === 'function') {
        handled();
        window.openChanRehostModal({ itemId: Number(edit.dataset.itemId), anchorEl: edit });
        return;
      }

      const ref = e.target.closest('#main.chan-thread-page .chan-ref, #main.chan-thread-page .quotelink, .chan-quote-preview-popup .chan-ref, .chan-quote-preview-popup .quotelink');
      if (ref) {
        const card = quotedCard(ref);
        if (!card) return;
        handled();
        hidePreview();
        history.pushState(history.state, '', location.pathname + location.search + '#p' + card.dataset.no);
        jumpTo(card, true);
      }
    }, true);

    document.addEventListener('mouseover', (e) => {
      if (preview && preview.contains(e.target)) {
        keepPreview();
        return;
      }
      const ref = e.target.closest('#main.chan-thread-page .chan-ref, #main.chan-thread-page .quotelink');
      if (!ref) return;
      const card = quotedCard(ref);
      if (!card) return;
      keepPreview();
      showPreview(ref, card);
    });

    document.addEventListener('mouseout', (e) => {
      const to = e.relatedTarget;
      if (preview && preview.contains(e.target)) {
        if (!to || !preview.contains(to)) hideSoon();
        return;
      }
      const ref = e.target.closest('#main.chan-thread-page .chan-ref, #main.chan-thread-page .quotelink');
      if (ref && (!to || !ref.contains(to))) hideSoon();
    });

    // Back/forward between quoted posts
    window.addEventListener('hashchange', () => {
      if (!page() || !/^#p\d+$/.test(location.hash)) return;
      const card = document.getElementById(location.hash.slice(1));
      if (card) jumpTo(card, true);
    });

    // Leaving the page (in-place navigation) drops a visible preview
    window.addEventListener('pjax:start', hidePreview);

    // The player view's active post loses its mark when the player closes
    new MutationObserver(() => {
      if (document.body.classList.contains('onara-modal-open')) return;
      document.querySelectorAll('.chan-post-card.active-thread-post, .chan-post-card.onara-active, .chan-thumb-link.onara-active').forEach((el) => {
        el.classList.remove('active-thread-post', 'onara-active');
      });
    }).observe(document.body, { attributes: true, attributeFilter: ['class'] });
  }

  // ── Per page: start at the active post (player view) or the one in the address ──
  const jumpToInitialActive = () => {
    let target = document.querySelector('.chan-post-card.active-thread-post, .chan-post-card.onara-active');
    if (!target && /^#p\d+$/.test(location.hash)) target = document.getElementById(location.hash.slice(1));
    if (!target) return;
    scrollToPost(target, false);
    if (location.hash) highlightPost(target);
  };
  if (page()) {
    jumpToInitialActive();
    setTimeout(jumpToInitialActive, 100);
  }
})();
