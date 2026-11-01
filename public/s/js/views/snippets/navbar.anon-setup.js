(function() {
  function closeSetupModal() {
    var m = document.getElementById('anon-setup-modal'); if (m) m.style.display = 'none';
  }
  var closeBtn = document.getElementById('anon-setup-modal-close');
  if (closeBtn) closeBtn.addEventListener('click', closeSetupModal);
  var overlay = document.getElementById('anon-setup-modal');
  if (overlay) overlay.addEventListener('click', function(e) { if (e.target === overlay) closeSetupModal(); });
  var switchToExisting = document.getElementById('anon-setup-switch-existing');
  var switchToNew = document.getElementById('anon-setup-switch-new');
  if (switchToExisting) switchToExisting.addEventListener('click', function() {
    document.getElementById('anon-setup-new').style.display = 'none';
    document.getElementById('anon-setup-returning').style.display = '';
  });
  if (switchToNew) switchToNew.addEventListener('click', function() {
    document.getElementById('anon-setup-returning').style.display = 'none';
    document.getElementById('anon-setup-new').style.display = '';
  });
  // Privacy consent (new passkeys only): the checkbox can only be ticked by reading the privacy page in
  // the consent modal to the end. Accepting is remembered in localStorage and the checkbox never shows again.
  var CONSENT_KEY = 'f0ck_privacy_accepted';
  var consentInput = document.getElementById('anon-consent-new');
  var hasConsent = function() {
    try { return !!localStorage.getItem(CONSENT_KEY); } catch (e) { return false; }
  };
  var syncConsent = function() {
    var accepted = hasConsent();
    document.querySelectorAll('#anon-setup-modal .anon-consent').forEach(function(el) { el.style.display = accepted ? 'none' : ''; });
    var btn = document.getElementById('anon-setup-create-btn');
    if (btn && !btn.querySelector('.fa-spinner')) btn.disabled = !(accepted || (consentInput && consentInput.checked));
  };
  var recordConsent = function() {
    if (hasConsent()) return true;
    if (!consentInput || !consentInput.checked) return false;
    try { localStorage.setItem(CONSENT_KEY, JSON.stringify({ accepted_at: new Date().toISOString() })); } catch (e) {}
    return true;
  };

  var pcModal = document.getElementById('privacy-consent-modal');
  var pcBody = document.getElementById('privacy-consent-body');
  var pcAccept = document.getElementById('privacy-consent-accept');
  var pcHint = document.getElementById('privacy-consent-hint');
  var pcLoaded = false;
  var pcCheckEnd = function() {
    if (!pcLoaded) return;
    var atEnd = pcBody.scrollTop + pcBody.clientHeight >= pcBody.scrollHeight - 8;
    if (atEnd) {
      pcAccept.disabled = false;
      if (pcHint) pcHint.textContent = 'You reached the end.';
    }
  };
  var loadPrivacy = async function() {
    if (pcLoaded) return;
    pcBody.innerHTML = '<div class="pc-loading"><i class="fa-solid fa-spinner fa-spin"></i> Loading…</div>';
    try {
      var r = await fetch('/privacy', { credentials: 'same-origin' });
      var doc = new DOMParser().parseFromString(await r.text(), 'text/html');
      var pv = doc.querySelector('#main .pv');
      if (!r.ok || !pv) throw new Error('bad response');
      pcBody.innerHTML = '';
      doc.querySelectorAll('#main style').forEach(function(st) { pcBody.appendChild(st.cloneNode(true)); });
      pcBody.appendChild(document.importNode(pv, true));
      pcBody.querySelectorAll('a[href]').forEach(function(a) { a.target = '_blank'; a.rel = 'noopener'; });
    } catch (e) {
      pcBody.innerHTML = '<div class="pc-loading">Could not load the privacy page. <a href="/privacy" target="_blank" class="lm-link">Open it in a new tab</a>.</div>';
    }
    pcLoaded = true;
  };
  var openConsentModal = async function() {
    if (!pcModal) return;
    pcAccept.disabled = true;
    if (pcHint) pcHint.textContent = 'Scroll to the end to accept.';
    pcModal.style.display = 'flex';
    await loadPrivacy();
    pcBody.scrollTop = 0;
    // Short content that doesn't scroll counts as read
    requestAnimationFrame(pcCheckEnd);
  };
  var closeConsentModal = function() { if (pcModal) pcModal.style.display = 'none'; };
  if (pcBody) pcBody.addEventListener('scroll', pcCheckEnd, { passive: true });
  if (pcAccept) pcAccept.addEventListener('click', function() {
    if (consentInput) consentInput.checked = true;
    closeConsentModal();
    syncConsent();
  });
  var pcClose = document.getElementById('privacy-consent-close');
  if (pcClose) pcClose.addEventListener('click', closeConsentModal);

  if (consentInput) consentInput.addEventListener('click', function(e) {
    // Ticking goes through the privacy modal; unticking is always allowed
    if (!consentInput.checked) return syncConsent();
    e.preventDefault();
    openConsentModal();
  });
  document.querySelectorAll('#anon-setup-modal .anon-consent-open').forEach(function(a) {
    a.addEventListener('click', function(e) { e.preventDefault(); e.stopPropagation(); openConsentModal(); });
  });

  syncConsent();
  // Re-sync whenever the modal is opened (another tab may have accepted meanwhile)
  if (overlay) new MutationObserver(function() { if (overlay.style.display !== 'none') syncConsent(); }).observe(overlay, { attributes: true, attributeFilter: ['style'] });

  var createBtn = document.getElementById('anon-setup-create-btn');
  if (createBtn) createBtn.addEventListener('click', async function() {
    var errEl = document.getElementById('anon-setup-new-error');
    if (errEl) errEl.style.display = 'none';
    if (!recordConsent()) return;
    createBtn.disabled = true;
    createBtn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Saving passkey...';
    try {
      await window.f0ckAnonPasskey.doRegister();
    } catch(e) {
      console.warn('[ANON_PASSKEY] Registration failed:', e);
      if (errEl) {
        errEl.style.display = '';
        if (e && e.name === 'NotAllowedError') {
          errEl.textContent = 'Passkey creation was cancelled or blocked. If Bitwarden or another password manager is locked, please unlock it or disable passkey interception.';
        } else {
          errEl.textContent = (e && e.message) || 'Something went wrong.';
        }
      }
      createBtn.disabled = false;
      createBtn.innerHTML = '<i class="fa-solid fa-fingerprint"></i> Create';
    }
  });
  var authBtn = document.getElementById('anon-setup-auth-btn');
  if (authBtn) authBtn.addEventListener('click', async function() {
    var errEl = document.getElementById('anon-setup-ret-error');
    if (errEl) errEl.style.display = 'none';
    authBtn.disabled = true;
    authBtn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Waiting for passkey...';
    try {
      await window.f0ckAnonPasskey.doAuthenticate();
    } catch(e) {
      console.warn('[ANON_PASSKEY] Authentication failed:', e);
      if (errEl) {
        errEl.style.display = '';
        if (e && e.name === 'NotAllowedError') {
          errEl.textContent = 'Passkey request was cancelled or blocked. If Bitwarden or another password manager is locked, please unlock it.';
        } else {
          errEl.textContent = (e && e.message) || 'Authentication failed.';
        }
      }
      authBtn.disabled = false;
      authBtn.innerHTML = '<i class="fa-solid fa-fingerprint"></i> Authenticate';
    }
  });
})();
