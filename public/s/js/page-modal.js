/**
 * page-modal.js — open info pages (ranking, rules, about, privacy, terms) in a modal over the content area
 * (below the navbar, between the sidebars).
 *
 * Normal navigation swaps #main, which kills a playing video/audio. Links marked [data-page-modal]
 * instead fetch the page, lift its #main content (with the page's <style>s and the #main class, which
 * some page CSS hangs on) into the modal and re-run its inline scripts. The page underneath is untouched.
 *
 * Inside the modal, links to other modal pages load in place; any other link closes the modal and
 * navigates as usual. Direct visits to these URLs are unaffected.
 */
(() => {
  if (window.f0ckPageModal) return;

  const MODAL_PAGES = /^\/(ranking|rules|about|privacy|terms|settings)\/?$/;

  const modal = document.getElementById('page-modal');
  if (!modal) return;
  const body = modal.querySelector('.pgm-body');
  const titleEl = modal.querySelector('.pgm-title');
  let currentUrl = null;
  let loadSeq = 0;
  // Open state is tracked separately from display: while the close animation runs the modal is still shown
  let isOpen = false;
  let fx = null; // running open/close animation
  const reduceMotion = window.matchMedia ? window.matchMedia('(prefers-reduced-motion: reduce)') : { matches: false };
  const animate = (el, frames, opts) => (typeof el.animate === 'function' && !reduceMotion.matches) ? el.animate(frames, opts) : null;
  const pathOf = (href) => { try { return new URL(href, location.origin).pathname.replace(/\/+$/, '') || '/'; } catch { return null; } };
  // Page being shown: the one open in the modal, else the current page when it is one of the modal pages
  // (direct visit, or a reload while the modal had put its URL in the address bar)
  const shownPath = () => {
    if (isOpen && currentUrl) return pathOf(currentUrl);
    return MODAL_PAGES.test(location.pathname) ? pathOf(location.pathname) : null;
  };
  // Sidebar/footer entries of the page being shown get .is-active (a second click on a modal entry closes it)
  const markActive = () => {
    const cur = shownPath();
    document.querySelectorAll('a[data-page-modal]').forEach((a) => {
      a.classList.toggle('is-active', !!cur && pathOf(a.getAttribute('href')) === cur);
    });
  };
  // URL bar + title of the page underneath, restored on close. replaceState (never push/back): a popstate
  // would make the site's AJAX router reload the page underneath, which this modal exists to avoid.
  let baseUrl = null;
  let baseTitle = null;
  const showUrl = (url) => {
    try { history.replaceState(history.state, '', url); } catch {}
  };

  const isModalPath = (href) => {
    try {
      const u = new URL(href, location.origin);
      return u.origin === location.origin && MODAL_PAGES.test(u.pathname);
    } catch { return false; }
  };

  // Re-create the page's scripts while `root` is still detached: connecting an element clears its
  // nonce attribute, which cloneScript checks. The copies run once `root` is inserted.
  const prepareScripts = (root, nonce) => {
    root.querySelectorAll('script').forEach((old) => {
      const s = F0ckActions.cloneScript(old, nonce);
      if (s) old.replaceWith(s);
      else old.remove();
    });
  };

  const load = async (url, title) => {
    const seq = ++loadSeq;
    currentUrl = url;
    showUrl(url);
    markActive();
    titleEl.textContent = title || '';
    body.innerHTML = '<div class="pgm-loading"><i class="fa-solid fa-spinner fa-spin"></i></div>';
    try {
      const res = await fetch(url, { credentials: 'same-origin' });
      const doc = new DOMParser().parseFromString(await res.text(), 'text/html');
      if (seq !== loadSeq) return;
      const main = doc.getElementById('main');
      if (!res.ok || !main) throw new Error(`HTTP ${res.status}`);

      const wrap = document.createElement('div');
      wrap.className = `${main.className} pgm-main`.trim();
      wrap.innerHTML = main.innerHTML;
      prepareScripts(wrap, F0ckActions.responseNonce(res));
      body.innerHTML = '';
      body.appendChild(wrap);
      body.scrollTop = 0;
      animate(wrap, [{ opacity: 0, transform: 'translateY(6px)' }, { opacity: 1, transform: 'none' }], { duration: 200, easing: 'cubic-bezier(0.2, 0, 0, 1)' });
      if (!title) {
        const h = wrap.querySelector('h1, h2, h3');
        titleEl.textContent = h ? h.textContent.trim() : '';
      }
      if (doc.title) document.title = doc.title;
    } catch (e) {
      if (seq !== loadSeq) return;
      body.innerHTML = `<div class="pgm-loading">Could not load this page. <a href="${url}" class="pgm-fallback">Open it directly</a>.</div>`;
    }
  };

  // ── Placement: the modal fills the content area only, so the navbar and sidebars stay usable ──
  const visibleRect = (sel) => {
    for (const el of document.querySelectorAll(sel)) {
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== 'hidden') return r;
    }
    return null;
  };
  // Top follows the navbar live (e.g. mobile burger menu collapsing). The sides follow the sidebars' *state*,
  // not their animated position, so the content never re-lays out while a sidebar slides: it changes width
  // once, when the toggle has finished, masked by a short fade (same as the main content).
  let lastSides = null;
  // Set by toggleSidebarRight at the moment the main content changes width (before the slide when
  // opening, after it when closing); wins over the body class, which only flips once the slide ends.
  let rightOverride = null;
  // Title bar of a modal page loaded as a full page (see "Full page" below); null when there is none
  let pageHeader = null;
  const pageHeaderShown = () => !!pageHeader && pageHeader.isConnected;
  // Something to keep in place: the open modal, or a full page's title bar
  const placing = () => modal.style.display !== 'none' || pageHeaderShown();
  const place = () => {
    const vw = window.innerWidth;
    const nav = visibleRect('body > nav.navbar, nav.navbar');
    const top = nav ? Math.max(0, Math.round(nav.bottom)) : 0;
    // Right sidebar: open unless body.sidebar-right-hidden; its layout width ignores the slide transform
    const rsEl = document.querySelector('.global-sidebar-right, .item-sidebar-right, .index-sidebar-right');
    const rsOpen = !!rsEl && !document.body.classList.contains('sidebar-right-hidden')
      && getComputedStyle(rsEl).display !== 'none' && rsEl.offsetWidth > 0;
    let right = rsOpen ? Math.min(Math.round(rsEl.offsetWidth), Math.round(vw / 2)) : 0;
    if (rightOverride && performance.now() < rightOverride.until) right = rightOverride.right;
    // Small phones: the sidebar is a pure overlay there, the content never makes room for it
    if (vw <= 599) right = 0;
    // Left (comments) sidebar on item pages
    const ls = visibleRect('.item-sidebar-left');
    const left = ls && ls.right > 1 && ls.right < vw / 2 ? Math.round(ls.right) : 0;
    // Desktop, sidebar beside the content: the header takes the exact height of the sidebar's tab row,
    // so the two form one continuous bar under the navbar
    const tabs = right > 0 && rsEl ? rsEl.querySelector('.sidebar-tabs') : null;
    const tabsH = tabs ? tabs.getBoundingClientRect().height : 0;
    for (const el of pageHeaderShown() ? [modal, pageHeader] : [modal]) {
      el.style.setProperty('--pgm-top', `${top}px`);
      if (tabsH > 0) {
        el.style.setProperty('--pgm-header-h', `${tabsH}px`);
        el.classList.add('pgm-header-synced');
      } else {
        el.style.removeProperty('--pgm-header-h');
        el.classList.remove('pgm-header-synced');
      }
    }
    if (pageHeaderShown()) {
      pageHeader.style.setProperty('--pgm-right', `${right}px`);
      pageHeader.style.setProperty('--pgm-left', `${left}px`);
      // The bar is fixed: the spacer keeps the page's content from starting underneath it
      const spacer = pageHeader.nextElementSibling;
      if (spacer && spacer.classList.contains('pgm-page-spacer')) spacer.style.height = `${Math.ceil(pageHeader.getBoundingClientRect().height)}px`;
    }
    const sides = `${left}|${right}`;
    if (sides !== lastSides) {
      const changed = lastSides !== null && modal.style.display !== 'none';
      lastSides = sides;
      modal.style.setProperty('--pgm-right', `${right}px`);
      modal.style.setProperty('--pgm-left', `${left}px`);
      if (changed && typeof body.animate === 'function') {
        body.animate([{ opacity: 0.55 }, { opacity: 1 }], { duration: 220, easing: 'ease-out' });
      }
    }
  };
  // Follow sidebar slide animations for a moment after a toggle
  let followUntil = 0;
  const follow = () => {
    if (!placing()) return;
    place();
    if (performance.now() < followUntil) requestAnimationFrame(follow);
  };
  const followFor = (ms) => {
    const running = performance.now() < followUntil;
    followUntil = performance.now() + ms;
    if (!running) requestAnimationFrame(follow);
  };
  window.addEventListener('resize', () => { if (placing()) place(); }, { passive: true });
  window.addEventListener('f0ck:sidebar-right-layout', (e) => {
    const d = e.detail || {};
    rightOverride = { right: d.open && !d.overlay ? Math.min(Math.round(d.width || 0), Math.round(window.innerWidth / 2)) : 0, until: performance.now() + 800 };
    if (placing()) place();
  });

  // Sidebar toggles commit their body class when the slide has finished: update once then. The left
  // (comments) sidebar may still be transitioning, so check again after it settles.
  new MutationObserver(() => {
    if (!placing()) return;
    place();
    setTimeout(place, 400);
  }).observe(document.body, { attributes: true, attributeFilter: ['class'] });

  // The navbar and sidebars change size on their own (mobile burger menu expanding/collapsing, sidebar resize):
  // follow every size change so the modal's edges stay glued to them
  const LAYOUT_SEL = 'nav.navbar, .global-sidebar-right, .item-sidebar-right, .index-sidebar-right, .item-sidebar-left';
  const watched = new WeakSet();
  const ro = typeof ResizeObserver === 'function'
    ? new ResizeObserver(() => { if (placing()) place(); })
    : null;
  const watchLayout = () => {
    document.querySelectorAll(LAYOUT_SEL).forEach((el) => {
      if (watched.has(el)) return;
      watched.add(el);
      if (ro) ro.observe(el);
      // Class/style toggles inside the navbar (collapse "show", inline heights) may animate: follow for a bit
      if (el.matches('nav.navbar')) {
        new MutationObserver(() => { if (placing()) followFor(600); })
          .observe(el, { attributes: true, attributeFilter: ['class', 'style'], subtree: true });
      }
    });
  };

  const open = (url, title) => {
    if (!isOpen || baseUrl === null) {
      baseUrl = location.pathname + location.search + location.hash;
      baseTitle = document.title;
      // The address bar shows the modal page, so a reload loses the page underneath: remember it for
      // the close button of the reloaded page (settings.html)
      try { sessionStorage.setItem('f0ck_pgm_base', baseUrl); } catch {}
    }
    // Fade in on a fresh open, and when reopened while the close animation is still running
    const fadeIn = !isOpen;
    isOpen = true;
    // Item pages bring their own sidebars after AJAX navigation, so (re)attach observers on each open
    watchLayout();
    place();
    modal.style.display = 'flex';
    // No visible scrollbar: focus the body so arrow keys / PageUp / PageDown / Space scroll it right away
    try { body.focus({ preventScroll: true }); } catch {}
    followFor(300);
    if (fadeIn) {
      // Mid-close: continue from where the fade-out is instead of flashing back to full opacity
      const from = fx ? parseFloat(getComputedStyle(modal).opacity) || 0 : 0;
      if (fx) { fx.cancel(); fx = null; }
      fx = animate(modal, [{ opacity: from, transform: `translateY(${10 * (1 - from)}px)` }, { opacity: 1, transform: 'none' }], { duration: 220, easing: 'cubic-bezier(0.2, 0, 0, 1)' });
      if (fx) fx.onfinish = () => { fx = null; };
    }
    load(url, title);
  };

  // restoreUrl = false when history already moved (back/forward): the URL bar is correct then
  const close = (restoreUrl = true) => {
    if (!isOpen) return;
    isOpen = false;
    currentUrl = null;
    ++loadSeq; // drop a load still in flight so it can't rewrite the URL after closing
    if (restoreUrl && baseUrl !== null) {
      showUrl(baseUrl);
      if (baseTitle !== null) document.title = baseTitle;
    }
    baseUrl = baseTitle = null;
    try { sessionStorage.removeItem('f0ck_pgm_base'); } catch {}
    markActive();

    const hide = () => {
      fx = null;
      if (isOpen) return; // reopened meanwhile
      modal.style.display = 'none';
      lastSides = null;
      body.innerHTML = '';
    };
    if (fx) { fx.cancel(); fx = null; }
    const out = animate(modal, [{ opacity: 1, transform: 'none' }, { opacity: 0, transform: 'translateY(10px)' }], { duration: 160, easing: 'cubic-bezier(0.4, 0, 1, 1)', fill: 'forwards' });
    if (!out) { hide(); return; }
    fx = out;
    out.onfinish = () => { if (fx === out) { out.cancel(); hide(); } };
  };

  // Capture phase: runs before the global AJAX link handler
  document.addEventListener('click', (e) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = e.target.closest?.('a[href]');
    if (!a) return;

    if (a.hasAttribute('data-page-modal')) {
      e.preventDefault();
      e.stopPropagation();
      // Already on that page (e.g. the gear on /settings): its entry is highlighted, never open the same page twice
      if (!isOpen && pathOf(a.getAttribute('href')) === pathOf(location.pathname)) return;
      // Same page already open: the entry works as a toggle
      if (isOpen && currentUrl && pathOf(currentUrl) === pathOf(a.getAttribute('href'))) {
        close();
        return;
      }
      // Mobile: the sidebar overlays the content, so close it to bring the opened page into focus
      const fromSidebar = a.closest('.global-sidebar-right, .item-sidebar-right, .index-sidebar-right');
      if (fromSidebar && window.matchMedia('(max-width: 999px)').matches
          && !document.body.classList.contains('sidebar-right-hidden')
          && typeof window.toggleSidebarRight === 'function') {
        window.toggleSidebarRight();
      }
      open(a.getAttribute('href'), a.getAttribute('title') || '');
      return;
    }

    if (!isOpen || a.target === '_blank') return;

    // Outside the modal (navbar brand, sidebar links, …): a real navigation closes it so the new page shows
    if (!modal.contains(a)) {
      const href = a.getAttribute('href') || '';
      if (href.startsWith('#') || href.startsWith('javascript:')) return;
      try { if (new URL(a.href, location.origin).origin !== location.origin) return; } catch { return; }
      close();
      return;
    }

    // In-page anchors (#section, e.g. the settings quicknav) belong to the loaded page's own scripts:
    // never reload the page for them
    const rawHref = a.getAttribute('href') || '';
    if (rawHref.startsWith('#')) return;

    if (isModalPath(a.href)) {
      e.preventDefault();
      e.stopPropagation();
      load(a.getAttribute('href'), '');
      return;
    }
    // Leaving the modal for a regular page: close it and let normal navigation take over
    close();
  }, true);

  modal.querySelector('.pgm-close').addEventListener('click', () => close());
  // Back/forward changes the page underneath: don't leave the modal covering it
  // ── Full page: a modal page loaded as a page (direct visit, or F5 while it was open in the modal) ──
  // It gets the modal's title bar, fixed over the content area like the modal itself (place() keeps it
  // there). Close returns to the page the modal was opened over, else home. /settings brings its own
  // bar, since its quicknav is fixed under the navbar too (settings.html).
  const titleFor = (path) => {
    const link = [...document.querySelectorAll('a[data-page-modal][title]')].find((l) => pathOf(l.getAttribute('href')) === path);
    return link?.getAttribute('title') || document.title.split(/\s+[-|–]\s+/)[0] || path.slice(1);
  };
  const leavePage = () => {
    let back = null;
    try { back = sessionStorage.getItem('f0ck_pgm_base'); sessionStorage.removeItem('f0ck_pgm_base'); } catch {}
    // Paths on this site only, and never the page itself
    if (!back || back[0] !== '/' || back[1] === '/' || back[1] === '\\' || pathOf(back) === pathOf(location.pathname)) back = '/';
    location.assign(back);
  };
  const syncPageHeader = () => {
    if (isOpen) return; // the modal shows its own URL in the address bar meanwhile
    const main = document.getElementById('main');
    const path = pathOf(location.pathname);
    const want = !!main && MODAL_PAGES.test(location.pathname) && path !== '/settings';
    if (want && pageHeaderShown() && pageHeader.parentNode === main && pageHeader.dataset.path === path) return;
    // Gone or stale (AJAX navigation replaces #main's content, which takes the bar with it)
    if (pageHeader) {
      const spacer = pageHeader.nextElementSibling;
      if (spacer && spacer.classList.contains('pgm-page-spacer')) spacer.remove();
      pageHeader.remove();
      pageHeader = null;
    }
    // Copies restored with a cached #main have no click handler: drop them too
    main?.querySelectorAll(':scope > #pgm-page-header, :scope > .pgm-page-spacer').forEach((el) => el.remove());
    if (!want) return;
    const bar = document.createElement('div');
    bar.id = 'pgm-page-header';
    bar.className = 'pgm-header';
    bar.dataset.path = path;
    bar.innerHTML = '<h3 class="pgm-title"></h3><div class="pgm-actions"><button type="button" class="pgm-close" title="Close" aria-label="Close">&times;</button></div>';
    bar.querySelector('.pgm-title').textContent = titleFor(path);
    bar.querySelector('.pgm-close').addEventListener('click', leavePage);
    const spacer = document.createElement('div');
    spacer.className = 'pgm-page-spacer';
    main.prepend(bar, spacer);
    pageHeader = bar;
    watchLayout();
    place();
  };
  const mainEl = document.getElementById('main');
  if (mainEl) new MutationObserver(syncPageHeader).observe(mainEl, { childList: true });

  window.addEventListener('popstate', () => { if (isOpen) close(false); else { markActive(); syncPageHeader(); } });
  // AJAX navigation (push/replaceState) can enter or leave a modal page: keep the highlight in step
  for (const fn of ['pushState', 'replaceState']) {
    const orig = history[fn];
    history[fn] = function (...args) {
      const r = orig.apply(this, args);
      if (!isOpen) { markActive(); syncPageHeader(); }
      return r;
    };
  }
  markActive();
  syncPageHeader();
  // Capture phase + stop: the page sits on top (e.g. over the Onara viewer), so Escape closes only it
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || !isOpen) return;
    e.stopImmediatePropagation();
    close();
  }, true);

  // Shift+S toggles the settings (signed-in users: a settings link is on the page). Not while typing,
  // not on /settings itself. Plain "s" stays Flash Yank.
  document.addEventListener('keydown', (e) => {
    if (!e.shiftKey || e.ctrlKey || e.altKey || e.metaKey || e.repeat) return;
    if (e.key !== 'S' && e.key !== 's') return;
    const t = e.target;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable)) return;
    if (document.body.classList.contains('scroller-active')) return;
    const link = document.querySelector('a[href="/settings"][data-page-modal]');
    if (!link || pathOf(location.pathname) === '/settings' && !isOpen) return;
    e.preventDefault();
    if (isOpen && currentUrl && pathOf(currentUrl) === '/settings') close();
    else open('/settings', link.getAttribute('title') || 'Settings');
  });

  window.f0ckPageModal = { open, close, get url() { return currentUrl; } };
})();
