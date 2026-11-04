// f0ckv2 login page: passkey login. The WebAuthn round trip is f0ckPasskeyLogin (f0ck-core.js).
(() => {
  const btn = document.querySelector('.v2-passkey-btn');
  const err = document.querySelector('.v2-passkey-error');
  if (!btn || !err) return;
  btn.addEventListener('click', async () => {
    const label = btn.textContent;
    err.hidden = true;
    btn.disabled = true;
    btn.textContent = 'waiting for passkey...';
    try {
      await window.f0ckPasskeyLogin();
      location.href = '/';
    } catch (e) {
      err.textContent = (e && e.message) || 'passkey login failed';
      err.hidden = false;
      btn.disabled = false;
      btn.textContent = label;
    }
  });
})();
