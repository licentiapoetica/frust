      (function() {
        async function loadPasskeys() {
          const container = document.getElementById('settings-passkey-list');
          if (!container) return;
          try {
            const res = await fetch('/api/v2/settings/passkeys');
            const data = await res.json();
            const passkeys = data.passkeys || [];
            if (passkeys.length === 0) {
              container.innerHTML = '<p class="settings-muted settings-small-note">No passkeys registered yet.</p>';
              return;
            }
            container.classList.add('pk-list');
            container.innerHTML = passkeys.map(function(pk) {
              return window.f0ckPasskeyCard(pk, {
                actions: '<button class="button button-danger" title="Remove this passkey" data-on="click:settingsPasskeyDelete" data-args="' + escHtml(JSON.stringify([pk.credential_id])) + '"><i class="fa-solid fa-trash"></i></button>'
              });
            }).join('');
          } catch (e) {
            container.innerHTML = '<p class="settings-pk-error settings-small-note">Failed to load passkeys.</p>';
          }
        }

        function escHtml(s) { return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }

        window.settingsPasskeyAdd = async function() {
          var status = document.getElementById('settings-passkey-status');
          var btn = document.getElementById('settings-passkey-add-btn');
          if (!window.f0ckPasskeyManager) { alert('Passkey manager not loaded.'); return; }
          if (!window.PublicKeyCredential) { alert('Passkeys are not supported in this browser.'); return; }
          var name = prompt('Name this passkey (e.g. "Bitwarden", "iPhone"):', 'Passkey') || 'Passkey';
          if (btn) btn.disabled = true;
          if (status) status.style.display = 'none';
          try {
            await window.f0ckPasskeyManager.addPasskey(name);
            if (status) { status.style.display = 'block'; status.style.color = '#98c379'; status.textContent = 'Passkey added!'; }
            await loadPasskeys();
          } catch (err) {
            console.warn('[SETTINGS_PASSKEY] Add passkey error:', err);
            if (status) {
              status.style.display = 'block';
              status.style.color = '#e06c75';
              if (err && err.name === 'NotAllowedError') {
                status.textContent = 'Passkey creation was cancelled or blocked. If Bitwarden or another password manager is locked, please unlock it.';
              } else {
                status.textContent = 'Error: ' + ((err && err.message) || 'Unknown error');
              }
            }
          } finally {
            if (btn) btn.disabled = false;
          }
        };

        window.settingsPasskeyDelete = async function(credentialId) {
          if (!confirm('Remove this passkey? You will no longer be able to use it to sign in.')) return;
          var status = document.getElementById('settings-passkey-status');
          try {
            await window.f0ckPasskeyManager.deletePasskey(credentialId);
            if (status) { status.style.display = 'block'; status.style.color = '#98c379'; status.textContent = 'Passkey removed.'; }
            await loadPasskeys();
          } catch (err) {
            if (status) { status.style.display = 'block'; status.style.color = '#e06c75'; status.textContent = 'Error: ' + err.message; }
          }
        };

        F0ckActions.define({
          settingsPasskeyAdd: () => { window.settingsPasskeyAdd(); },
          settingsPasskeyDelete: (el, e, credentialId) => { window.settingsPasskeyDelete(credentialId); },
        });

        loadPasskeys();
      })();
