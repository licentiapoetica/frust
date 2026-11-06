// Admin font manager (views/admin/fonts.html). The page works without this: upload and delete
// are plain forms. This only asks before deleting a font.
(() => {
    const list = document.getElementById('font-list');
    if (!list) return;
    list.addEventListener('submit', (e) => {
        const form = e.target.closest('form.adm-font-delete');
        if (!form) return;
        if (!confirm('Delete the font ' + (form.dataset.file || '') + '? Users who picked it get the default font.')) {
            e.preventDefault();
        }
    });
})();
