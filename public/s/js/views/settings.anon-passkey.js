        F0ckActions.define({
          settingsOpenAnonPasskey: () => {
            if (window.f0ckAnonPasskey) { window.f0ckAnonPasskey.openModal(); }
            else { const m = document.getElementById('anon-passkey-modal'); if (m) m.style.display = 'flex'; }
          },
        });
        (function() {
          var list = document.getElementById('settings-anon-pk-list');
          var count = document.getElementById('settings-anon-pk-count');
          function load() {
            fetch('/api/v2/anon/passkeys', { credentials: 'same-origin' })
              .then(function(r) { return r.json(); })
              .then(function(d) {
                if (!list.isConnected) return;
                if (!d.success) { list.innerHTML = '<p class="settings-pk-error settings-small-note">Failed to load passkeys.</p>'; return; }
                var pks = d.passkeys || [];
                count.textContent = pks.length + ' / ' + d.max + ' passkeys';
                list.innerHTML = pks.length
                  ? pks.map(function(pk) { return window.f0ckPasskeyCard(pk); }).join('')
                  : '<p class="settings-muted settings-small-note">No passkeys found.</p>';
              })
              .catch(function() { list.innerHTML = '<p class="settings-pk-error settings-small-note">Failed to load passkeys.</p>'; });
          }
          load();
          // Passkeys are added in the Manage modal: refresh when coming back to the page
          window.addEventListener('focus', function onFocus() {
            if (!list.isConnected) { window.removeEventListener('focus', onFocus); return; }
            load();
          });
        })();
