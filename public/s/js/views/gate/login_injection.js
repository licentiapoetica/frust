function openLoginGate(view) {
  var m = document.getElementById('gate-modal');
  if (!m) return;
  m.style.display = 'flex';
  gateShowView(view || 'login');
}
// Markup action for the hidden trigger in gate/cloudflare.html (<span data-on="click:openLoginGate" data-arg="login">)
F0ckActions.define({ openLoginGate: function(el, e, view) { openLoginGate(view); } });
function gateShowView(view) {
  document.getElementById('gate-login-view').style.display    = view === 'login'    ? 'block' : 'none';
  document.getElementById('gate-register-view').style.display = view === 'register' ? 'block' : 'none';
}
function gateSetError(id, msg) {
  var el = document.getElementById(id);
  if (!el) return;
  el.textContent = msg;
  el.style.display = msg ? 'block' : 'none';
}
function gateSetBtn(id, loading) {
  var btn = document.getElementById(id);
  if (!btn) return;
  btn.disabled = loading;
  btn.style.opacity = loading ? '0.65' : '1';
  btn.style.cursor = loading ? 'default' : 'pointer';
}

// Cap captcha: tokens are single-use, so reset after every failed attempt
function gateResetCaptcha() {
  if (window.f0ckCapReset) window.f0ckCapReset(document.getElementById('gate-register-form'));
}

document.addEventListener('DOMContentLoaded', function() {
  var modal = document.getElementById('gate-modal');
  var close = document.getElementById('gate-modal-close');
  if (close) close.onclick = function() { modal.style.display = 'none'; };
  if (modal) modal.addEventListener('click', function(e) { if (e.target === this) this.style.display = 'none'; });

  document.getElementById('gate-to-register').onclick = function(e) { e.preventDefault(); gateShowView('register'); };
  document.getElementById('gate-to-login').onclick    = function(e) { e.preventDefault(); gateShowView('login'); };

  var hc = document.getElementById('hot-corner');
  if (hc) hc.onclick = function() { openLoginGate('login'); };

  // Login form
  document.getElementById('gate-login-form').onsubmit = function(e) {
    e.preventDefault();
    gateSetError('gate-login-error', '');
    gateSetBtn('gate-login-btn', true);
    var fd = new FormData(this);
    var body = new URLSearchParams(fd).toString();
    fetch('/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'Accept': 'application/json' },
      body: body
    })
    .then(function(r) { return r.json(); })
    .then(function(d) {
      if (d.success === false) {
        gateSetError('gate-login-error', d.msg || 'Login failed.');
        gateSetBtn('gate-login-btn', false);
      } else {
        window.location.reload();
      }
    })
    .catch(function() { window.location.reload(); });
  };

  // Register form
  document.getElementById('gate-register-form').onsubmit = function(e) {
    e.preventDefault();
    gateSetError('gate-register-error', '');
    gateSetError('gate-register-ok', '');
    gateSetBtn('gate-register-btn', true);
    // The widget's hidden cap-token input is part of the form
    var fd = new FormData(this);
    if (this.querySelector('cap-widget') && !fd.get('cap-token')) {
      gateSetError('gate-register-error', 'Please complete the captcha.');
      gateSetBtn('gate-register-btn', false);
      return;
    }
    var body = new URLSearchParams(fd).toString();
    fetch('/register', {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'Accept': 'application/json' },
      body: body
    })
    .then(function(r) { return r.json(); })
    .then(function(d) {
      gateSetBtn('gate-register-btn', false);
      if (d.success === false) {
        gateSetError('gate-register-error', d.msg || 'Registration failed.');
        gateResetCaptcha();
      } else {
        document.getElementById('gate-register-ok').textContent = d.msg || 'Account created! You can now sign in.';
        document.getElementById('gate-register-ok').style.display = 'block';
        document.getElementById('gate-register-form').reset();
        setTimeout(function() { gateShowView('login'); }, 2000);
      }
    })
    .catch(function() {
      gateSetBtn('gate-register-btn', false);
      gateSetError('gate-register-error', 'An error occurred. Please try again.');
      gateResetCaptcha();
    });
  };
  // Passkey sign-in button (works for both registered users and anonymous passkey holders)
  var passkeyBtn = document.getElementById('gate-passkey-btn');
  if (passkeyBtn && window.PublicKeyCredential) {
    passkeyBtn.addEventListener('click', async function() {
      gateSetError('gate-login-error', '');
      passkeyBtn.disabled = true;
      passkeyBtn.style.opacity = '0.65';
      try {
        // 1. Get challenge from server
        var beginRes = await fetch('/api/v2/anon/passkey/auth/begin', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({})
        });
        var beginData = await beginRes.json();
        if (!beginData.success) throw new Error(beginData.msg || 'Failed to begin passkey authentication');
        var rawChallenge = beginData.options.challenge; // save original base64url string for finish
        var opts = beginData.options;

        // 2. Decode base64url fields for WebAuthn API
        function b64urlToArr(b64) {
          var bin = atob(b64.replace(/-/g,'+').replace(/_/g,'/'));
          var arr = new Uint8Array(bin.length);
          for (var i=0; i<bin.length; i++) arr[i] = bin.charCodeAt(i);
          return arr;
        }
        function arrToB64url(arr) {
          var bin = '';
          new Uint8Array(arr).forEach(function(b) { bin += String.fromCharCode(b); });
          return btoa(bin).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
        }

        var pkOpts = {
          challenge: b64urlToArr(rawChallenge),
          rpId: opts.rpId,
          userVerification: opts.userVerification || 'preferred',
          timeout: opts.timeout || 60000,
          allowCredentials: (opts.allowCredentials || []).map(function(c) {
            return { type: c.type, id: b64urlToArr(c.id) };
          })
        };

        // 3. Invoke browser passkey picker
        var assertion = await navigator.credentials.get({ publicKey: pkOpts });

        // 4. Send to server — use original base64url challenge string
        var finishRes = await fetch('/api/v2/anon/passkey/auth/finish', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            challenge: rawChallenge,
            clientDataJSON: arrToB64url(assertion.response.clientDataJSON),
            authenticatorData: arrToB64url(assertion.response.authenticatorData),
            signature: arrToB64url(assertion.response.signature),
            userHandle: assertion.response.userHandle ? arrToB64url(assertion.response.userHandle) : null,
            credentialId: arrToB64url(assertion.rawId)
          })
        });
        var finishData = await finishRes.json();
        if (finishData.banned) {
          gateSetError('gate-login-error', 'You are banned: ' + (finishData.reason || ''));
          return;
        }
        if (!finishData.success) throw new Error(finishData.msg || 'Passkey authentication failed');

        window.location.reload();
      } catch(err) {
        if (err && err.name === 'NotAllowedError') {
          gateSetError('gate-login-error', 'Passkey prompt was cancelled.');
        } else {
          gateSetError('gate-login-error', err.message || 'Passkey sign-in failed.');
        }
      } finally {
        passkeyBtn.disabled = false;
        passkeyBtn.style.opacity = '1';
      }
    });
  } else if (passkeyBtn) {
    passkeyBtn.style.display = 'none'; // hide if no WebAuthn support
  }
});

var _sb = '';
document.addEventListener('keydown', function(e) {
  _sb += e.key.toLowerCase();
  if (_sb.endsWith('foobarbaz')) { openLoginGate('login'); _sb = ''; }
  if (_sb.length > 12) _sb = _sb.slice(-12);
});
