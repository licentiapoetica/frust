/*
 * Captcha popup before uploads and comments (`cap.uploads` / `cap.comments` in the config).
 *
 *   try { await window.f0ckCaptchaGate('upload'); } catch { return; }   // rejects when cancelled
 *
 * Asks the server whether this account needs a captcha right now (it may still hold a pass from an
 * earlier one). If so, shows the popup from views/snippets/cap-gate.html, loads the Cap widget on
 * first use, and after solving trades the token for a pass (POST /api/cap/pass). The server
 * enforces the same rule, so skipping this only gets the request refused.
 */
(() => {
  if (window.f0ckCaptchaGate) return;
  const tpl = document.getElementById('cap-gate-template');
  if (!tpl) return;
  const scopes = new Set((tpl.dataset.scopes || '').split(',').filter(Boolean));
  const pending = new Map(); // scope -> promise, so parallel calls share one popup

  const loadScript = (src) => new Promise((resolve, reject) => {
    const s = document.createElement('script');
    s.src = src;
    s.onload = resolve;
    s.onerror = () => reject(new Error('Failed to load ' + src));
    document.head.appendChild(s);
  });
  const loadCap = async () => {
    if (!window.f0ckCapReset) await loadScript(tpl.dataset.configSrc);
    if (!window.customElements.get('cap-widget')) await loadScript(tpl.dataset.widgetSrc);
  };
  const csrf = () => window.f0ckSession?.csrf_token || window.scrollerCsrf || document.querySelector('meta[name="csrf-token"]')?.content || '';

  const show = (scope) => new Promise((resolve, reject) => {
    const overlay = document.importNode(tpl.content.firstElementChild, true);
    const widget = overlay.querySelector('cap-widget');
    const errorEl = overlay.querySelector('.cap-gate-error');
    let done = false;
    const finish = (ok, err) => {
      if (done) return;
      done = true;
      document.removeEventListener('keydown', onKey, true);
      overlay.remove();
      ok ? resolve() : reject(err || new Error('Captcha cancelled'));
    };
    const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); finish(false); } };
    overlay.querySelector('.cap-gate-cancel')?.addEventListener('click', () => finish(false));
    overlay.addEventListener('click', (e) => { if (e.target === overlay) finish(false); });
    document.addEventListener('keydown', onKey, true);

    widget.addEventListener('solve', async (e) => {
      errorEl.textContent = '';
      try {
        const res = await fetch('/api/cap/pass', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf() },
          body: JSON.stringify({ scope, 'cap-token': e.detail?.token || '' }),
        });
        const data = await res.json().catch(() => ({}));
        if (data.success) return finish(true);
        errorEl.textContent = data.error || data.msg || 'Captcha failed, please try again.';
      } catch {
        errorEl.textContent = 'Network error, please try again.';
      }
      try { widget.reset(); } catch {}
    });

    document.body.appendChild(overlay);
    loadCap().then(() => window.f0ckCapWatch?.(widget)).catch((err) => {
      errorEl.textContent = 'Captcha failed to load.';
      console.error('[captcha]', err);
    });
  });

  window.f0ckCaptchaGate = async (scope) => {
    if (!scopes.has(scope)) return;
    if (pending.has(scope)) return pending.get(scope);
    const p = (async () => {
      const status = await fetch(`/api/cap/pass?scope=${encodeURIComponent(scope)}`, { credentials: 'same-origin', cache: 'no-store' })
        .then((r) => r.json()).catch(() => null);
      // Unknown state (network error): go ahead, the server still checks
      if (!status || status.required !== true) return;
      await show(scope);
    })();
    pending.set(scope, p);
    try { return await p; } finally { pending.delete(scope); }
  };
})();
