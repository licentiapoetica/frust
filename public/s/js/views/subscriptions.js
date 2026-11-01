// Use a self-invoking function to avoid global scope pollution if possible
// but since it's a click listener on document, it's already global.
// We only want to add it once.
if (!window._subsInit) {
    window._subsInit = true;
    document.addEventListener('click', async (e) => {
        const btn = e.target.closest('.unsub-btn');
        if (!btn) return;

        if (!confirm('Unsubscribe from this thread?')) return;

        const id = btn.dataset.id;
        const card = document.getElementById('sub-' + id);
        const originalText = btn.textContent;
        btn.textContent = '...';

        try {
            const res = await fetch('/api/subscriptions/' + id + '/delete', { method: 'POST' });
            const json = await res.json();

            if (json.success) {
                if (card) {
                    card.style.opacity = '0';
                    setTimeout(() => {
                        card.remove();
                        if (document.querySelectorAll('.sub-card').length === 0) {
                            location.reload();
                        }
                    }, 300);
                }
            } else {
                alert('Error: ' + (json.message || 'Failed'));
                btn.textContent = originalText;
            }
        } catch (err) {
            console.error(err);
            alert('Error removing subscription');
            btn.textContent = originalText;
        }
    });
}
