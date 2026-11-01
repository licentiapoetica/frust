// Shared by admin/terms, admin/rules and admin/about. The page name comes from the script tag's
// data-page; the stored text arrives base64-encoded in the textarea's data-b64.
(function() {
    var page = document.currentScript.dataset.page;
    var ta = document.getElementById(page + '-text');
    ta.value = new TextDecoder('utf-8').decode(Uint8Array.from(atob(ta.dataset.b64), function(c) { return c.charCodeAt(0); }));
})();

F0ckActions.define({
    savePage: (el, e, page) => savePage(el, page),
    clearPage: (el, e, page) => {
        document.getElementById(page + '-text').value = '';
        savePage(document.getElementById(page + '-form'), page);
    },
});

async function savePage(form, page) {
    const statusId = page + '-status';
    const status = document.getElementById(statusId);

    status.textContent = 'Saving...';
    status.style.color = 'var(--accent)';
    status.style.display = 'inline';

    try {
        const res = await fetch(form.action, {
            method: 'POST',
            headers: {
                'X-Requested-With': 'XMLHttpRequest',
                'Content-Type': 'application/x-www-form-urlencoded',
                'X-CSRF-Token': window.f0ckSession?.csrf_token
            },
            body: new URLSearchParams(new FormData(form))
        });

        const data = await res.json();
        if (data.success) {
            status.textContent = 'Saved!';
            status.style.color = '#28a745';
            setTimeout(() => { status.style.display = 'none'; }, 2000);
        } else {
            throw new Error(data.msg || 'Unknown error');
        }
    } catch (err) {
        console.error('Save Error:', err);
        status.textContent = 'Error: ' + err.message;
        status.style.color = '#d9534f';
    }
}
