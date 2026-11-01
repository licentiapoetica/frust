(() => {
  // Highlight + scroll to #i<id> (AJAX navigation uses pushState, which doesn't trigger :target)
  const focusRow = () => {
    document.querySelectorAll('.up-row.is-target').forEach(r => r.classList.remove('is-target'));
    const id = (location.hash || '').slice(1);
    const row = id && document.getElementById(id);
    if (!row || !row.classList.contains('up-row')) return;
    row.classList.add('is-target');
    row.scrollIntoView({ block: 'center', behavior: 'smooth' });
  };
  setTimeout(focusRow, 50);
  window.addEventListener('hashchange', focusRow);

  // Withdraw a pending upload: first click arms the button, second click deletes
  const csrf = () => (window.f0ckSession && window.f0ckSession.csrf_token) || document.querySelector('meta[name="csrf-token"]')?.content || '';
  const disarm = (btn) => { btn.classList.remove('is-armed'); btn.querySelector('span').textContent = 'Delete'; };
  document.querySelectorAll('.up-del').forEach(btn => {
    let timer = null;
    btn.addEventListener('click', async () => {
      if (!btn.classList.contains('is-armed')) {
        btn.classList.add('is-armed');
        btn.querySelector('span').textContent = 'Confirm';
        timer = setTimeout(() => disarm(btn), 4000);
        return;
      }
      clearTimeout(timer);
      btn.disabled = true;
      try {
        const r = await fetch('/pending/delete', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'Accept': 'application/json', 'X-Requested-With': 'XMLHttpRequest', 'X-CSRF-Token': csrf() },
          body: JSON.stringify({ id: btn.dataset.id })
        });
        const data = await r.json().catch(() => ({}));
        if (!r.ok || !data.success) throw new Error(data.msg || 'Delete failed');
        const row = btn.closest('.up-row');
        row.classList.add('is-gone');
        setTimeout(() => row.remove(), 250);
        const cnt = document.getElementById('up-pending-count');
        if (cnt) cnt.textContent = Math.max(0, (+cnt.textContent || 0) - 1);
        if (typeof window.flashMessage === 'function') window.flashMessage('Upload deleted', 2000, 'success');
      } catch (e) {
        btn.disabled = false;
        disarm(btn);
        if (typeof window.flashMessage === 'function') window.flashMessage(e.message, 3000, 'error'); else alert(e.message);
      }
    });
  });
})();
