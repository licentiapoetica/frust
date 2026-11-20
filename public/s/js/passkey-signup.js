/**
 * A passkey besides the password at registration (server: routes/register.rs).
 *
 * Every register form has an "Also create a passkey" checkbox (`add_passkey`, in a
 * `.reg-passkey-opt` element). It stays hidden and unticked until this script finds WebAuthn in the
 * browser: then it adds `pk-signup-ready` to the form and ticks the box. Without JavaScript (or
 * passkeys) the form registers with the password alone, as it always did.
 *
 * window.f0ckPasskeySignup(form): posts the form to /register with `add_passkey=1`. The server
 * checks everything (captcha, name, password, invite token or application) and answers with the
 * passkey creation options, the browser makes the passkey, /register/passkey creates the account
 * with the password and the passkey. Resolves with the server's last answer:
 * { success, msg, logged_in, applied, upgraded }. `logged_in`: a new account, signed in with the
 * passkey; `upgraded`: an anonymous identity got its name (it keeps its passkeys, so no new one is
 * made). The pages whose own script sends the register form call it when the box is ticked.
 *
 * Forms the page sends as plain posts carry `data-passkey-signup="auto"`: with the box ticked this
 * script sends them itself and shows the answer in their `[data-passkey-signup-msg]` element (class
 * from its `data-ok-class` / `data-error-class`). After a success the form gets `pk-signup-done`,
 * which hides its `.reg-fields`, so only the message (and the page's own links) stay.
 */
(() => {
  'use strict';
  if (window.f0ckPasskeySignup) return; // loaded twice (a page with a register page and modal)

  const supported = !!(window.PublicKeyCredential && navigator.credentials && navigator.credentials.create);

  const b64urlToArr = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));
  const arrToB64url = (buf) => {
    let bin = '';
    new Uint8Array(buf).forEach((b) => { bin += String.fromCharCode(b); });
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  };

  const box = (form) => form.querySelector('input[name="add_passkey"]');
  const usesPasskey = (form) => supported && !!(box(form) && box(form).checked);

  const setup = (form) => {
    const b = box(form);
    if (form.dataset.pkSignupSetup || !b) return;
    form.dataset.pkSignupSetup = '1';
    if (!supported) {
      b.checked = false; // stays hidden: a password-only form
      return;
    }
    form.classList.add('pk-signup-ready');
    b.checked = true;
    // a form reset unticks it (its markup default), tick it again
    form.addEventListener('reset', () => setTimeout(() => { b.checked = true; }, 0));
  };

  const setupAll = () => document.querySelectorAll('form').forEach(setup);

  const postJson = async (url, body, type) => {
    const res = await fetch(url, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'X-Requested-With': 'XMLHttpRequest', 'Accept': 'application/json', 'Content-Type': type },
      body,
    });
    return res.json();
  };

  window.f0ckPasskeySignup = async (form) => {
    if (!supported) return { success: false, msg: 'Passkeys are not supported in this browser.' };
    const params = new URLSearchParams(new FormData(form));
    params.set('add_passkey', '1');
    const begin = await postJson('/register', params, 'application/x-www-form-urlencoded');
    // A refusal, or done without a new passkey (an anonymous identity keeps the ones it has)
    if (!begin || !begin.success || !begin.passkey_options) return begin;

    const o = begin.passkey_options;
    let cred;
    try {
      cred = await navigator.credentials.create({ publicKey: {
        rp: o.rp,
        user: { id: b64urlToArr(o.user.id), name: o.user.name, displayName: o.user.displayName },
        challenge: b64urlToArr(o.challenge),
        pubKeyCredParams: o.pubKeyCredParams,
        timeout: o.timeout || 60000,
        excludeCredentials: (o.excludeCredentials || []).map((c) => ({ type: c.type, id: b64urlToArr(c.id) })),
        authenticatorSelection: o.authenticatorSelection,
        attestation: o.attestation || 'none',
      } });
    } catch (e) {
      const msg = e && e.name === 'NotAllowedError'
        ? 'No passkey was created (cancelled or blocked), so no account either. Unlock your password manager and try again, or untick "Also create a passkey".'
        : ((e && e.message) || 'Creating the passkey failed.');
      return { success: false, msg };
    }
    return postJson('/register/passkey', JSON.stringify({
      challenge: o.challenge,
      credentialId: arrToB64url(cred.rawId),
      clientDataJSON: arrToB64url(cred.response.clientDataJSON),
      attestationObject: arrToB64url(cred.response.attestationObject),
    }), 'application/json');
  };

  // Plain-post register forms (the frontends' register pages)
  document.addEventListener('submit', async (e) => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement) || form.dataset.passkeySignup !== 'auto' || !usesPasskey(form)) return;
    e.preventDefault();
    if (form.noValidate && !form.reportValidity()) return;
    const box = form.querySelector('[data-passkey-signup-msg]');
    const show = (text, ok) => {
      if (!box) return;
      box.textContent = text || '';
      box.className = (ok ? box.dataset.okClass : box.dataset.errorClass) || '';
      box.hidden = !text;
    };
    const btn = form.querySelector('[type="submit"]');
    if (btn) btn.disabled = true;
    show('', true);
    try {
      const res = await window.f0ckPasskeySignup(form);
      if (res && res.success && (res.logged_in || res.upgraded)) {
        location.href = '/';
        return;
      }
      show((res && res.msg) || 'Registration failed.', !!(res && res.success));
      if (res && res.success) {
        // only the message stays: the fields (.reg-fields) go
        form.reset();
        form.classList.add('pk-signup-done');
      } else {
        window.f0ckCapReset?.(form); // captcha tokens are single-use
      }
    } catch (err) {
      console.error('[PASSKEY] Registration failed:', err);
      show('Network error.', false);
    } finally {
      if (btn) btn.disabled = false;
    }
  });

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', setupAll);
  setupAll();
})();
