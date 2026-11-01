F0ckActions.define({
    switchTab: (el, e, tabId) => switchTab(tabId),
    copyText: (el, e, text) => copyText(text),
    openBanModal: (el, e, ...args) => openBanModal(...args),
    closeBanModal: () => closeBanModal(),
    promptBanIdentity: (el, e, userId, fingerprint) => promptBanIdentity(userId, fingerprint),
    submitBan: (el, e) => submitBan(e),
    unbanFingerprint: (el, e, fp) => unbanFingerprint(fp),
    unbanHardware: (el, e, hw) => unbanHardware(hw),
    unbanIp: (el, e, ip) => unbanIp(ip),
});

function switchTab(tabId) {
    document.querySelectorAll('.admin-tab-btn').forEach(btn => btn.classList.remove('active-tab'));
    document.querySelectorAll('.tab-pane').forEach(pane => {
        pane.style.display = 'none';
        pane.classList.remove('active-pane');
    });

    const btn = document.getElementById('tab-btn-' + tabId);
    const pane = document.getElementById('tab-pane-' + tabId);
    if (btn) btn.classList.add('active-tab');
    if (pane) {
        pane.style.display = 'block';
        pane.classList.add('active-pane');
    }
}

function copyText(text) {
    if (navigator.clipboard) {
        navigator.clipboard.writeText(text).then(() => {
            alert('Copied to clipboard: ' + text);
        }).catch(() => {
            prompt('Copy text:', text);
        });
    } else {
        prompt('Copy text:', text);
    }
}

function openBanModal(type, targetValue = '', userId = null) {
    document.getElementById('ban-target-type').value = type;
    document.getElementById('ban-target-user-id').value = userId || '';
    document.getElementById('ban-target-value').value = targetValue;

    const modalTitle = document.getElementById('modal-title');
    const valueLabel = document.getElementById('target-value-label');
    const cascadeGroup = document.getElementById('cascade-ips-group');
    const targetInput = document.getElementById('ban-target-value');

    if (type === 'fingerprint') {
        modalTitle.innerHTML = '<i class="fa-solid fa-key adm-bans-icon-ban"></i> Ban Key Fingerprint';
        valueLabel.innerText = 'OpenSSH Key Fingerprint (e.g. SHA256:...)';
        targetInput.placeholder = 'SHA256:...';
        cascadeGroup.style.display = 'flex';
    } else if (type === 'hardware') {
        modalTitle.innerHTML = '<i class="fa-solid fa-microchip adm-bans-icon-hw"></i> Ban Hardware ID';
        valueLabel.innerText = 'Hardware Fingerprint (e.g. HW:...)';
        targetInput.placeholder = 'HW:...';
        cascadeGroup.style.display = 'flex';
    } else {
        modalTitle.innerHTML = '<i class="fa-solid fa-network-wired adm-bans-icon-ip"></i> Ban IP Address';
        valueLabel.innerText = 'IP Address (IPv4 or IPv6 or Hash)';
        targetInput.placeholder = '198.51.100.42';
        cascadeGroup.style.display = 'none';
    }

    document.getElementById('ban-modal').style.display = 'flex';
}

function closeBanModal() {
    document.getElementById('ban-modal').style.display = 'none';
}

function promptBanIdentity(userId, fingerprint) {
    openBanModal('fingerprint', fingerprint, userId);
}

function getCsrfToken() {
    return (window.f0ckSession && window.f0ckSession.csrf_token) ||
           document.querySelector('meta[name="csrf-token"]')?.content ||
           document.getElementById('ban-form').dataset.csrf ||
           document.getElementById('ban-form').dataset.sessionCsrf ||
           '';
}

async function submitBan(e) {
    e.preventDefault();
    const type = document.getElementById('ban-target-type').value;
    const value = document.getElementById('ban-target-value').value.trim();
    const reason = document.getElementById('ban-reason').value.trim();
    const duration = document.getElementById('ban-duration').value;
    const cascadeIps = document.getElementById('ban-cascade-ips').checked;
    const userId = document.getElementById('ban-target-user-id').value;

    if (!value) return alert('Target value is required');

    try {
        const csrfToken = getCsrfToken();
        let endpoint = type === 'fingerprint' 
            ? '/api/v2/admin/bans/fingerprint/ban' 
            : (type === 'hardware' ? '/api/v2/admin/bans/hardware/ban' : '/api/v2/admin/bans/ip/ban');

        const payload = type === 'fingerprint'
            ? { fingerprint: value, reason, duration, ban_ips: cascadeIps, user_id: userId || undefined, csrf_token: csrfToken }
            : (type === 'hardware'
                ? { hw_fingerprint: value, reason, duration, ban_ips: cascadeIps, user_id: userId || undefined, csrf_token: csrfToken }
                : { ip: value, reason, duration, csrf_token: csrfToken });

        const res = await fetch(endpoint, {
            method: 'POST',
            headers: { 
                'Content-Type': 'application/json',
                'X-CSRF-Token': csrfToken
            },
            body: JSON.stringify(payload)
        });

        const data = await res.json();
        if (data.success) {
            alert('Ban successfully applied!');
            window.location.reload();
        } else {
            alert(data.msg || 'Failed to apply ban');
        }
    } catch (err) {
        console.error(err);
        alert('Network error while enforcing ban');
    }
}

async function unbanFingerprint(fingerprint) {
    if (!confirm('Are you sure you want to unban this key fingerprint: ' + fingerprint + '?')) return;

    try {
        const csrfToken = getCsrfToken();
        const res = await fetch('/api/v2/admin/bans/fingerprint/unban', {
            method: 'POST',
            headers: { 
                'Content-Type': 'application/json',
                'X-CSRF-Token': csrfToken
            },
            body: JSON.stringify({ fingerprint, csrf_token: csrfToken })
        });
        const data = await res.json();
        if (data.success) {
            alert('Fingerprint successfully unbanned!');
            window.location.reload();
        } else {
            alert(data.msg || 'Failed to unban fingerprint');
        }
    } catch (err) {
        console.error(err);
        alert('Network error while unbanning');
    }
}

async function unbanHardware(hw_fingerprint) {
    if (!confirm('Are you sure you want to unban this Hardware ID: ' + hw_fingerprint + '?')) return;

    try {
        const csrfToken = getCsrfToken();
        const res = await fetch('/api/v2/admin/bans/hardware/unban', {
            method: 'POST',
            headers: { 
                'Content-Type': 'application/json',
                'X-CSRF-Token': csrfToken
            },
            body: JSON.stringify({ hw_fingerprint, csrf_token: csrfToken })
        });
        const data = await res.json();
        if (data.success) {
            alert('Hardware ID successfully unbanned!');
            window.location.reload();
        } else {
            alert(data.msg || 'Failed to unban Hardware ID');
        }
    } catch (err) {
        console.error(err);
        alert('Network error while unbanning Hardware ID');
    }
}

async function unbanIp(ip) {
    if (!confirm('Are you sure you want to unban this IP: ' + ip + '?')) return;

    try {
        const csrfToken = getCsrfToken();
        const res = await fetch('/api/v2/admin/bans/ip/unban', {
            method: 'POST',
            headers: { 
                'Content-Type': 'application/json',
                'X-CSRF-Token': csrfToken
            },
            body: JSON.stringify({ ip, csrf_token: csrfToken })
        });
        const data = await res.json();
        if (data.success) {
            alert('IP successfully unbanned!');
            window.location.reload();
        } else {
            alert(data.msg || 'Failed to unban IP');
        }
    } catch (err) {
        console.error(err);
        alert('Network error while unbanning IP');
    }
}
