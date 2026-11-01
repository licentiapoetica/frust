        (function() {
          var statusEl = document.getElementById('invite-req-status');
          var formWrapper = document.getElementById('invite-req-form-wrapper');
          var tokenDisplay = document.getElementById('invite-req-token-display');
          var errorEl = document.getElementById('invite-req-error');
          var submitBtn = document.getElementById('invite-req-submit');
          var reasonEl = document.getElementById('invite-req-reason');

          function showError(msg) {
            errorEl.textContent = msg;
            errorEl.style.display = msg ? 'block' : 'none';
          }

          function renderStatus(data) {
            if (!data || !data.status) {
              statusEl.style.display = 'none';
              formWrapper.style.display = '';
              tokenDisplay.style.display = 'none';
              return;
            }

            statusEl.style.display = 'block';

            if (data.status === 'pending') {
              formWrapper.style.display = 'none';
              statusEl.innerHTML =
                '<div class="settings-req-box settings-req-pending">' +
                  '<i class="fa-solid fa-clock settings-req-icon"></i>' +
                  '<div>' +
                    '<strong class="settings-req-title">Request pending</strong>' +
                    '<div class="settings-req-sub">Submitted ' + new Date(data.created_at).toLocaleString() + ' — waiting for admin review.</div>' +
                  '</div>' +
                '</div>';
            } else if (data.status === 'approved') {
              formWrapper.style.display = 'none';
              statusEl.innerHTML =
                '<div class="settings-req-box settings-req-approved">' +
                  '<i class="fa-solid fa-circle-check settings-req-icon"></i>' +
                  '<div>' +
                    '<strong class="settings-req-title">Request approved!</strong>' +
                    '<div class="settings-req-sub">Your invite token is ready. Use it to register your account.</div>' +
                  '</div>' +
                '</div>';
              if (data.token) {
                tokenDisplay.style.display = 'block';
                tokenDisplay.innerHTML =
                  '<div class="settings-copy-row">' +
                    '<code class="settings-req-token-code">' + data.token + '</code>' +
                    '<button type="button" class="button button-primary" id="invite-register-now-btn">' +
                      '<i class="fa-solid fa-user-plus"></i> Register Now' +
                    '</button>' +
                  '</div>';
                var regBtn = document.getElementById('invite-register-now-btn');
                if (regBtn) regBtn.addEventListener('click', function() {
                  var modal = document.getElementById('register-modal');
                  if (modal) {
                    var tokenInput = modal.querySelector('input[name="token"]');
                    if (tokenInput) tokenInput.value = data.token;
                    if (typeof openModal === 'function') {
                      openModal(modal);
                    } else {
                      modal.style.display = 'flex';
                    }
                  }
                });
              }
            } else if (data.status === 'denied' || data.status === 'revoked') {
              formWrapper.style.display = '';
              const isRevoked = data.status === 'revoked';
              statusEl.innerHTML =
                '<div class="settings-req-box settings-req-denied">' +
                  '<i class="fa-solid fa-circle-xmark settings-req-icon"></i>' +
                  '<div>' +
                    '<strong class="settings-req-title">' + (isRevoked ? 'Token revoked' : 'Request denied') + '</strong>' +
                    '<div class="settings-req-sub">' + (isRevoked ? 'Your invite token was revoked by an admin. You can submit a new request.' : 'Your previous request was denied. You can submit a new one.') + '</div>' +
                  '</div>' +
                '</div>';
            }
          }

          // Check status on load
          fetch('/api/v2/invite-request/status')
            .then(function(r) { return r.json(); })
            .then(function(d) {
              if (d.success) renderStatus(d);
            })
            .catch(function() {});

          // Submit request
          if (submitBtn) submitBtn.addEventListener('click', function() {
            showError('');
            submitBtn.disabled = true;
            submitBtn.style.opacity = '0.6';
            var reason = reasonEl ? reasonEl.value : '';

            fetch('/api/v2/invite-request', {
              method: 'POST',
              headers: {
                'Content-Type': 'application/json',
                'X-CSRF-Token': window.f0ckSession ? window.f0ckSession.csrf_token : ''
              },
              body: JSON.stringify({ reason: reason })
            })
            .then(function(r) { return r.json(); })
            .then(function(d) {
              submitBtn.disabled = false;
              submitBtn.style.opacity = '1';
              if (d.success) {
                renderStatus({ status: 'pending', created_at: new Date().toISOString() });
              } else {
                showError(d.msg || 'Failed to submit request');
              }
            })
            .catch(function() {
              submitBtn.disabled = false;
              submitBtn.style.opacity = '1';
              showError('Network error. Please try again.');
            });
          });

          // Poll for status changes when there's a pending request
          var _invitePollTimer = null;
          function startInvitePoll() {
            if (_invitePollTimer) return;
            _invitePollTimer = setInterval(function() {
              fetch('/api/v2/invite-request/status')
                .then(function(r) { return r.json(); })
                .then(function(d) {
                  if (d.success) {
                    renderStatus(d);
                    // Stop polling once resolved
                    if (d.status !== 'pending' && _invitePollTimer) {
                      clearInterval(_invitePollTimer);
                      _invitePollTimer = null;
                    }
                  }
                })
                .catch(function() {});
            }, 15000); // Check every 15 seconds
          }

          // Start polling if initial status is pending
          fetch('/api/v2/invite-request/status')
            .then(function(r) { return r.json(); })
            .then(function(d) {
              if (d.success) {
                renderStatus(d);
                if (d.status === 'pending') startInvitePoll();
              }
            })
            .catch(function() {});
        })();
