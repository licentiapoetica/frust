/*
 * Cap captcha (https://trycap.dev), self-hosted. Load this before cap.min.js: the widget reads these
 * globals when it starts. The widget posts to /api/cap/ and puts its single-use token in the
 * surrounding form as `cap-token`.
 */
(() => {
  if (window.f0ckCapReset) return; // already loaded (e.g. by cap-gate.js)
  // Self-hosted solver (the widget would otherwise fetch it from cdn.jsdelivr.net)
  window.CAP_CUSTOM_WASM_URL = '/s/cap/cap_wasm_bg.wasm';
  // No tracking parameters on the credits link (in case it ever shows)
  window.CAP_DISABLE_WIDGET_REF = true;

  // Remove the widget's "Cap" credits link. The widget puts it back with !important inline styles
  // when it starts and on every solve, so each widget's shadow root is watched and the link dropped
  // whenever it reappears (before it is painted). Cap is credited in public/s/cap/LICENSE and the README.
  const dropCredits = (root) => root.querySelectorAll('a.credits').forEach((a) => a.remove());
  const watchWidget = (w) => {
    if (!w.shadowRoot || w._f0ckCreditsWatched) return;
    w._f0ckCreditsWatched = true;
    dropCredits(w.shadowRoot);
    new MutationObserver(() => dropCredits(w.shadowRoot)).observe(w.shadowRoot, { childList: true, subtree: true });
  };
  window.customElements.whenDefined('cap-widget').then(() => {
    document.querySelectorAll('cap-widget').forEach(watchWidget);
  });
  // For widgets added later (the upload/comment captcha popup)
  window.f0ckCapWatch = (w) => window.customElements.whenDefined('cap-widget').then(() => watchWidget(w));

  // Reset every widget inside `root`: tokens are single-use, so a failed submit needs a new one.
  window.f0ckCapReset = (root) => {
    (root || document).querySelectorAll('cap-widget').forEach((w) => { try { w.reset(); } catch {} });
  };

  // Captcha the server asked for after a failed submit (login after repeated failures): reset the
  // widget inside `form`, or put it in from the form's <template class="cap-on-demand"> first.
  window.f0ckCapShow = (form) => {
    if (!form) return;
    if (form.querySelector('cap-widget')) return window.f0ckCapReset(form);
    const tpl = form.querySelector('template.cap-on-demand');
    if (!tpl) return;
    const widget = document.importNode(tpl.content.firstElementChild, true);
    tpl.replaceWith(widget);
    window.f0ckCapWatch(widget);
  };

  // Current token of the widget inside `root` ('' when unsolved).
  window.f0ckCapToken = (root) => (root || document).querySelector('cap-widget input[name="cap-token"]')?.value || '';

  // Square corners everywhere; dark look for widgets with class "cap-dark"
  const style = document.createElement('style');
  style.textContent = `cap-widget { --cap-border-radius: 0px; --cap-checkbox-border-radius: 0px; }
  cap-widget.cap-dark {
    --cap-background: #1b1b1b; --cap-border-color: #3a3a3a; --cap-color: #e0e0e0;
    --cap-checkbox-background: #262626; --cap-checkbox-border: 1px solid #555;
    --cap-spinner-background-color: #333; --cap-spinner-color: #fff; --cap-troubleshoot-color: #6ab0ff;
  }`;
  document.head.appendChild(style);
})();
