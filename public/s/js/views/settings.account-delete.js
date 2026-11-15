// Settings: "Delete my account". The form also works as a plain post; here it is sent with fetch so
// a passkey-only account can confirm with its passkey (f0ckWithReauth, f0ck-core.js).
(function () {
  var form = document.getElementById('account-delete-form');
  if (!form) return;
  var status = document.getElementById('account-delete-status');
  var btn = form.querySelector('button[type="submit"]');
  function setStatus(msg, ok) {
    status.textContent = msg;
    status.classList.toggle('success', !!ok);
    status.classList.toggle('error', !ok);
  }
  form.addEventListener('submit', function (e) {
    e.preventDefault();
    var password = form.elements.current_password;
    var confirmBox = form.elements.confirm;
    if (!form.reportValidity()) return;
    btn.disabled = true;
    var send = function () {
      return fetch(form.action, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'X-Requested-With': 'XMLHttpRequest',
          'X-CSRF-Token': (window.f0ckSession && window.f0ckSession.csrf_token) || form.elements.csrf_token.value
        },
        body: JSON.stringify({ current_password: password ? password.value : '', confirm: confirmBox.checked })
      });
    };
    var request = window.f0ckWithReauth ? window.f0ckWithReauth(send) : send().then(function (r) { return r.json(); });
    request.then(function (d) {
      if (d && d.success) {
        setStatus(d.msg || 'Your account was deleted.', true);
        // Logged out: a full load of the front page
        window.location.href = d.redirect || '/';
        return;
      }
      btn.disabled = false;
      setStatus((d && d.msg) || 'Failed to delete the account.', false);
    }).catch(function (err) {
      btn.disabled = false;
      setStatus((err && err.name === 'NotAllowedError') ? 'Passkey confirmation was cancelled.' : 'Network error.', false);
    });
  });
})();
