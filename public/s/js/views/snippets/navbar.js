// Passkey login for registered users — fires only on explicit button click
async function loginWithPasskey() {
  const btn = document.getElementById('modal-login-passkey-btn');
  const errEl = document.getElementById('modal-passkey-error');
  if (errEl) errEl.style.display = 'none';
  if (!window.PublicKeyCredential) {
    if (errEl) { errEl.style.display = ''; errEl.textContent = 'Passkeys are not supported in this browser.'; }
    return;
  }
  if (btn) { btn.disabled = true; btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Waiting for passkey...'; }

  function b64urlToArr(s) { return Uint8Array.from(atob(s.split('-').join('+').split('_').join('/')), c=>c.charCodeAt(0)); }
  function arrToB64url(buf) { return btoa(String.fromCharCode(...new Uint8Array(buf))).split('+').join('-').split('/').join('_').replace(/=+$/,''); }

  try {
    const beginRes = await fetch('/api/v2/settings/passkeys/login/begin', { method: 'POST', headers: {'Content-Type':'application/json'}, body: '{}' });
    const beginData = await beginRes.json();
    if (!beginData.success) throw new Error(beginData.msg || 'Server error');

    const opts = beginData.options;
    const cred = await navigator.credentials.get({ publicKey: {
      challenge: b64urlToArr(opts.challenge),
      rpId: opts.rpId,
      allowCredentials: (opts.allowCredentials || []).map(c => ({ type: c.type, id: b64urlToArr(c.id) })),
      userVerification: opts.userVerification || 'preferred',
      timeout: opts.timeout || 60000
    }});

    const finishRes = await fetch('/api/v2/settings/passkeys/login/finish', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        challenge: opts.challenge,
        credentialId: arrToB64url(cred.rawId),
        clientDataJSON: arrToB64url(cred.response.clientDataJSON),
        authenticatorData: arrToB64url(cred.response.authenticatorData),
        signature: arrToB64url(cred.response.signature),
        userHandle: cred.response.userHandle ? arrToB64url(cred.response.userHandle) : null
      })
    });
    const finishData = await finishRes.json();
    if (!finishData.success) throw new Error(finishData.msg || 'Authentication failed');

    window.location.reload();
  } catch (e) {
    console.warn('[PASSKEY] Login failed:', e);
    if (errEl) {
      errEl.style.display = '';
      if (e && e.name === 'NotAllowedError') {
        errEl.textContent = 'Passkey request was cancelled or blocked. If Bitwarden or another password manager is locked, please unlock it or check extension permissions.';
      } else {
        errEl.textContent = (e && e.message) || 'Passkey login failed.';
      }
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
