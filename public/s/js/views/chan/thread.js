(() => {
  // Toggle filter: All vs Media Only
  const filterAllBtn = document.getElementById('filter-all-btn');
  const filterMediaBtn = document.getElementById('filter-media-btn');
  const filterRehostedBtn = document.getElementById('filter-rehosted-btn');
  const posts = document.querySelectorAll('.chan-post-card');

  if (filterAllBtn && filterMediaBtn) {
    filterAllBtn.addEventListener('click', () => {
      filterAllBtn.classList.add('active');
      filterMediaBtn.classList.remove('active');
      if (filterRehostedBtn) filterRehostedBtn.classList.remove('active');
      posts.forEach(p => p.style.display = 'block');
    });

    filterMediaBtn.addEventListener('click', () => {
      filterMediaBtn.classList.add('active');
      filterAllBtn.classList.remove('active');
      if (filterRehostedBtn) filterRehostedBtn.classList.remove('active');
      posts.forEach(p => {
        p.style.display = p.dataset.hasMedia === '1' ? 'block' : 'none';
      });
    });

    if (filterRehostedBtn) {
      filterRehostedBtn.addEventListener('click', () => {
        filterRehostedBtn.classList.add('active');
        filterAllBtn.classList.remove('active');
        filterMediaBtn.classList.remove('active');
        posts.forEach(p => {
          p.style.display = p.dataset.isRehosted === '1' ? 'block' : 'none';
        });
      });
    }
  }

  // Inline Rehosting
  document.addEventListener('click', async (e) => {
    const btn = e.target.closest('.chan-rehost-btn:not(.rehosted)');
    if (!btn) return;

    e.preventDefault();
    const url = btn.dataset.url;
    const board = btn.dataset.board;
    const tid = btn.dataset.tid;
    const postNo = btn.dataset.post;
    const filename = btn.dataset.filename;
    if (!url) return;

    btn.classList.add('loading');
    btn.innerHTML = '<i class="fa-solid fa-circle-notch fa-spin"></i> Rehosting…';

    try {
      const csrfToken = window.f0ckSession?.csrf_token || '';
      const resp = await fetch('/api/v2/scroller/rehost', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/x-www-form-urlencoded',
          ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
        },
        body: new URLSearchParams({
          url,
          original_filename: filename || '',
          width: btn.dataset.width || '',
          height: btn.dataset.height || ''
        })
      });

      const data = await resp.json();
      if (data.success && data.item_id) {
        btn.classList.remove('loading');
        btn.classList.add('rehosted');
        btn.outerHTML = '<span class="chan-rehosted-wrap">' +
          '<a href="/' + (data.item_path || data.item_id) + '" class="chan-rehost-btn rehosted" target="_blank">' +
          '<i class="fa-solid fa-check"></i> Rehosted /' + (data.item_path || data.item_id) + '</a>' +
          '<button type="button" class="chan-rehost-edit-btn" data-item-id="' + data.item_id + '" title="Edit rating">' +
          '<i class="fa-solid fa-pen"></i></button></span>';

        const postCard = document.getElementById('p' + postNo) || btn.closest('.chan-post-card');
        if (postCard) {
          const timeEl = postCard.querySelector('.chan-post-time, .timeago');
          if (timeEl) {
            const nowIso = new Date().toISOString();
            timeEl.dataset.iso = nowIso;
            const fullDate = typeof window.f0ckFormatDateFull === 'function' ? window.f0ckFormatDateFull(nowIso) : new Date().toLocaleString();
            timeEl.setAttribute('tooltip', fullDate);
            timeEl.setAttribute('title', fullDate);
            timeEl.textContent = (window.f0ckTimeAgo ? window.f0ckTimeAgo(nowIso) : (window.f0ckI18n?.timeago_just_now || 'just now'));
          }
        }

        if (typeof window.showToast === 'function') {
          window.showToast(data.repost ? 'Already on site — linked to existing post' : 'Rehosted successfully!');
        }
        if (typeof window.openChanRehostModal === 'function') {
          const postCardEl = document.getElementById('p' + postNo) || postCard;
          const anchor = (postCardEl ? (postCardEl.querySelector('.chan-rehost-edit-btn') || postCardEl.querySelector('.chan-rehosted-wrap')) : null) || document.getElementById('rehost-box-' + postNo);
          window.openChanRehostModal({ itemId: data.item_id, anchorEl: anchor });
        }
      } else {
        btn.classList.remove('loading');
        btn.innerHTML = '<i class="fa-solid fa-cloud-arrow-up"></i> Retry';
        alert(data.msg || data.message || 'Rehosting failed');
      }
    } catch (err) {
      console.error('[CHAN] Rehost error:', err);
      btn.classList.remove('loading');
      btn.innerHTML = '<i class="fa-solid fa-cloud-arrow-up"></i> Error';
    }
  });

  document.addEventListener('click', (e) => {
    const editBtn = e.target.closest('.chan-rehost-edit-btn');
    if (editBtn) {
      e.preventDefault();
      e.stopImmediatePropagation();
      const itemId = editBtn.dataset.itemId;
      if (itemId && typeof window.openChanRehostModal === 'function') {
        window.openChanRehostModal({ itemId: Number(itemId), anchorEl: editBtn });
      }
    }
  });

  // Favorite from Thread Card (Auto-rehost if needed, then toggle favorite)
  document.addEventListener('click', async (e) => {
    const favBtn = e.target.closest('.chan-card-fav-btn');
    if (!favBtn || favBtn.classList.contains('loading')) return;

    e.preventDefault();

    if (window.f0ckSession?.is_anon && window.f0ckSession?.anon_permissions && window.f0ckSession.anon_permissions.favorite === false) {
      if (typeof window.flashMessage === 'function') window.flashMessage('Anonymous favoriting is disabled.', 3000, 'error');
      return;
    }
    if (!window.f0ckSession || !window.f0ckSession.user) {
      if (typeof window.flashMessage === 'function') window.flashMessage('Login to favorite posts', 3000, 'warning');
      return;
    }

    const postNo = favBtn.dataset.post;
    let localId = favBtn.dataset.localId;
    const mediaUrl = favBtn.dataset.url;

    const wasFaved = favBtn.classList.contains('favorited');
    const isNowFav = !wasFaved;

    // 1. Instantly apply client UI
    favBtn.classList.toggle('favorited', isNowFav);
    favBtn.innerHTML = '<i class="fa-' + (isNowFav ? 'solid' : 'regular') + ' fa-heart"></i>';
    favBtn.title = isNowFav ? 'Remove from favorites' : 'Favorite';
    if (typeof window.flashMessage === 'function') {
      window.flashMessage(isNowFav ? 'ADDED TO FAVORITES' : 'REMOVED FROM FAVORITES');
    }
    if (navigator.vibrate) navigator.vibrate(50);

    // 2. Perform server sync in background
    (async () => {
      try {
        // If not yet rehosted, auto-rehost first
        if (!localId && mediaUrl) {
          const csrfToken = window.f0ckSession?.csrf_token || '';
          const rehostResp = await fetch('/api/v2/scroller/rehost', {
            method: 'POST',
            headers: {
              'Content-Type': 'application/x-www-form-urlencoded',
              ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
            },
            body: new URLSearchParams({
              url: mediaUrl,
              original_filename: favBtn.dataset.filename || '',
              width: favBtn.dataset.width || '',
              height: favBtn.dataset.height || ''
            })
          });
          const rehostData = await rehostResp.json();
          if (rehostData.success && rehostData.item_id) {
            localId = rehostData.item_id;
            const localPath = rehostData.item_path || localId;
            favBtn.dataset.localId = localId;

            // Update rehost box
            const rehostBox = document.getElementById('rehost-box-' + postNo);
            if (rehostBox) {
              const rehostBtn = rehostBox.querySelector('.chan-rehost-btn');
              if (rehostBtn) {
                rehostBtn.outerHTML = '<span class="chan-rehosted-wrap">' +
                  '<a href="/' + localPath + '" class="chan-rehost-btn rehosted" target="_blank">' +
                  '<i class="fa-solid fa-check"></i> Rehosted /' + localPath + '</a>' +
                  '<button type="button" class="chan-rehost-edit-btn" data-item-id="' + localId + '" title="Edit rating">' +
                  '<i class="fa-solid fa-pen"></i></button></span>';
              }
            }
            // Add thumb badge if missing
            const postCard = document.getElementById('p' + postNo) || favBtn.closest('.chan-post-card');
            if (postCard) {
              const thumbLink = postCard.querySelector('.chan-thumb-link');
              if (thumbLink && !thumbLink.querySelector('.chan-thumb-rehost-badge')) {
                const badge = document.createElement('span');
                badge.className = 'chan-thumb-rehost-badge';
                badge.title = 'Rehosted on f0ckm as /' + localPath;
                badge.innerHTML = '<i class="fa-solid fa-check"></i> /' + localPath;
                thumbLink.appendChild(badge);
              }
              const timeEl = postCard.querySelector('.chan-post-time, .timeago');
              if (timeEl) {
                const nowIso = new Date().toISOString();
                timeEl.dataset.iso = nowIso;
                const fullDate = typeof window.f0ckFormatDateFull === 'function' ? window.f0ckFormatDateFull(nowIso) : new Date().toLocaleString();
                timeEl.setAttribute('tooltip', fullDate);
                timeEl.setAttribute('title', fullDate);
                timeEl.textContent = (window.f0ckTimeAgo ? window.f0ckTimeAgo(nowIso) : (window.f0ckI18n?.timeago_just_now || 'just now'));
              }
            }
          } else {
            throw new Error(rehostData.msg || 'Rehost failed');
          }
        }

        // Now sync favorite to server
        const csrfToken = window.f0ckSession?.csrf_token || '';
        const favResp = await fetch('/api/v2/togglefav', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
          },
          body: JSON.stringify({
            postid: Number(localId),
            chan_url: mediaUrl,
            action: isNowFav ? 'add' : 'delete',
            favorited: isNowFav
          })
        });
        const favData = await favResp.json();

        if (favData.success) {
          const finalIsFav = (favData.favorited !== undefined) ? !!favData.favorited : isNowFav;
          favBtn.classList.toggle('favorited', finalIsFav);
          favBtn.innerHTML = '<i class="fa-' + (finalIsFav ? 'solid' : 'regular') + ' fa-heart"></i>';
          favBtn.title = finalIsFav ? 'Remove from favorites' : 'Favorite';
        } else {
          // Rollback on server rejection
          favBtn.classList.toggle('favorited', wasFaved);
          favBtn.innerHTML = '<i class="fa-' + (wasFaved ? 'solid' : 'regular') + ' fa-heart"></i>';
          favBtn.title = wasFaved ? 'Remove from favorites' : 'Favorite';
          if (typeof window.flashMessage === 'function') {
            window.flashMessage(favData.msg || 'Failed to update favorite', 3000, 'error');
          }
        }
      } catch (err) {
        console.error('[CHAN-CARD-FAV] Error:', err);
        // Rollback on network/rehost error
        favBtn.classList.toggle('favorited', wasFaved);
        favBtn.innerHTML = '<i class="fa-' + (wasFaved ? 'solid' : 'regular') + ' fa-heart"></i>';
        favBtn.title = wasFaved ? 'Remove from favorites' : 'Favorite';
        if (typeof window.flashMessage === 'function') {
          window.flashMessage('Failed to update favorite', 3000, 'error');
        }
      }
    })();
  });

  // Flash highlight a post card
  const highlightPost = (el) => {
    if (!el) return;
    el.classList.remove('chan-flash-highlight');
    void el.offsetWidth;
    el.classList.add('chan-flash-highlight');
    setTimeout(() => {
      el.classList.remove('chan-flash-highlight');
    }, 2000);
  };

  // Quote click navigation (smooth jump + flash highlight)
  document.addEventListener('click', (e) => {
    const ref = e.target.closest('.chan-ref, .quotelink');
    if (!ref) return;

    const targetPostNo = ref.dataset.post || ref.getAttribute('href')?.replace(/^#p?/, '');
    if (!targetPostNo) return;
    const targetCard = document.getElementById('p' + targetPostNo);
    if (!targetCard) return;

    e.preventDefault();
    if (previewEl) previewEl.style.display = 'none';

    // If card is hidden due to active filter ("Media" or "Rehosted"), reset to "All"
    if (targetCard.style.display === 'none') {
      const filterAllBtn = document.getElementById('filter-all-btn');
      if (filterAllBtn) filterAllBtn.click();
    }

    history.pushState(null, '', '#p' + targetPostNo);
    try {
      targetCard.scrollIntoView({ behavior: 'smooth', block: 'center' });
    } catch (_) {
      targetCard.scrollIntoView();
    }
    highlightPost(targetCard);
  });

  // Handle browser back/forward buttons navigating hash
  window.addEventListener('hashchange', () => {
    if (window.location.hash && window.location.hash.startsWith('#p')) {
      const el = document.querySelector(window.location.hash);
      if (el) {
        if (el.style.display === 'none') {
          const filterAllBtn = document.getElementById('filter-all-btn');
          if (filterAllBtn) filterAllBtn.click();
        }
        try {
          el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        } catch (_) {
          el.scrollIntoView();
        }
        highlightPost(el);
      }
    }
  });

  // Quote hover preview
  let previewEl = null;
  document.addEventListener('mouseover', (e) => {
    const ref = e.target.closest('.chan-ref, .quotelink');
    if (!ref) return;

    const targetPostNo = ref.dataset.post || ref.getAttribute('href')?.replace(/^#p?/, '');
    if (!targetPostNo) return;
    const targetCard = document.getElementById('p' + targetPostNo);
    if (!targetCard) return;

    if (!previewEl) {
      previewEl = document.createElement('div');
      previewEl.className = 'chan-quote-preview-popup';
      document.body.appendChild(previewEl);
    }

    previewEl.innerHTML = targetCard.innerHTML;
    previewEl.style.display = 'block';

    const rect = ref.getBoundingClientRect();
    const popHeight = previewEl.offsetHeight || 150;
    const popWidth = previewEl.offsetWidth || 380;

    let top = rect.bottom + 8;
    if (top + popHeight > window.innerHeight - 10) {
      top = rect.top - popHeight - 8;
    }
    if (top < 10) top = 10;

    let left = rect.left;
    if (left + popWidth > window.innerWidth - 10) {
      left = window.innerWidth - popWidth - 10;
    }
    if (left < 10) left = 10;

    previewEl.style.top = top + 'px';
    previewEl.style.left = left + 'px';
  });

  document.addEventListener('mouseout', (e) => {
    const ref = e.target.closest('.chan-ref, .quotelink');
    if (ref && previewEl) {
      previewEl.style.display = 'none';
    }
  });

  // Jump to active post in thread if onara active or hash present
  const jumpToInitialActive = () => {
    let target = document.querySelector('.chan-post-card.active-thread-post, .chan-post-card.onara-active');
    if (!target && window.location.hash) {
      target = document.querySelector(window.location.hash);
    }
    if (target) {
      try {
        target.scrollIntoView({ block: 'center', behavior: 'instant' });
      } catch (_) {
        target.scrollIntoView();
      }
      if (window.location.hash) {
        highlightPost(target);
      }
    }
  };
  // Clear active card highlight whenever onara modal closes
  const clearOnaraActiveCards = () => {
    if (!document.body.classList.contains('onara-modal-open')) {
      document.querySelectorAll('.chan-post-card.active-thread-post, .chan-post-card.onara-active, .chan-thumb-link.onara-active').forEach(el => {
        el.classList.remove('active-thread-post', 'onara-active');
      });
    }
  };
  const bodyObserver = new MutationObserver(() => {
    clearOnaraActiveCards();
  });
  bodyObserver.observe(document.body, { attributes: true, attributeFilter: ['class'] });

  jumpToInitialActive();
  setTimeout(jumpToInitialActive, 100);
})();
