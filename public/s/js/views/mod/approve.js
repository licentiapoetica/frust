(() => {
  // Album viewer: swap the main viewer to the clicked strip item
  const buildViewer = (m) => {
    if (m.kind === 'video') {
      const v = document.createElement('video');
      v.controls = true; v.loop = true; v.muted = true; v.preload = 'metadata'; v.poster = m.thumb;
      const src = document.createElement('source'); src.src = m.src; src.type = m.mime; v.appendChild(src);
      return v;
    }
    if (m.kind === 'audio') {
      const wrap = document.createElement('div'); wrap.className = 'mq-audio';
      const img = document.createElement('img'); img.src = m.thumb; img.alt = '';
      const a = document.createElement('audio'); a.controls = true; a.src = m.src;
      wrap.append(img, a); return wrap;
    }
    if (m.kind === 'pdf') {
      const f = document.createElement('iframe'); f.src = m.src + '#toolbar=0'; return f;
    }
    const img = document.createElement('img');
    img.src = m.kind === 'image' ? m.src : m.thumb; img.alt = 'Preview'; img.dataset.full = m.src;
    return img;
  };

  document.querySelectorAll('.mq-review .mq-card').forEach(card => {
    let media = [];
    try { media = JSON.parse(card.dataset.media || '[]'); } catch (e) {}
    const viewer = card.querySelector('.mq-viewer');
    const current = card.querySelector('.mq-current');
    card.querySelectorAll('.mq-strip-item').forEach(itemEl => {
      itemEl.querySelector('.mq-strip-select')?.addEventListener('click', () => {
        const m = media.find(x => String(x.index) === itemEl.dataset.index);
        if (!m || !viewer) return;
        viewer.querySelectorAll('video, audio').forEach(el => { try { el.pause(); } catch (e) {} });
        viewer.replaceChildren(buildViewer(m));
        card.querySelectorAll('.mq-strip-item').forEach(b => b.classList.toggle('is-active', b === itemEl));
        if (current) current.textContent = m.index;
      });
    });
    // Deny a single album item (the rest stays; one item left → becomes a single post)
    card.querySelectorAll('.mq-strip-deny').forEach(btn => {
      btn.addEventListener('click', e => {
        e.stopPropagation();
        const left = card.querySelectorAll('.mq-strip-item').length - 1;
        mq.confirm({
          heading: 'Deny album item',
          body: left <= 1
            ? 'Deny only this item? The album will become a single post with the remaining item.'
            : 'Deny only this item? ' + left + ' items stay in the album.',
          confirmLabel: 'Deny item',
          withReason: true,
          action: async (reason) => {
            await mq.post('/mod/album-item/deny', { item_id: btn.dataset.itemId, album_item_id: btn.dataset.aid, reason });
            mq.flash(left <= 1 ? 'Item denied, album is now a single post' : 'Album item denied', 'success');
            const here = location.pathname + location.search;
            if (window.loadPageAjax) window.loadPageAjax(here, true, { skipCache: true });
            else location.reload();
          }
        });
      });
    });
    // Click a still image to open the full file
    if (viewer) viewer.addEventListener('click', e => {
      const img = e.target.closest('img[data-full]');
      if (img) window.open(img.dataset.full, '_blank', 'noopener');
    });
    // Keep the rating badge in sync after a rating toggle (tags are re-rendered by mod-cards)
    const badge = card.querySelector('.mq-badge-rating');
    const tags = card.querySelector('.mq-tags');
    if (badge && tags) new MutationObserver(() => {
      const names = Array.from(tags.querySelectorAll('.badge')).map(b => b.textContent.trim().toLowerCase());
      const r = names.includes('nsfl') ? 'nsfl' : names.includes('nsfw') ? 'nsfw' : names.includes('sfw') ? 'sfw' : 'untagged';
      badge.classList.remove('is-sfw', 'is-nsfw', 'is-nsfl', 'is-untagged');
      badge.classList.add('is-' + r);
      badge.textContent = r === 'untagged' ? '?' : r.toUpperCase();
    }).observe(tags, { childList: true });
  });
})();
(() => {
  const DATA = JSON.parse(document.getElementById('mod-approve-data').textContent);
  // Approve → public
  document.querySelectorAll('.btn-approve-async').forEach(btn => {
    btn.addEventListener('click', async e => {
      e.preventDefault();
      btn.disabled = true;
      try {
        await mq.post('/mod/approve', { id: btn.dataset.id });
        mq.flash(DATA.msg_approve_success, 'success');
        mq.removeCard(btn.closest('.mq-card'));
      } catch (err) {
        mq.flash(err.message || DATA.msg_approve_error, 'error');
        btn.disabled = false;
      }
    });
  });
  // Deny → soft deleted (uploader is notified with the reason)
  document.querySelectorAll('.btn-deny-async').forEach(btn => {
    btn.addEventListener('click', e => {
      e.preventDefault();
      mq.confirm({
        heading: 'Deny upload',
        body: 'Deny this upload? It moves to Soft Deleted and the uploader is notified with your reason.',
        confirmLabel: 'Deny',
        withReason: true,
        action: async (reason) => {
          await mq.post('/mod/deny', { id: btn.dataset.id, reason });
          mq.flash('Upload denied', 'success');
          mq.removeCard(btn.closest('.mq-card'));
        }
      });
    });
  });
})();
