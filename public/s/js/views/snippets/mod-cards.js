// Shared helpers for the moderation card pages. Re-assigned on every (AJAX) page load.
window.mq = (() => {
  const modal = document.getElementById('mq-modal');
  const title = document.getElementById('mq-modal-title');
  const text = document.getElementById('mq-modal-text');
  const reasonWrap = document.getElementById('mq-modal-reason-wrap');
  const reasonInput = document.getElementById('mq-modal-reason');
  const btnConfirm = document.getElementById('mq-modal-confirm');
  const btnCancel = document.getElementById('mq-modal-cancel');

  const csrf = () => (window.f0ckSession && window.f0ckSession.csrf_token) || '';
  const flash = (msg, type) => { if (window.flashMessage) window.flashMessage(msg, type === 'error' ? 3000 : 2000, type); };

  let pendingAction = null;
  let needsReason = false;

  const close = () => { modal.style.display = 'none'; pendingAction = null; };
  const confirm = ({ heading, body, confirmLabel = 'Confirm', withReason = false, action }) => {
    title.innerText = heading;
    text.innerText = body;
    pendingAction = action;
    needsReason = withReason;
    btnConfirm.innerText = confirmLabel;
    btnConfirm.dataset.label = confirmLabel;
    reasonWrap.style.display = withReason ? 'block' : 'none';
    reasonInput.value = '';
    reasonInput.classList.remove('is-invalid');
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
    card.style.opacity = '0';
    setTimeout(() => {
      card.remove();
      const grid = document.querySelector('.mq-grid');
      if (grid && !grid.querySelector('.mq-card')) location.reload();
    }, 300);
  };

  // Rating toggle (same on both pages)
  document.querySelectorAll('.btn-rating-toggle-async').forEach(btn => {
    btn.addEventListener('click', async e => {
      e.preventDefault();
      const tagsContainer = btn.closest('.mq-card')?.querySelector('.mq-tags');
      btn.disabled = true;
      try {
        const res = await fetch('/api/v2/tags/' + btn.dataset.id + '/toggle', {
          method: 'PUT',
          headers: { 'X-Requested-With': 'XMLHttpRequest', 'Accept': 'application/json', 'X-CSRF-Token': csrf() }
        });
        const data = await res.json();
        if (!data.success || !data.tags) throw new Error(data.msg || 'Unknown error');
        if (tagsContainer) {
          tagsContainer.innerHTML = '';
          data.tags.forEach(tag => {
            let cls = 'badge-light';
            if (tag.normalized === 'sfw') cls = 'badge-success';
            else if (tag.normalized === 'nsfw') cls = 'badge-danger';
            else if (tag.tag.startsWith('>')) cls = 'badge-greentext badge-light';
            const span = document.createElement('span');
            span.className = 'badge ' + cls;
            span.innerText = tag.tag;
            tagsContainer.appendChild(span);
          });
        }
      } catch (err) {
        flash('Error: ' + err.message, 'error');
      } finally {
        btn.disabled = false;
      }
    });
  });

  return { confirm, post, flash, removeCard };
})();
