(() => {
  const DATA = JSON.parse(document.getElementById('mod-trash-data').textContent);
  // Restore → public again (no "approved" notification)
  document.querySelectorAll('.btn-approve-async').forEach(btn => {
    btn.addEventListener('click', async e => {
      e.preventDefault();
      btn.disabled = true;
      try {
        await mq.post('/mod/trash/restore', { id: btn.dataset.id });
        mq.flash('Item restored', 'success');
        mq.removeCard(btn.closest('.mq-card'));
      } catch (err) {
        mq.flash(err.message, 'error');
        btn.disabled = false;
      }
    });
  });
  // Purge one item permanently (admins)
  document.querySelectorAll('.btn-deny-async').forEach(btn => {
    btn.addEventListener('click', e => {
      e.preventDefault();
      mq.confirm({
        heading: 'Purge item',
        body: 'Permanently delete this item, its files and its comments? This cannot be undone.',
        confirmLabel: 'Purge',
        withReason: true,
        action: async (reason) => {
          await mq.post('/mod/trash/purge', { id: btn.dataset.id, reason });
          mq.flash(DATA.msg_item_deleted_success, 'success');
          mq.removeCard(btn.closest('.mq-card'));
        }
      });
    });
  });
  // Purge everything (admins)
  const purgeAll = document.getElementById('btn-purge-trash');
  if (purgeAll) {
    purgeAll.addEventListener('click', () => {
      mq.confirm({
        heading: 'Purge all',
        body: 'Permanently delete ALL soft-deleted items, their files and comments? This cannot be undone.',
        confirmLabel: 'Purge all',
        action: async () => {
          const data = await mq.post('/mod/trash/purge-all');
          mq.flash('Purged ' + (data.count || 0) + ' item(s)', 'success');
          location.reload();
        }
      });
    });
  }
})();
