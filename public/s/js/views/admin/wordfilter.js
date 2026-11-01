(function() {
    const escapeHtml = (str) => {
        if (!str) return '';
        return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#039;");
    };

    const loadRules = async () => {
        try {
            const res = await fetch('/api/v2/admin/wordfilter');
            const data = await res.json();
            if (data.success) {
                const tbody = document.getElementById('filter-list');
                const noRulesMsg = document.getElementById('no-rules-msg');
                if (!tbody) return;

                if (!data.filters || data.filters.length === 0) {
                    tbody.innerHTML = '';
                    noRulesMsg.style.display = 'block';
                    return;
                }

                noRulesMsg.style.display = 'none';
                tbody.innerHTML = data.filters.map(f =>
                    '<tr>' +
                        '<td data-label="Original Word" class="adm-wordfilter-word">' + escapeHtml(f.word) + '</td>' +
                        '<td data-label="Replacement" class="adm-wordfilter-replacement">' + escapeHtml(f.replacement) + '</td>' +
                        '<td data-label="Created">' + (f.created_at ? new Date(f.created_at).toLocaleString() : '—') + '</td>' +
                        '<td data-label="Actions" class="adm-wordfilter-actions-cell">' +
                            '<button data-on="click:wfDeleteRule" data-args="[' + f.id + ']" class="btn-remove adm-wordfilter-delete">Delete</button>' +
                        '</td>' +
                    '</tr>'
                ).join('');
            }
        } catch (e) {
            console.error('Failed to load rules:', e);
        }
    };

    const addRule = async (form) => {
        const status = document.getElementById('form-status');
        status.textContent = 'Saving...';
        status.style.color = 'var(--accent)';

        try {
            const res = await fetch('/api/v2/admin/wordfilter', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/x-www-form-urlencoded',
                    'X-CSRF-Token': window.f0ckSession?.csrf_token
                },
                body: new URLSearchParams(new FormData(form))
            });
            const data = await res.json();
            if (data.success) {
                status.textContent = 'Rule added successfully!';
                status.style.color = '#28a745';
                form.reset();
                loadRules();
                setTimeout(() => status.textContent = '', 3000);
            } else {
                throw new Error(data.msg || 'Save failed');
            }
        } catch (e) {
            status.textContent = 'Error: ' + e.message;
            status.style.color = '#d9534f';
        }
    };

    const deleteRule = async (id) => {
        if (!confirm('Are you sure you want to delete this wordfilter rule?')) return;
        try {
            const res = await fetch('/api/v2/admin/wordfilter/delete', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                    'X-CSRF-Token': window.f0ckSession?.csrf_token
                },
                body: JSON.stringify({ id })
            });
            const data = await res.json();
            if (data.success) {
                loadRules();
            } else {
                alert('Delete failed: ' + data.msg);
            }
        } catch (e) {
            alert('Error deleting: ' + e.message);
        }
    };

    window.wordfilterAdmin = {
        loadRules,
        addRule,
        deleteRule
    };

    F0ckActions.define({
        wfAddRule: (el) => addRule(el),
        wfDeleteRule: (el, e, id) => deleteRule(id),
    });

    // Initialize view
    loadRules();
})();
