    (function () {
        const banBtn = document.getElementById('ban-user-btn');
        const unbanBtn = document.getElementById('unban-user-btn');
        const warnBtn = document.getElementById('warn-user-btn');
        const banModal = document.getElementById('ban-modal');
        const warnModal = document.getElementById('warn-modal');
        const banClose = document.getElementById('ban-modal-close');
        const warnClose = document.getElementById('warn-modal-close');
        const banForm = document.getElementById('ban-form');
        const warnForm = document.getElementById('warn-form');
        const DATA = JSON.parse(document.getElementById('user-partial-mod-data').textContent);
        const userId = DATA.user_id;

        if (banBtn) {
            banBtn.onclick = () => {
                // Filter duration options for moderators
                const isAdmin = DATA.is_admin;
                const durationSelect = document.getElementById('ban-duration');
                if (durationSelect && !isAdmin) {
                    Array.from(durationSelect.options).forEach(opt => {
                        const val = opt.value;
                        if (val === 'permanent' || parseInt(val) > 48) {
                            opt.style.display = 'none';
                            opt.disabled = true;
                        }
                    });
                    // Ensure a valid option is selected
                    if (durationSelect.value === 'permanent' || parseInt(durationSelect.value) > 48) {
                        durationSelect.value = "1";
                    }
                }
                banModal.style.display = 'block';
            };
        }
        if (warnBtn) {
            warnBtn.onclick = () => warnModal.style.display = 'block';
        }

        if (unbanBtn) {
            unbanBtn.onclick = async () => {
                if (!confirm(DATA.msg_confirm_unban)) return;
                try {
                    const res = await fetch('/api/v2/admin/unban', {
                        method: 'POST',
                        headers: { 
                            'Content-Type': 'application/json',
                            'X-CSRF-Token': window.f0ckSession?.csrf_token
                        },
                        body: JSON.stringify({ user_id: userId })
                    });
                    const data = await res.json();
                    if (data.success) {
                        if (window.showFlash) window.showFlash(DATA.msg_unban_success, 'success');
                        else alert(DATA.msg_unban_success);
                        location.reload();
                    } else {
                        alert('Error: ' + data.msg);
                    }
                } catch (err) {
                    alert('Failed to unban user: ' + err.message);
                }
            };
        }

        if (banClose) {
            banClose.onclick = () => banModal.style.display = 'none';
        }
        if (warnClose) {
            warnClose.onclick = () => warnModal.style.display = 'none';
        }

        window.onclick = (event) => {
            if (event.target == banModal) {
                banModal.style.display = 'none';
            }
            if (event.target == warnModal) {
                warnModal.style.display = 'none';
            }
        };

        if (banForm) {
            banForm.onsubmit = async (e) => {
                e.preventDefault();
                const reason = document.getElementById('ban-reason').value;
                const duration = document.getElementById('ban-duration').value;

                try {
                    const res = await fetch('/api/v2/admin/ban', {
                        method: 'POST',
                        headers: { 
                            'Content-Type': 'application/json',
                            'X-CSRF-Token': window.f0ckSession?.csrf_token
                        },
                        body: JSON.stringify({ user_id: userId, reason, duration })
                    });
                    const data = await res.json();
                    if (data.success) {
                        if (window.showFlash) window.showFlash(DATA.msg_ban_success, 'success');
                        else alert(DATA.msg_ban_success);
                        location.reload();
                    } else {
                        alert('Error: ' + data.msg);
                    }
                } catch (err) {
                    alert('Failed to ban user: ' + err.message);
                }
            };
        }

        if (warnForm) {
            warnForm.onsubmit = async (e) => {
                e.preventDefault();
                const reason = document.getElementById('warn-reason').value;
                const submitBtn = warnForm.querySelector('button[type="submit"]');

                try {
                    submitBtn.disabled = true;
                    submitBtn.innerText = DATA.msg_warning_issuing;
                    
                    const payload = new URLSearchParams();
                    payload.append('user_id', userId);
                    payload.append('reason', reason);

                    const res = await fetch('/api/v2/mod/warnings/issue', {
                        method: 'POST',
                        headers: { 
                            'Content-Type': 'application/x-www-form-urlencoded',
                            'X-CSRF-Token': window.f0ckSession?.csrf_token
                        },
                        body: payload
                    });
                    const data = await res.json();
                    if (data.success) {
                        document.getElementById('warn-reason').value = '';
                        warnModal.style.display = 'none';
                        if (window.showFlash) window.showFlash(DATA.msg_warning_success, 'success');
                    } else {
                        alert('Error: ' + data.msg);
                    }
                } catch (err) {
                    alert('Failed to issue warning: ' + err.message);
                } finally {
                    submitBtn.disabled = false;
                    submitBtn.innerText = DATA.msg_warning_issue_btn;
                }
            };
        }
    })();
