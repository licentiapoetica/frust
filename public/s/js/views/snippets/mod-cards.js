// Shared helpers for the moderation card pages. Re-assigned on every (AJAX) page load.
window.mq = (() => {
  const modal = document.getElementById('mq-modal');
  const title = document.getElementById('mq-modal-title');
  const text = document.getElementById('mq-modal-text');
  const reasonWrap = document.getElementById('mq-modal-reason-wrap');
  const reasonInput = document.getElementById('mq-modal-reason');
  const btnConfirm = document.getElementById('mq-modal-confirm');
  const btnCancel = document.getElementById('mq-modal-cancel');
  const btnSuggest = document.getElementById('mq-modal-suggest');

  const csrf = () => (window.f0ckSession && window.f0ckSession.csrf_token) || '';
  const flash = (msg, type) => { if (window.flashMessage) window.flashMessage(msg, type === 'error' ? 3000 : 2000, type); };

  let pendingAction = null;
  let needsReason = false;
  let suggested = '';

  const close = () => { modal.style.display = 'none'; pendingAction = null; };
  // suggestReason: a reason offered next to the field ("Use delete reason" copies it in, still editable)
  const confirm = ({ heading, body, confirmLabel = 'Confirm', withReason = false, suggestReason = '', action }) => {
    title.innerText = heading;
    text.innerText = body;
    pendingAction = action;
    needsReason = withReason;
    btnConfirm.innerText = confirmLabel;
    btnConfirm.dataset.label = confirmLabel;
    reasonWrap.style.display = withReason ? 'block' : 'none';
    reasonInput.value = '';
    reasonInput.classList.remove('is-invalid');
    suggested = withReason ? String(suggestReason || '').trim() : '';
    btnSuggest.hidden = !suggested;
    btnSuggest.title = suggested;
    modal.style.display = 'flex';
    if (withReason) reasonInput.focus();
  };

  btnConfirm.addEventListener('click', async () => {
    if (!pendingAction) return;
    let reason = null;
    if (needsReason) {
      reason = reasonInput.value.trim();
      if (!reason) { reasonInput.classList.add('is-invalid'); reasonInput.focus(); return; }
    }
    btnConfirm.disabled = true;
    btnConfirm.innerText = 'Processing…';
    try {
      await pendingAction(reason);
      close();
    } catch (e) {
      flash('Error: ' + e.message, 'error');
    } finally {
      btnConfirm.disabled = false;
      btnConfirm.innerText = btnConfirm.dataset.label || 'Confirm';
    }
  });
  btnCancel.addEventListener('click', close);
  btnSuggest.addEventListener('click', () => {
    reasonInput.value = suggested;
    reasonInput.classList.remove('is-invalid');
    reasonInput.focus();
  });
  modal.addEventListener('click', e => { if (e.target === modal) close(); });
  document.addEventListener('keydown', e => { if (e.key === 'Escape' && modal.style.display === 'flex') close(); });

  const post = async (url, body) => {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Requested-With': 'XMLHttpRequest', 'Accept': 'application/json', 'X-CSRF-Token': csrf() },
      body: JSON.stringify(body || {})
    });
    let data = {};
    try { data = await res.json(); } catch {}
    if (!res.ok || !data.success) throw new Error(data.msg || ('Request failed (' + res.status + ')'));
    return data;
  };

  const removeCard = (card) => {
    if (!card) return;
    card.classList.add('is-removing');
    const count = document.querySelector('.mq-count');
    if (count) count.textContent = Math.max(0, (parseInt(count.textContent, 10) || 0) - 1);
    setTimeout(() => {
      card.remove();
      const grid = document.querySelector('.mq-grid');
      if (!grid || grid.querySelector('.mq-card')) return;
      // Page is empty: load the next items (or the empty state) in place
      if (window.loadPageAjax) window.loadPageAjax(location.pathname + location.search, true, { skipCache: true });
      else location.reload();
    }, 300);
  };

  // Rating toggle (same on both pages): clicking a rating badge switches SFW / NSFW.
  // Delegated on the grid because the badges are rebuilt after every toggle.
  const ratingBadge = (tag, id) => {
    const rating = ['sfw', 'nsfw', 'nsfl'].includes(tag.normalized);
    const el = document.createElement(rating ? 'button' : 'span');
    let cls = 'badge-light';
    if (tag.normalized === 'sfw') cls = 'badge-success';
    else if (tag.normalized === 'nsfw') cls = 'badge-danger';
    else if (tag.tag.startsWith('>')) cls = 'badge-greentext badge-light';
    el.className = 'badge ' + cls;
    el.innerText = tag.tag;
    if (rating) {
      el.type = 'button';
      el.dataset.id = id;
      el.title = 'Click to switch SFW / NSFW';
      el.classList.add('mq-rating-toggle', 'btn-rating-toggle-async');
    }
    return el;
  };
  document.querySelectorAll('.mq-grid').forEach(grid => {
    grid.addEventListener('click', async e => {
      // Click a still image to open the full file
      const full = e.target.closest('.mq-viewer img[data-full]');
      if (full) { window.open(full.dataset.full, '_blank', 'noopener'); return; }
      const btn = e.target.closest('.btn-rating-toggle-async');
      if (!btn || btn.disabled) return;
      e.preventDefault();
      const card = btn.closest('.mq-card');
      const toggles = card ? card.querySelectorAll('.btn-rating-toggle-async') : [btn];
      toggles.forEach(b => { b.disabled = true; });
      try {
        const res = await fetch('/api/v2/tags/' + btn.dataset.id + '/toggle', {
          method: 'PUT',
          headers: { 'X-Requested-With': 'XMLHttpRequest', 'Accept': 'application/json', 'X-CSRF-Token': csrf() }
        });
        const data = await res.json();
        if (!data.success || !data.tags) throw new Error(data.msg || 'Unknown error');
        const tagsContainer = card?.querySelector('.mq-tags');
        if (tagsContainer) tagsContainer.replaceChildren(...data.tags.map(tag => ratingBadge(tag, btn.dataset.id)));
        // The rating badge on the preview follows
        const badge = card?.querySelector('.mq-badge-rating');
        if (badge) {
          const names = data.tags.map(tag => tag.normalized);
          const r = names.includes('nsfl') ? 'nsfl' : names.includes('nsfw') ? 'nsfw' : names.includes('sfw') ? 'sfw' : 'untagged';
          badge.classList.remove('is-sfw', 'is-nsfw', 'is-nsfl', 'is-untagged');
          badge.classList.add('is-' + r);
          badge.textContent = r === 'untagged' ? '?' : r.toUpperCase();
        }
      } catch (err) {
        flash('Error: ' + err.message, 'error');
      } finally {
        toggles.forEach(b => { b.disabled = false; });
      }
    });
  });

  return { confirm, post, flash, removeCard };
})();
