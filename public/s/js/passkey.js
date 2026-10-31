/**
 * f0ckm Anonymous Passkey Identity Manager
 *
 * Replaces the old OpenSSH Ed25519 approach.
 * Private keys NEVER touch localStorage — they live in the OS / Bitwarden credential store.
 *
 * Flow:
 *   First visit:  "Login as Anonymous" → register/begin → browser passkey prompt → register/finish → session
 *   Return visit: "Login as Anonymous" → auth/begin → browser passkey picker → auth/finish → session
 *
 * For registered users:
 *   Settings page calls window.f0ckPasskeyManager.addPasskey() to add a passkey.
 */

(function () {
  'use strict';

  // ─── base64url helpers (browser) ───────────────────────────────────────────

  function b64urlToArr(b64) {
    const bin = atob(b64.replace(/-/g, '+').replace(/_/g, '/'));
    const arr = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
    return arr;
  }

  function arrToB64url(buf) {
    const arr = buf instanceof ArrayBuffer ? new Uint8Array(buf) : new Uint8Array(buf);
    let bin = '';
    arr.forEach(b => bin += String.fromCharCode(b));
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }

  // ─── Hardware fingerprint (kept for ban enforcement) ──────────────────────

  async function getHardwareFingerprint() {
    // Off unless the operator enabled it (websrv.anon_hw_fingerprint: true): compute nothing, forget any cached value
    if (window.f0ckHwFingerprint !== true) {
      try { localStorage.removeItem('f0ck_anon_hw_fp'); } catch (e) {}
      return null;
    }
    try {
      const cached = localStorage.getItem('f0ck_anon_hw_fp');
      if (cached) return cached;
    } catch (e) {}

    try {
      let glVendor = '', glRenderer = '', glLimits = '';
      try {
        const canvas = document.createElement('canvas');
        const gl = canvas.getContext('webgl') || canvas.getContext('experimental-webgl');
        if (gl) {
          const ext = gl.getExtension('WEBGL_debug_renderer_info');
          if (ext) {
            glVendor = gl.getParameter(ext.UNMASKED_VENDOR_WEBGL) || '';
            glRenderer = gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) || '';
          }
          glLimits = [
            gl.getParameter(gl.MAX_TEXTURE_SIZE) || 0,
            gl.getParameter(gl.MAX_RENDERBUFFER_SIZE) || 0,
            gl.getParameter(gl.MAX_VERTEX_ATTRIBS) || 0,
            gl.getParameter(gl.MAX_VERTEX_UNIFORM_VECTORS) || 0,
            gl.getParameter(gl.MAX_VARYING_VECTORS) || 0,
            gl.getParameter(gl.MAX_COMBINED_TEXTURE_IMAGE_UNITS) || 0
          ].join(',');
        }
      } catch (e) {}

      let gpuArch = '';
      try {
        if (navigator.gpu) {
          const adapter = await navigator.gpu.requestAdapter();
          if (adapter && adapter.info) {
            gpuArch = [adapter.info.architecture, adapter.info.vendor, adapter.info.description].filter(Boolean).join(':');
          }
        }
      } catch (e) {}

      const concurrency = navigator.hardwareConcurrency || 0;
      const memory = navigator.deviceMemory || 0;
      const platform = navigator.platform || '';
      const screenInfo = [
        window.screen ? window.screen.width : 0,
        window.screen ? window.screen.height : 0,
        window.screen ? window.screen.colorDepth : 0,
        window.devicePixelRatio || 1
      ].join('x');
      const touchPoints = navigator.maxTouchPoints || 0;

      let canvasFp = '';
      try {
        const c2d = document.createElement('canvas');
        c2d.width = 240; c2d.height = 60;
        const ctx = c2d.getContext('2d');
        if (ctx) {
          ctx.fillStyle = '#f60'; ctx.fillRect(10, 5, 60, 20);
          ctx.fillStyle = '#069'; ctx.font = '14pt Arial, sans-serif';
          ctx.fillText('f0ck.dev 😃', 4, 35);
          const imgData = ctx.getImageData(0, 0, 240, 60).data;
          let sum = 0;
          for (let i = 0; i < imgData.length; i += 4) {
            sum = (sum * 31 + imgData[i] + imgData[i+1] + imgData[i+2] + imgData[i+3]) >>> 0;
          }
          canvasFp = sum.toString(16);
        }
      } catch (e) {}

      let audioFp = '';
      try {
        const AudioCtx = window.OfflineAudioContext || window.webkitOfflineAudioContext;
        if (AudioCtx) {
          const actx = new AudioCtx(1, 44100, 44100);
          const osc = actx.createOscillator();
          osc.type = 'triangle';
          osc.frequency.setValueAtTime(10000, actx.currentTime);
          const comp = actx.createDynamicsCompressor();
          comp.threshold.setValueAtTime(-50, actx.currentTime);
          comp.knee.setValueAtTime(40, actx.currentTime);
          comp.ratio.setValueAtTime(12, actx.currentTime);
          comp.attack.setValueAtTime(0, actx.currentTime);
          comp.release.setValueAtTime(0.25, actx.currentTime);
          osc.connect(comp); comp.connect(actx.destination); osc.start(0);
          const buf = await actx.startRendering();
          const ch = buf.getChannelData(0);
          let sum = 0;
          for (let i = 4500; i < Math.min(ch.length, 5000); i++) sum += Math.abs(ch[i] || 0);
          audioFp = sum.toFixed(7);
        }
      } catch (e) {}

      const raw = [glVendor, glRenderer, glLimits, gpuArch, concurrency, memory, platform, screenInfo, touchPoints, canvasFp, audioFp].join('~~~');
      const hashBuf = await window.crypto.subtle.digest('SHA-256', new TextEncoder().encode(raw));
      const hashHex = Array.from(new Uint8Array(hashBuf)).map(b => b.toString(16).padStart(2, '0')).join('');
      const fp = `HW:${hashHex}`;
      try { localStorage.setItem('f0ck_anon_hw_fp', fp); } catch (e) {}
      return fp;
    } catch (err) {
      console.warn('[ANON_PASSKEY] Failed to compute hardware fingerprint:', err);
      return null;
    }
  }

  // ─── Tombstone (ban state persisted client-side) ───────────────────────────

  function getTombstone() {
    try { return JSON.parse(localStorage.getItem('f0ck_anon_tombstone') || 'null'); } catch (e) { return null; }
  }

  function setTombstone(t) {
    try {
      localStorage.setItem('f0ck_anon_tombstone', JSON.stringify(t));
      if (t && t.banned) {
        document.cookie = `f0ck_banned=${encodeURIComponent(JSON.stringify(t))}; Path=/; Max-Age=31536000; SameSite=Lax`;
      }
    } catch (e) {}
  }

  // ─── AnonPasskey class ────────────────────────────────────────────────────

  class AnonPasskey {
    constructor() {
      this.isSessionReady = false;
      this._loginInProgress = false;
    }

    // ── Check WebAuthn support ──────────────────────────────────────────────

    get supported() {
      return !!(window.PublicKeyCredential && navigator.credentials && navigator.credentials.create);
    }

    // ── UI helpers ──────────────────────────────────────────────────────────

    updateNavUI(isAnon) {
      const icon = document.getElementById('nav-visitor-icon');
      const label = document.getElementById('nav-anon-label');
      if (icon) {
        icon.classList.toggle('fa-user-secret', isAnon);
        icon.classList.toggle('fa-user', !isAnon);
      }
      if (label) label.textContent = isAnon ? 'anonymous' : 'guest';

      if (window.f0ckSession && window.f0ckSession.enable_anonymous_access === false) {
        if (label) label.textContent = 'guest';
        ['nav-login-anon-btn', 'nav-anon-identity-btn', 'nav-anon-settings-btn', 'nav-anon-logout-btn', 'nav-anon-divider'].forEach(id => {
          const el = document.getElementById(id);
          if (el) el.style.display = 'none';
        });
        const modalAnonBtn = document.getElementById('modal-login-as-anon-btn');
        if (modalAnonBtn) {
          const w = modalAnonBtn.closest('div');
          if (w) w.style.display = 'none'; else modalAnonBtn.style.display = 'none';
        }
        return;
      }

      const loginAnonBtn = document.getElementById('nav-login-anon-btn');
      const anonIdentityBtn = document.getElementById('nav-anon-identity-btn');
      const anonSettingsBtn = document.getElementById('nav-anon-settings-btn');
      const anonLogoutBtn = document.getElementById('nav-anon-logout-btn');
      const anonDivider = document.getElementById('nav-anon-divider');
      const guestFavsNav = document.getElementById('nav-guest-favs');
      const guestFavsLink = document.getElementById('nav-guest-favs-link');

      if (loginAnonBtn) loginAnonBtn.style.display = isAnon ? 'none' : '';
      if (anonIdentityBtn) anonIdentityBtn.style.display = isAnon ? '' : 'none';
      if (anonSettingsBtn) anonSettingsBtn.style.display = isAnon ? '' : 'none';
      if (anonLogoutBtn) anonLogoutBtn.style.display = isAnon ? '' : 'none';
      if (anonDivider) anonDivider.style.display = isAnon ? '' : 'none';
      if (guestFavsNav) guestFavsNav.style.display = isAnon ? '' : 'none';
      if (guestFavsLink) guestFavsLink.style.display = isAnon ? '' : 'none';

      if (isAnon && window.f0ckSession) {
        window.f0ckSession.user = window.f0ckSession.user || 'anonymous';
        window.f0ckSession.is_anon = true;
        window.f0ckSession.logged_in = true;
      }
      if (typeof window.syncRatingButtonUI === 'function') window.syncRatingButtonUI();
    }

    _applySessionData(data, hwFingerprint) {
      this.isSessionReady = true;
      if (window.f0ckSession) {
        window.f0ckSession.user = 'anonymous';
        window.f0ckSession.is_anon = true;
        window.f0ckSession.logged_in = true;
        window.f0ckSession.id = data.user_id;
        window.f0ckSession.user_id = data.user_id;
        if (data.csrf_token) window.f0ckSession.csrf_token = data.csrf_token;
      }
      const metaCsrf = document.querySelector('meta[name="csrf-token"]');
      if (metaCsrf && data.csrf_token) metaCsrf.content = data.csrf_token;
      window.f0ckAnonIdentity = {
        userId: data.user_id,
        fingerprint: data.fingerprint,
        shortFingerprint: data.short_fingerprint,
        hwFingerprint: data.hw_fingerprint || hwFingerprint,
        credentialId: data.credential_id
      };
      window.dispatchEvent(new CustomEvent('f0ck:anon_session_ready', { detail: window.f0ckAnonIdentity }));
      if (typeof window.syncRatingButtonUI === 'function') window.syncRatingButtonUI();
    }

    // ── Register a new passkey (anonymous user) ────────────────────────────

    async register() {
      if (!this.supported) throw new Error('WebAuthn / Passkeys are not supported in this browser.');

      // 1. Get registration options from server
      const beginRes = await fetch('/api/v2/anon/passkey/register/begin', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({})
      });
      const beginData = await beginRes.json();
      if (!beginData.success) throw new Error(beginData.msg || 'Server error during registration setup');

      const rawChallenge = beginData.options.challenge;
      const opts = beginData.options;

      // 2. Decode options for the WebAuthn API
      const pkOpts = {
        rp: opts.rp,
        user: {
          id: b64urlToArr(opts.user.id),
          name: opts.user.name,
          displayName: opts.user.displayName
        },
        challenge: b64urlToArr(rawChallenge),
        pubKeyCredParams: opts.pubKeyCredParams,
        timeout: opts.timeout || 60000,
        excludeCredentials: (opts.excludeCredentials || []).map(c => ({ type: c.type, id: b64urlToArr(c.id) })),
        authenticatorSelection: opts.authenticatorSelection,
        attestation: opts.attestation || 'none'
      };

      // 3. Browser passkey creation prompt
      const cred = await navigator.credentials.create({ publicKey: pkOpts });

      // 4. Send attestation to server
      const hwFp = await getHardwareFingerprint();
      const tombstone = getTombstone();
      const finishRes = await fetch('/api/v2/anon/passkey/register/finish', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          challenge: rawChallenge,
          credentialId: arrToB64url(cred.rawId),
          clientDataJSON: arrToB64url(cred.response.clientDataJSON),
          attestationObject: arrToB64url(cred.response.attestationObject),
          hw_fingerprint: hwFp,
          tombstone: tombstone
        })
      });
      const finishData = await finishRes.json();

      if (finishData.banned) {
        setTombstone({
          banned: true,
          fingerprint: finishData.fingerprint,
          hw_fingerprint: finishData.hw_fingerprint || hwFp,
          reason: finishData.reason,
          expires: finishData.expires
        });
        if (window.location.pathname !== '/banned') window.location.href = finishData.redirect || '/banned';
        throw new Error('Banned: ' + (finishData.reason || ''));
      }
      if (!finishData.success) throw new Error(finishData.msg || 'Registration failed');

      // Mark this browser as having a registered passkey for this site
      this._markLocalPasskey();
      return finishData;
    }

    // ── Authenticate with an existing passkey (anonymous user) ────────────

    async authenticate() {
      if (!this.supported) throw new Error('WebAuthn / Passkeys are not supported in this browser.');

      // 1. Get auth challenge from server
      const beginRes = await fetch('/api/v2/anon/passkey/auth/begin', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({})
      });
      const beginData = await beginRes.json();
      if (!beginData.success) throw new Error(beginData.msg || 'Server error during authentication setup');

      const rawChallenge = beginData.options.challenge;
      const opts = beginData.options;

      // 2. Invoke browser passkey picker
      const pkOpts = {
        challenge: b64urlToArr(rawChallenge),
        rpId: opts.rpId,
        userVerification: opts.userVerification || 'preferred',
        timeout: opts.timeout || 60000,
        allowCredentials: (opts.allowCredentials || []).map(c => ({ type: c.type, id: b64urlToArr(c.id) }))
      };

      const assertion = await navigator.credentials.get({ publicKey: pkOpts });

      // 3. Send assertion to server
      const hwFp = await getHardwareFingerprint();
      const tombstone = getTombstone();
      const finishRes = await fetch('/api/v2/anon/passkey/auth/finish', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          challenge: rawChallenge,
          credentialId: arrToB64url(assertion.rawId),
          clientDataJSON: arrToB64url(assertion.response.clientDataJSON),
          authenticatorData: arrToB64url(assertion.response.authenticatorData),
          signature: arrToB64url(assertion.response.signature),
          userHandle: assertion.response.userHandle ? arrToB64url(assertion.response.userHandle) : null,
          hw_fingerprint: hwFp,
          tombstone: tombstone
        })
      });
      const finishData = await finishRes.json();

      if (finishData.banned) {
        setTombstone({
          banned: true,
          fingerprint: finishData.fingerprint,
          hw_fingerprint: finishData.hw_fingerprint || hwFp,
          reason: finishData.reason,
          expires: finishData.expires
        });
        if (window.location.pathname !== '/banned') window.location.href = finishData.redirect || '/banned';
        throw new Error('Banned: ' + (finishData.reason || ''));
      }
      if (!finishData.success) throw new Error(finishData.msg || 'Authentication failed');

      return finishData;
    }

    // ── Add a passkey to the current anonymous identity ───────────────────

    async addPasskey() {
      if (!this.supported) throw new Error('WebAuthn / Passkeys are not supported in this browser.');

      const csrfToken = (window.f0ckSession && window.f0ckSession.csrf_token)
        || document.querySelector('meta[name="csrf-token"]')?.content || '';
      const beginRes = await fetch('/api/v2/anon/passkey/add/begin', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'x-csrf-token': csrfToken },
        body: JSON.stringify({})
      });
      const beginData = await beginRes.json();
      if (!beginData.success) throw new Error(beginData.msg || 'Server error');

      const rawChallenge = beginData.options.challenge;
      const opts = beginData.options;

      const cred = await navigator.credentials.create({ publicKey: {
        rp: opts.rp,
        user: {
          id: b64urlToArr(opts.user.id),
          name: opts.user.name,
          displayName: opts.user.displayName
        },
        challenge: b64urlToArr(rawChallenge),
        pubKeyCredParams: opts.pubKeyCredParams,
        timeout: opts.timeout || 60000,
        excludeCredentials: (opts.excludeCredentials || []).map(c => ({ type: c.type, id: b64urlToArr(c.id) })),
        authenticatorSelection: opts.authenticatorSelection,
        attestation: opts.attestation || 'none'
      }});

      const finishRes = await fetch('/api/v2/anon/passkey/add/finish', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'x-csrf-token': csrfToken },
        body: JSON.stringify({
          challenge: rawChallenge,
          credentialId: arrToB64url(cred.rawId),
          clientDataJSON: arrToB64url(cred.response.clientDataJSON),
          attestationObject: arrToB64url(cred.response.attestationObject),
          hw_fingerprint: await getHardwareFingerprint()
        })
      });
      const finishData = await finishRes.json();
      if (!finishData.success) throw new Error(finishData.msg || 'Registration failed');
      return finishData;
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    _hasLocalPasskey() {
      try { return !!localStorage.getItem('f0ck_anon_has_passkey'); } catch (e) { return false; }
    }

    _markLocalPasskey() {
      try { localStorage.setItem('f0ck_anon_has_passkey', '1'); } catch (e) {}
    }

    async _finishLogin(data) {
      const hwFp = await getHardwareFingerprint();
      this._applySessionData(data, hwFp);
      if (window.f0ckGuestFavs && typeof window.f0ckGuestFavs.importToAccount === 'function') {
        await window.f0ckGuestFavs.importToAccount();
      }
      this.updateNavUI(true);
      // Close both modals
      const sm = document.getElementById('anon-setup-modal'); if (sm) sm.style.display = 'none';
      const lm = document.getElementById('login-modal'); if (lm) lm.style.display = 'none';
      if (typeof window.showToastNotification === 'function') {
        window.showToastNotification('Logged in as anonymous');
      }
      window.location.reload();
    }

    // ── Public: called by the setup modal's "Create my passkey" button ────────

    async doRegister() {
      if (this._loginInProgress) return;
      this._loginInProgress = true;
      try {
        const data = await this.register(); // throws on error/cancel
        this._markLocalPasskey();
        await this._finishLogin(data);
      } catch (err) {
        this._loginInProgress = false;
        throw err; // modal handles the error display
      }
    }

    // ── Public: called by the setup modal's "Use my passkey" button ───────────

    async doAuthenticate() {
      if (this._loginInProgress) return;
      this._loginInProgress = true;
      try {
        const data = await this.authenticate();
        await this._finishLogin(data);
      } catch (err) {
        this._loginInProgress = false;
        throw err;
      }
    }

    // ── Public: opens the setup modal (new vs returning view) ─────────────────

    openSetupModal() {
      if (window.f0ckSession && window.f0ckSession.enable_anonymous_access === false) return;
      const tombstone = getTombstone();
      if (tombstone && tombstone.banned) {
        if (window.location.pathname !== '/banned') window.location.href = '/banned';
        return;
      }
      if (!this.supported) {
        alert('Passkeys are not supported in this browser. Please use a modern browser with WebAuthn support.');
        return;
      }

      // Close the login modal first
      const lm = document.getElementById('login-modal'); if (lm) lm.style.display = 'none';

      const hasPasskey = this._hasLocalPasskey();
      const newView = document.getElementById('anon-setup-new');
      const retView = document.getElementById('anon-setup-returning');
      if (newView) newView.style.display = hasPasskey ? 'none' : '';
      if (retView) retView.style.display = hasPasskey ? '' : 'none';

      const modal = document.getElementById('anon-setup-modal');
      if (modal) modal.style.display = 'flex';
    }

    // ── Legacy: clicking "Login as anonymous" anywhere just opens the modal ───

    loginAsAnonymous() {
      this.openSetupModal();
    }

    // ── Logout ────────────────────────────────────────────────────────────────

    async logoutAnonymous() {
      try {
        await fetch('/api/v2/anon/logout', { method: 'POST', credentials: 'same-origin' }).catch(() => {});
      } finally {
        this.updateNavUI(false);
        if (typeof window.showToastNotification === 'function') {
          window.showToastNotification('Logged out from anonymous session');
        }
        window.location.reload();
      }
    }

    // ── Identity modal (shown when already logged in as anon) ─────────────────

    openModal() {
      const modal = document.getElementById('anon-passkey-modal');
      if (!modal) return;
      this._refreshModalContent();
      modal.style.display = 'flex';
    }

    closeModal() {
      const modal = document.getElementById('anon-passkey-modal');
      if (modal) modal.style.display = 'none';
    }

    async _refreshModalContent() {
      try {
        const res = await fetch('/api/v2/anon/identity');
        const data = await res.json();
        const fpEl = document.getElementById('anon-pk-fp-display');
        if (fpEl) fpEl.textContent = data.fingerprint ? data.fingerprint.slice(7, 15) : 'none';
        this._renderPasskeyCount(data.passkey_count, data.passkey_max);
      } catch (e) {}
    }

    _renderPasskeyCount(count, max) {
      const countEl = document.getElementById('anon-pk-count');
      const maxEl = document.getElementById('anon-pk-max');
      const addBtn = document.getElementById('anon-pk-add-btn');
      if (countEl && typeof count === 'number') countEl.textContent = count;
      if (maxEl && typeof max === 'number') maxEl.textContent = max;
      if (addBtn && typeof count === 'number' && typeof max === 'number') {
        const full = count >= max;
        addBtn.disabled = full;
        addBtn.innerHTML = full
          ? '<i class="fa-solid fa-lock"></i> Limit reached'
          : '<i class="fa-solid fa-plus"></i> Add passkey';
      }
    }

    // ── Init ──────────────────────────────────────────────────────────────────

    async init() {
      if (window.location.pathname === '/banned') return;

      if (window.f0ckSession && window.f0ckSession.enable_anonymous_access === false) {
        if (window.f0ckSession.is_anon && window.f0ckSession.logged_in) {
          await fetch('/api/v2/anon/logout', { method: 'POST', credentials: 'same-origin' }).catch(() => {});
          window.location.reload();
          return;
        }
        this.updateNavUI(false);
        return;
      }

      // Registered user — nothing to do
      if (window.f0ckSession && window.f0ckSession.user && !window.f0ckSession.is_anon) return;

      // Check tombstone — if banned, don't auto-login
      const tombstone = getTombstone();
      if (tombstone && tombstone.banned) {
        this.updateNavUI(false);
        this.attachUIListeners();
        return;
      }

      // If backend session already shows is_anon, mark ready
      if (window.f0ckSession && window.f0ckSession.is_anon && window.f0ckSession.logged_in) {
        this.isSessionReady = true;
        this.updateNavUI(true);
      } else {
        // Stale backend session without a local key no longer applies — just show guest
        if (window.f0ckSession && window.f0ckSession.is_anon && !window.f0ckSession.logged_in) {
          await fetch('/api/v2/anon/logout', { method: 'POST', credentials: 'same-origin' }).catch(() => {});
          window.location.reload();
          return;
        }
        this.updateNavUI(false);
      }

      this.attachUIListeners();
    }

    attachUIListeners() {
      // Modal login-as-anonymous button
      const modalAnonBtn = document.getElementById('modal-login-as-anon-btn');
      if (modalAnonBtn) {
        modalAnonBtn.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); this.loginAsAnonymous(); });
      }

      document.addEventListener('click', e => {
        if (e.target.closest('#nav-anon-identity-btn')) {
          e.preventDefault(); this.openModal(); return;
        }
        if (e.target.closest('#nav-login-anon-btn, #modal-login-as-anon-btn, .trigger-login-anon')) {
          e.preventDefault(); this.loginAsAnonymous(); return;
        }
        const logoutTarget = e.target.closest('#nav-anon-logout-btn, a[href="/logout"]');
        if (logoutTarget && window.f0ckSession && window.f0ckSession.is_anon && window.f0ckSession.logged_in) {
          e.preventDefault(); this.logoutAnonymous(); return;
        }
      });

      const anonBtn = document.getElementById('nav-anon-identity-btn');
      if (anonBtn) anonBtn.addEventListener('click', e => { e.preventDefault(); this.openModal(); });

      const modalClose = document.getElementById('anon-passkey-modal-close');
      if (modalClose) modalClose.addEventListener('click', () => this.closeModal());

      const modalOverlay = document.getElementById('anon-passkey-modal');
      if (modalOverlay) modalOverlay.addEventListener('click', e => { if (e.target === modalOverlay) this.closeModal(); });

      // Add-passkey button inside modal — attaches a new passkey to this identity (max enforced server-side)
      const addBtn = document.getElementById('anon-pk-add-btn');
      if (addBtn) {
        addBtn.addEventListener('click', async () => {
          const errEl = document.getElementById('anon-pk-error');
          if (errEl) errEl.style.display = 'none';
          addBtn.disabled = true;
          try {
            const data = await this.addPasskey();
            if (typeof window.showToastNotification === 'function') window.showToastNotification('Passkey added');
            this._renderPasskeyCount(data.passkey_count, data.passkey_max);
          } catch (err) {
            console.warn('[ANON_PASSKEY] Modal add passkey error:', err);
            if (errEl) {
              errEl.style.display = '';
              errEl.textContent = (err && err.name === 'NotAllowedError')
                ? 'Cancelled or blocked. Unlock your password manager and try again.'
                : ((err && err.message) || 'Failed to add passkey.');
            }
            addBtn.disabled = false;
            this._refreshModalContent();
          }
        });
      }
    }
  }

  // ─── Passkey manager for registered users (used by settings page) ──────────

  class PasskeyManager {
    async listPasskeys() {
      const res = await fetch('/api/v2/settings/passkeys');
      return (await res.json()).passkeys || [];
    }

    async addPasskey(name) {
      // 1. Begin
      const beginRes = await fetch('/api/v2/settings/passkeys/register/begin', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: name || 'Passkey' })
      });
      const beginData = await beginRes.json();
      if (!beginData.success) throw new Error(beginData.msg || 'Server error');

      const rawChallenge = beginData.options.challenge;
      const opts = beginData.options;

      const pkOpts = {
        rp: opts.rp,
        user: {
          id: b64urlToArr(opts.user.id),
          name: opts.user.name,
          displayName: opts.user.displayName
        },
        challenge: b64urlToArr(rawChallenge),
        pubKeyCredParams: opts.pubKeyCredParams,
        timeout: opts.timeout || 60000,
        excludeCredentials: (opts.excludeCredentials || []).map(c => ({ type: c.type, id: b64urlToArr(c.id) })),
        authenticatorSelection: opts.authenticatorSelection,
        attestation: opts.attestation || 'none'
      };

      const cred = await navigator.credentials.create({ publicKey: pkOpts });

      // 2. Finish
      const csrfToken = document.querySelector('meta[name="csrf-token"]')?.content || '';
      const finishRes = await fetch('/api/v2/settings/passkeys/register/finish', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'x-csrf-token': csrfToken },
        body: JSON.stringify({
          challenge: rawChallenge,
          credentialId: arrToB64url(cred.rawId),
          clientDataJSON: arrToB64url(cred.response.clientDataJSON),
          attestationObject: arrToB64url(cred.response.attestationObject),
          name: name || 'Passkey'
        })
      });
      const finishData = await finishRes.json();
      if (!finishData.success) throw new Error(finishData.msg || 'Registration failed');
      return finishData;
    }

    async deletePasskey(credentialId) {
      const csrfToken = document.querySelector('meta[name="csrf-token"]')?.content || '';
      const res = await fetch('/api/v2/settings/passkeys/delete', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'x-csrf-token': csrfToken },
        body: JSON.stringify({ credential_id: credentialId })
      });
      const data = await res.json();
      if (!data.success) throw new Error(data.msg || 'Delete failed');
      return data;
    }
  }

  // ─── Bootstrap ────────────────────────────────────────────────────────────

  window.f0ckAnonPasskey = new AnonPasskey();
  window.f0ckPasskeyManager = new PasskeyManager();

  // Backwards compat alias (so any code that checks window.f0ckAnonSSH still works for guards)
  window.f0ckAnonSSH = window.f0ckAnonPasskey;

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => window.f0ckAnonPasskey.init());
  } else {
    window.f0ckAnonPasskey.init();
  }
})();
