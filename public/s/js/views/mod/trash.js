(() => {
  const DATA = JSON.parse(document.getElementById('mod-trash-data').textContent);
  const root = document.querySelector('.mq-trash');
  if (!root) return;

  const refresh = () => {
    const here = location.pathname + location.search;
    if (window.loadPageAjax) window.loadPageAjax(here, true, { skipCache: true });
    else location.reload();
  };

  // Restore → public again (no "approved" notification). The form posts by itself without JavaScript.
  root.addEventListener('submit', async e => {
    const form = e.target.closest('.mq-restore-form');
    if (!form) return;
    e.preventDefault();
    const btn = form.querySelector('button[type="submit"]');
    btn.disabled = true;
    try {
      await mq.post(form.getAttribute('action'), { id: form.elements.id.value });
      mq.flash('Item restored', 'success');
      mq.removeCard(form.closest('.mq-card'));
    } catch (err) {
      mq.flash(err.message, 'error');
      btn.disabled = false;
    }
  });

  root.addEventListener('click', e => {
    if (e.ctrlKey || e.shiftKey || e.altKey || e.metaKey || e.button !== 0) return;
    // Purge one item permanently (admins). The link leads to the plain confirmation form without JavaScript.
    const purge = e.target.closest('.btn-purge');
    if (purge) {
      e.preventDefault();
      e.stopPropagation();
      const card = purge.closest('.mq-card');
      mq.confirm({
        heading: 'Purge item #' + purge.dataset.id,
        body: 'Permanently delete this item, its files and its comments? This cannot be undone.',
        confirmLabel: 'Purge',
        withReason: true,
        suggestReason: (card && card.dataset.reason) || '',
        action: async (reason) => {
          await mq.post('/mod/trash/purge', { id: purge.dataset.id, reason });
          mq.flash(DATA.msg_item_deleted_success, 'success');
          mq.removeCard(card);
        }
      });
      return;
    }
    // Purge everything (admins)
    if (e.target.closest('#btn-purge-trash')) {
      e.preventDefault();
      e.stopPropagation();
      mq.confirm({
        heading: 'Purge all',
        body: 'Permanently delete ALL soft-deleted items, their files and comments? This cannot be undone.',
        confirmLabel: 'Purge all',
        action: async () => {
          const data = await mq.post('/mod/trash/purge-all');
          mq.flash('Purged ' + (data.count || 0) + ' item(s)', 'success');
          refresh();
        }
      });
    }
  });

  // The plain confirmation form (page opened as ?purge=…): send it in place too
  const plain = document.getElementById('mq-purge-form');
  if (plain) {
    const back = plain.elements.back.value;
    const leave = () => {
      if (window.loadPageAjax) window.loadPageAjax(back, true, { skipCache: true });
      else location.href = back;
    };
    plain.addEventListener('submit', async e => {
      e.preventDefault();
      const btn = plain.querySelector('button[type="submit"]');
      const body = {};
      if (plain.elements.id) {
        body.id = plain.elements.id.value;
        body.reason = plain.elements.reason.value.trim();
        const same = plain.elements.use_delete_reason;
        if (same && same.checked) body.use_delete_reason = true;
      }
      btn.disabled = true;
      try {
        const data = await mq.post(plain.getAttribute('action'), body);
        mq.flash(plain.elements.id ? DATA.msg_item_deleted_success : 'Purged ' + (data.count || 0) + ' item(s)', 'success');
        leave();
      } catch (err) {
        mq.flash(err.message, 'error');
        btn.disabled = false;
      }
    });
    plain.querySelector('.mq-modal-actions a').addEventListener('click', e => {
      if (e.ctrlKey || e.shiftKey || e.altKey || e.metaKey || e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      leave();
    });
  }
})();
