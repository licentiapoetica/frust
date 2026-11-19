// Forced password change (views/password-change.html). Without JavaScript the form posts to
// /password-change and goes on to the homepage. Here it goes to the API, then asks whether to set
// up a passkey too (window.f0ckPasskeyManager, passkey.js; the new password confirms it).
(() => {
  const form = document.getElementById('force-password-form');
  if (!form) return;
  const status = document.getElementById('force-password-status');
  const btn = form.querySelector('button[type="submit"]');
  const label = btn.textContent;

  const show = (el, msg, ok) => {
    el.textContent = msg;
    el.classList.remove('force-pw-hidden', 'force-pw-status-error', 'force-pw-status-ok');
    el.classList.add(ok ? 'force-pw-status-ok' : 'force-pw-status-error');
  };
  const hide = (el) => el && el.classList.add('force-pw-hidden');
  const reveal = (el) => el && el.classList.remove('force-pw-hidden');

  const canPasskey = () => !!(window.PublicKeyCredential && navigator.credentials && navigator.credentials.create && window.f0ckPasskeyManager);

  // ── step 2: offer a passkey ──
  const offerPasskey = (password) => {
    const box = document.getElementById('force-pw-passkey');
    const add = document.getElementById('force-pw-passkey-add');
    const done = document.getElementById('force-pw-passkey-done');
    const nameInput = document.getElementById('force-pw-passkey-name');
    const pkStatus = document.getElementById('force-pw-passkey-status');
    hide(document.getElementById('force-pw-step-password'));
    reveal(box);
    add.focus();

    add.addEventListener('click', async () => {
      add.disabled = true;
      hide(pkStatus);
      try {
        await window.f0ckPasskeyManager.addPasskey(nameInput.value.trim() || 'Passkey', password);
        password = '';
        show(pkStatus, box.dataset.msgAdded || 'Passkey added.', true);
        hide(add);
        hide(document.getElementById('force-pw-passkey-name-group'));
        hide(document.getElementById('force-pw-passkey-skip-row'));
        reveal(done);
        done.focus();
      } catch (err) {
        const cancelled = err && err.name === 'NotAllowedError';
        show(pkStatus, cancelled ? box.dataset.msgCancelled : ((err && err.message) || box.dataset.msgFailed), false);
        add.disabled = false;
      }
    });
  };

  // ── step 1: the new password ──
  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const new_password = form.elements.new_password.value;
    const new_password_confirm = form.elements.new_password_confirm.value;
    if (new_password !== new_password_confirm) return show(status, 'New passwords do not match', false);
    if (new_password.length < 20) return show(status, 'New password must be at least 20 characters long', false);

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
        form.reset();
        if (canPasskey()) offerPasskey(new_password);
        else window.location.href = '/';
        return;
      }
      show(status, json.msg || 'Error updating password.', false);
    } catch (err) {
      show(status, 'Network error.', false);
    }
    btn.disabled = false;
    btn.textContent = label;
  });
})();
