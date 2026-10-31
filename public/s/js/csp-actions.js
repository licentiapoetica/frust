/*
 * Markup event handlers without inline JavaScript.
 *
 * The Content-Security-Policy forbids inline handlers (onclick="…", onerror="…"), so markup names an
 * action instead and this file binds it with addEventListener:
 *
 *   <button data-on="click:deleteSession" data-args='[5]'>            → deleteSession(el, event, 5)
 *   <img src="/t/1.webp" data-on="error:img-fallback" data-fallback="/t/2.webp">
 *   <a href="/user/x" data-on="click:stop,ajax-nav">                   → actions run left to right
 *   <form data-on="submit:prevent,saveMotd">
 *   <div data-on="mouseover:… mouseout:…">                             → several events, space separated
 *
 * Arguments: data-args (JSON array) or data-arg (one string). An action returning false stops the
 * remaining actions for that event. Handlers are bound directly on the element (like inline handlers),
 * so event order and stopPropagation behave the same as before. Elements added later (innerHTML,
 * PJAX) are bound by a MutationObserver before any of their events can fire.
 *
 * Only actions registered with F0ckActions.define() can run; the page sanitizer strips data-on and
 * data-args from user content so comments cannot trigger them.
 *
 * Loaded synchronously in <head> so img error/load handlers are bound before images resolve.
 */
(() => {
  if (window.F0ckActions) return;

  const actions = Object.create(null);
  const bound = new WeakMap(); // element -> data-on value it was bound with

  const define = (name, fn) => {
    if (typeof name === 'object') {
      for (const [k, v] of Object.entries(name)) define(k, v);
      return;
    }
    if (typeof fn !== 'function') throw new TypeError(`F0ckActions.define(${name}): not a function`);
    actions[name] = fn;
  };

  const argsOf = (el) => {
    if (el.hasAttribute('data-args')) {
      try {
        const a = JSON.parse(el.getAttribute('data-args'));
        return Array.isArray(a) ? a : [a];
      } catch (err) {
        console.error('[F0ckActions] bad data-args on', el, err);
        return [];
      }
    }
    return el.hasAttribute('data-arg') ? [el.getAttribute('data-arg')] : [];
  };

  // "click:a,b mouseover:c" -> [["click", ["a","b"]], ["mouseover", ["c"]]]
  const parse = (spec) => spec.trim().split(/\s+/).filter(Boolean).map((part) => {
    const i = part.indexOf(':');
    return i < 0 ? null : [part.slice(0, i), part.slice(i + 1).split(',').filter(Boolean)];
  }).filter(Boolean);

  const listeners = new WeakMap(); // element -> [[type, fn], …] so a changed data-on rebinds cleanly

  const bind = (el) => {
    const spec = el.getAttribute('data-on');
    if (bound.get(el) === spec) return;
    for (const [type, fn] of listeners.get(el) || []) el.removeEventListener(type, fn);
    const added = [];
    if (spec) {
      for (const [type, names] of parse(spec)) {
        const fn = function (event) {
          const args = argsOf(el);
          for (const name of names) {
            const action = actions[name];
            if (!action) {
              console.error(`[F0ckActions] unknown action "${name}" for ${type} on`, el);
              return;
            }
            if (action(el, event, ...args) === false) return;
          }
        };
        el.addEventListener(type, fn);
        added.push([type, fn]);
      }
    }
    listeners.set(el, added);
    bound.set(el, spec);
  };

  const scan = (root) => {
    if (root.nodeType !== 1 && root.nodeType !== 9 && root.nodeType !== 11) return;
    if (root.nodeType === 1 && root.hasAttribute('data-on')) bind(root);
    root.querySelectorAll('[data-on]').forEach(bind);
  };

  new MutationObserver((records) => {
    for (const r of records) {
      if (r.type === 'attributes') bind(r.target);
      else r.addedNodes.forEach(scan);
    }
  }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-on'] });
  scan(document);

  // ── Built-in actions ────────────────────────────────────────────────────────
  define({
    stop: (el, e) => { e.stopPropagation(); },
    prevent: (el, e) => { e.preventDefault(); },
    hide: (el) => { el.style.display = 'none'; },
    remove: (el) => { el.remove(); },
    'submit-form': (el) => { (el.form || el.closest('form'))?.submit(); },
    // data-confirm="Really?" — stops the following actions (and the default action) unless confirmed
    confirm: (el, e) => {
      if (window.confirm(el.getAttribute('data-confirm') || 'Are you sure?')) return true;
      e.preventDefault();
      return false;
    },
    // Navigate to data-href (or href) through PJAX when available
    'ajax-nav': (el, e) => {
      const href = el.getAttribute('data-href') || el.getAttribute('href');
      if (!href) return;
      e.preventDefault();
      if (typeof window.loadPageAjax === 'function') window.loadPageAjax(href, true);
      else window.location.href = href;
    },
    go: (el) => {
      const href = el.getAttribute('data-href');
      if (href) window.location.href = href;
    },
    // <img data-on="error:img-fallback" data-fallback="/a.webp /b.webp" [data-fallback-hide]>
    // Tries each fallback URL once, then hides the image if data-fallback-hide is present.
    'img-fallback': (el) => {
      const list = (el.getAttribute('data-fallback') || '').split(/\s+/).filter(Boolean);
      const next = list.shift();
      el.setAttribute('data-fallback', list.join(' '));
      if (next) el.src = next;
      else if (el.hasAttribute('data-fallback-hide')) el.style.display = 'none';
    },
    // data-toggle-class="show" data-target="#navbarContent" (target defaults to the element itself)
    'toggle-class': (el) => {
      const sel = el.getAttribute('data-target');
      const target = sel ? document.querySelector(sel) : el;
      target?.classList.toggle(el.getAttribute('data-toggle-class') || 'active');
    },
    'sync-navbar': () => { window.syncNavbarHeight?.(); },
    // data-arg="cancel-btn": click another element (e.g. a modal's × forwarding to its Cancel button)
    'click-by-id': (el, e, id) => { document.getElementById(id)?.click(); },
  });

  // ── Re-running scripts from fetched HTML (PJAX) ─────────────────────────────
  // Scripts parsed with DOMParser are inert, so page loaders re-create them. An inline script only
  // runs again if it carries the nonce of the response it came from (so markup injected into that
  // page can't sneak a script through), and the copy gets this page's nonce.

  // Nonce in a fetch() response's policy header; null when the response has no policy at all.
  const responseNonce = (response) => {
    const h = response?.headers;
    const csp = h && (h.get('content-security-policy') || h.get('content-security-policy-report-only'));
    if (!csp) return null;
    return csp.match(/'nonce-([^']+)'/)?.[1] || '';
  };

  let pageNonce = null;
  const getPageNonce = () => {
    if (pageNonce === null) pageNonce = document.querySelector('script[nonce]')?.nonce || '';
    return pageNonce;
  };

  const isExecutable = (s) => !s.type || /^(text|application)\/(java|ecma)script$|^module$/i.test(s.type);

  // Copy of a fetched <script> that is allowed to run here, or null. Call it before the original is
  // inserted into the page: connecting an element clears its nonce attribute.
  const cloneScript = (old, nonce) => {
    const exec = isExecutable(old);
    if (exec && nonce !== null) {
      if (old.hasAttribute('src')) {
        const u = new URL(old.getAttribute('src'), location.href);
        if (u.origin !== location.origin || !u.pathname.startsWith('/s/')) return null;
      } else if (!nonce || old.getAttribute('nonce') !== nonce) {
        return null;
      }
    }
    const s = document.createElement('script');
    for (const a of old.attributes) if (a.name !== 'nonce') s.setAttribute(a.name, a.value);
    if (exec && !old.hasAttribute('src')) s.nonce = getPageNonce();
    s.textContent = old.textContent;
    return s;
  };

  window.F0ckActions = Object.freeze({ define, bind, scan, has: (n) => n in actions, responseNonce, cloneScript });
})();
