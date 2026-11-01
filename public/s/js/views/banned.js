(async function() {
    try {
        const raw = localStorage.getItem('f0ck_anon_tombstone');
        const tombstone = raw ? JSON.parse(raw) : null;
        if (tombstone && tombstone.banned) {
            const bView = document.getElementById('banned-view');
            const uView = document.getElementById('unbanned-view');
            if (bView) bView.style.display = 'block';
            if (uView) uView.style.display = 'none';
            const rEl = document.getElementById('ban-reason-display');
            const eEl = document.getElementById('ban-expires-display');
            if (rEl && tombstone.reason) rEl.textContent = tombstone.reason;
            if (eEl && tombstone.expires) eEl.textContent = new Date(tombstone.expires).toLocaleString();
        }
    } catch (e) {}
})();
