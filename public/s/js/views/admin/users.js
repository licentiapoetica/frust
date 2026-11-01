        // Row buttons in admin/users_list.html (also rendered alone for paging/search): each calls window[name](button)
        F0ckActions.define(Object.fromEntries([
            'activateUser', 'unbanUser', 'banUser', 'deleteUploads', 'deleteComments',
            'adminBulkDeleteHalls', 'adminReassignUploads', 'adminResetLoginAttempts',
            'adminSetDisplayName', 'adminRenameUser', 'adminSetPassword', 'adminLockLayout',
            'adminSetRole', 'adminSetGroups', 'adminDeleteUser'
        ].map(function(name) { return [name, function(el) { return window[name](el); }]; })));
        F0ckActions.define({
            openCreateUserModal: function() { openCreateUserModal(); },
            closeCreateUserModal: function() { closeCreateUserModal(); },
            submitCreateUser: function() { submitCreateUser(); }
        });

        function updateUI(id, data, action) {
            var rowId = id ? id : ('ghost-' + data.username);
            var statusCell = document.getElementById('status-cell-' + rowId);
            var actionsDiv = document.getElementById('actions-' + rowId);
            if (!statusCell || !actionsDiv) return console.error('UI elements not found for ID', rowId);

            if (action === 'activate') {
                statusCell.innerHTML = '<span class="status-badge status-active">Active</span>';
                var verifyBtn = actionsDiv.querySelector('.btn-verify');
                if (verifyBtn) verifyBtn.remove();
            } else if (action === 'ban') {
                statusCell.innerHTML = '<span class="status-badge status-banned">Banned</span>';
                var banBtn = actionsDiv.querySelector('.btn-ban');
                if (banBtn) {
                    banBtn.textContent = 'Unban';
                    banBtn.className = 'btn-modern btn-unban';
                    // We re-bind the onclick via HTML because simple assignment doesn't preserve the 'this' context in the same way with these handlers
                }
                var verifyBtn = actionsDiv.querySelector('.btn-verify');
                if (verifyBtn) verifyBtn.remove();
            } else if (action === 'unban') {
                statusCell.innerHTML = '<span class="status-badge status-active">Active</span>';
                var unbanBtn = actionsDiv.querySelector('.btn-unban');
                if (unbanBtn) {
                    unbanBtn.textContent = 'Ban';
                    unbanBtn.className = 'btn-modern btn-ban';
                }
            }
        }

        async function activateUser(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            
            ModAction.confirm('Verify User', 'Manually verify account for <strong>' + escHTML(userName) + '</strong>?', async () => {
                var res = await fetch('/api/v2/admin/users/activate', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ user_id: id })
                });
                var data = await res.json();
                if (data.success) {
                    showFlash(data.msg, 'success');
                    updateUI(id, data, 'activate');
                } else {
                    throw new Error(data.msg || 'Failed to verify user');
                }
            }, { hideReason: true });
        }

        async function banUser(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            
            ModAction.confirm('Ban User', 'Reason for banning <strong>' + escHTML(userName) + '</strong>?', async (reason) => {
                var res = await fetch('/api/v2/admin/ban', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ user_id: id, reason: reason, duration: 'permanent' })
                });
                var data = await res.json();
                if (data.success) {
                    showFlash('User ' + escHTML(userName) + ' banned.', 'success');
                    updateUI(id, data, 'ban');
                } else {
                    throw new Error(data.msg || 'Failed to ban user');
                }
            }, { hideReason: false, confirmText: 'Ban User' });
        }

        async function unbanUser(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            
            ModAction.confirm('Unban User', 'Unban account for <strong>' + escHTML(userName) + '</strong>?', async () => {
                var res = await fetch('/api/v2/admin/unban', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ user_id: id })
                });
                var data = await res.json();
                if (data.success) {
                    showFlash('User ' + escHTML(userName) + ' unbanned.', 'success');
                    updateUI(id, data, 'unban');
                } else {
                    throw new Error(data.msg || 'Failed to unban user');
                }
            }, { hideReason: true });
        }

        async function deleteUploads(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            
            ModAction.confirm('Delete Uploads', 'Are you SURE you want to delete ALL uploads by <strong>' + escHTML(userName) + '</strong>? This cannot be undone.', async () => {
                var res = await fetch('/api/v2/admin/users/bulk-delete-items', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ user_id: id, username: btn.dataset.username })
                });
                var data = await res.json();
                if (data.success) {
                    showFlash(data.msg, 'success');
                } else {
                    throw new Error(data.msg || 'Deletion failed');
                }
            }, { hideReason: true, confirmText: 'Delete Everything' });
        }

        async function deleteComments(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            
            ModAction.confirm('Delete Comments', 'Are you SURE you want to delete ALL comments by <strong>' + escHTML(userName) + '</strong>? This will be permanent.', async () => {
                var res = await fetch('/api/v2/admin/users/bulk-delete-comments', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ user_id: id, username: btn.dataset.username })
                });
                var data = await res.json();
                if (data.success) {
                    showFlash(data.msg, 'success');
                } else {
                    throw new Error(data.msg || 'Deletion failed');
                }
            }, { hideReason: true, confirmText: 'Delete All Comments' });
        }

        async function adminSetDisplayName(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            var currentDisplay = btn.dataset.display || '';

            var hint = currentDisplay
                ? 'Current nick: <strong class="adm-users-hint-nick">' + escHTML(currentDisplay) + '</strong><br>Enter a new stylized name, or leave empty to clear it.'
                : 'Enter a stylized display name for <strong>' + escHTML(userName) + '</strong>. Leave empty to clear.';

            ModAction.confirm('Set Display Name', hint, async (newName) => {
                var res = await fetch('/api/v2/admin/users/set-display-name', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ user_id: id, display_name: newName || '' })
                });
                var data = await res.json();
                if (data.success) {
                    showFlash(data.msg || 'Display name updated.', 'success');
                    // Update button data attribute for next edit
                    btn.dataset.display = data.display_name || '';
                    // Refresh the row name link in the table
                    var rowId = 'user-row-' + id;
                    var row = document.getElementById(rowId);
                    if (row) {
                        var link = row.querySelector('.user-info-cell a');
                        if (link && data.display_name) {
                            link.innerHTML = '<span class="adm-users-display-name">' + escHTML(data.display_name) + '</span> <span class="adm-users-login">(' + escHTML(userName) + ')</span>';
                        } else if (link) {
                            link.textContent = userName;
                        }
                    }
                } else {
                    throw new Error(data.msg || 'Failed to set display name');
                }
            }, { hideReason: false, singleLine: true, allowEmpty: true, confirmText: 'Set Nick', placeholder: currentDisplay || 'Set nickname' });
        }

        async function adminLockLayout(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            var isLocked = btn.dataset.locked === '1';
            var currentMode = btn.dataset.mode || '0';

            if (isLocked) {
                ModAction.confirm('Unlock Layout', 'Unlock comment layout for <strong>' + escHTML(userName) + '</strong>? They will be able to change it again.', async () => {
                    var res = await fetch('/api/v2/admin/users/lock-layout', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                        body: JSON.stringify({ user_id: id, lock: false })
                    });
                    var data = await res.json();
                    if (data.success) {
                        showFlash('Layout unlocked for ' + escHTML(userName), 'success');
                        btn.dataset.locked = '0';
                        btn.innerHTML = '<i class="fa fa-lock"></i> Lock';
                        btn.title = 'Lock Layout';
                    } else {
                        throw new Error(data.msg || 'Failed to unlock layout');
                    }
                }, { hideReason: true });
            } else {
                var hint = 'Select comment display mode to force for <strong>' + escHTML(userName) + '</strong>:<br><br>' +
                           '<select id="force-mode-select" class="input adm-users-modal-select">' +
                           '<option value="0" ' + (currentMode == '0' ? 'selected' : '') + '>Tree</option>' +
                           '<option value="1" ' + (currentMode == '1' ? 'selected' : '') + '>Linear</option>' +
                           '</select>';

                ModAction.confirm('Lock Layout', hint, async () => {
                    var mode = document.getElementById('force-mode-select').value;
                    var res = await fetch('/api/v2/admin/users/lock-layout', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                        body: JSON.stringify({ user_id: id, lock: true, mode: mode })
                    });
                    var data = await res.json();
                    if (data.success) {
                        showFlash('Layout locked to ' + (mode == '0' ? 'Tree' : 'Linear') + ' for ' + escHTML(userName), 'success');
                        btn.dataset.locked = '1';
                        btn.dataset.mode = mode;
                        btn.innerHTML = '<i class="fa fa-lock-open"></i> Unlock';
                        btn.title = 'Unlock Layout';
                    } else {
                        throw new Error(data.msg || 'Failed to lock layout');
                    }
                }, { hideReason: true, confirmText: 'Lock & Apply' });
            }
        }

        var adminUsersData = JSON.parse(document.getElementById('admin-users-data').textContent);
        var currentPage = adminUsersData.page;
        var hasMore = adminUsersData.hasMore;
        var isLoading = false;
        var searchQuery = adminUsersData.q;
        var onlyLegacy = adminUsersData.onlyLegacy;
        var currentStatus = adminUsersData.status;
        var currentRole = adminUsersData.role;

        var searchInput = document.getElementById('user-search');
        var statusFilter = document.getElementById('status-filter');
        var roleFilter = document.getElementById('role-filter');
        var legacyFilter = document.getElementById('legacy-filter');
        var legacyFilterLabel = document.getElementById('legacy-filter-label');
        var resetFiltersBtn = document.getElementById('reset-filters-btn');
        var totalLabel = document.getElementById('total-label');
        var tableBody = document.getElementById('user-table-body');
        var loadingTrigger = document.getElementById('loading-trigger');
        var noUsersMsg = document.getElementById('no-users-msg');
        var spinner = document.getElementById('search-spinner');
        var countSpan = document.getElementById('total-count');

        function updateUrlAndControls() {
            var urlParams = new URLSearchParams();
            if (searchQuery) urlParams.set('q', searchQuery);
            if (onlyLegacy) {
                urlParams.set('legacy', '1');
            } else {
                if (currentStatus) urlParams.set('status', currentStatus);
                if (currentRole) urlParams.set('role', currentRole);
            }
            var newUrl = window.location.pathname + (urlParams.toString() ? ('?' + urlParams.toString()) : '');
            window.history.replaceState({}, '', newUrl);

            // Update reset button
            var hasFilter = !!(searchQuery || onlyLegacy || currentStatus || currentRole);
            if (resetFiltersBtn) resetFiltersBtn.style.display = hasFilter ? 'inline-flex' : 'none';

            // Update control states
            if (statusFilter) {
                statusFilter.disabled = onlyLegacy;
                statusFilter.classList.toggle('active', !!currentStatus && !onlyLegacy);
            }
            if (roleFilter) {
                roleFilter.disabled = onlyLegacy;
                roleFilter.classList.toggle('active', !!currentRole && !onlyLegacy);
            }
            if (legacyFilterLabel) {
                legacyFilterLabel.classList.toggle('active', onlyLegacy);
            }
        }

        async function fetchUsers(page, q, append) {
            if (isLoading) return;
            isLoading = true;
            if (!append) spinner.style.display = 'block';

            try {
                var params = new URLSearchParams();
                params.set('page', page);
                if (q) params.set('q', q);
                if (onlyLegacy) {
                    params.set('legacy', '1');
                } else {
                    if (currentStatus) params.set('status', currentStatus);
                    if (currentRole) params.set('role', currentRole);
                }

                var url = '/admin/users?' + params.toString();
                var res = await fetch(url, { headers: { 'X-Requested-With': 'XMLHttpRequest' } });
                
                // Update state from headers
                var total = res.headers.get('X-Total-Count');
                var hasMoreHeader = res.headers.get('X-Has-More');
                var totalLabelHeader = res.headers.get('X-Total-Label');
                var emptyMsgHeader = res.headers.get('X-Empty-Msg');
                
                if (total !== null) countSpan.textContent = total;
                if (totalLabelHeader && totalLabel) totalLabel.textContent = totalLabelHeader;
                if (emptyMsgHeader && noUsersMsg) noUsersMsg.textContent = emptyMsgHeader;
                if (hasMoreHeader !== null) hasMore = (hasMoreHeader === 'true');

                var html = await res.text();
                
                if (append) {
                    tableBody.insertAdjacentHTML('beforeend', html);
                } else {
                    tableBody.innerHTML = html;
                }
                
                noUsersMsg.style.display = (tableBody.children.length === 0) ? 'block' : 'none';
            } catch (e) {
                console.error('Fetch failed', e);
            } finally {
                isLoading = false;
                spinner.style.display = 'none';
            }
        }

        // Search Input Handling (Debounced)
        var searchTimeout;
        searchInput.addEventListener('input', function() {
            clearTimeout(searchTimeout);
            searchTimeout = setTimeout(function() {
                searchQuery = searchInput.value.trim();
                currentPage = 1;
                updateUrlAndControls();
                fetchUsers(1, searchQuery, false);
            }, 300);
        });

        // Status Filter Handling
        if (statusFilter) {
            statusFilter.addEventListener('change', function() {
                currentStatus = this.value;
                if (currentStatus && onlyLegacy) {
                    onlyLegacy = false;
                    if (legacyFilter) legacyFilter.checked = false;
                }
                updateUrlAndControls();
                currentPage = 1;
                fetchUsers(1, searchQuery, false);
            });
        }

        // Role Filter Handling
        if (roleFilter) {
            roleFilter.addEventListener('change', function() {
                currentRole = this.value;
                if (currentRole && onlyLegacy) {
                    onlyLegacy = false;
                    if (legacyFilter) legacyFilter.checked = false;
                }
                updateUrlAndControls();
                currentPage = 1;
                fetchUsers(1, searchQuery, false);
            });
        }

        // Legacy Only Filter Handling
        if (legacyFilter) {
            legacyFilter.addEventListener('change', function() {
                onlyLegacy = this.checked;
                if (onlyLegacy) {
                    currentStatus = '';
                    currentRole = '';
                    if (statusFilter) statusFilter.value = '';
                    if (roleFilter) roleFilter.value = '';
                }
                updateUrlAndControls();
                currentPage = 1;
                fetchUsers(1, searchQuery, false);
            });
        }

        // Reset All Filters
        if (resetFiltersBtn) {
            resetFiltersBtn.addEventListener('click', function() {
                searchQuery = '';
                searchInput.value = '';
                onlyLegacy = false;
                currentStatus = '';
                currentRole = '';
                if (legacyFilter) legacyFilter.checked = false;
                if (statusFilter) statusFilter.value = '';
                if (roleFilter) roleFilter.value = '';
                updateUrlAndControls();
                currentPage = 1;
                fetchUsers(1, '', false);
            });
        }

        // Infinite Scroll
        var observer = new IntersectionObserver(function(entries) {
            if (entries[0].isIntersecting && hasMore && !isLoading) {
                currentPage++;
                fetchUsers(currentPage, searchQuery, true);
            }
        }, { threshold: 0.1 });
        observer.observe(loadingTrigger);

        // --- Create User Modal ---
        var createUserModal = document.getElementById('create-user-modal');

        function openCreateUserModal() {
            document.getElementById('cum-username').value = '';
            document.getElementById('cum-email').value = '';
            document.getElementById('cum-password').value = '';
            document.getElementById('cum-role').value = 'user';
            document.getElementById('cum-error').style.display = 'none';
            document.getElementById('cum-error').textContent = '';
            createUserModal.classList.add('open');
            setTimeout(function() { document.getElementById('cum-username').focus(); }, 80);
        }

        function closeCreateUserModal() {
            createUserModal.classList.remove('open');
        }

        // Close on backdrop click
        createUserModal.addEventListener('click', function(e) {
            if (e.target === createUserModal) closeCreateUserModal();
        });

        // Close on Escape
        document.addEventListener('keydown', function(e) {
            if (e.key === 'Escape' && createUserModal.classList.contains('open')) closeCreateUserModal();
        });

        async function submitCreateUser() {
            var btn = document.getElementById('cum-submit-btn');
            var errEl = document.getElementById('cum-error');
            var username = document.getElementById('cum-username').value.trim();
            var email    = document.getElementById('cum-email').value.trim();
            var password = document.getElementById('cum-password').value;
            var role     = document.getElementById('cum-role').value;

            errEl.style.display = 'none';
            errEl.textContent = '';

            if (!username) { errEl.textContent = 'Username is required.'; errEl.style.display = 'block'; return; }
            if (!password) { errEl.textContent = 'Password is required.'; errEl.style.display = 'block'; return; }

            btn.disabled = true;
            btn.innerHTML = '<i class="fa fa-spinner fa-spin"></i> Creating…';

            try {
                var res = await fetch('/api/v2/admin/users/create', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ username, email: email || null, password, role })
                });
                var data = await res.json();

                if (data.success) {
                    closeCreateUserModal();
                    showFlash(data.msg, 'success');
                    // If we were filtering by legacy or banned, reset filter so admin can see newly created user
                    if (onlyLegacy || currentStatus === 'banned') {
                        onlyLegacy = false;
                        currentStatus = '';
                        if (legacyFilter) legacyFilter.checked = false;
                        if (statusFilter) statusFilter.value = '';
                        updateUrlAndControls();
                    }
                    // Refresh the table from page 1
                    currentPage = 1;
                    fetchUsers(1, searchQuery, false);
                    // Update counter
                    var cnt = parseInt(countSpan.textContent) || 0;
                    countSpan.textContent = cnt + 1;
                } else {
                    errEl.textContent = data.msg || 'Failed to create user.';
                    errEl.style.display = 'block';
                }
            } catch (e) {
                errEl.textContent = 'Network error: ' + e.message;
                errEl.style.display = 'block';
            } finally {
                btn.disabled = false;
                btn.innerHTML = '<i class="fa fa-plus"></i> Create User';
            }
        }
        async function adminSetRole(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            var currentRole = btn.dataset.role || 'user';

            var hint = 'Select the new role for <strong>' + escHTML(userName) + '</strong>:<br><br>' +
                       '<select id="set-role-select" class="input adm-users-modal-select adm-users-role-select">' +
                       '<option value="user"'   + (currentRole === 'user'  ? ' selected' : '') + '>User (no special permissions)</option>' +
                       '<option value="mod"'    + (currentRole === 'mod'   ? ' selected' : '') + '>Moderator</option>' +
                       '<option value="admin"'  + (currentRole === 'admin' ? ' selected' : '') + '>Admin (full access)</option>' +
                       '</select>';

            ModAction.confirm('Set Role', hint, async () => {
                // Use querySelector as fallback since Sanitizer may strip the id attribute
                var selectEl = document.getElementById('set-role-select') ||
                               document.querySelector('#mod-action-content select');
                if (!selectEl) throw new Error('Role selector not found in modal.');
                var role = selectEl.value;
                var res = await fetch('/api/v2/admin/users/set-role', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ user_id: id, role })
                });
                var data = await res.json();
                if (data.success) {
                    showFlash(data.msg, 'success');
                    // Update button state in place
                    btn.dataset.role = role;
                    var labelMap = { user: 'User', mod: 'Mod', admin: 'Admin' };
                    var labelEl = document.querySelector('.role-label-' + id);
                    if (labelEl) labelEl.textContent = labelMap[role] || role;
                } else {
                    throw new Error(data.msg || 'Failed to set role');
                }
            }, { hideReason: true, confirmText: 'Apply Role', unsafeContent: true });
        }

        async function adminSetGroups(btn) {
            var id = btn.dataset.id;
            var userName = btn.dataset.name;
            var currentGroups = (btn.dataset.groups || '').split(',').map(function(s) { return s.trim().toLowerCase(); }).filter(Boolean);
            var is4chan = currentGroups.indexOf('4chan') !== -1;

            var hint = 'Manage groups for <strong>' + escHTML(userName) + '</strong>:<br><br>' +
                       '<label class="adm-users-group-option">' +
                       '<input type="checkbox" id="group-4chan-checkbox" value="4chan"' + (is4chan ? ' checked' : '') + ' class="adm-users-group-checkbox">' +
                       '<span><strong class="adm-users-group-name">🍀 4chan</strong> — Grants access to the 4chan catalogue, thread viewer, and rehosting</span>' +
                       '</label>';

            ModAction.confirm('Manage Groups', hint, async () => {
                var chk = document.getElementById('group-4chan-checkbox') || document.querySelector('#mod-action-content input[type="checkbox"]');
                var newGroups = [];
                if (chk && chk.checked) newGroups.push('4chan');

                var res = await fetch('/api/v2/admin/users/set-groups', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ user_id: id, groups: newGroups })
                });
                var data = await res.json();
                if (data.success) {
                    showFlash(data.msg, 'success');
                    btn.dataset.groups = newGroups.join(',');
                    var labelEl = document.querySelector('.groups-label-' + id);
                    if (labelEl) labelEl.textContent = newGroups.indexOf('4chan') !== -1 ? '4chan' : 'Groups';
                } else {
                    throw new Error(data.msg || 'Failed to update groups');
                }
            }, { hideReason: true, confirmText: 'Save Groups', unsafeContent: true });
        }
