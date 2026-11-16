// f0ckgle sign-in page: passkey login. The WebAuthn round trip is f0ckPasskeyLogin (f0ck-core.js).
(() => {
  const btn = document.querySelector('.fg-passkey');
  const err = document.querySelector('.fg-passkey-error');
  if (!btn || !err) return;
  btn.addEventListener('click', async () => {
    const label = btn.textContent;
    err.hidden = true;
    btn.disabled = true;
    btn.textContent = 'Waiting for your passkey...';
    try {
      await window.f0ckPasskeyLogin();
      location.href = '/';
    } catch (e) {
      err.textContent = (e && e.message) || 'Passkey sign-in failed';
      err.hidden = false;
      btn.disabled = false;
      btn.textContent = label;
    }
  });
})();
