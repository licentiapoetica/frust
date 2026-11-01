F0ckActions.define({
    saveCleanupSettings: (el) => saveCleanupSettings(el),
    runCleanup: () => runCleanup(),
});

async function saveCleanupSettings(form) {
    const status = document.getElementById('cleanup-status');

    status.textContent = 'Saving...';
    status.style.color = 'var(--accent)';
    status.style.display = 'inline';

    try {
        const formData = new FormData(form);

        const res = await fetch(form.action, {
            method: 'POST',
            headers: {
                'X-Requested-With': 'XMLHttpRequest',
                'Content-Type': 'application/x-www-form-urlencoded',
                'X-CSRF-Token': window.f0ckSession?.csrf_token || document.getElementById('cleanup-form').dataset.csrf
            },
            body: new URLSearchParams(formData)
        });

        const cleanup_response = await res.json();
        if (cleanup_response.success) {
            status.textContent = 'Settings saved!';
            status.style.color = '#28a745';
            setTimeout(() => {
                status.style.display = 'none';
            }, 3000);
        } else {
            throw new Error(cleanup_response.msg || 'Unknown error');
        }
    } catch (err) {
        console.error('Cleanup Save Error:', err);
        status.textContent = 'Error: ' + err.message;
        status.style.color = '#d9534f';
    }
}

async function runCleanup() {
    const btn = document.getElementById('run-cleanup-btn');
    const status = document.getElementById('run-status');
    const includeEngaged = document.getElementById('cleanup_include_engaged')?.checked || false;

    const warningMsg = includeEngaged 
        ? 'SECURITY CLEANUP WARNING: You have selected to INCLUDE posts WITH engagement (comments/favorites). Are you ABSOLUTELY SURE you want to permanently delete ALL posts in this date range?'
        : 'Are you absolutely sure? This will PERMANENTLY delete files from disk. This action cannot be undone.';

    if (!confirm(warningMsg)) return;

    btn.disabled = true;
    btn.textContent = 'CLEANING UP...';
    status.textContent = 'Processing files, please wait...';
    status.style.color = 'var(--accent)';

    try {
        const formData = new URLSearchParams();
        formData.append('include_engaged', includeEngaged ? 'true' : 'false');

        const res = await fetch('/admin/cleanup/run', {
            method: 'POST',
            headers: {
                'X-Requested-With': 'XMLHttpRequest',
                'Content-Type': 'application/x-www-form-urlencoded',
                'X-CSRF-Token': window.f0ckSession?.csrf_token || document.getElementById('cleanup-form').dataset.csrf
            },
            body: formData
        });

        const cleanup_response = await res.json();
        if (cleanup_response.success) {
            status.textContent = 'Success! ' + cleanup_response.msg;
            status.style.color = '#28a745';
        } else {
            throw new Error(cleanup_response.msg || 'Unknown error during cleanup');
        }
    } catch (err) {
        console.error('Cleanup Run Error:', err);
        status.textContent = 'Error: ' + err.message;
        status.style.color = '#d9534f';
    } finally {
        btn.disabled = false;
        btn.textContent = 'RUN CLEANUP NOW';
    }
}
