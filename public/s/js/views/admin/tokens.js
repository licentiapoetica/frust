F0ckActions.define({
    approveInviteReq: (el, e, id) => window.approveInviteReq(id),
    denyInviteReq: (el, e, id) => window.denyInviteReq(id),
    deleteToken: (el, e, id) => window.deleteToken(id),
});
window.f0ckDebug = window.f0ckDebug || (() => {});

// ═══════════════════ Invite Requests ═══════════════════
window.loadInviteRequests = async () => {
    const tbody = document.getElementById('invite-req-list');
    try {
        const res = await fetch('/api/v2/admin/invite-requests');
        const data = await res.json();
        if (data.success) {
            const countBadge = document.getElementById('invite-req-count');
            if (!tbody) return;

            const pendingCount = data.requests.filter(r => r.status === 'pending').length;
            if (countBadge) {
                if (pendingCount > 0) {
                    countBadge.textContent = pendingCount;
                    countBadge.style.display = 'inline';
                } else {
                    countBadge.style.display = 'none';
                }
            }

            if (data.requests.length === 0) {
                tbody.innerHTML = '<tr><td colspan="6" class="adm-tokens-empty">No invite requests yet.</td></tr>';
                return;
            }

            tbody.innerHTML = data.requests.map(r => {

                const reason = r.reason ? '<span title="' + r.reason.replace(/"/g, '&quot;') + '">' + (r.reason.length > 60 ? r.reason.slice(0, 60) + '…' : r.reason) + '</span>' : '<span class="adm-tokens-no-reason">No reason given</span>';

                let statusBadge;
                if (r.status === 'pending') {
                    statusBadge = '<span class="adm-tokens-status adm-tokens-status-pending"><i class="fa-solid fa-clock"></i> Pending</span>';
                } else if (r.status === 'approved') {
                    statusBadge = '<span class="adm-tokens-status adm-tokens-status-approved"><i class="fa-solid fa-circle-check"></i> Approved</span>';
                } else if (r.status === 'revoked') {
                    statusBadge = '<span class="adm-tokens-status adm-tokens-status-revoked"><i class="fa-solid fa-ban"></i> Revoked</span>';
                } else {
                    statusBadge = '<span class="adm-tokens-status adm-tokens-status-denied"><i class="fa-solid fa-circle-xmark"></i> Denied</span>';
                }

                let actions = '';
                if (r.status === 'pending') {
                    actions =
                        '<div class="adm-tokens-req-actions">' +
                            '<button data-on="click:approveInviteReq" data-args="[' + r.id + ']" class="btn-upload adm-tokens-approve"><i class="fa-solid fa-check"></i> Approve</button>' +
                            '<button data-on="click:denyInviteReq" data-args="[' + r.id + ']" class="btn-remove adm-tokens-deny"><i class="fa-solid fa-xmark"></i> Deny</button>' +
                        '</div>';
                } else {
                    actions = '<span class="adm-tokens-reviewed">' + (r.reviewed_at ? new Date(r.reviewed_at).toLocaleString() : '') + '</span>';
                }

                const username = r.requester_login || r.requester_name || (r.fingerprint ? r.fingerprint.slice(0, 12) + '…' : '—');
                const userCell = r.requester_login
                    ? '<a href="/user/' + r.requester_login + '" class="adm-tokens-user-link">' + username + '</a>'
                    : '<span class="adm-tokens-user-anon">' + username + '</span>';

                return '<tr' + (r.status === 'pending' ? ' class="adm-tokens-req-pending"' : '') + '>' +
                    '<td data-label="User">' + userCell + '</td>' +
                    '<td data-label="Reason" class="adm-tokens-reason">' + reason + '</td>' +
                    '<td data-label="Status">' + statusBadge + '</td>' +
                    '<td data-label="Submitted">' + new Date(r.created_at).toLocaleString() + '</td>' +
                    '<td data-label="Reviewed By">' + (r.reviewed_by_name || '—') + '</td>' +
                    '<td data-label="Actions">' + actions + '</td>' +
                    '</tr>';
            }).join('');
        } else {
            if (tbody) tbody.innerHTML = '<tr><td colspan="6" class="adm-tokens-empty">Failed to load invite requests.</td></tr>';
        }
    } catch (e) {
        console.error('Failed to load invite requests:', e);
        if (tbody) tbody.innerHTML = '<tr><td colspan="6" class="adm-tokens-empty">Failed to load invite requests.</td></tr>';
    }
};

window.approveInviteReq = async (id) => {
    if (!confirm('Approve this invite request? A token will be generated and sent to the user.')) return;
    try {
        const res = await fetch('/api/v2/admin/invite-requests/approve', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-CSRF-Token': window.f0ckSession?.csrf_token
            },
            body: JSON.stringify({ id })
        });
        const data = await res.json();
        if (data.success) {
            window.loadInviteRequests();
            window.loadTokens();
        } else {
            alert('Failed: ' + (data.msg || 'Unknown error'));
        }
    } catch (e) {
        console.error(e);
        alert('Error: ' + e.message);
    }
};

window.denyInviteReq = async (id) => {
    if (!confirm('Deny this invite request?')) return;
    try {
        const res = await fetch('/api/v2/admin/invite-requests/deny', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-CSRF-Token': window.f0ckSession?.csrf_token
            },
            body: JSON.stringify({ id })
        });
        const data = await res.json();
        if (data.success) {
            window.loadInviteRequests();
        } else {
            alert('Failed: ' + (data.msg || 'Unknown error'));
        }
    } catch (e) {
        console.error(e);
        alert('Error: ' + e.message);
    }
};

// ═══════════════════ Tokens ═══════════════════
window.loadTokens = async () => {
    const tbody = document.getElementById('token-list');
    try {
        window.f0ckDebug('Loading tokens...');
        const res = await fetch('/api/v2/admin/tokens');
        const data = await res.json();
        window.f0ckDebug('Tokens data:', data);
        if (data.success) {
            if (!tbody) return;
            tbody.innerHTML = data.tokens.map(t =>
                '<tr>' +
                '<td data-label="Token" class="adm-tokens-token">' + t.token + '</td>' +
                '<td data-label="Status">' +
                (t.is_used ? '<span class="adm-tokens-used">Used</span>' : '<span class="adm-tokens-available">Available</span>') +
                '</td>' +
                '<td data-label="Source">' +
                (t.created_by_matrix ? '<span class="adm-tokens-src-matrix">[Matrix] ' + t.created_by_matrix + '</span>' :
                 (t.created_by_discord ? '<span class="adm-tokens-src-discord"><i class="fab fa-discord"></i> ' + t.created_by_discord + '</span>' :
                  (t.created_by_name ? '<span class="adm-tokens-src-user">' + t.created_by_name + '</span>' : '<span class="adm-tokens-src-admin">Admin</span>'))) +
                '</td>' +
                '<td data-label="Used By">' + (t.used_by_name || '—') + '</td>' +
                '<td data-label="Created">' + (t.created_at ? new Date(parseInt(t.created_at) * 1000).toLocaleString() : '—') + '</td>' +
                '<td data-label="Used At">' + (t.used_at ? new Date(t.used_at).toLocaleString() : '—') + '</td>' +
                '<td data-label="Actions">' +
                (!t.is_used ? '<button data-on="click:deleteToken" data-args="[' + t.id + ']" class="btn-remove adm-tokens-delete">Delete</button>' : '') +
                '</td>' +
                '</tr>'
            ).join('');
        } else {
            if (tbody) tbody.innerHTML = '<tr><td colspan="7" class="adm-tokens-empty">Failed to load tokens.</td></tr>';
        }
    } catch (e) {
        console.error(e);
        if (tbody) tbody.innerHTML = '<tr><td colspan="7" class="adm-tokens-empty">Failed to load tokens.</td></tr>';
    }
};

window.generateToken = async () => {
    window.f0ckDebug('Generating...');
    try {
        const res = await fetch('/api/v2/admin/tokens/create', { 
            method: 'POST',
            headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token }
        });
        const data = await res.json();
        window.f0ckDebug('Gen result:', data);
        if (data.success) {
            window.loadTokens();
        } else {
            alert('Failed: ' + data.msg);
        }
    } catch (e) {
        console.error(e);
        alert('Error: ' + e.message);
    }
};

window.deleteToken = async (id) => {
    if (!confirm('Delete this token?')) return;
    const res = await fetch('/api/v2/admin/tokens/delete', {
        method: 'POST',
        headers: { 
            'Content-Type': 'application/json',
            'X-CSRF-Token': window.f0ckSession?.csrf_token
        },
        body: JSON.stringify({ id })
    });
    const data = await res.json();
    if (data.success) {
        window.loadTokens();
    }
};

var btnGenToken = document.getElementById('generate-token');
if (btnGenToken) btnGenToken.addEventListener('click', window.generateToken);
window.loadTokens();
window.loadInviteRequests();
