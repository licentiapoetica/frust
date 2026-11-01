(() => {
    const DATA = JSON.parse(document.getElementById('admin-approve-data').textContent);
    const modal = document.getElementById('custom-modal');
    const modalTitle = document.getElementById('modal-title');
    const modalText = document.getElementById('modal-text');
    const btnConfirm = document.getElementById('modal-confirm');
    const btnCancel = document.getElementById('modal-cancel');

    let pendingAction = null;

    const showModal = (title, text, action) => {
        modalTitle.innerText = title;
        modalText.innerText = text;
        pendingAction = action;
        modal.style.display = 'flex';

        btnConfirm.onclick = async () => {
            if (!pendingAction) return;
            btnConfirm.disabled = true;
            btnConfirm.innerText = 'Processing...';
            try {
                await pendingAction();
                closeModal();
            } catch (e) {
                alert('Error: ' + e.message);
            } finally {
                btnConfirm.disabled = false;
                btnConfirm.innerText = 'Confirm';
            }
        };
    };

    const closeModal = () => {
        modal.style.display = 'none';
        pendingAction = null;
    };

    if (btnCancel) btnCancel.onclick = closeModal;

    // Single Deny
    document.querySelectorAll('.btn-deny-async').forEach(btn => {
        btn.addEventListener('click', e => {
            e.preventDefault();
            const url = btn.getAttribute('href');
            const row = btn.closest('tr');

            showModal('Deny Item', 'Permanently delete this item?', async () => {
                const res = await fetch(url, { 
                    headers: { 
                        'X-Requested-With': 'XMLHttpRequest',
                        'Accept': 'application/json'
                    } 
                });
                const data = await res.json();
                if (res.ok && data.success) {
                    if (window.flashMessage) window.flashMessage(DATA.item_deleted_success, 2000, 'success');
                    row.style.opacity = '0';
                    setTimeout(() => row.remove(), 300);
                } else {
                    if (window.flashMessage) window.flashMessage(data.msg || DATA.report_error, 3000, 'error');
                    throw new Error(data.msg || 'Request failed');
                }
            });
        });
    });

    // Single Approve / Restore
    document.querySelectorAll('.btn-approve-async').forEach(btn => {
        btn.addEventListener('click', async e => {
            e.preventDefault();
            const url = btn.getAttribute('href');
            const row = btn.closest('tr');

            try {
                const res = await fetch(url, {
                    headers: {
                        'X-Requested-With': 'XMLHttpRequest',
                        'Accept': 'application/json'
                    }
                });
                const data = await res.json();
                if (data.success) {
                    if (window.flashMessage) window.flashMessage(DATA.approve_success, 2000, 'success');
                    row.style.opacity = '0';
                    setTimeout(() => row.remove(), 300);
                } else {
                    if (window.flashMessage) window.flashMessage(data.msg || DATA.approve_error, 3000, 'error');
                }
            } catch (err) {
                console.error(err);
                if (window.flashMessage) window.flashMessage(DATA.network_error, 3000, 'error');
            }
        });
    });

    // Purge All Trash
    const btnPurgeTrash = document.getElementById('btn-purge-trash');
    if (btnPurgeTrash) {
        btnPurgeTrash.addEventListener('click', () => {
            showModal('Purge Trash', 'Permanently delete ALL items in the trash? This cannot be undone.', async () => {
                const res = await fetch('/admin/purge-trash-all', { method: 'POST' });
                const data = await res.json();
                if (data.success) {
                    location.reload();
                } else {
                    throw new Error(data.msg || 'Purge failed');
                }
            });
        });
    }
})();
