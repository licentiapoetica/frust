F0ckActions.define({
    saveMotd: (el) => saveMotd(el),
    clearMotd: () => {
        document.getElementById('motd-text').value = '';
        saveMotd(document.getElementById('motd-form'));
    },
});

async function saveMotd(form) {
    const status = document.getElementById('motd-status');
    const textarea = document.getElementById('motd-text');
    const motd = textarea.value;

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

            if (typeof window.updateMotdUI === 'function') {
                window['motd_dismissed'] = false; // Force show on save
                window.updateMotdUI(motd);
            }

            setTimeout(() => {
                status.style.display = 'none';
            }, 2000);
        } else {
            throw new Error(data.msg || 'Unknown error');
        }
    } catch (err) {
        console.error('MOTD Save Error:', err);
        status.textContent = 'Error: ' + err.message;
        status.style.color = '#d9534f';
    }
}
