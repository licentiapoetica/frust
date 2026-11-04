// Passkey login for registered users — fires only on explicit button click.
// The WebAuthn round trip is f0ckPasskeyLogin (f0ck-core.js, shared with the other frontends).
async function loginWithPasskey() {
  const btn = document.getElementById('modal-login-passkey-btn');
  const errEl = document.getElementById('modal-passkey-error');
  if (errEl) errEl.style.display = 'none';
  if (btn) { btn.disabled = true; btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Waiting for passkey...'; }
  try {
    await window.f0ckPasskeyLogin();
    window.location.reload();
  } catch (e) {
    console.warn('[PASSKEY] Login failed:', e);
    if (errEl) {
      errEl.style.display = '';
      errEl.textContent = (e && e.message) || 'Passkey login failed.';
    }
    if (btn) { btn.disabled = false; btn.innerHTML = '<i class="fa-solid fa-fingerprint"></i> Use my passkey'; }
  }
}

// CSP-safe markup actions for the navbar and login/register modals (see /s/js/csp-actions.js).
// switchLoginTab / toggleLoginPassword are declared at the end of this file; they are looked up at click time.
F0ckActions.define({
  loginWithPasskey: () => { loginWithPasskey(); },
  switchLoginTab: (el, e, tab) => { switchLoginTab(tab); },
  toggleLoginPassword: (el) => { toggleLoginPassword(el); },
  toggleNavbar: (el) => {
    document.getElementById('navbarContent').classList.toggle('show');
    el.classList.toggle('is-open');
  },
  openAnonPasskey: () => {
    if (window.f0ckAnonPasskey) { window.f0ckAnonPasskey.openModal(); }
    else { const m = document.getElementById('anon-passkey-modal'); if (m) m.style.display = 'flex'; }
  },
  openAnonSetup: () => {
    if (window.f0ckAnonPasskey) window.f0ckAnonPasskey.openSetupModal();
    else { const m = document.getElementById('anon-setup-modal'); if (m) m.style.display = 'flex'; }
  },
});

// Login modal: tab switch and password visibility toggle
function switchLoginTab(tab) {
  var isCreds = tab === 'creds';
  document.getElementById('login-panel-creds').style.display = isCreds ? '' : 'none';
  document.getElementById('login-panel-passkey').style.display = isCreds ? 'none' : '';
  [['login-tab-creds', isCreds], ['login-tab-passkey', !isCreds]].forEach(function (t) {
    var el = document.getElementById(t[0]);
    if (!el) return;
    el.classList.toggle('is-active', t[1]);
    el.setAttribute('aria-selected', t[1] ? 'true' : 'false');
  });
}
function toggleLoginPassword(btn) {
  var input = btn.parentNode.querySelector('input');
  if (!input) return;
  var show = input.type === 'password';
  input.type = show ? 'text' : 'password';
  btn.innerHTML = show ? '<i class="fa-solid fa-eye-slash"></i>' : '<i class="fa-solid fa-eye"></i>';
  btn.setAttribute('aria-label', show ? 'Hide password' : 'Show password');
  btn.title = show ? 'Hide password' : 'Show password';
  input.focus();
}
