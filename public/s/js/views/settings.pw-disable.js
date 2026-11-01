            (function() {
              var openBtn = document.getElementById('btn-pw-disable-open');
              var form = document.getElementById('pw-disable-form');
              var input = document.getElementById('pw-disable-current');
              var status = document.getElementById('pw-disable-status');
              if (!openBtn || !form) return;
              function setStatus(msg, ok) { status.textContent = msg; status.style.color = ok ? '#51cf66' : '#ff6b6b'; }
              openBtn.addEventListener('click', function() {
                form.style.display = 'flex'; openBtn.style.display = 'none'; setStatus('', true); input.focus();
              });
              document.getElementById('btn-pw-disable-cancel').addEventListener('click', function() {
                form.style.display = 'none'; openBtn.style.display = ''; input.value = ''; setStatus('', true);
              });
              form.addEventListener('submit', function(e) {
                e.preventDefault();
                var btn = form.querySelector('button[type="submit"]');
                btn.disabled = true;
                fetch('/api/v2/settings/password/disable', {
                  method: 'POST',
                  headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': (window.f0ckSession && window.f0ckSession.csrf_token) || '' },
                  body: JSON.stringify({ current_password: input.value })
                })
                  .then(function(r) { return r.json(); })
                  .then(function(d) {
                    btn.disabled = false;
                    setStatus(d.msg || (d.success ? 'Done' : 'Failed'), !!d.success);
                    if (d.success) setTimeout(function() { location.reload(); }, 900);
                  })
                  .catch(function() { btn.disabled = false; setStatus('Network error', false); });
              });
            })();
