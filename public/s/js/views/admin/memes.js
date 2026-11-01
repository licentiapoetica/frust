window.f0ckDebug = window.f0ckDebug || (() => {});
(() => {
    var i18n = window.f0ckI18n || {};
    window.f0ckDebug('[MEME_ADMIN] Initializing');
    const esc = (s) => (s || '').toString().replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#039;');
    const loadMemes = async () => {
        try {
            const res = await fetch('/api/v2/memes');
            const data = await res.json();
            if (data.success) {
                window.memeAdmin.memes = data.memes;
                const tbody = document.getElementById('meme-list');
                if (!tbody) return;
                tbody.innerHTML = data.memes.map(m =>
                    '<tr class="adm-memes-row">' +
                    '<td class="adm-memes-cell"><img src="' + m.url + '" class="adm-memes-preview"></td>' +
                    '<td class="adm-memes-cell">' + esc(m.name) + '</td>' +
                    '<td class="adm-memes-cell"><span class="adm-memes-category">' + esc(m.category || 'General') + '</span></td>' +
                    '<td class="adm-memes-cell adm-memes-template-id">' + esc(m.template_id) + '</td>' +
                    '<td class="adm-memes-cell">' +
                    '<button data-on="click:memeOpenEditModal" data-args="[' + m.id + ']" class="btn-edit adm-memes-edit-btn">Edit</button>' +
                    '<button data-on="click:memeDelete" data-args="[' + m.id + ']" class="btn-remove adm-memes-delete-btn">Delete</button>' +
                    '</td>' +
                    '</tr>'
                ).join('');
            }
        } catch (err) { console.error('[MEME_ADMIN] Load Error:', err); }
    };

    const addMeme = async (e) => {
        if (e) e.preventDefault();
        window.f0ckDebug('[MEME_ADMIN] addMeme triggered');

        const template_id = document.getElementById('meme-id').value;
        const name = document.getElementById('meme-name').value;
        const url = document.getElementById('meme-url').value;
        const category = document.getElementById('meme-category').value;
        const fileInput = document.getElementById('meme-file');

        if (!template_id || !name || (!url && !fileInput.files[0])) {
            return alert('Fill all fields (ID, Name, and either URL or File)');
        }

        const btn = document.getElementById('add-meme');
        const oldText = btn.textContent;
        btn.disabled = true;
        btn.textContent = i18n.uploading || 'Uploading...';

        const formData = new FormData();
        formData.append('template_id', template_id);
        formData.append('name', name);
        formData.append('url', url);
        formData.append('category', category);
        if (fileInput.files[0]) {
            formData.append('file', fileInput.files[0]);
        }

        try {
            const headers = { 
                'X-Requested-With': 'XMLHttpRequest'
            };
            const csrf = document.getElementById('meme-list').dataset.csrf;
            if (csrf) headers['X-CSRF-Token'] = csrf;

            const res = await fetch('/api/v2/admin/memes', {
                method: 'POST',
                headers: headers,
                body: formData
            });

            const data = await res.json();
            if (data.success) {
                document.getElementById('meme-id').value = '';
                document.getElementById('meme-name').value = '';
                document.getElementById('meme-url').value = '';
                document.getElementById('meme-category').value = '';
                document.getElementById('meme-file').value = '';
                loadMemes();
            } else {
                alert('Server Error: ' + (data.message || data.msg || 'Unknown error'));
            }
        } catch (e) {
            console.error('[MEME_ADMIN] Post Error:', e);
            alert('Submission failed: ' + e.message);
        } finally {
            btn.disabled = false;
            btn.textContent = oldText;
        }
    };

    const deleteMeme = async (id) => {
        if (!confirm('Delete this template?')) return;
        try {
            const res = await fetch('/api/v2/admin/memes/' + id + '/delete', { 
                method: 'POST',
                headers: { 'X-CSRF-Token': document.getElementById('meme-list').dataset.csrf }
            });
            const data = await res.json();
            if (data.success) {
                loadMemes();
            } else {
                alert('Delete failed');
            }
        } catch (e) { console.error(e); }
    };

     const openEditModal = (id) => {
        const meme = (window.memeAdmin.memes || []).find(m => m.id === id);
        if (!meme) return;

        document.getElementById('edit-meme-id-db').value = meme.id;
        document.getElementById('edit-meme-template-id').value = meme.template_id;
        document.getElementById('edit-meme-name').value = meme.name;
        document.getElementById('edit-meme-category').value = meme.category || 'General';
        document.getElementById('edit-meme-url').value = meme.url;
        document.getElementById('edit-meme-file').value = '';

        const modal = document.getElementById('edit-meme-modal');
        if (modal) modal.style.display = 'flex';
    };

    const closeEditModal = () => {
        const modal = document.getElementById('edit-meme-modal');
        if (modal) modal.style.display = 'none';
        document.getElementById('edit-meme-file').value = '';
    };

    const saveMeme = async () => {
        const id = document.getElementById('edit-meme-id-db').value;
        const template_id = document.getElementById('edit-meme-template-id').value.trim().toLowerCase();
        const name = document.getElementById('edit-meme-name').value.trim();
        const category = document.getElementById('edit-meme-category').value.trim() || 'General';
        const url = document.getElementById('edit-meme-url').value.trim();
        const fileInput = document.getElementById('edit-meme-file');

        if (!template_id || !name) {
            return alert('Template ID and Display Name are required');
        }

        if (!/^[a-z0-9-]+$/.test(template_id)) {
            return alert('Invalid ID. Use lowercase a-z, 0-9, - only.');
        }

        const btn = document.querySelector('#edit-meme-modal .btn-save');
        const oldText = btn.textContent;
        btn.disabled = true;
        btn.textContent = 'Saving...';

        const formData = new FormData();
        formData.append('template_id', template_id);
        formData.append('name', name);
        formData.append('category', category);
        formData.append('url', url);
        if (fileInput.files[0]) {
            formData.append('file', fileInput.files[0]);
        }

        try {
            const headers = {
                'X-Requested-With': 'XMLHttpRequest'
            };
            const csrf = document.getElementById('meme-list').dataset.csrf;
            if (csrf) headers['X-CSRF-Token'] = csrf;

            const res = await fetch('/api/v2/admin/memes/' + id + '/edit', {
                method: 'POST',
                headers: headers,
                body: formData
            });

            const data = await res.json();
            if (data.success) {
                closeEditModal();
                loadMemes();
            } else {
                alert('Server Error: ' + (data.message || 'Unknown error'));
            }
        } catch (e) {
            console.error('[MEME_ADMIN] Edit Error:', e);
            alert('Save failed: ' + e.message);
        } finally {
            btn.disabled = false;
            btn.textContent = oldText;
        }
    };

    // Global scope for onclick handlers
    window.memeAdmin = { deleteMeme, openEditModal, closeEditModal, saveMeme, memes: [] };

    F0ckActions.define({
        memeOpenEditModal: (el, e, id) => openEditModal(id),
        memeDelete: (el, e, id) => deleteMeme(id),
        memeCloseEditModal: () => closeEditModal(),
        memeSave: () => saveMeme(),
    });

    const btnAddMeme = document.getElementById('add-meme');
    if (btnAddMeme) {
        window.f0ckDebug('[MEME_ADMIN] Registering click listener');
        btnAddMeme.addEventListener('click', addMeme);
    } else {
        console.error('[MEME_ADMIN] Add button not found!');
    }

    loadMemes();
})();
