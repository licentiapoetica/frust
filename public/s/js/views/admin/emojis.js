        (() => {
            var i18n = window.f0ckI18n || {};
            var csrf = JSON.parse(document.getElementById('admin-emojis-data').textContent).csrf;

            function esc(s) {
                return (s || '').toString()
                    .replace(/&/g, '&amp;')
                    .replace(/</g, '&lt;')
                    .replace(/>/g, '&gt;')
                    .replace(/"/g, '&quot;')
                    .replace(/'/g, '&#039;');
            }

            /* Returns an HTML string for a sticker preview — <video> for .webm, <img> otherwise */
            function stickerEl(url, name, extraAttrs) {
                var a = extraAttrs || '';
                if (url && url.slice(-5) === '.webm') {
                    return '<video class="emoji-preview" src="' + url + '" autoplay loop muted playsinline title=":' + esc(name) + ':"' + a + '></video>';
                }
                return '<img class="emoji-preview" src="' + url + '" alt=":' + esc(name) + ':"' + a + '>';
            }

            /* ── DATA LOAD ── */
            function loadAll() {
                Promise.all([
                    fetch('/api/v2/emojis'),
                    fetch('/api/v2/admin/sticker-packs', { headers: { 'X-Requested-With': 'XMLHttpRequest' } })
                ]).then(function(responses) {
                    return Promise.all([responses[0].json(), responses[1].json()]);
                }).then(function(results) {
                    var emojisData = results[0];
                    var packsData  = results[1];
                    window.emojiAdmin.emojis = emojisData.emojis || [];
                    window.emojiAdmin.packs  = packsData.packs   || [];
                    renderStandaloneEmojis(window.emojiAdmin.emojis);
                    renderPacksList(window.emojiAdmin.packs);
                }).catch(function(err) {
                    console.error('[EMOJI_ADMIN] Load Error:', err);
                });
            }

            /* ── STANDALONE EMOJIS ── */
            function renderStandaloneEmojis(emojis) {
                var grid = document.getElementById('emoji-list');
                if (!grid) return;
                var standalone = emojis.filter(function(e) { return !e.pack_id; });
                if (standalone.length === 0) {
                    grid.innerHTML = '<p class="adm-emojis-empty">No standalone emojis yet.</p>';
                    return;
                }
                grid.innerHTML = standalone.map(function(e) {
                    return '<div class="emoji-card">' +
                        '<button class="emoji-delete" data-on="click:emojiDelete" data-args="[' + e.id + ']" title="Delete">x</button>' +
                        stickerEl(e.url, e.name) +
                        '<span class="emoji-label">:' + esc(e.name) + ':</span>' +
                        '<button data-on="click:emojiOpenEditModal" data-args="[' + e.id + ']" class="adm-emojis-card-edit-btn">Edit</button>' +
                        '</div>';
                }).join('');
            }

            /* ── STICKER PACKS LIST ── */
            function renderPacksList(packs) {
                var container = document.getElementById('sp-packs-list');
                if (!container) return;
                if (packs.length === 0) {
                    container.innerHTML = '<p class="adm-emojis-empty">No sticker packs imported yet.</p>';
                    return;
                }
                container.innerHTML = packs.map(function(p) {
                    var tgLink = p.tg_name
                        ? '<a href="https://t.me/addstickers/' + esc(p.tg_name) + '" target="_blank" rel="noopener" class="adm-emojis-pack-tg-link">@' + esc(p.tg_name) + ' &nearr;</a>'
                        : '';
                    var count = (p.actual_count !== undefined && p.actual_count !== null) ? p.actual_count : (p.sticker_count || 0);
                    return '<div class="sp-pack-row adm-emojis-pack-row" data-pack-id="' + p.id + '">' +
                        '<div class="adm-emojis-pack-head">' +
                            '<div class="adm-emojis-pack-info">' +
                                '<div class="adm-emojis-pack-name-row">' +
                                    '<span class="sp-pack-name adm-emojis-pack-name">' + esc(p.name) + '</span>' +
                                    tgLink +
                                '</div>' +
                                '<div class="adm-emojis-pack-count">' + count + ' sticker(s)</div>' +
                            '</div>' +
                            '<div class="adm-emojis-pack-actions">' +
                                '<button data-on="click:emojiRenamePack" data-args="[' + p.id + ']" class="adm-emojis-pack-btn">Rename</button>' +
                                '<button data-on="click:emojiTogglePack" data-args="[' + p.id + ']" class="adm-emojis-pack-btn">View</button>' +
                                '<button data-on="click:emojiDeletePack" data-args="[' + p.id + ']" class="adm-emojis-pack-btn adm-emojis-pack-btn-delete">Delete</button>' +
                            '</div>' +
                        '</div>' +
                        '<div id="sp-emojis-' + p.id + '" class="adm-emojis-pack-grid"></div>' +
                        '</div>';
                }).join('');
            }

            function renderPackGrid(packId) {
                var container = document.getElementById('sp-emojis-' + packId);
                if (!container) return;
                // Preserve scroll position of the inner grid div
                var prevScroll = (container.querySelector('.emoji-grid') || {}).scrollTop || 0;
                var pack = (window.emojiAdmin.packs || []).find(function(p) { return p.id === packId; });
                var currentThumb = pack ? (pack.thumb_url || '') : '';
                var packEmojis = (window.emojiAdmin.emojis || []).filter(function(e) { return e.pack_id === packId; });
                if (packEmojis.length === 0) {
                    container.innerHTML = '<p class="adm-emojis-pack-empty">No stickers.</p>';
                } else {
                    container.innerHTML = '<div class="emoji-grid adm-emojis-pack-scroll">' +
                        packEmojis.map(function(e) {
                            var isCurrent = currentThumb && e.url === currentThumb;
                            var btnCls = isCurrent ? 'adm-emojis-thumb-btn adm-emojis-thumb-btn-current' : 'adm-emojis-thumb-btn';
                            var btnLbl = isCurrent ? '\u2713 Current Icon' : '\u2B50 Set as icon';
                            return '<div class="emoji-card">' +
                                '<button class="emoji-delete" data-on="click:emojiDelete" data-args="[' + e.id + ']" title="Delete">x</button>' +
                                stickerEl(e.url, e.name, ' loading="lazy"') +
                                '<span class="emoji-label">:' + esc(e.name) + ':</span>' +
                                '<button data-on="click:emojiSetPackThumb" data-args="[' + packId + ',' + e.id + ']"' +
                                ' title="Use as tab icon" class="' + btnCls + '">' + btnLbl + '</button>' +
                                '</div>';
                        }).join('') +
                        '</div>';
                    // Restore scroll position
                    var newGrid = container.querySelector('.emoji-grid');
                    if (newGrid && prevScroll) newGrid.scrollTop = prevScroll;
                }
                container.dataset.loaded = '1';
            }

            function togglePackEmojis(packId) {
                var container = document.getElementById('sp-emojis-' + packId);
                if (!container) return;
                if (getComputedStyle(container).display !== 'none') { container.style.display = 'none'; return; }
                container.style.display = 'block';
                renderPackGrid(packId);
            }

            /* ── STANDALONE EMOJI ACTIONS ── */
            function addEmoji(e) {
                if (e) e.preventDefault();
                var name = document.getElementById('emoji-name').value;
                var fileInput = document.getElementById('emoji-file');
                if (!name || !fileInput.files[0]) return alert('Fill Name and select a File');

                var btn = document.getElementById('add-emoji');
                var oldText = btn.textContent;
                btn.disabled = true;
                btn.textContent = i18n.uploading || 'Uploading...';

                var formData = new FormData();
                formData.append('name', name);
                formData.append('file', fileInput.files[0]);

                var headers = { 'X-Requested-With': 'XMLHttpRequest' };
                if (csrf) headers['X-CSRF-Token'] = csrf;

                fetch('/api/v2/admin/emojis', { method: 'POST', headers: headers, body: formData })
                    .then(function(r) { return r.json(); })
                    .then(function(data) {
                        if (data.success) {
                            document.getElementById('emoji-name').value = '';
                            document.getElementById('emoji-file').value = '';
                            loadAll();
                        } else {
                            alert('Failed: ' + (data.message || data.msg || 'Unknown error'));
                        }
                    })
                    .catch(function(err) { alert('Error: ' + err.message); })
                    .finally(function() { btn.disabled = false; btn.textContent = oldText; });
            }

            function deleteEmoji(id) {
                if (!confirm('Delete this emoji?')) return;
                fetch('/api/v2/admin/emojis/' + id + '/delete', { method: 'POST', headers: { 'X-CSRF-Token': csrf } })
                    .then(function(r) { return r.json(); })
                    .then(function(data) { if (data.success) loadAll(); else alert('Delete failed'); })
                    .catch(function(err) { console.error(err); });
            }

            function openEditModal(id) {
                var emoji = (window.emojiAdmin.emojis || []).find(function(e) { return e.id === id; });
                if (!emoji) return;
                document.getElementById('edit-emoji-id').value = emoji.id;
                document.getElementById('edit-emoji-name').value = emoji.name;
                document.getElementById('edit-emoji-file').value = '';
                var preview = document.getElementById('edit-emoji-preview');
                preview.src = emoji.url;
                preview.alt = ':' + emoji.name + ':';
                var modal = document.getElementById('edit-emoji-modal');
                if (modal) modal.style.display = 'flex';
            }

            function closeEditModal() {
                var modal = document.getElementById('edit-emoji-modal');
                if (modal) modal.style.display = 'none';
                document.getElementById('edit-emoji-file').value = '';
            }

            function saveEmoji() {
                var id   = document.getElementById('edit-emoji-id').value;
                var name = document.getElementById('edit-emoji-name').value.trim().toLowerCase();
                var fileInput = document.getElementById('edit-emoji-file');

                if (!name) return alert('Emoji name is required');
                if (!/^[a-z0-9_-]+$/.test(name)) return alert('Invalid name. Use lowercase a-z, 0-9, _, - only.');

                var btn = document.querySelector('#edit-emoji-modal .btn-save');
                var oldText = btn.textContent;
                btn.disabled = true;
                btn.textContent = 'Saving...';

                var formData = new FormData();
                formData.append('name', name);
                if (fileInput.files[0]) formData.append('file', fileInput.files[0]);

                var headers = { 'X-Requested-With': 'XMLHttpRequest' };
                if (csrf) headers['X-CSRF-Token'] = csrf;

                fetch('/api/v2/admin/emojis/' + id + '/edit', { method: 'POST', headers: headers, body: formData })
                    .then(function(r) { return r.json(); })
                    .then(function(data) {
                        if (data.success) { closeEditModal(); loadAll(); }
                        else alert('Save failed: ' + (data.message || data.msg || 'Unknown error'));
                    })
                    .catch(function(err) { alert('Save failed: ' + err.message); })
                    .finally(function() { btn.disabled = false; btn.textContent = oldText; });
            }

            /* ── STICKER PACK ACTIONS ── */
            function setImportStatusState(el, ok) {
                el.classList.toggle('adm-emojis-import-status-ok', ok);
                el.classList.toggle('adm-emojis-import-status-error', !ok);
            }

            function importPack() {
                var packUrl     = (document.getElementById('sp-pack-url').value || '').trim();
                var displayName = (document.getElementById('sp-display-name').value || '').trim();
                var statusEl    = document.getElementById('sp-import-status');
                var btn         = document.getElementById('sp-import-btn');

                if (!packUrl) return alert('Please enter a sticker pack URL or name.');

                btn.disabled = true;
                btn.textContent = 'Importing...';
                statusEl.style.display = 'none';

                fetch('/api/v2/admin/sticker-packs/import', {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                        'X-CSRF-Token': csrf,
                        'X-Requested-With': 'XMLHttpRequest'
                    },
                    body: JSON.stringify({ pack_name: packUrl, display_name: displayName })
                })
                    .then(function(r) { return r.json(); })
                    .then(function(data) {
                        statusEl.style.display = 'block';
                        if (data.success) {
                            setImportStatusState(statusEl, true);
                            var packName = (data.pack && data.pack.name) ? data.pack.name : packUrl;
                            statusEl.innerHTML = 'Imported "' + esc(packName) + '" &mdash; ' + (data.imported || 0) + ' sticker(s) added' + (data.failed ? ', ' + data.failed + ' failed' : '') + '.';
                            document.getElementById('sp-pack-url').value = '';
                            document.getElementById('sp-display-name').value = '';
                            loadAll();
                        } else {
                            setImportStatusState(statusEl, false);
                            statusEl.textContent = data.message || 'Import failed';
                        }
                    })
                    .catch(function(err) {
                        statusEl.style.display = 'block';
                        setImportStatusState(statusEl, false);
                        statusEl.textContent = 'Error: ' + err.message;
                    })
                    .finally(function() {
                        btn.disabled = false;
                        btn.innerHTML = '<i class="fa-solid fa-download"></i> Import Pack';
                    });
            }

            function deletePack(id) {
                if (!confirm('Delete this sticker pack AND all its stickers? This cannot be undone.')) return;
                fetch('/api/v2/admin/sticker-packs/' + id, {
                    method: 'DELETE',
                    headers: { 'X-CSRF-Token': csrf, 'X-Requested-With': 'XMLHttpRequest' }
                })
                    .then(function(r) { return r.json(); })
                    .then(function(data) { if (data.success) loadAll(); else alert('Delete failed: ' + (data.message || 'Unknown error')); })
                    .catch(function(err) { alert('Error: ' + err.message); });
            }

            var _renamePendingId = null;
            var _renamePendingCurrentName = null;

            function renamePack(id) {
                var pack = (window.emojiAdmin.packs || []).find(function(p) { return p.id === id; });
                _renamePendingId = id;
                _renamePendingCurrentName = pack ? pack.name : '';

                document.getElementById('rename-pack-input').value = _renamePendingCurrentName;
                document.getElementById('rename-pack-backfill').checked = false;
                document.getElementById('rename-pack-result').style.display = 'none';
                document.getElementById('rename-pack-result').textContent = '';
                document.getElementById('rename-pack-submit').disabled = false;
                document.getElementById('rename-pack-submit').textContent = 'Rename';
                document.getElementById('rename-pack-modal').style.display = 'flex';
                setTimeout(function() { document.getElementById('rename-pack-input').focus(); }, 50);
            }

            document.getElementById('rename-pack-cancel').addEventListener('click', function() {
                document.getElementById('rename-pack-modal').style.display = 'none';
            });

            document.getElementById('rename-pack-submit').addEventListener('click', function() {
                var newName = (document.getElementById('rename-pack-input').value || '').trim();
                var backfill = document.getElementById('rename-pack-backfill').checked;
                if (!newName) return;

                // Allow same name when backfill is requested
                if (newName === _renamePendingCurrentName && !backfill) {
                    document.getElementById('rename-pack-modal').style.display = 'none';
                    return;
                }

                var btn = document.getElementById('rename-pack-submit');
                btn.disabled = true;
                btn.textContent = 'Renaming...';

                fetch('/api/v2/admin/sticker-packs/' + _renamePendingId + '/rename', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf, 'X-Requested-With': 'XMLHttpRequest' },
                    body: JSON.stringify({ name: newName, backfill: backfill })
                })
                    .then(function(r) { return r.json(); })
                    .then(function(data) {
                        if (data.success) {
                            if (backfill) {
                                var res = document.getElementById('rename-pack-result');
                                res.textContent = (data.emojiRenamed || 0) + ' sticker(s) renamed, ' + (data.commentRowsUpdated || 0) + ' comment row(s) updated.';
                                res.style.display = 'block';
                                setTimeout(function() {
                                    document.getElementById('rename-pack-modal').style.display = 'none';
                                    loadAll();
                                }, 2000);
                            } else {
                                document.getElementById('rename-pack-modal').style.display = 'none';
                                loadAll();
                            }
                        } else {
                            var res = document.getElementById('rename-pack-result');
                            res.textContent = 'Rename failed: ' + (data.message || 'Unknown error');
                            res.classList.add('adm-emojis-rename-result-error');
                            res.style.display = 'block';
                            btn.disabled = false;
                            btn.textContent = 'Rename';
                        }
                    })
                    .catch(function(err) {
                        var res = document.getElementById('rename-pack-result');
                        res.textContent = 'Error: ' + err.message;
                        res.style.display = 'block';
                        btn.disabled = false;
                        btn.textContent = 'Rename';
                    });
            });

            // Close modal on overlay click
            document.getElementById('rename-pack-modal').addEventListener('click', function(e) {
                if (e.target === this) this.style.display = 'none';
            });

            function setPackThumb(packId, emojiId) {
                var emoji = (window.emojiAdmin.emojis || []).find(function(e) { return e.id === emojiId; });
                if (!emoji || !emoji.url) return alert('Could not find emoji URL.');
                var thumbUrl = emoji.url;
                fetch('/api/v2/admin/sticker-packs/' + packId + '/thumb', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf, 'X-Requested-With': 'XMLHttpRequest' },
                    body: JSON.stringify({ thumb_url: thumbUrl })
                })
                    .then(function(r) { return r.json(); })
                    .then(function(data) {
                        if (data.success) {
                            // Use the URL the server actually stored (may be a generated WebP for .webm stickers)
                            var pack = (window.emojiAdmin.packs || []).find(function(p) { return p.id === packId; });
                            if (pack) pack.thumb_url = data.thumb_url || thumbUrl;
                            // Re-render only this pack's grid — grid stays open
                            renderPackGrid(packId);
                        } else {
                            alert('Failed to set icon: ' + (data.message || 'Unknown error'));
                        }
                    })
                    .catch(function(err) { alert('Error: ' + err.message); });
            }

            // Global scope for onclick handlers
            window.emojiAdmin = {
                deleteEmoji: deleteEmoji,
                openEditModal: openEditModal,
                closeEditModal: closeEditModal,
                saveEmoji: saveEmoji,
                importPack: importPack,
                deletePack: deletePack,
                renamePack: renamePack,
                setPackThumb: setPackThumb,
                togglePackEmojis: togglePackEmojis,
                emojis: [],
                packs: []
            };

            F0ckActions.define({
                emojiDelete: function(el, ev, id) { deleteEmoji(id); },
                emojiOpenEditModal: function(el, ev, id) { openEditModal(id); },
                emojiCloseEditModal: function() { closeEditModal(); },
                emojiSave: function() { saveEmoji(); },
                emojiRenamePack: function(el, ev, id) { renamePack(id); },
                emojiTogglePack: function(el, ev, id) { togglePackEmojis(id); },
                emojiDeletePack: function(el, ev, id) { deletePack(id); },
                emojiSetPackThumb: function(el, ev, packId, emojiId) { setPackThumb(packId, emojiId); }
            });

            document.getElementById('add-emoji').addEventListener('click', addEmoji);
            document.getElementById('sp-import-btn').addEventListener('click', importPack);

            // Reload when SSE emits emojis_updated
            document.addEventListener('f0ck:emojis_updated', loadAll);

            loadAll();
        })();
