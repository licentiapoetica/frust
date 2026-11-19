// Forced password change (views/password-change.html). Without JavaScript the form posts to
// /password-change; here it goes to the API and, once changed, on to the homepage.
(() => {
  const form = document.getElementById('force-password-form');
  if (!form) return;
  const status = document.getElementById('force-password-status');
  const btn = form.querySelector('button[type="submit"]');
  const label = btn.textContent;

  const show = (msg, ok) => {
    status.textContent = msg;
    status.classList.remove('force-pw-hidden', 'force-pw-status-error', 'force-pw-status-ok');
    status.classList.add(ok ? 'force-pw-status-ok' : 'force-pw-status-error');
  };

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const new_password = form.elements.new_password.value;
    const new_password_confirm = form.elements.new_password_confirm.value;
    if (new_password !== new_password_confirm) return show('New passwords do not match', false);
    if (new_password.length < 20) return show('New password must be at least 20 characters long', false);

    btn.disabled = true;
    btn.textContent = '…';
    try {
      const res = await fetch('/api/v2/settings/password', {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
          'Accept': 'application/json',
          'X-CSRF-Token': form.elements.csrf_token.value
        },
        body: JSON.stringify({ new_password, new_password_confirm })
      });
      const json = await res.json().catch(() => ({}));
      if (json.success) {
        show(json.msg || 'Password updated successfully', true);
        window.location.href = '/';
        return;
      }
      show(json.msg || 'Error updating password.', false);
    } catch (err) {
      show('Network error.', false);
    }
    btn.disabled = false;
    btn.textContent = label;
  });
})();
