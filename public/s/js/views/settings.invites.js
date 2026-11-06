      (function() {
        const ds = document.getElementById('invite-i18n')?.dataset || {};
        const T = (key, vars) => {
          let str = ds[key] || key;
          if (vars) Object.entries(vars).forEach(([k, v]) => { str = str.replaceAll('{' + k + '}', String(v)); });
          return str;
        };

        const statusEl = document.getElementById('invite-action-status');
        const genBtn   = document.getElementById('btn-gen-invite');
        const tokenList = document.getElementById('invite-token-list');

        const setStatus = (msg, ok) => {
          statusEl.textContent = msg;
          statusEl.style.color = ok ? '#51cf66' : '#ff6b6b';
        };

        const formatDate = ts => {
          if (!ts) return '—';
          const d = (typeof ts === 'number' || /^\d+$/.test(String(ts)))
            ? new Date(parseInt(ts) * 1000)
            : new Date(ts);
          return d.toLocaleDateString();
        };

        const loadInvites = async () => {
          try {
            const res = await fetch('/api/v2/settings/invites');
            const data = await res.json();
            if (!data.success) {
              tokenList.innerHTML = '<span class="settings-invite-fail">' + (data.msg || 'Error') + '</span>';
              return;
            }

            if (data.is_admin) {
              const desc = document.getElementById('invite-section-desc');
              if (desc) desc.textContent = T('adminDesc');
              const grid = document.getElementById('invite-criteria-grid');
              const statusLine = document.getElementById('invite-status-line');
              if (grid) grid.style.display = 'none';
              if (statusLine) statusLine.style.display = 'none';
            } else {
              const criteriaMap = {
                'ic-uploads':  data.criteria.uploads,
                'ic-age':      data.criteria.age_days,
                'ic-comments': data.criteria.comments,
                'ic-tags':     data.criteria.tags,
              };
              Object.entries(criteriaMap).forEach(([id, c]) => {
                const el = document.getElementById(id);
                if (!el) return;
                el.classList.toggle('met',   c.met);
                el.classList.toggle('unmet', !c.met);
                el.querySelector('.ic-icon').textContent = c.met ? '✓' : '✗';
                const unit = id === 'ic-age' ? ' ' + T('days') : '';
                el.querySelector('.ic-values').textContent = c.current + ' / ' + c.required + unit;
              });
            }

            const badge = document.getElementById('invite-eligible-badge');
            badge.textContent = data.eligible ? T('eligible') : T('notEligible');
            badge.style.color = data.eligible ? '#51cf66' : '#ff6b6b';

            document.getElementById('invite-slots-info').textContent =
              T('slotsUsed', { used: data.slots_consumed, total: data.slots_total });

            genBtn.disabled = !data.eligible || (!data.is_admin && data.slots_available <= 0);

            if (!data.tokens || data.tokens.length === 0) {
              tokenList.innerHTML = '<span class="text-muted settings-small-note">' + T('noInvites') + '</span>';
            } else {
              tokenList.innerHTML = data.tokens.map(tok => {
                const isUsed = tok.is_used;
                const usedAt = tok.used_at ? formatDate(tok.used_at) : null;
                let refreshNote = '';
                if (isUsed && tok.used_at) {
                  const usedAtMs = (typeof tok.used_at === 'number' || /^\d+$/.test(String(tok.used_at)))
                    ? parseInt(tok.used_at) * 1000
                    : new Date(tok.used_at).getTime();
                  const refreshDate = new Date(usedAtMs + 30 * 24 * 60 * 60 * 1000);
                  refreshNote = '<span class="invite-refresh-note"> — ' +
                    (refreshDate > new Date()
                      ? T('slotRefreshesOn', { date: refreshDate.toLocaleDateString() })
                      : T('slotRefreshed')) +
                    '</span>';
                }
                let expiryNote = '';
                if (!isUsed && tok.created_at && data.expire_days) {
                  const expiresAt = new Date(parseInt(tok.created_at) * 1000 + data.expire_days * 24 * 60 * 60 * 1000);
                  expiryNote = '<span class="invite-refresh-note"> — ' + T('expiresOn', { date: expiresAt.toLocaleDateString() }) + '</span>';
                }
                const userLink = tok.used_by_name
                  ? '<a href="/user/' + encodeURIComponent(tok.used_by_name.toLowerCase()) + '" class="settings-invite-user">' + tok.used_by_name + '</a>'
                  : '?';
                const statusHtml = isUsed
                  ? '<span class="invite-token-status-used">' + T('statusUsedBy', { user: userLink }) + ' (' + (usedAt || '?') + ')</span>' + refreshNote
                  : '<span class="invite-token-status-unused">' + T('statusUnused') + '</span>' + expiryNote;
                const deleteBtn = !isUsed
                  ? '<button class="button button-danger settings-invite-delete" data-invite-delete="' + tok.id + '">' + T('deleteBtn') + '</button>'
                  : '';
                return '<div class="invite-token-row">' +
                  '<span class="invite-token-code">' + tok.token + '</span>' +
                  statusHtml + deleteBtn +
                  '</div>';
              }).join('');

              tokenList.querySelectorAll('[data-invite-delete]').forEach(btn => {
                btn.addEventListener('click', async () => {
                  if (!confirm(T('deleteConfirm'))) return;
                  const res = await fetch('/api/v2/settings/invites/delete', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                    body: JSON.stringify({ id: +btn.dataset.inviteDelete })
                  });
                  const d = await res.json();
                  if (d.success) { loadInvites(); } else { setStatus(d.msg || 'Error', false); }
                });
              });
            }
          } catch (e) {
            console.error('[Invites] load error:', e);
            tokenList.innerHTML = '<span class="settings-invite-fail">Failed to load</span>';
          }
        };

        genBtn.addEventListener('click', async () => {
          const origLabel = genBtn.textContent;
          genBtn.disabled = true;
          genBtn.textContent = T('generating');
          statusEl.textContent = '';
          try {
            const res = await fetch('/api/v2/settings/invites/create', {
              method: 'POST',
              headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token }
            });
            const data = await res.json();
            if (data.success) {
              setStatus('✓', true);
              loadInvites();
            } else {
              setStatus(data.msg || 'Error', false);
              genBtn.disabled = false;
            }
          } catch (e) {
            setStatus('Network error', false);
            genBtn.disabled = false;
          } finally {
            genBtn.textContent = origLabel;
          }
        });

        loadInvites();
      })();
