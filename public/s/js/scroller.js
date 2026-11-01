/**
 * scroller.js — Doomscroll controller
 *
 * Fixes in this version:
 *  1. Infinite scroll never stops — random order always retries, fallback
 *     scroll listener if IntersectionObserver misses the sentinel.
 *  2. Volume slider with localStorage persistence.
 *  3. Comment input: emoji bar + @mention autocomplete.
 *  4. Double-tap / double-click to toggle favourite (POST /api/v2/togglefav).
 *  5. URL hash sync + sessionStorage back-navigation restore.
 */
(function () {
  'use strict';

  // ── PJAX guard ─────────────────────────────────────────────────────────────
  // When navigated to via PJAX, scroller.js re-executes. Prevent the stale
  // closure from the previous instance from acting on the NEW feed element
  // by tagging each feed element on first bind.
  const initFeed = document.getElementById('scroller-feed');
  if (!initFeed) return; // safety: no feed = wrong page
  if (initFeed.__scrollerBound) return; // already bound to THIS feed element
  initFeed.__scrollerBound = true;

  // "Adjust filters" button in the empty-feed message
  F0ckActions.define('scroller-open-filters', () => { document.getElementById('filter-open-btn')?.click(); });

  // ── DOM ─────────────────────────────────────────────────────────────────
  const feed             = initFeed;
  const sentinel         = document.getElementById('scroller-sentinel');
  const emptyEl          = document.getElementById('scroller-empty');
  const muteBtn          = document.getElementById('scroller-mute-btn');
  const volumePopup      = document.getElementById('volume-popup');
  const volumeSlider     = document.getElementById('volume-slider');
  const volumePct        = document.getElementById('volume-pct');
  const loader           = document.getElementById('scroller-loader');
  const filterOpenBtn    = document.getElementById('filter-open-btn');
  const filterPanel      = document.getElementById('filter-panel');
  const filterBackdrop   = document.getElementById('filter-backdrop');
  const filterResetBtn   = document.getElementById('filter-reset-btn');
  const filterApplyBtn   = document.getElementById('filter-apply-btn');
  const filterSummary    = document.getElementById('filter-active-summary');
  let filterSummaryText  = document.getElementById('filter-active-summary-text');
  let filterClearBtn     = document.getElementById('filter-active-clear');
  const tagInput         = document.getElementById('filter-tag-input');
  const tagClear         = document.getElementById('filter-tag-clear');
  const tagSuggestEl     = document.getElementById('tag-suggestions');
  const activeTagsEl     = document.getElementById('filter-active-tags');
  const commentsPanel    = document.getElementById('comments-panel');
  const commentsBackdrop = document.getElementById('comments-backdrop');
  const commentsList     = document.getElementById('comments-list');
  const commentsCount    = document.getElementById('comments-count');
  const commentsOpenLink = document.getElementById('comments-open-link');
  const commentsLoading  = document.getElementById('comments-loading');
  const commentsEmpty    = document.getElementById('comments-empty');
  const commentInput     = document.getElementById('comment-input');
  const commentSendBtn   = document.getElementById('comment-send-btn');
  const mentionDropdown  = document.getElementById('mention-dropdown');
  const settingsOpenBtn  = document.getElementById('settings-open-btn');
  const settingsPanel    = document.getElementById('settings-panel');
  const settingsBackdrop = document.getElementById('settings-backdrop');
  const stHideUi         = document.getElementById('st-hide-ui');
  const stStartUnmuted   = document.getElementById('st-start-unmuted');
  const stCanvasBg       = document.getElementById('st-canvas-bg');
  const stLeftHand       = document.getElementById('st-left-hand');
  const stAutoNext       = document.getElementById('st-auto-next');
  const autoNextLoopsInp = document.getElementById('st-auto-next-loops');
  const settingsSaveBtn  = document.getElementById('settings-save-preset-btn');
  const settingsPresetsList = document.getElementById('settings-presets-list');
  const externalUrlInput = document.getElementById('external-url-input');
  const externalUrlBtn   = document.getElementById('external-url-btn');
  const chanOpenBtn      = document.getElementById('chan-open-btn');
  const chanPanel        = document.getElementById('chan-panel');
  const chanBackdrop     = document.getElementById('chan-backdrop');
  const chanUrlInput     = document.getElementById('chan-url-input');
  const chanUrlLoadBtn   = document.getElementById('chan-url-load-btn');
  const chanGrid         = document.getElementById('chan-catalog-grid');
  const chanLoading      = document.getElementById('chan-catalog-loading');
  const chanGalleryBtn   = document.getElementById('chan-gallery-btn');
  const chanGallerySidebar = document.getElementById('chan-gallery-sidebar');

  // ── State ────────────────────────────────────────────────────────────────
  const defaultMode = window.scrollerMode ?? 0;
  const CACHE_KEY   = 'scroller_state';
  const CACHE_MAX   = 200;
  const CACHE_TTL   = 30 * 60 * 1000;
  const VOL_KEY     = 'scroller_volume';
  const PREFS_KEY      = 'scroller_prefs';
  const PRESETS_KEY    = 'scroller_presets';

  // ── User Preferences ──────────────────────────────────────────────────────
  function loadPrefs() {
    try { return JSON.parse(localStorage.getItem(PREFS_KEY) || '{}'); } catch { return {}; }
  }
  function savePrefs(p) { localStorage.setItem(PREFS_KEY, JSON.stringify(p)); }
  const prefs = loadPrefs();
  // canvasBg: true by default (animated background enabled)
  let canvasBgEnabled = prefs.canvasBg !== false;
  // hideUI: false by default
  let hideUIEnabled   = prefs.hideUI === true;
  // autoNext: false by default
  let autoNextEnabled = prefs.autoNext === true;
  let autoNextLoops   = Math.max(0, parseInt(prefs.autoNextLoops ?? 1, 10));
  let leftHandEnabled = prefs.leftHand === true;

  const getIsScrollerGuest = () => {
    if (!window.f0ckSession) return true;
    if (window.f0ckSession.user && !window.f0ckSession.is_anon) return false;
    if (window.f0ckSession.is_anon && (window.f0ckSession.logged_in || window.f0ckSession.user)) return false;
    if (window.f0ckAnonSSH && (window.f0ckAnonSSH.isSessionReady || window.f0ckAnonSSH.pubkey)) return false;
    if (typeof localStorage !== 'undefined' && localStorage.getItem('f0ck_anon_ssh_pub')) return false;
    return !window.f0ckSession.logged_in;
  };
  const isScrollerGuest = getIsScrollerGuest();
  const isScrollerAnon = !(window.f0ckSession && window.f0ckSession.user && !window.f0ckSession.is_anon);
  const scrollerAllowedMimes = window.f0ckSession?.anon_permissions?.allowed_mimes;
  const scrollerSingleMime = (isScrollerAnon && Array.isArray(scrollerAllowedMimes) && scrollerAllowedMimes.length === 1) ? scrollerAllowedMimes[0] : null;

  const scrollerAllowedModes = isScrollerGuest ? ['sfw'] : window.f0ckSession?.anon_permissions?.allowed_modes;
  const scrollerModeNames = ['sfw', 'nsfw', 'untagged', 'all', 'nsfl'];
  let effectiveScrollerMode = defaultMode;
  if (isScrollerGuest) {
    effectiveScrollerMode = 0;
  } else if (isScrollerAnon && Array.isArray(scrollerAllowedModes)) {
    const curName = scrollerModeNames[effectiveScrollerMode] || 'sfw';
    if (!scrollerAllowedModes.includes(curName)) {
      const fallbackName = scrollerAllowedModes[0] || 'sfw';
      const fallbackIdx = scrollerModeNames.indexOf(fallbackName);
      effectiveScrollerMode = fallbackIdx >= 0 ? fallbackIdx : 0;
    }
  }

  let applied   = { mode: effectiveScrollerMode, mime: scrollerSingleMime || '', order: 'random', tags: [], externalUrl: null };
  let pending   = { ...applied, tags: [] };

  // Volume / mute
  let isMuted   = prefs.startUnmuted !== true; // default muted unless user opted for sound
  let volume    = parseFloat(localStorage.getItem(VOL_KEY) ?? '1');
  if (isNaN(volume) || volume < 0 || volume > 1) volume = 1;

  let isFetching     = false;
  let hasMore        = true;
  let consecutiveEmpty = 0;     // how many empty fetches in a row (caps random infinite scroll)
  let lastCursor     = null;
  let seenIds        = new Set(); // tracks all seen item IDs to prevent random-mode repeats
  let currentSlide   = null;
  let currentMedia   = null;

  // Comments
  let commentsItemId  = null;
  let tagBarItemId    = null;
  let commentsPosting = false;

  // Mention autocomplete
  let mentionQuery     = '';
  let mentionTimer     = null;
  let mentionResults   = [];
  let mentionIndex     = -1;

  // Tag autocomplete
  let tagTimer   = null;
  let tagCache   = {};
  let lastSugg   = [];
  let tagSuggIdx = -1;  // keyboard nav index for filter-panel tag suggestions

  // Volume popup
  let volPopupTimer = null;

  // Emoji autocomplete
  let emojiMode    = false;   // true when showing emoji suggestions, false for mentions
  let emojiResults = [];      // current emoji suggestion list [[code, char], ...]
  let acIndex      = -1;      // shared keyboard nav index


  // Session cache helpers ──────────────────────────────────────────────
  function readCache() {
    try {
      const c = JSON.parse(sessionStorage.getItem(CACHE_KEY) || 'null');
      if (!c || !c.ts || Date.now() - c.ts > CACHE_TTL) { clearCache(); return null; }
      return c;
    } catch { return null; }
  }
  function clearCache() { try { sessionStorage.removeItem(CACHE_KEY); } catch {} }
  function appendToCache(items) {
    try {
      const existing = readCache();
      const existingItems = existing?.items || [];
      const ids = new Set(existingItems.map(i => i.id));
      const merged = [...existingItems, ...items.filter(i => !ids.has(i.id))].slice(-CACHE_MAX);
      sessionStorage.setItem(CACHE_KEY, JSON.stringify({
        ts: Date.now(), filters: { ...applied }, items: merged,
        activeId: currentSlide ? currentSlide.dataset.id : (merged[0]?.id ?? null)
      }));
    } catch { clearCache(); }
  }
  function updateCacheActiveId(id) {
    try {
      const raw = sessionStorage.getItem(CACHE_KEY);
      if (!raw) return;
      const c = JSON.parse(raw); c.activeId = String(id); c.ts = Date.now();
      sessionStorage.setItem(CACHE_KEY, JSON.stringify(c));
    } catch {}
  }

  function tryRestoreFromCache() {
    const cache = readCache();
    if (!cache?.items?.length) return false;

    // If there's a hash ID (e.g. /abyss#1234 shared via DM), check whether
    // that item is actually in the cache. If it isn't, skip the cache restore
    // entirely so that fetchItems() runs with ?anchor=<id> and the correct
    // item is guaranteed to be included in the fresh batch.
    const hid = hashId();

    // If hash is board/postid (e.g. wsg/6132740), auto-load that 4chan thread
    const chanHash = hid.match(/^([a-z0-9]+)\/\d+$/);
    if (chanHash) {
      const board = chanHash[1];
      // We need to find the thread ID from the post. Use the board catalog or
      // just load a search — but simplest: set externalUrl and skip cache restore.
      // The thread ID is unknown from just the post ID, so we skip cache and
      // let the init block below handle it.
      return false;
    }
    if (hid && !cache.items.some(item => String(item.id) === hid)) return false;

    if (cache.filters) {
      applied = { mode: defaultMode, mime: scrollerSingleMime || '', order: 'random', tags: [], ...cache.filters };
      if (scrollerSingleMime) applied.mime = scrollerSingleMime;
      applied.tags = Array.isArray(cache.filters.tags) ? [...cache.filters.tags] : [];
      pending = { ...applied, tags: [...applied.tags] };
    }
    cache.items.forEach(item => feed.insertBefore(createSlide(item), sentinel));
    const targetId = hid || cache.activeId;
    const target = targetId ? feed.querySelector(`.scroll-slide[data-id="${targetId}"]`) : null;
    const toActivate = target || feed.querySelector('.scroll-slide');
    if (toActivate) setTimeout(() => { toActivate.scrollIntoView({ behavior: 'instant', block: 'start' }); activateSlide(toActivate); hideLoader(); updateFilterSummary(); syncPanelUI(); }, 50);
    return true;
  }

  // ── Helpers ───────────────────────────────────────────────────────────────
  function esc(str) {
    if (!str) return '';
    return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }
  function decodeHtmlEntities(str) {
    if (!str) return '';
    const txt = document.createElement('textarea');
    txt.innerHTML = String(str);
    return txt.value;
  }
  function timeAgo(iso) {
    const s = Math.floor((Date.now() - new Date(iso)) / 1000);
    const i = window.f0ckI18n || {};
    const fmt = (tpl, n, unit) => (tpl || `{n} ${unit}${n !== 1 ? 's' : ''}`).replace('{n}', n);
    const ago = (t) => (i.ta_ago || '{t} ago').replace('{t}', t);
    if (s < 60) return i.ta_just_now || 'just now';
    const m = Math.floor(s / 60);
    if (m < 60) return ago(fmt(m === 1 ? i.ta_minute : i.ta_minutes, m, 'minute'));
    const h = Math.floor(s / 3600);
    if (h < 24) return ago(fmt(h === 1 ? i.ta_hour : i.ta_hours, h, 'hour'));
    const d = Math.floor(s / 86400);
    if (d < 7) return ago(fmt(d === 1 ? i.ta_day : i.ta_days, d, 'day'));
    const w = Math.floor(d / 7);
    if (d < 30) return ago(fmt(w === 1 ? i.ta_week : i.ta_weeks, w, 'week'));
    const mo = Math.floor(d / 30);
    if (d < 365) return ago(fmt(mo === 1 ? i.ta_month : i.ta_months, mo, 'month'));
    const y = Math.floor(d / 365);
    return ago(fmt(y === 1 ? i.ta_year : i.ta_years, y, 'year'));
  }
  function hashId() {
    // Check path first /abyss/1234 or /abyss/gif/1234
    const pathClean = location.pathname.replace(/\/$/, '');
    const pathMatch = pathClean.match(/\/abyss\/([a-zA-Z0-9_\/-]+)$/);
    if (pathMatch) return pathMatch[1];

    // Fallback to hash
    const raw = location.hash.replace(/^#/, '').trim();
    if (/^\d+$/.test(raw)) return raw;
    if (/^[a-z0-9]+\/\d+$/.test(raw)) return raw;
    return '';
  }
  let lastPushedUrl = location.pathname + location.hash;
  function pushHash(id) {
    if (!id) return;
    const newUrl = '/abyss/' + id;
    if (newUrl === lastPushedUrl) return;
    lastPushedUrl = newUrl;
    history.pushState({ scrollerId: id }, '', newUrl);
    updateCacheActiveId(id);
  }

  // Handle back/forward within abyss — scroll to the target slide
  window.addEventListener('popstate', (e) => {
    if (!document.body.classList.contains('scroller-active')) return;
    const id = e.state?.scrollerId || hashId();
    if (!id) return;
    lastPushedUrl = '/abyss/' + id;
    const slide = feed.querySelector(`.scroll-slide[data-id="${id}"], .scroll-slide[data-local-id="${id}"]`);
    if (slide) {
      slide.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
  });


  // ── Volume / Mute ────────────────────────────────────────────────────────
  function syncVolumeUI() {
    const icon = muteBtn.querySelector('i');
    if (isMuted || volume === 0) {
      icon.className = 'fa-solid fa-volume-xmark';
    } else if (volume < 0.4) {
      icon.className = 'fa-solid fa-volume-low';
    } else {
      icon.className = 'fa-solid fa-volume-high';
    }
    volumeSlider.value = isMuted ? 0 : volume;
    volumePct.textContent = isMuted ? '0%' : Math.round(volume * 100) + '%';
  }
  function applyVolumeToAll() {
    feed.querySelectorAll('video, audio').forEach(el => {
      el.muted = isMuted;
      el.volume = volume;
    });
    feed.querySelectorAll('.ruffle-container').forEach(el => {
      if (el._rufflePlayer) {
        el._rufflePlayer.volume = isMuted ? 0 : volume;
      }
    });
  }
  function saveVolume() { localStorage.setItem(VOL_KEY, String(volume)); }

  // Toggle mute on click; show popup; long-hold => just popup
  muteBtn.addEventListener('click', () => {
    if (isMuted) {
      isMuted = false;
      if (volume === 0) volume = 0.5;
    } else {
      isMuted = true;
    }
    syncVolumeUI(); applyVolumeToAll(); saveVolume();
    // Persist mute state so it's restored on next visit
    prefs.startUnmuted = !isMuted; savePrefs(prefs);
    applyStartUnmuted(!isMuted);
    showVolumePopup();
  });

  function showVolumePopup() {
    volumePopup.classList.add('open');
    clearTimeout(volPopupTimer);
    volPopupTimer = setTimeout(() => volumePopup.classList.remove('open'), 3000);
  }

  volumeSlider.addEventListener('input', () => {
    volume = parseFloat(volumeSlider.value);
    isMuted = volume === 0;
    syncVolumeUI(); applyVolumeToAll(); saveVolume();
    // Persist mute state
    prefs.startUnmuted = !isMuted; savePrefs(prefs);
    applyStartUnmuted(!isMuted);
    clearTimeout(volPopupTimer);
    volPopupTimer = setTimeout(() => volumePopup.classList.remove('open'), 2500);
  });

  // Open popup on hover (mouseenter); close on mouseleave with a short grace period
  // so the cursor can migrate from the button to the popup without it snapping shut.
  let volHoverTimer = null;
  const openVolPopupHover = () => {
    clearTimeout(volHoverTimer);
    clearTimeout(volPopupTimer);
    volumePopup.classList.add('open');
  };
  const closeVolPopupHover = () => {
    clearTimeout(volHoverTimer);
    volHoverTimer = setTimeout(() => volumePopup.classList.remove('open'), 200);
  };
  muteBtn.addEventListener('mouseenter', openVolPopupHover);
  muteBtn.addEventListener('mouseleave', closeVolPopupHover);
  volumePopup.addEventListener('mouseenter', () => clearTimeout(volHoverTimer));
  volumePopup.addEventListener('mouseleave', closeVolPopupHover);

  // Close popup if click outside
  document.addEventListener('click', e => {
    if (!document.body.classList.contains('scroller-active')) return;
    if (!volumePopup.contains(e.target) && e.target !== muteBtn && !muteBtn.contains(e.target)) {
      volumePopup.classList.remove('open');
    }
  }, true);

  // ── Panels ───────────────────────────────────────────────────────────────
  let panelOpen = false; // true while any sheet panel is open

  function openPanel(panel, backdrop) {
    closeAllPanels();
    panel.classList.add('open'); backdrop.classList.add('open');
    document.body.style.overflow = 'hidden';
    panelOpen = true;
  }
  function closePanel(panel, backdrop) {
    panel.classList.remove('open'); backdrop.classList.remove('open');
    document.body.style.overflow = '';
    panelOpen = !!(filterPanel.classList.contains('open') || commentsPanel.classList.contains('open') || settingsPanel.classList.contains('open'));
    // Blur any focused input so keyboard shortcuts (arrows etc.) are immediately restored
    if (document.activeElement && (document.activeElement.tagName === 'INPUT' || document.activeElement.tagName === 'TEXTAREA')) {
      document.activeElement.blur();
    }
    // Capture currentSlide NOW — reloadFeed() (called from filterApply) will null it
    const snapTarget = currentSlide;
    if (snapTarget) {
      setTimeout(() => {
        snapTarget.scrollIntoView({ behavior: 'instant', block: 'start' });
      }, 250);
    }
  }
  function closeAllPanels() {
    closePanel(filterPanel, filterBackdrop);
    closePanel(commentsPanel, commentsBackdrop);
    closePanel(settingsPanel, settingsBackdrop);
    if (typeof closeTagBar === 'function') closeTagBar();
    if (typeof closeSharePanel === 'function') closeSharePanel();
    if (typeof closeChanPanel === 'function') closeChanPanel();
    const active = document.activeElement;
    if (active && (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA')) active.blur();
  }
  function addSwipeClose(panel, backdrop) {
    let startY = 0;
    panel.addEventListener('touchstart', e => { startY = e.touches[0].clientY; }, { passive: true });
    panel.addEventListener('touchend', e => { if (e.changedTouches[0].clientY - startY > 60) closePanel(panel, backdrop); }, { passive: true });
  }
  addSwipeClose(filterPanel, filterBackdrop);
  addSwipeClose(commentsPanel, commentsBackdrop);
  addSwipeClose(settingsPanel, settingsBackdrop);
  filterBackdrop.addEventListener('click', () => closePanel(filterPanel, filterBackdrop));
  commentsBackdrop.addEventListener('click', () => closePanel(commentsPanel, commentsBackdrop));
  settingsBackdrop.addEventListener('click', () => closePanel(settingsPanel, settingsBackdrop));

  // ── Settings panel logic ──────────────────────────────────────────────────
  function applyHideUI(val) {
    hideUIEnabled = val;
    document.body.classList.toggle('ui-hidden', val);
    if (stHideUi) stHideUi.classList.toggle('on', val);
  }
  function applyStartUnmuted(val) {
    if (stStartUnmuted) stStartUnmuted.classList.toggle('on', val);
  }
  function applyCanvasBg(val) {
    canvasBgEnabled = val;
    if (stCanvasBg) stCanvasBg.classList.toggle('on', val);
    // If turning off on a currently active slide, cancel the rAF loop and show thumbnail
    if (!val && currentSlide) {
      if (currentSlide._bgRaf) { cancelAnimationFrame(currentSlide._bgRaf); currentSlide._bgRaf = null; }
      const canvas = currentSlide.querySelector('canvas.scroll-bg-blur');
      const video  = currentSlide.querySelector('video');
      if (canvas && !video && currentSlide._thumbnail) {
        const img = new Image(); img.onload = () => {
          const ctx = canvas.getContext('2d');
          if (ctx) ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
        }; img.src = currentSlide._thumbnail;
      }
    }
  }
  // Apply prefs on load
  applyHideUI(hideUIEnabled);
  applyStartUnmuted(prefs.startUnmuted === true);
  applyCanvasBg(canvasBgEnabled);

  // Left-hand mode
  function applyLeftHand(val) {
    leftHandEnabled = val;
    document.body.classList.toggle('left-hand-mode', val);
    if (stLeftHand) stLeftHand.classList.toggle('on', val);
    // Inject / remove 4chan action buttons in every slide's scroll-actions
    document.querySelectorAll('.scroll-actions .chan-action-btn').forEach(el => el.remove());
    if (val) {
      document.querySelectorAll('.scroll-actions').forEach(actions => {
        // Only add if 4chan buttons are visible (a 4chan feed is active)
        if (chanOpenBtn && chanOpenBtn.style.display === 'flex') {
          const openBtn = document.createElement('button');
          openBtn.className = 'chan-action-btn';
          openBtn.title = chanOpenBtn.title;
          openBtn.innerHTML = `<div class="scroll-btn-icon"><i class="fa-solid fa-clover" style="color:#789922"></i></div>`;
          openBtn.addEventListener('click', () => chanOpenBtn.click());
          actions.appendChild(openBtn);
        }
        if (chanGalleryBtn && chanGalleryBtn.style.display === 'flex') {
          const galBtn = document.createElement('button');
          galBtn.className = 'chan-action-btn';
          galBtn.title = chanGalleryBtn.title;
          galBtn.innerHTML = `<div class="scroll-btn-icon"><i class="fa-solid fa-grip"></i></div>`;
          galBtn.addEventListener('click', () => chanGalleryBtn.click());
          actions.appendChild(galBtn);
        }
      });
    }
  }
  applyLeftHand(leftHandEnabled);

  // Toggle clicks
  if (stHideUi) stHideUi.addEventListener('click', () => {
    const next = !hideUIEnabled;
    prefs.hideUI = next; savePrefs(prefs);
    applyHideUI(next);
  });
  if (stStartUnmuted) stStartUnmuted.addEventListener('click', () => {
    const next = !(prefs.startUnmuted === true);
    prefs.startUnmuted = next; savePrefs(prefs);
    // Also apply to current session immediately
    isMuted = !next;
    if (!isMuted && volume === 0) volume = 0.5;
    syncVolumeUI(); applyVolumeToAll(); saveVolume();
    applyStartUnmuted(next);
  });
  if (stCanvasBg) stCanvasBg.addEventListener('click', () => {
    const next = !canvasBgEnabled;
    prefs.canvasBg = next; savePrefs(prefs);
    applyCanvasBg(next);
  });
  if (stLeftHand) stLeftHand.addEventListener('click', () => {
    const next = !leftHandEnabled;
    prefs.leftHand = next; savePrefs(prefs);
    applyLeftHand(next);
  });
  // Auto-next toggle
  function applyAutoNext(val) {
    autoNextEnabled = val;
    if (stAutoNext) stAutoNext.classList.toggle('on', val);
    if (autoNextLoopsInp) autoNextLoopsInp.disabled = !val;
  }
  applyAutoNext(autoNextEnabled);
  if (autoNextLoopsInp) {
    autoNextLoopsInp.value = autoNextLoops;
    autoNextLoopsInp.addEventListener('change', () => {
      const v = Math.max(0, Math.min(99, parseInt(autoNextLoopsInp.value, 10)));
      autoNextLoops = isNaN(v) ? 1 : v; autoNextLoopsInp.value = autoNextLoops;
      prefs.autoNextLoops = v; savePrefs(prefs);
    });
  }
  if (stAutoNext) stAutoNext.addEventListener('click', () => {
    const next = !autoNextEnabled;
    prefs.autoNext = next; savePrefs(prefs);
    applyAutoNext(next);
  });
  // Helper: advance to next slide
  let autoNextCooldown = false;
  function hideLoader() {
    loader.classList.add('hidden');
    // After the 0.3s CSS opacity transition, pull it fully out of the touch layer
    // (iOS Safari can still swallow scroll events through a pointer-events:none fixed overlay)
    setTimeout(() => { loader.style.display = 'none'; }, 350);
  }
  function goNextSlide() {
    if (!currentSlide || autoNextCooldown) return;
    autoNextCooldown = true;
    // 1.5s grace: prevents chaining on very short videos; user can swipe back during this window.
    setTimeout(() => { autoNextCooldown = false; }, 1500);
    const slides = [...feed.querySelectorAll('.scroll-slide')];
    const idx = slides.indexOf(currentSlide);
    if (idx < 0 || idx + 1 >= slides.length) return;
    // Use index-based scroll position: each slide is exactly feed.clientHeight tall.
    // Avoids the offsetTop-vs-scroll-container mismatch that can land between snap points.
    const targetScrollTop = (idx + 1) * feed.clientHeight;
    setTimeout(() => { feed.scrollTop = targetScrollTop; }, 50);
  }

  // ── Filter Presets CRUD ───────────────────────────────────────────────────
  const PRESETS_LABELS = { mode: { 0: 'SFW', 1: 'NSFW', 2: 'Unrated', 3: 'All', 4: 'NSFL' } };
  function getPresets() { try { return JSON.parse(localStorage.getItem(PRESETS_KEY) || '[]'); } catch { return []; } }
  function savePresets(arr) { localStorage.setItem(PRESETS_KEY, JSON.stringify(arr)); }

  function renderPresets() {
    if (!settingsPresetsList) return;
    const list = getPresets();
    settingsPresetsList.innerHTML = '';
    if (!list.length) {
      settingsPresetsList.innerHTML = `<div style="color:rgba(255,255,255,.35);font-size:.79rem;padding:4px 0;">${(window.f0ckI18n && window.f0ckI18n.no_presets) || 'No saved presets yet.'}</div>`;
      return;
    }
    const buildMeta = (p) => [
      PRESETS_LABELS.mode[p.mode] ?? `Mode ${p.mode}`,
      p.mime || 'All types',
      p.order,
      ...(p.tags.length ? [`Tags: ${p.tags.slice(0, 3).join(', ')}${p.tags.length > 3 ? '\u2026' : ''}`] : [])
    ].join(' \u00b7 ');

    list.forEach((preset, i) => {
      const row = document.createElement('div');
      row.className = 'preset-row';

      const topRow = document.createElement('div');
      topRow.style.cssText = 'display:flex;align-items:center;gap:8px;width:100%;min-width:0;';
      topRow.innerHTML =
        '<div class="preset-row-info" style="flex:1;min-width:0;cursor:pointer;">' +
          '<div class="preset-row-name">' + esc(preset.name) + '</div>' +
          '<div class="preset-row-meta">' + esc(buildMeta(preset)) + '</div>' +
        '</div>' +
        '<button class="preset-row-edit" title="Edit tags"><i class="fa-solid fa-pen"></i></button>' +
        '<button class="preset-row-del" title="Delete preset"><i class="fa-solid fa-trash"></i></button>';
      row.appendChild(topRow);

      topRow.querySelector('.preset-row-info').addEventListener('click', () => {
        pending.mode  = preset.mode;
        pending.order = preset.order;
        pending.mime  = preset.mime;
        pending.tags  = [...preset.tags];
        closePanel(filterPanel, filterBackdrop);
        applied = { ...pending, tags: [...pending.tags] };
        reloadFeed(); updateFilterUI(); updateFilterSummary();
      });

      topRow.querySelector('.preset-row-del').addEventListener('click', e => {
        e.stopPropagation();
        savePresets(getPresets().filter((_, j) => j !== i));
        renderPresets();
      });

      const editor = document.createElement('div');
      editor.className = 'preset-editor';
      let editTags = [...preset.tags];

      const tagList = document.createElement('div');
      tagList.className = 'preset-editor-tags';

      // Helper: persist current editTags to storage and refresh the row meta immediately.
      // Uses preset.name as stable key (index i can drift if other presets are deleted).
      const persistTags = () => {
        const presets = getPresets();
        const idx = presets.findIndex(p => p.name === preset.name);
        if (idx === -1) { console.error('[presets] persistTags: preset not found:', preset.name); return; }
        presets[idx].tags = [...editTags];
        savePresets(presets);
        topRow.querySelector('.preset-row-meta').textContent = buildMeta(presets[idx]);
      };

      const renderEditorTags = () => {
        tagList.innerHTML = editTags.length
          ? editTags.map((t, ti) =>
              '<span class="preset-editor-tag">' + esc(t) +
              '<button class="preset-editor-tag-del" data-ti="' + ti + '" title="Remove">&times;</button></span>'
            ).join('')
          : '<span style="font-size:.72rem;color:rgba(255,255,255,.3)">No tags</span>';
        tagList.querySelectorAll('.preset-editor-tag-del').forEach(btn => {
          btn.addEventListener('click', ev => {
            ev.stopPropagation();
            editTags.splice(+btn.dataset.ti, 1);
            renderEditorTags();
            persistTags(); // auto-save on remove
          });
        });
      };

      const addWrap = document.createElement('div');
      addWrap.className = 'preset-editor-add';
      const tagInp = document.createElement('input');
      tagInp.type = 'text'; tagInp.placeholder = 'Add tag\u2026'; tagInp.autocomplete = 'off'; tagInp.spellcheck = false;
      const addBtnEl = document.createElement('button');
      addBtnEl.textContent = (window.f0ckI18n && window.f0ckI18n.add) || 'Add';
      addWrap.appendChild(tagInp); addWrap.appendChild(addBtnEl);
      const doAdd = () => {
        const v = tagInp.value.trim().toLowerCase().replace(/\s+/g, '_');
        if (v && !editTags.includes(v)) {
          editTags.push(v);
          renderEditorTags();
          persistTags(); // auto-save on add
        }
        tagInp.value = ''; tagInp.focus();
      };
      addBtnEl.addEventListener('click', ev => { ev.stopPropagation(); doAdd(); });
      tagInp.addEventListener('keydown', ev => { if (ev.key === 'Enter') { ev.preventDefault(); ev.stopPropagation(); doAdd(); } });

      const actions = document.createElement('div');
      actions.className = 'preset-editor-actions';

      const overwriteBtn = document.createElement('button');
      overwriteBtn.className = 'preset-editor-overwrite';
      overwriteBtn.innerHTML = `<i class="fa-solid fa-arrows-rotate" style="margin-right:6px"></i>${(window.f0ckI18n && window.f0ckI18n.update_preset) || 'Update & apply preset'} <span style="display:block;font-size:.68rem;opacity:.6;margin-top:2px">${(window.f0ckI18n && window.f0ckI18n.update_preset_sub) || 'Save changes and reload feed now'}</span>`;
      overwriteBtn.addEventListener('click', ev => {
        ev.stopPropagation();
        const presets = getPresets();
        const idx = presets.findIndex(p => p.name === preset.name);
        if (idx === -1) return;
        // Save: keep editTags, sync mode/order/mime from active filters
        presets[idx] = { ...presets[idx], mode: applied.mode, order: applied.order, mime: applied.mime, tags: [...editTags] };
        savePresets(presets);
        const saved = presets[idx];
        // Apply immediately — same as clicking the preset name
        applied = { mode: saved.mode, order: saved.order, mime: saved.mime, tags: [...saved.tags] };
        pending = { ...applied, tags: [...applied.tags] };
        closePanel(filterPanel, filterBackdrop);
        reloadFeed();
        updateFilterUI();
        updateFilterSummary();
        showShareToast('Preset updated & applied');
      });

      actions.appendChild(overwriteBtn);
      editor.appendChild(tagList); editor.appendChild(addWrap); editor.appendChild(actions);
      row.appendChild(editor);


      topRow.querySelector('.preset-row-edit').addEventListener('click', ev => {
        ev.stopPropagation();
        const opening = !editor.classList.contains('open');
        settingsPresetsList.querySelectorAll('.preset-editor.open').forEach(el => {
          el.classList.remove('open'); el.closest('.preset-row')?.classList.remove('editing');
        });
        if (opening) {
          const fresh = getPresets().find(p => p.name === preset.name);
          editTags = [...(fresh?.tags ?? preset.tags)];
          renderEditorTags(); editor.classList.add('open'); row.classList.add('editing');
          setTimeout(() => tagInp.focus(), 50);
        }
      });

      settingsPresetsList.appendChild(row);
    });
  }

  if (settingsSaveBtn) {
    settingsSaveBtn.addEventListener('click', () => {
      const name = prompt('Preset name:', `${PRESETS_LABELS.mode[applied.mode] ?? 'Mode'} / ${applied.order}`);
      if (!name || !name.trim()) return;
      const list = getPresets();
      list.push({ name: name.trim(), mode: applied.mode, order: applied.order, mime: applied.mime, tags: [...applied.tags] });
      savePresets(list);
      renderPresets();
      showShareToast('Preset saved');
    });
  }

  if (filterOpenBtn) {
    filterOpenBtn.addEventListener('click', () => {
      pending = { ...applied, tags: [...applied.tags] };
      syncPanelUI();
      renderPresets();
      openPanel(filterPanel, filterBackdrop);
    });
  }

  if (settingsOpenBtn) {
    settingsOpenBtn.addEventListener('click', () => {
      openPanel(settingsPanel, settingsBackdrop);
    });
  }

  // Spoiler/blur reveal & context links — delegated click on the comments list
  commentsList.addEventListener('click', e => {
    const sp = e.target.closest('.scroller-spoiler, .scroller-blur');
    if (sp) sp.classList.toggle('revealed');

    const contextLink = e.target.closest('.comment-context-link');
    if (contextLink) {
      e.preventDefault();
      const id = contextLink.dataset.id;
      const target = commentsList.querySelector(`.comment-item[data-comment-id="${id}"]`);
      if (target) {
        // Clear any previous persistent highlight
        commentsList.querySelectorAll('.comment-linked').forEach(el => el.classList.remove('comment-linked'));
        target.scrollIntoView({ behavior: 'smooth', block: 'center' });
        // Flash the animation for attention, then keep the persistent highlight
        target.classList.add('highlight-comment', 'comment-linked');
        setTimeout(() => target.classList.remove('highlight-comment'), 2500);
      }
    }
  });

  // ── Slide activation ──────────────────────────────────────────────────────
  function activateSlide(slide) {
    if (!slide || slide === currentSlide) return;
    if (currentSlide) deactivateSlide(currentSlide);
    currentSlide = slide;
    slide._deactivated = false;
    slide.classList.add('active');
    pushHash(slide.dataset.id);
    updateGalleryActive(slide.dataset.id);

    const video = slide.querySelector('video');
    const audio = slide.querySelector('audio');
    const cover = slide.querySelector('.audio-cover');
    const fill  = slide.querySelector('.scroll-progress-fill');
    const thumb = slide.querySelector('.scroll-progress-thumb');

    if (video) {
      currentMedia = video;
      // Restore src if we aborted loading when deactivating
      if (video._savedSrc && !video.src) {
        video.src = video._savedSrc;
        video.currentTime = video._savedTime || 0;
      } else {
        video.currentTime = 0;
      }
      video.muted = isMuted; video.volume = volume;
      video.classList.add('ready');
      // Force load if video hasn't started loading (preload=none for external)
      if (video.readyState === 0) {
        video.preload = 'auto';
        video.load();
      } else {
        video.currentTime = 0;
      }
      video.play().catch(err => {
        if (err && err.name === 'NotAllowedError') { video.muted = true; isMuted = true; syncVolumeUI(); video.play().catch(() => {}); }
      });
      wireProgress(slide, video, fill, thumb);
      // Start live canvas background loop (only if pref enabled)
      const bgC = slide.querySelector('canvas.scroll-bg-blur');
      if (bgC && canvasBgEnabled) {
        const bgX = bgC.getContext('2d');
        const rafLoop = () => {
          if (!video || video.paused || video.ended) { slide._bgRaf = null; return; }
          try { bgX.drawImage(video, 0, 0, bgC.width, bgC.height); } catch {}
          slide._bgRaf = requestAnimationFrame(rafLoop);
        };
        video.addEventListener('playing', () => {
          if (slide._bgRaf) cancelAnimationFrame(slide._bgRaf);
          rafLoop();
        });
      }
      // Auto-next: count loops and advance after N plays
      if (autoNextEnabled) {
        video.loop = false;
        slide._playCount = 0; // reset each time this slide is activated
        const onVideoEnded = () => {
          if (!autoNextEnabled || currentSlide !== slide) return;
          slide._playCount++;
          if (slide._playCount <= autoNextLoops) { video.currentTime = 0; video.play().catch(() => {}); }
          else { goNextSlide(); }
        };
        slide._autoNextMedia = video;
        slide._autoNextHandler = onVideoEnded;
        video.addEventListener('ended', onVideoEnded);
      } else {
        video.loop = true;
      }
    } else if (audio) {
      currentMedia = audio;
      if (audio._savedSrc && !audio.src) {
        audio.src = audio._savedSrc;
        audio.currentTime = audio._savedTime || 0;
      } else {
        audio.currentTime = 0;
      }
      audio.muted = isMuted; audio.volume = volume;
      audio.play().catch(err => {
        if (err && err.name === 'NotAllowedError') { audio.muted = true; isMuted = true; syncVolumeUI(); audio.play().catch(() => {}); }
      });
      if (cover) cover.classList.add('playing');
      wireProgress(slide, audio, fill, thumb);
      // Always ensure the canvas bg shows the thumbnail (onload may have raced ahead of activation)
      const audioBgC = slide.querySelector('canvas.scroll-bg-blur');
      if (audioBgC && slide._thumbnail) {
        const audioBgX = audioBgC.getContext('2d');
        if (audioBgX) {
          const t = new Image();
          t.onload = () => { try { audioBgX.drawImage(t, 0, 0, audioBgC.width, audioBgC.height); } catch {} };
          t.src = slide._thumbnail;
        }
      }
      // Auto-next for audio
      if (autoNextEnabled) {
        audio.loop = false;
        slide._playCount = 0; // reset each time this slide is activated
        const onAudioEnded = () => {
          if (!autoNextEnabled || currentSlide !== slide) return;
          slide._playCount++;
          if (slide._playCount <= autoNextLoops) { audio.currentTime = 0; audio.play().catch(() => {}); }
          else { goNextSlide(); }
        };
        slide._autoNextMedia = audio;
        slide._autoNextHandler = onAudioEnded;
        audio.addEventListener('ended', onAudioEnded);
      } else {
        audio.loop = true;
      }
    } else {
      // YouTube: inject src now so only the active slide autoplays
      const ytContainer = slide.querySelector('.yt-container');
      if (ytContainer) {
        const ytId = ytContainer.dataset.ytId;
        const iframe = ytContainer.querySelector('iframe');
        if (iframe && ytId && !iframe.src) {
          iframe.src = `https://www.youtube.com/embed/${ytId}?autoplay=1&loop=1&playlist=${ytId}`;
        }
      }
      // Flash (SWF) slide
      const ruffleEl = slide.querySelector('.ruffle-container');
      if (ruffleEl && window.scrollerEnableSwf) {
        if (ruffleEl._rufflePlayer) {
          // Already initialised — just resume
          try { ruffleEl._rufflePlayer.play(); } catch {}
          currentMedia = ruffleEl._rufflePlayer;
        } else {
          if (!window.customElements.get('f0ck-flash')) {
            console.error('[Flash] flash-embed.js is not loaded');
          } else {
            // Sandboxed player (flash-embed.js): the SWF runs without the viewer's session
            const player = document.createElement('f0ck-flash');
            const ph = ruffleEl.querySelector('.ruffle-placeholder');
            if (ph) ph.style.display = 'none';
            ruffleEl.appendChild(player);
            player.load({ url: ruffleEl.dataset.swf, volume: isMuted ? 0 : volume })
              .catch(e => console.warn('[Flash] load failed', e));
            ruffleEl._rufflePlayer = player;
          }
        }
      }
      // Only set currentMedia to null if we didn't find a ruffle player
      if (!ruffleEl || !ruffleEl._rufflePlayer) {
        currentMedia = null;
      } else {
        currentMedia = ruffleEl._rufflePlayer;
      }
    }
    preloadNeighbors(slide);
    // Schedule a live meta refresh for this slide (debounced batch)
    scheduleMetaRefresh(slide);
  }

  // ── Live meta refresh ────────────────────────────────────────────────────
  // Batches slide IDs that need refreshing and fires a single API call
  const metaRefreshPending = new Set();
  let metaRefreshTimer = null;

  function scheduleMetaRefresh(slide) {
    if (!slide || slide._metaRefreshed) return; // only refresh once per slide
    metaRefreshPending.add(slide.dataset.id);
    clearTimeout(metaRefreshTimer);
    metaRefreshTimer = setTimeout(flushMetaRefresh, 150);
  }

  async function flushMetaRefresh() {
    if (!metaRefreshPending.size) return;
    const ids = [...metaRefreshPending];
    metaRefreshPending.clear();
    try {
      const resp = await fetch(`/api/v2/scroller/meta?ids=${ids.join(',')}`);
      if (!resp.ok) return;
      const meta = await resp.json();
      for (const [id, data] of Object.entries(meta)) {
        const slide = feed.querySelector(`.scroll-slide[data-id="${id}"]`);
        if (!slide) continue;
        slide._metaRefreshed = true;
        applySlideMetaUpdate(slide, data);
      }
    } catch {}
  }

  function applySlideMetaUpdate(slide, data) {
    // Update fav count + faved state
    const favBtn = slide.querySelector('.js-fav-btn');
    if (favBtn) {
      const countEl = favBtn.querySelector('.scroll-btn-count');
      if (countEl) countEl.textContent = data.fav_count ?? countEl.textContent;
      if (data.is_faved != null) {
        favBtn.classList.toggle('faved', data.is_faved);
        const icon = favBtn.querySelector('i');
        if (icon) icon.className = (data.is_faved ? 'fa-solid' : 'fa-regular') + ' fa-heart';
      }
    }
    // Update comment count
    const commBtn = slide.querySelector('.js-comments-btn');
    if (commBtn) {
      const countEl = commBtn.querySelector('.scroll-btn-count');
      if (countEl && data.comment_count != null) countEl.textContent = data.comment_count;
    }
    // Update tags
    if (data.tags != null) applyFreshTags(slide, data.tags);
  }

  function applyFreshTags(slide, tagString) {
    const metaInner = slide.querySelector('.scroll-meta-inner');
    if (!metaInner) return;
    let tagsDiv = slide.querySelector('.scroll-tags');
    if (!tagString) {
      if (tagsDiv) tagsDiv.remove();
      return;
    }
    if (!tagsDiv) {
      tagsDiv = document.createElement('div'); tagsDiv.className = 'scroll-tags';
      const idLink = metaInner.querySelector('.scroll-id-link');
      if (idLink) metaInner.insertBefore(tagsDiv, idLink); else metaInner.appendChild(tagsDiv);
    }
    const newTags = tagString.split(', ').map(t => t.trim()).filter(Boolean);
    const existingTags = new Set([...tagsDiv.querySelectorAll('.scroll-tag-pill')].map(p => p.dataset.tag));
    for (const tag of newTags) {
      if (!existingTags.has(tag)) {
        const pill = document.createElement('span');
        pill.className = 'scroll-tag-pill'; pill.dataset.tag = tag;
        pill.innerHTML = `${esc(tag)}`;
        if (window.scrollerIsMod) {
          const del = document.createElement('button');
          del.className = 'scroll-tag-pill-del'; del.title = `Remove tag '${tag}'`;
          del.innerHTML = '&times;';
          del.addEventListener('click', async (e) => {
            e.stopPropagation();
            const itemId = slide.dataset.id;
            try {
              const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
              const resp = await fetch(`/api/v2/tags/${itemId}/${encodeURIComponent(tag)}`, {
                method: 'DELETE',
                headers: { ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {}) }
              });
              const data = await resp.json();
              if (data.success) pill.remove();
              else console.warn('[scroller] tag delete failed', data.msg);
            } catch (err) { console.error('[scroller] tag delete error', err); }
          });
          pill.appendChild(del);
        }
        // Filter on tag text click (not delete button)
        pill.addEventListener('click', e => {
          if (e.target.closest('.scroll-tag-pill-del')) return;
          e.stopPropagation(); filterByTag(tag);
        });
        tagsDiv.appendChild(pill);
      }
    }
  }

  function deactivateSlide(slide) {
    if (!slide) return;
    slide.classList.remove('active');
    const v = slide.querySelector('video');
    const a = slide.querySelector('audio');
    const c = slide.querySelector('.audio-cover');

    if (v) {
      v.pause();
      // Stop canvas bg loop
      if (slide._bgRaf) { cancelAnimationFrame(slide._bgRaf); slide._bgRaf = null; }
      // Strip .ready so the blurry bg shows again before the video re-buffers
      v.classList.remove('ready');
      // Clean up auto-next handler
      if (slide._autoNextHandler) { v.removeEventListener('ended', slide._autoNextHandler); slide._autoNextHandler = null; }
      // Save position, then abort all network loading immediately.
      // If the video already ended, save 0 so re-activation restarts from the beginning.
      v._savedSrc  = v._savedSrc  || v.src  || v.currentSrc;
      v._savedTime = v.ended ? 0 : v.currentTime;
      v.removeAttribute('src');
      // Remove any <source> children too
      v.querySelectorAll('source').forEach(s => s.removeAttribute('src'));
      v.load(); // aborts buffering
    }
    if (a) {
      a.pause();
      a._savedSrc  = a._savedSrc  || a.src  || a.currentSrc;
      a._savedTime = a.ended ? 0 : a.currentTime;
      a.removeAttribute('src');
      a.load();
      if (c) c.classList.remove('playing');
    }
    // Stop YouTube iframe by clearing src
    const ytContainer = slide.querySelector('.yt-container');
    if (ytContainer) {
      const iframe = ytContainer.querySelector('iframe');
      if (iframe) iframe.removeAttribute('src');
    }
    // Stop Ruffle player by removing it (destroys WASM instance)
    slide._deactivated = true;
    const ruffleEl = slide.querySelector('.ruffle-container');
    if (ruffleEl && ruffleEl._rufflePlayer) {
      try {
        ruffleEl._rufflePlayer.pause();
        ruffleEl._rufflePlayer.remove();
        ruffleEl._rufflePlayer = null;
        const ph = ruffleEl.querySelector('.ruffle-placeholder');
        if (ph) ph.style.display = 'block';
      } catch {}
    }
  }

  // No‑op: preloading neighbours eats bandwidth on real connections.
  // The browser will buffer naturally once the video src is set on activate.
  function preloadNeighbors() {}

  // ── Seekable progress bar ─────────────────────────────────────────────────
  function wireProgress(slide, media, fill, thumb) {
    if (!fill) return;
    const bar = fill.parentElement;
    const onTime = () => {
      if (!slide.classList.contains('active')) { media.removeEventListener('timeupdate', onTime); return; }
      const pct = media.duration ? (media.currentTime / media.duration) * 100 : 0;
      fill.style.width = pct + '%';
      if (thumb) thumb.style.right = (100 - pct) + '%';
    };
    media.addEventListener('timeupdate', onTime);
    const seek = (cx) => {
      if (!media.duration) return;
      const rect = bar.getBoundingClientRect();
      const pct = Math.max(0, Math.min(1, (cx - rect.left) / rect.width));
      media.currentTime = pct * media.duration;
      fill.style.width = (pct * 100) + '%';
      if (thumb) thumb.style.right = ((1 - pct) * 100) + '%';
    };
    let dragging = false;
    const onMove = e => { if (!dragging) return; bar.classList.add('dragging'); seek(e.touches ? e.touches[0].clientX : e.clientX); };
    const onEnd = () => { dragging = false; bar.classList.remove('dragging'); document.removeEventListener('mousemove', onMove); document.removeEventListener('mouseup', onEnd); document.removeEventListener('touchmove', onMove); document.removeEventListener('touchend', onEnd); };
    bar.addEventListener('click', e => seek(e.clientX));
    bar.addEventListener('mousedown', e => { dragging = true; seek(e.clientX); document.addEventListener('mousemove', onMove); document.addEventListener('mouseup', onEnd); });
    bar.addEventListener('touchstart', e => { dragging = true; seek(e.touches[0].clientX); document.addEventListener('touchmove', onMove, { passive: true }); document.addEventListener('touchend', onEnd); }, { passive: true });
  }

  // ── Double-tap / double-click → favourite ────────────────────────────────
  let lastTapTime    = 0;
  let lastTapSlide   = null;
  const DOUBLE_DELAY = 320;

  // ── Floating heart animation ──────────────────────────────────────────────
  const HEART_EMOJIS = ['❤️', '💕', '💖', '💗', '💓', '💞', '💝'];
  let heartEmojIdx = 0;

  function spawnHeart(slide, clientX, clientY) {
    const rect = slide.getBoundingClientRect();
    const x = clientX - rect.left;
    const y = clientY - rect.top;
    const emoji = HEART_EMOJIS[heartEmojIdx % HEART_EMOJIS.length];
    heartEmojIdx++;
    const el = document.createElement('div');
    el.className = 'tap-heart';
    el.textContent = emoji;
    // Random horizontal drift ±30px, random size 52–76px
    const drift = (Math.random() - 0.5) * 60;
    const size  = 52 + Math.random() * 24;
    el.style.cssText = `left:${x}px;top:${y}px;font-size:${size}px;--drift:${drift}px`;
    slide.appendChild(el);
    // Remove after animation completes
    el.addEventListener('animationend', () => el.remove(), { once: true });
  }

  function flashFav(slide) {
    const el = slide.querySelector('.fav-flash');
    if (!el) return;
    el.classList.remove('show', 'hide'); void el.offsetWidth;
    el.classList.add('show');
    setTimeout(() => { el.classList.remove('show'); el.classList.add('hide'); }, 800);
  }

  async function toggleFav(slide) {
    if (window.f0ckSession?.is_anon && window.f0ckSession?.anon_permissions && window.f0ckSession.anon_permissions.favorite === false) {
      const errMsg = 'Anonymous favoriting is disabled.';
      if (typeof window.flashMessage === 'function') {
        window.flashMessage(errMsg, 3000, 'error');
      } else if (typeof showShareToast === 'function') {
        showShareToast(errMsg);
      }
      return;
    }
    if (!window.scrollerLoggedIn) {
      if (typeof showShareToast === 'function') {
        showShareToast('Login to favorite posts');
      } else if (typeof window.flashMessage === 'function') {
        window.flashMessage('Login to favorite posts', 3000, 'warning');
      }
      return;
    }
    const id = slide.dataset.localId || slide.dataset.id;
    // External items have non-numeric IDs (e.g. "gif/123") — can't fav until rehosted
    if (!/^\d+$/.test(id)) { showShareToast('Can\u2019t fav external items'); return; }
    const favBtn = slide.querySelector('.js-fav-btn');
    // Optimistic: immediately flip the button state before the server responds
    const wasFaved = favBtn ? favBtn.classList.contains('faved') : false;
    if (favBtn) {
      const nowFaved = !wasFaved;
      favBtn.classList.toggle('faved', nowFaved);
      const icon = favBtn.querySelector('i');
      if (icon) icon.className = (nowFaved ? 'fa-solid' : 'fa-regular') + ' fa-heart';
      const countEl = favBtn.querySelector('.scroll-btn-count');
      if (countEl) countEl.textContent = Math.max(0, (parseInt(countEl.textContent || '0', 10)) + (nowFaved ? 1 : -1));
      if (nowFaved) flashFav(slide);
    }
    try {
      const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
      const resp = await fetch('/api/v2/togglefav', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/x-www-form-urlencoded',
          ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
        },
        body: `postid=${id}&action=${nowFaved ? 'add' : 'delete'}&favorited=${nowFaved}${csrfToken ? `&csrf_token=${encodeURIComponent(csrfToken)}` : ''}`
      });
      const data = await resp.json().catch(() => ({}));
      if (!resp.ok || !data.success) {
        // Rollback optimistic update
        if (favBtn) {
          favBtn.classList.toggle('faved', wasFaved);
          const icon = favBtn.querySelector('i');
          if (icon) icon.className = (wasFaved ? 'fa-solid' : 'fa-regular') + ' fa-heart';
          const countEl = favBtn.querySelector('.scroll-btn-count');
          if (countEl) countEl.textContent = Math.max(0, (parseInt(countEl.textContent || '0', 10)) + (wasFaved ? 0 : -1));
        }
        const errMsg = (data && (data.msg || data.error)) || 'Anonymous favoriting is disabled.';
        if (typeof window.flashMessage === 'function') {
          window.flashMessage(errMsg, 3000, 'error');
        } else if (typeof showShareToast === 'function') {
          showShareToast(errMsg);
        }
        return;
      }
      // Sync state and count to server truth (handles race conditions)
      if (data.success && favBtn) {
        if (data.favorited !== undefined) {
          favBtn.classList.toggle('faved', data.favorited);
          const icon = favBtn.querySelector('i');
          if (icon) icon.className = (data.favorited ? 'fa-solid' : 'fa-regular') + ' fa-heart';
        }
        if (data.favs) {
          const countEl = favBtn.querySelector('.scroll-btn-count');
          if (countEl) countEl.textContent = data.favs.length;
        }
      }
    } catch {
      // Rollback optimistic update on error
      if (favBtn) {
        favBtn.classList.toggle('faved', wasFaved);
        const icon = favBtn.querySelector('i');
        if (icon) icon.className = (wasFaved ? 'fa-solid' : 'fa-regular') + ' fa-heart';
        const countEl = favBtn.querySelector('.scroll-btn-count');
        if (countEl) countEl.textContent = Math.max(0, (parseInt(countEl.textContent || '0', 10)) + (wasFaved ? 0 : -1));
      }
    }
  }

  // ── Tap overlay + double-tap ──────────────────────────────────────────────
  function setupTapOverlay(slide, getSpeedEndedAt) {
    const tapEl = slide.querySelector('.tap-overlay');
    const icon  = slide.querySelector('.tap-pause-icon');
    if (!tapEl) return;
    let singleTimer = null;

    const FAV_BURST_WINDOW = 2500; // ms — while in burst, extra taps just show hearts
    let lastDoubleTapAt = 0;

    const handleTap = (e) => {
      if (getSpeedEndedAt && Date.now() - getSpeedEndedAt() < 250) return;
      const now = Date.now();
      if (lastTapSlide === slide && now - lastTapTime < DOUBLE_DELAY) {
        // Double tap
        clearTimeout(singleTimer);
        lastTapTime = 0; lastTapSlide = null;
        spawnHeart(slide, e.clientX, e.clientY);
        const favBtn = slide.querySelector('.js-fav-btn');
        const isFaved = favBtn && favBtn.classList.contains('faved');
        if (!isFaved) {
          // Not faved → fav it
          toggleFav(slide);
          lastDoubleTapAt = now;
        } else if (now - lastDoubleTapAt > FAV_BURST_WINDOW) {
          // Already faved, but been a while → unfav
          toggleFav(slide);
          lastDoubleTapAt = 0;
        } else {
          // In burst window → just hearts, keep faved
          lastDoubleTapAt = now;
        }
        return;
      }
      lastTapTime = now; lastTapSlide = slide;
      // Delay single-tap action so we can detect double-tap
      clearTimeout(singleTimer);
      singleTimer = setTimeout(() => {
        const media = slide.querySelector('video') || slide.querySelector('audio');
        if (!media) return;
        if (media.paused) { media.play().catch(() => {}); icon.innerHTML = '<i class="fa-solid fa-play"></i>'; }
        else { media.pause(); icon.innerHTML = '<i class="fa-solid fa-pause"></i>'; }
        icon.classList.remove('show', 'hide'); void icon.offsetWidth; icon.classList.add('show');
        setTimeout(() => { icon.classList.remove('show'); icon.classList.add('hide'); }, 850);
      }, DOUBLE_DELAY);
    };

    tapEl.addEventListener('click', handleTap);
    tapEl.addEventListener('contextmenu', e => e.preventDefault());

    // Long-press: hold to peek (hide UI + pause media), release to restore
    let holdTimer = null;
    let peekPaused = false; // tracks if WE paused the media
    const startHold = () => {
      holdTimer = setTimeout(() => {
        slide.classList.add('ui-peek');
        const media = slide.querySelector('video') || slide.querySelector('audio');
        if (media && !media.paused) { media.pause(); peekPaused = true; }
      }, 300);
    };
    const endHold = () => {
      clearTimeout(holdTimer); holdTimer = null;
      slide.classList.remove('ui-peek');
      if (peekPaused) {
        const media = slide.querySelector('video') || slide.querySelector('audio');
        if (media) media.play().catch(() => {});
        peekPaused = false;
      }
    };
    tapEl.addEventListener('touchstart', startHold,  { passive: true });
    tapEl.addEventListener('touchend',   endHold,    { passive: true });
    tapEl.addEventListener('touchcancel',endHold,    { passive: true });
    // Only cancel the timer on move (prevent swipe mis-activation);
    // if peek is already showing, moving the finger keeps it active.
    tapEl.addEventListener('touchmove', () => { clearTimeout(holdTimer); holdTimer = null; }, { passive: true });
  }

  // ── Create slide ──────────────────────────────────────────────────────────
  function createSlide(item) {
    const slide = document.createElement('div');
    slide.className = 'scroll-slide';
    slide.dataset.id = item.id;
    if (item.local_id) slide.dataset.localId = item.local_id;
    slide._thumbnail = item.thumbnail; // stored for canvas redraw + applyCanvasBg fallback

    const bgCanvas = document.createElement('canvas');
    bgCanvas.className = 'scroll-bg-blur';
    // Low resolution — blurred so heavily that full res is wasted
    bgCanvas.width = 64; bgCanvas.height = 36;
    // Draw thumbnail immediately for a static bg before video starts
    const bgCtx = bgCanvas.getContext('2d');
    const bgThumb = new Image();
    bgThumb.onload = () => { try { bgCtx.drawImage(bgThumb, 0, 0, 64, 36); } catch {} };
    bgThumb.src = item.thumbnail;
    slide.appendChild(bgCanvas);

    const mediaEl = document.createElement('div');
    mediaEl.className = 'scroll-media';
    if (item.is_video) {
      const v = document.createElement('video');
      v.src = item.dest; v.loop = true; v.playsInline = true;
      v.muted = isMuted; v.volume = volume; v.preload = 'none';
      // .ready class is registered fresh in activateSlide each time
      mediaEl.appendChild(v);
    } else if (item.is_youtube) {
      const ytId = (item.dest || '').replace('yt:', '');
      const d = document.createElement('div'); d.className = 'yt-container';
      // NOTE: iframe src is injected in activateSlide to prevent all slides autoplaying at once
      d.dataset.ytId = ytId;
      d.innerHTML = `<iframe allow="autoplay; encrypted-media" allowfullscreen></iframe>`;
      mediaEl.appendChild(d);
    } else if (item.is_audio) {
      // Audio: blurry bg acts as album art — vinyl disc shows cover art
      const cover = document.createElement('div');
      cover.className = 'audio-cover';
      if (item.thumbnail) {
        // Quote and escape the URL so a ')' or '"' in the path can't break the
        // CSS url() value or inject arbitrary styles.
        cover.style.backgroundImage = `url("${item.thumbnail.replace(/["\\]/g, '')}")` ;
        cover.style.backgroundSize = 'cover';
        cover.style.backgroundPosition = 'center';
      }
      mediaEl.appendChild(cover);
      const au = document.createElement('audio');
      au.src = item.dest; au.loop = true; au.muted = isMuted; au.volume = volume; au.preload = 'none';
      mediaEl.appendChild(au);
    } else if (item.is_swf && window.scrollerEnableSwf) {
      // SWF via Ruffle — container is populated lazily in activateSlide
      const ruffleContainer = document.createElement('div');
      ruffleContainer.className = 'ruffle-container';
      ruffleContainer.dataset.swf = item.dest;
      ruffleContainer.style.cssText = 'width:100%;height:100%;display:flex;align-items:center;justify-content:center;background:#000;';
      // Thumbnail placeholder until activated
      const placeholder = document.createElement('img');
      placeholder.src = item.thumbnail; placeholder.alt = ''; placeholder.className = 'ruffle-placeholder';
      placeholder.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;object-fit:contain;pointer-events:none;';
      ruffleContainer.style.position = 'relative';
      ruffleContainer.appendChild(placeholder);
      mediaEl.appendChild(ruffleContainer);
    } else {
      const img = document.createElement('img');
      img.src = item.dest; img.alt = ''; img.loading = 'lazy'; img.decoding = 'async';
      mediaEl.appendChild(img);
    }
    slide.appendChild(mediaEl);

    slide.appendChild(Object.assign(document.createElement('div'), { className: 'scroll-overlay' }));

    const tap = document.createElement('div'); tap.className = 'tap-overlay';
    const tapIcon = document.createElement('div'); tapIcon.className = 'tap-pause-icon';
    // YouTube and Flash need direct interaction
    if (item.is_youtube || item.is_swf) tap.style.pointerEvents = 'none';
    tap.appendChild(tapIcon); slide.appendChild(tap);

    // Fav flash element
    const favFlash = document.createElement('div');
    favFlash.className = 'fav-flash'; favFlash.innerHTML = '❤️'; slide.appendChild(favFlash);

    // Meta (z:10)
    const meta = document.createElement('div'); meta.className = 'scroll-meta';
    // esc() the color value: a raw '"' in username_color would break out of the
    // style attribute and allow arbitrary HTML injection (XSS).
    const isAnonGuest = window.f0ckSession?.is_anonymized ?? (window.f0ckSession?.guest_anonymize && !window.f0ckSession?.logged_in);
    const colorStyle = (!isAnonGuest && item.username_color) ? `color:${esc(item.username_color)}` : '';
    const ratingHtml = `<span class="scroll-rating ${esc(item.rating_class)}" data-item-id="${item.id}" data-rating="${esc(item.rating_class)}">${esc(item.rating_label)}</span>`;
    const ocHtml = item.is_oc ? `<span class="scroll-oc"><i class="fa-solid fa-star" style="font-size:.6rem"></i> OC</span>` : '';
    const badgesHtml = `<div class="scroll-badges">${ratingHtml}${ocHtml}</div>`;
    // Build clickable tag pills
    const tagsHtml = item.tags
      ? `<div class="scroll-tags"><i class="fa-solid fa-tag" style="font-size:.6rem;margin-right:4px;opacity:.5"></i>${
          item.tags.split(', ').map(t => `<span class="scroll-tag-pill" data-tag="${esc(t.trim())}">${esc(t.trim())}</span>`).join('')
        }</div>`
      : '';
    const userMetaHtml = isAnonGuest
      ? `<div class="scroll-meta-top">
          <img class="scroll-avatar" src="/a/default.png" alt="" loading="lazy">
          <div>
            <div class="scroll-username">anonymous</div>
            <div class="scroll-timeago">${esc(item.stamp ? timeAgo(item.stamp * 1000) : (item.timeago || ''))}</div>
          </div>
        </div>`
      : `<div class="scroll-meta-top">
          <a href="/user/${esc(item.username)}" class="scroll-user-link">
            <img class="scroll-avatar" src="${esc(item.avatar)}" alt="" loading="lazy" data-on="error:img-fallback" data-fallback="/a/default.png">
          </a>
          <div>
            <a href="/user/${esc(item.username)}" class="scroll-user-link">
              <div class="scroll-username" style="${colorStyle}">${esc(item.display_name || item.username)}</div>
            </a>
            <div class="scroll-timeago">${esc(item.stamp ? timeAgo(item.stamp * 1000) : (item.timeago || ''))}</div>
          </div>
        </div>`;
    meta.innerHTML = `
      <div class="scroll-meta-inner">
        ${badgesHtml}
        ${userMetaHtml}
        ${tagsHtml}
        ${item.is_external && item.external_board && item.external_tid
          ? `<a class="scroll-id-link" href="https://boards.4chan.org/${item.external_board}/thread/${item.external_tid}#p${item.external_id}" target="_blank">/${item.external_board}/thread/${item.external_tid}</a>`
          : (item.local_id
            ? `<a class="scroll-id-link" href="/${item.local_path || item.local_id}" target="_blank">/${item.local_path || item.local_id}</a>`
            : `<a class="scroll-id-link" href="/abyss/${item.id}">#${item.id}</a>`)}
      </div>`;
    slide.appendChild(meta);

    // Actions (z:10) — no Filter button here, it lives in the topbar
    const actions = document.createElement('div'); actions.className = 'scroll-actions';
    const _i = window.f0ckI18n || {};
    actions.innerHTML = `
      ${(() => {
        const isFaved = !!item.is_faved;
        return `
        <button class="scroll-btn js-fav-btn${isFaved ? ' faved' : ''}" title="${_i.favourite || 'Favourite'} (double-tap)">
          <div class="scroll-btn-icon"><i class="${isFaved ? 'fa-solid' : 'fa-regular'} fa-heart"></i></div>
          <span class="scroll-btn-count">${item.fav_count ?? 0}</span>
        </button>`;
      })()}
      <button class="scroll-btn js-comments-btn" data-id="${item.id}" title="${_i.comments_label || 'Comments'} (C)">
        <div class="scroll-btn-icon"><i class="fa-regular fa-comment"></i></div>
        <span class="scroll-btn-count">${item.comment_count ?? 0}</span>
      </button>
      ${window.scrollerLoggedIn ? `
      <button class="scroll-btn js-tag-btn" data-id="${item.id}" title="${_i.add_tag || 'Add tag'}">
        <div class="scroll-btn-icon"><i class="fa-solid fa-tag"></i></div>
      </button>` : ''}
      <button class="scroll-btn js-share-btn" data-id="${item.id}" title="${_i.share_label || 'Share'}">
        <div class="scroll-btn-icon"><i class="fa-solid fa-share-nodes"></i></div>
        <span class="scroll-btn-label">${_i.share_label || 'Share'}</span>
      </button>
      ${item.is_external ? (
        item.local_id
          ? `<a class="scroll-btn rehost-btn success" href="/${item.local_path || item.local_id}" target="_blank" title="${_i.already_added || 'Already added'}">
               <div class="scroll-btn-icon"><i class="fa-solid fa-check"></i></div>
               <span class="scroll-btn-label">${_i.view_label || 'View'}</span>
             </a>`
          : `<button class="scroll-btn rehost-btn" data-id="${item.id}" title="${_i.add_to_site || 'Add to site'}">
               <div class="scroll-btn-icon"><i class="fa-solid fa-plus"></i></div>
               <span class="scroll-btn-label">${_i.add_label || 'Add'}</span>
             </button>`
      ) : `
      <a class="scroll-btn" href="/${item.id}" target="_blank" title="${_i.open_post || 'Open post'}">
        <div class="scroll-btn-icon"><i class="fa-solid fa-up-right-from-square"></i></div>
        <span class="scroll-btn-label">${_i.open_label || 'Open'}</span>
      </a>`}
    `;
    slide.appendChild(actions);

    // ── Speed-hold zones (Direct children of slide for maximum reliability) ──
    // Right zone (above actions)
    const rightSpeedZone = document.createElement('button');
    rightSpeedZone.className = 'js-speed-hold-btn';
    rightSpeedZone.setAttribute('aria-hidden', 'true');
    rightSpeedZone.tabIndex = -1;
    Object.assign(rightSpeedZone.style, {
      position: 'absolute', right: '0', bottom: '0', width: '40px', height: '100vh',
      background: 'none', border: 'none', padding: '0', cursor: 'pointer', opacity: '0',
      pointerEvents: 'all', touchAction: 'none', zIndex: '5'
    });
    slide.appendChild(rightSpeedZone);

    // Left zone (above meta)
    const leftSpeedZone = document.createElement('button');
    leftSpeedZone.className = 'js-speed-hold-btn';
    leftSpeedZone.setAttribute('aria-hidden', 'true');
    leftSpeedZone.tabIndex = -1;
    Object.assign(leftSpeedZone.style, {
      position: 'absolute', left: '0', bottom: '0', width: '40px', height: '100vh',
      background: 'none', border: 'none', padding: '0', cursor: 'pointer', opacity: '0',
      pointerEvents: 'all', touchAction: 'none', zIndex: '5'
    });
    slide.appendChild(leftSpeedZone);

    // In left-hand mode, inject 4chan action buttons into this slide's actions
    if (leftHandEnabled) {
      if (chanOpenBtn && chanOpenBtn.style.display === 'flex') {
        const ob = document.createElement('button');
        ob.className = 'chan-action-btn';
        ob.title = chanOpenBtn.title;
        ob.innerHTML = `<div class="scroll-btn-icon"><i class="fa-solid fa-clover" style="color:#789922"></i></div>`;
        ob.addEventListener('click', () => chanOpenBtn.click());
        actions.appendChild(ob);
      }
      if (chanGalleryBtn && chanGalleryBtn.style.display === 'flex') {
        const gb = document.createElement('button');
        gb.className = 'chan-action-btn';
        gb.title = chanGalleryBtn.title;
        gb.innerHTML = `<div class="scroll-btn-icon"><i class="fa-solid fa-grip"></i></div>`;
        gb.addEventListener('click', () => chanGalleryBtn.click());
        actions.appendChild(gb);
      }
    }

    // Progress bar (z:10)
    const pBar = document.createElement('div'); pBar.className = 'scroll-progress-bar';
    pBar.innerHTML = '<div class="scroll-progress-fill"></div><div class="scroll-progress-thumb"></div>';
    slide.appendChild(pBar);

    setupTapOverlay(slide, () => speedEndedAt);
    // ── Hold-to-2× speed logic (shared by multiple triggers) ──
    let speedTimer   = null;
    let speedActive     = false;
    let speedEndedAt    = 0;
    let wasPausedByHold = false;

    const endSpeed = () => {
      clearTimeout(speedTimer);
      speedTimer = null;
      document.removeEventListener('pointerup',    endSpeed);
      document.removeEventListener('pointercancel',endSpeed);
      if (speedActive) {
        speedActive  = false;
        speedEndedAt = Date.now();
        const media = slide.querySelector('video') || slide.querySelector('audio');
        if (media) {
          media.playbackRate = 1;
          if (wasPausedByHold) {
            media.pause();
            wasPausedByHold = false;
          }
        }
        const ind = document.getElementById('speed-indicator');
        if (ind) ind.classList.remove('show');
      }
    };
    const wireSpeedHold = (el, onlyMouse = false) => {
      if (!el) return;
      el.addEventListener('pointerdown', (e) => {
        if (onlyMouse && e.pointerType !== 'mouse') return;
        document.addEventListener('pointerup',    endSpeed, { once: true });
        document.addEventListener('pointercancel',endSpeed, { once: true });
        speedTimer = setTimeout(() => {
          speedActive = true;
          const media = slide.querySelector('video') || slide.querySelector('audio');
          if (media) {
            if (media.paused) {
              wasPausedByHold = true;
              media.play().catch(() => {});
            }
            media.playbackRate = 2;
            const ind = document.getElementById('speed-indicator');
            if (ind) ind.classList.add('show');
          }
        }, 150);
      }, { passive: true });
    };

    wireSpeedHold(rightSpeedZone);
    wireSpeedHold(leftSpeedZone);
    // On desktop, holding anywhere (mouse) triggers speed hold
    wireSpeedHold(slide.querySelector('.tap-overlay'), true);


    const favBtn = actions.querySelector('.js-fav-btn');
    if (favBtn) {
      let favLastClick = 0;
      favBtn.addEventListener('click', () => {
        const now = Date.now();
        if (now - favLastClick < DOUBLE_DELAY) return;
        favLastClick = now;
        toggleFav(slide);
      });
    }

    actions.querySelector('.js-comments-btn').addEventListener('click', () => {
      const slide = actions.closest('.scroll-slide');
      const id = slide?.dataset.localId || item.id;
      openComments(id);
    });
    const tagBtn = actions.querySelector('.js-tag-btn');
    if (tagBtn) tagBtn.addEventListener('click', () => {
      const slide = tagBtn.closest('.scroll-slide');
      const id = slide?.dataset.localId || item.id;
      if (!/^\d+$/.test(id)) { showShareToast((window.f0ckI18n && window.f0ckI18n.add_to_site_first) || 'Add to site first to tag'); return; }
      openTagBar(id);
    });
    const shareBtn = actions.querySelector('.js-share-btn');
    if (shareBtn) shareBtn.addEventListener('click', () => {
      const slide = shareBtn.closest('.scroll-slide');
      const localId = slide?.dataset.localId;
      const id = localId || shareBtn.dataset.id;
      const abyssUrl = localId
        ? `${location.origin}/abyss/${localId}`
        : (id ? `${location.origin}/abyss/${id}` : `${location.origin}/abyss`);
      openSharePanel(abyssUrl);
    });
    const rehostBtn = actions.querySelector('.rehost-btn');
    if (rehostBtn) rehostBtn.addEventListener('click', () => rehostItem(item, rehostBtn));

    // Rating cycle — always wire the click; server enforces mod auth via 403
    const rBadge = slide.querySelector('.scroll-rating[data-item-id]');
    if (rBadge) {
      if (window.scrollerIsMod || item.local_id) rBadge.classList.add('can-cycle');
      rBadge.addEventListener('click', e => {
        if (Date.now() - speedEndedAt < 200) return; // ignore click if we just ended a speed-hold
        e.stopPropagation();
        const slideEl = rBadge.closest('.scroll-slide');
        const id = slideEl?.dataset.localId || rBadge.dataset.itemId;
        if (!id || isNaN(id)) { showShareToast('Rehost first to change rating'); return; }
        cycleRatingOptimistic(rBadge, id, false);
      });
    }

    // Wire clickable tag pills (pointer-events:all override comes from CSS)
    meta.querySelectorAll('.scroll-tag-pill').forEach(pill => {
      const t = pill.dataset.tag;
      pill.addEventListener('click', e => {
        if (e.target.closest('.scroll-tag-pill-del')) return;
        e.stopPropagation(); filterByTag(t);
      });
      if (window.scrollerIsMod) {
        const del = document.createElement('button');
        del.className = 'scroll-tag-pill-del'; del.title = `Remove tag '${t}'`;
        del.innerHTML = '&times;';
        del.addEventListener('click', async (e) => {
          e.stopPropagation();
          const itemId = slide.dataset.id;
          try {
            const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
            const resp = await fetch(`/api/v2/tags/${itemId}/${encodeURIComponent(t)}`, {
              method: 'DELETE',
              headers: { ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {}) }
            });
            const data = await resp.json();
            if (data.success) pill.remove();
            else console.warn('[scroller] tag delete failed', data.msg);
          } catch (err) { console.error('[scroller] tag delete error', err); }
        });
        pill.appendChild(del);
      }
    });

    return slide;
  }

  // ── Feed fetch ────────────────────────────────────────────────────────────
  async function fetchItems() {
    if (isFetching) return;
    // For random mode: allow up to 5 consecutive empty results before giving up
    if (!hasMore && applied.order !== 'random') return;
    if (applied.order === 'random' && consecutiveEmpty >= 5) {
      if (feed.querySelectorAll('.scroll-slide').length === 0) emptyEl.classList.add('show');
      return;
    }

    isFetching = true;
    const params = new URLSearchParams({ limit: 12, mode: applied.mode, orderby: applied.order });
    if (applied.mime) params.set('mime', applied.mime);
    if (applied.tags.length > 0) params.set('tag', applied.tags.join(','));
    if (applied.order !== 'random') {
      // Ordered modes: use cursor-based pagination
      if (lastCursor) params.set('after', lastCursor);
    } else if (seenIds.size > 0) {
      // Random mode: send the most-recently-seen IDs (tail of the set) so the server avoids them;
      // recent items are most likely to collide with the next RANDOM() batch.
      const allSeen = [...seenIds];
      const recentSeen = allSeen.slice(-200); // last 200 seen
      params.set('exclude', recentSeen.join(','));
    }
    // On the very first fetch, anchor to the hash ID so it's always in the batch
    const hid = hashId();
    if (hid && seenIds.size === 0 && !lastCursor) params.set('anchor', hid);

    try {
      let endpoint = `/api/v2/scroller/feed?${params}`;
      if (applied.externalUrl) {
         const match = applied.externalUrl.match(/(?:boards\.)?4chan\.org\/([a-z0-9]+)\/thread\/(\d+)/);
         if (match) {
            endpoint = `/api/v2/scroller/external/4chan/${match[1]}/${match[2]}`;
         }
      }
      const resp = await fetch(endpoint);
      const data = await resp.json();

      let items = data.items || [];
      if (applied.externalUrl && data.posts) {
        if (chanGalleryBtn) chanGalleryBtn.style.display = 'flex';
        // Load persisted rehosts from localStorage for this thread
        const threadKey = `rehosted_4chan_${data.board}_${data.tid}`;
        let cachedRehosts = {};
        try { cachedRehosts = JSON.parse(localStorage.getItem(threadKey) || '{}'); } catch(_) {}
        // Merge server-side rehosts with local cache (server is authoritative, cache fills gaps)
        const allRehosts = Object.assign({}, cachedRehosts, data.rehosts || {});
        // Persist merged map back
        try { localStorage.setItem(threadKey, JSON.stringify(allRehosts)); } catch(_) {}

        items = data.posts
          .filter(p => p.tim && p.ext)
          .map(p => {
            const ext = (p.ext || "").toLowerCase();
            const isVideo = ['.webm', '.mp4'].includes(ext);
            const isImage = ['.jpg', '.jpeg', '.png', '.gif', '.webp'].includes(ext);
            const isWsg = data.board === 'wsg';
            const isGif = data.board === 'gif';

            // Respect instance-level allowed mime categories
            const mimeCats = window.scrollerMimeCats || ['video', 'image', 'audio'];
            if (isVideo && !mimeCats.includes('video')) return null;
            if (isImage && !mimeCats.includes('image')) return null;
            if (!isVideo && !isImage) return null;

            // Respect user-selected mime filter
            if (applied.mime === 'video' && !isVideo) return null;
            if (applied.mime === 'image' && !isImage) return null;

            const external_media_url = `https://i.4cdn.org/${data.board}/${p.tim}${ext}`;
            const local_id = allRehosts[external_media_url] || null;
            const local_path = (local_id && data.rehost_paths && data.rehost_paths[local_id]) || local_id;

            return {
              id: `${data.board}/${p.no}`,
              external_id: p.no,
              external_tid: data.tid,
              external_board: data.board,
              external_source: '4chan',
              is_external: true,
              local_id,
              local_path,
              external_media_url,
              mime: isVideo ? 'video/unknown' : (isImage ? 'image/unknown' : 'application/octet-stream'),
              dest: `/api/v2/scroller/external/4chan/${data.board}/media/${p.tim}${ext}`,
              thumbnail: `/api/v2/scroller/external/4chan/${data.board}/media/${p.tim}s.jpg`,
              username: 'Anonymous',
              display_name: 'Anonymous',
              avatar: '/a/default.png',
              stamp: p.time,
              timeago: timeAgo(p.time * 1000),
              tags: ``,
              is_video: isVideo,
              is_image: isImage,
              is_audio: false,
              comment_count: p.replies || 0,
              rating_label: isWsg ? 'SFW' : (isGif ? 'NSFW' : 'External'),
              rating_class: isWsg ? 'sfw' : (isGif ? 'nsfw' : 'untagged'),
              original_filename: p.filename ? `${decodeHtmlEntities(p.filename)}${ext}` : null
            };
          })
          .filter(Boolean);

        // Fetch metadata for rehosted items (username, avatar, timestamp)
        const rehostItemIds = items.filter(i => i.local_id).map(i => i.local_id);
        if (rehostItemIds.length > 0) {
          try {
            const metaResp = await fetch('/api/v2/scroller/external/rehost-meta', {
              method: 'POST',
              headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
              body: `ids=${rehostItemIds.join(',')}`
            });

            const metaMap = await metaResp.json();

            items.forEach(item => {
              if (item.local_id && metaMap[item.local_id]) {
                const m = metaMap[item.local_id];

                item.username = m.username;
                item.display_name = m.display_name;
                item.avatar = m.avatar;
                if (m.comment_count != null) item.comment_count = m.comment_count;
                if (m.rating_class) { item.rating_class = m.rating_class; item.rating_label = m.rating_label; }

              }
            });

          } catch (e) { console.warn('[CHAN] rehost-meta fetch failed:', e); }
        }

        // Apply order filter client-side (API returns thread order)
        if (applied.order === 'random') {
          for (let i = items.length - 1; i > 0; i--) {
            const j = Math.floor(Math.random() * (i + 1));
            [items[i], items[j]] = [items[j], items[i]];
          }
        } else if (applied.order === 'newest') {
          items.sort((a, b) => b.stamp - a.stamp);
        } else if (applied.order === 'oldest') {
          items.sort((a, b) => a.stamp - b.stamp);
        }

        // If loading with a specific hash target, ensure it's first
        const hid = hashId();
        if (hid && !currentSlide) {
          const idx = items.findIndex(it => it.id === hid);
          if (idx > 0) {
            const [target] = items.splice(idx, 1);
            items.unshift(target);
          }
        }

        // Store post→thread mapping so #board/postno can resolve back to the thread URL
        try {
          const postMap = JSON.parse(localStorage.getItem('4chan_post_threads') || '{}');
          items.forEach(it => { postMap[it.id] = data.tid; });
          localStorage.setItem('4chan_post_threads', JSON.stringify(postMap));
        } catch(_) {}
      }

      if (!data.success || items.length === 0) {
        consecutiveEmpty++;
        const noSlides = feed.querySelectorAll('.scroll-slide').length === 0;
        if (applied.order !== 'random') {
          hasMore = false;
        }
        if (noSlides) {
          if (applied.externalUrl) {
            alert((window.f0ckI18n && window.f0ckI18n.chan_load_failed) || 'Could not load 4chan thread. It might be archived or have no compatible media.');
            applied.externalUrl = null;
            pending.externalUrl = null;
            if (galleryOpen) toggleGallery();
            if (externalUrlInput) externalUrlInput.value = '';
            reloadFeed();
            return;
          }

          // Anchor was requested but blocked (NSFW/NSFL for guest).
          const lockSlide = document.createElement('div');
          lockSlide.className = 'scroll-slide';
          lockSlide.dataset.lock = '1';
          lockSlide.style.cssText = 'background:#000;';
          lockSlide.innerHTML = [
            '<div style="position:absolute;inset:0;display:flex;flex-direction:column;',
            'align-items:center;justify-content:center;gap:18px;z-index:2;">',
            '<i class="fa-solid fa-lock" style="font-size:3rem;color:rgba(255,255,255,.35);"></i>',
            '<p style="color:rgba(255,255,255,.55);font-size:1rem;font-weight:600;margin:0;',
            'text-align:center;line-height:1.5;">This post is currently unavailable</p>',
            '</div>',
          ].join('');
          feed.insertBefore(lockSlide, sentinel);
          history.replaceState(null, '', '/abyss');
          consecutiveEmpty = 0;
          hasMore = true;
          hideLoader();
          setTimeout(() => fetchItems(), 0);
        } else {
          emptyEl.classList.add('show');
          hideLoader();
        }

        if (!data.success) alert(((window.f0ckI18n && window.f0ckI18n.fetch_failed) || 'Fetch failed: {msg}').replace('{msg}', data.msg || 'Unknown error'));
        return; // exit — no items to process
      }

      consecutiveEmpty = 0;
      let newCount = 0;

      items.forEach(item => {
        if (seenIds.has(item.id)) return;
        seenIds.add(item.id);

        feed.insertBefore(createSlide(item), sentinel);
        newCount++;
      });

      // If every item in the batch was a duplicate, treat it like an empty result
      if (newCount === 0) { consecutiveEmpty++; }
      // External threads load all items at once — no pagination
      if (applied.externalUrl) { hasMore = false; }
      if (data.nextCursor) lastCursor = data.nextCursor;
      appendToCache(items);

      if (!currentSlide) {
        const hid = hashId();
        const target = hid ? feed.querySelector(`.scroll-slide[data-id="${hid}"]`) : null;
        const first = feed.querySelector('.scroll-slide:not([data-lock])');
        const toActivate = target || first;
        if (toActivate) setTimeout(() => { toActivate.scrollIntoView({ behavior: 'instant', block: 'start' }); activateSlide(toActivate); hideLoader(); }, 200);
      }
    } catch (err) {
      console.error('[SCROLLER] Fetch error:', err);
    } finally {
      isFetching = false;
      hideLoader();
    }
  }

  async function rehostItem(item, btn) {
    if (btn.classList.contains('loading') || btn.classList.contains('success')) return;
    if (!window.scrollerLoggedIn) { alert((window.f0ckI18n && window.f0ckI18n.login_required) || 'You must be logged in to add items.'); return; }

    btn.classList.add('loading');
    const icon = btn.querySelector('i');
    const origClass = icon.className;
    icon.className = 'fa-solid fa-spinner';

    try {
      const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
      const resp = await fetch('/api/v2/scroller/rehost', {
        method: 'POST',
        headers: { 
          'Content-Type': 'application/x-www-form-urlencoded',
          ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
        },
        body: new URLSearchParams({
          url: item.external_media_url || item.dest,
          ...(item.original_filename ? { original_filename: item.original_filename } : {})
        })
      });
      const data = await resp.json();
      if (data.success) {
        // Persist to localStorage so it survives thread reloads
        if (item.external_board && item.external_media_url) {
          const threadKey = `rehosted_4chan_${item.external_board}_${applied.externalUrl?.match(/(\d+)\/?$/)?.[1] || ''}` ;
          try {
            const cached = JSON.parse(localStorage.getItem(threadKey) || '{}');
            cached[item.external_media_url] = data.item_id;
            localStorage.setItem(threadKey, JSON.stringify(cached));
          } catch(_) {}
        }

        btn.classList.remove('loading');
        btn.classList.add('success');
        icon.className = data.repost ? 'fa-solid fa-link' : 'fa-solid fa-check';
        if (data.repost) showShareToast('Already on site — linked to existing item');
        // Update slide's local ID so fav/comments work immediately
        const slide = btn.closest('.scroll-slide');
        if (slide) slide.dataset.localId = data.item_id;

        // Update button to link to the new site-internal post
        setTimeout(() => {
          btn.outerHTML = `
            <a href="/${data.item_path || data.item_id}" target="_blank" class="scroll-btn rehost-btn success" style="text-decoration:none;">
              <div class="scroll-btn-icon"><i class="fa-solid fa-arrow-up-right-from-square"></i></div>
              <span class="scroll-btn-label">View</span>
            </a>
          `;
        }, 800);

        // Update scroll-id-link to show local ID
        if (slide) {
          const idLink = slide.querySelector('.scroll-id-link');
          if (idLink) {
            idLink.href = `/${data.item_path || data.item_id}`;
            idLink.textContent = `/${data.item_path || data.item_id}`;
          }
          // Reflect rehoster's username and local timestamp
          if (window.scrollerUsername) {
            slide.querySelectorAll('.scroll-user-link').forEach(link => {
              link.href = `/user/${window.scrollerUsername}`;
            });
            const nameEl = slide.querySelector('.scroll-username');
            if (nameEl) nameEl.textContent = window.scrollerDisplayName || window.scrollerUsername;
            const avatarEl = slide.querySelector('.scroll-avatar');
            if (avatarEl) avatarEl.src = window.scrollerUserAvatar || '/a/default.png';
          }
          const timeEl = slide.querySelector('.scroll-timeago');
          if (timeEl) timeEl.textContent = (window.f0ckI18n && window.f0ckI18n.just_now) || 'just now';
          // Enable rating cycle now that item has a local_id
          const rBadge = slide.querySelector('.scroll-rating[data-item-id]');
          if (rBadge) rBadge.classList.add('can-cycle');
        }
      } else {
        throw new Error(data.msg);
      }
    } catch (e) {
      btn.classList.remove('loading');
      icon.className = origClass;
      alert(((window.f0ckI18n && window.f0ckI18n.rehost_failed) || 'Rehost failed: {msg}').replace('{msg}', e.message));
    }
  }

  function cycleRatingOptimistic(badge, id, showToast = false) {
    if (!badge || !id || isNaN(id)) return;

    let currentRating = badge.dataset.rating || '';
    if (!currentRating) {
      if (badge.classList.contains('sfw')) currentRating = 'sfw';
      else if (badge.classList.contains('nsfw')) currentRating = 'nsfw';
      else if (badge.classList.contains('nsfl')) currentRating = 'nsfl';
    }

    let nextRating = 'sfw';
    if (currentRating === 'sfw') nextRating = 'nsfw';
    else if (currentRating === 'nsfw') nextRating = 'nsfl';

    const mapping = {
      sfw: { label: 'SFW', cls: 'sfw', toast: '🛡 SFW' },
      nsfw: { label: 'NSFW', cls: 'nsfw', toast: '🔥 NSFW' },
      nsfl: { label: 'NSFL', cls: 'nsfl', toast: '💀 NSFL' }
    };

    const info = mapping[nextRating];

    const oldClassName = badge.className;
    const oldTextContent = badge.textContent;
    const oldDatasetRating = badge.dataset.rating;

    // Track active request ID to ignore out-of-order race conditions on rapid keypresses
    const reqId = (badge._lastCycleReqId || 0) + 1;
    badge._lastCycleReqId = reqId;

    // Optimistically apply new state
    badge.className = `scroll-rating ${info.cls}${window.scrollerIsMod ? ' can-cycle' : ''}`;
    badge.textContent = info.label;
    badge.dataset.rating = info.cls;

    if (showToast) {
      showShareToast(info.toast);
    }

    const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
    fetch(`/api/v2/tags/${id}/cycle-rating`, {
      method: 'PUT',
      headers: { ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {}) }
    })
      .then(r => r.json())
      .then(data => {
        if (badge._lastCycleReqId !== reqId) return; // ignore stale responses
        if (!data.success) {
          revert();
          return;
        }
        // Verify we match actual server result
        badge.className = `scroll-rating ${data.rating_class}${window.scrollerIsMod ? ' can-cycle' : ''}`;
        badge.textContent = data.rating_label;
        badge.dataset.rating = data.rating_class;
      })
      .catch(() => {
        if (badge._lastCycleReqId !== reqId) return; // ignore stale responses
        revert();
      });

    function revert() {
      badge.className = oldClassName;
      badge.textContent = oldTextContent;
      badge.dataset.rating = oldDatasetRating;
      showShareToast('⚠️ Failed to update rating');
    }
  }

  function reloadFeed() {
    clearCache();
    // Pause previous
    if (currentMedia) {
      try {
        currentMedia.pause();
        // If it's a Ruffle player, destroy it immediately for clean transition
        if (currentMedia.tagName === 'RUFFLE-PLAYER') {
          const container = currentMedia.parentElement;
          currentMedia.remove();
          if (container && container._rufflePlayer === currentMedia) {
            container._rufflePlayer = null;
            const ph = container.querySelector('.ruffle-placeholder');
            if (ph) ph.style.display = 'block';
          }
        }
      } catch {}
    }
    currentSlide = null; currentMedia = null;
    // Close gallery sidebar if open and hide button
    if (galleryOpen) toggleGallery();
    if (chanGalleryBtn) chanGalleryBtn.style.display = 'none';
    feed.querySelectorAll('.scroll-slide').forEach(s => { slideObserver.unobserve(s); s.remove(); });
    emptyEl.classList.remove('show');
    emptyEl.innerHTML = `<i class="fa-solid fa-binoculars"></i><p>${(window.f0ckI18n && window.f0ckI18n.nothing_found) || 'Nothing found with current filters'}</p><button data-on="click:scroller-open-filters" style="margin-top:8px;padding:8px 20px;border-radius:50px;background:rgba(255,255,255,.08);border:1px solid rgba(255,255,255,.15);color:#fff;cursor:pointer;font-size:.8rem;">${(window.f0ckI18n && window.f0ckI18n.adjust_filters) || 'Adjust filters'}</button>`;

    hasMore = true; isFetching = false; lastCursor = null; consecutiveEmpty = 0; seenIds = new Set();
    feed.scrollTop = 0;
    // Re-show loader for the next fetch cycle
    loader.style.display = '';
    loader.classList.remove('hidden');
    history.replaceState(null, '', '/abyss');
    fetchItems(); updateFilterSummary();
  }

  /** Instantly filter feed by a single tag (called from tag pill click). */
  function filterByTag(tagDisplay) {
    const normalized = tagDisplay.toLowerCase().replace(/[^a-z0-9]/g, '');
    applied = { ...applied, tags: [normalized] };
    pending = { ...applied, tags: [normalized] };
    reloadFeed();
    syncPanelUI();
  }

  // ── Observers ─────────────────────────────────────────────────────────────
  let scrollingToRandom = null; // slide element we're scrolling to, suppresses intermediate activations
  const slideObserver = new IntersectionObserver((entries) => {
    entries.forEach(e => {
      // Ignore intersection changes caused by viewport resize when the
      // keyboard hides after panel close — would activate the wrong slide
      if (panelOpen) return;
      // During random-scroll, only activate the target slide
      if (scrollingToRandom) {
        if (e.isIntersecting && e.intersectionRatio >= 0.6 && e.target === scrollingToRandom) {
          scrollingToRandom = null;
          activateSlide(e.target);
        }
        return;
      }
      if (e.isIntersecting && e.intersectionRatio >= 0.6) activateSlide(e.target);
    });
  }, { root: feed, threshold: 0.6 });

  // Primary: sentinel intersection
  const sentinelObserver = new IntersectionObserver(entries => {
    if (entries[0].isIntersecting) fetchItems();
  }, { root: feed, threshold: 0.1 });
  sentinelObserver.observe(sentinel);

  // Fallback: scroll event to catch cases where sentinel observer fires but
  // isFetching was true at that moment and the observer won't re-fire.
  let scrollFallbackTimer = null;
  feed.addEventListener('scroll', () => {
    clearTimeout(scrollFallbackTimer);
    scrollFallbackTimer = setTimeout(() => {
      if (panelOpen) return; // don't trigger fetch while a panel is open
      const { scrollTop, scrollHeight, clientHeight } = feed;
      if (scrollHeight - scrollTop - clientHeight < clientHeight * 1.5) {
        fetchItems();
      }
    }, 120);
  }, { passive: true });

  const mutObs = new MutationObserver(mutations => {
    mutations.forEach(m => m.addedNodes.forEach(node => {
      if (node.classList && node.classList.contains('scroll-slide')) slideObserver.observe(node);
    }));
  });
  mutObs.observe(feed, { childList: true });

  // ── Comments ──────────────────────────────────────────────────────────────
  // ── Reply state ──────────────────────────────────────────────────────────
  let replyToCommentId = null;
  let replyToUsername = null;

  function setReplyTo(commentId, username) {
    replyToCommentId = commentId;
    replyToUsername = username;
    const _i = window.f0ckI18n || {};
    const indicator = document.getElementById('reply-indicator');
    if (indicator) {
      indicator.querySelector('.reply-indicator-text').textContent =
        (_i.replying_to || 'Replying to {user}').replace('{user}', '@' + username);
      indicator.style.display = 'flex';
    }
    if (commentInput) {
      const quote = `>>${commentId} `;
      const start = commentInput.selectionStart;
      const end = commentInput.selectionEnd;
      const val = commentInput.value;
      commentInput.value = val.substring(0, start) + quote + val.substring(end);
      commentInput.focus();
      commentInput.selectionStart = commentInput.selectionEnd = start + quote.length;
      commentInput.dispatchEvent(new Event('input', { bubbles: true }));
    }
  }

  function quoteComment(id, username) {
    if (!commentInput) return;
    const commentEl = commentsList.querySelector(`.comment-item[data-comment-id="${id}"]`);
    if (!commentEl) return;
    const contentEl = commentEl.querySelector('.comment-content');
    if (!contentEl) return;

    let raw = (contentEl.dataset.raw || '').replace(/<br\s*\/?>/gi, '\n').trim();
    if (raw.includes('&gt;') || raw.includes('&lt;') || raw.includes('&amp;')) {
      const txt = document.createElement('textarea');
      txt.innerHTML = raw;
      raw = txt.value;
    }
    const lines = raw.split('\n');
    const quote = `>>${id}\n${lines.map(line => `>${line}`).join('\n')}\n`;

    const start = commentInput.selectionStart;
    const end = commentInput.selectionEnd;
    const val = commentInput.value;

    commentInput.value = val.substring(0, start) + quote + val.substring(end);
    commentInput.focus();
    commentInput.selectionStart = commentInput.selectionEnd = start + quote.length;
    commentInput.dispatchEvent(new Event('input', { bubbles: true }));
  }

  function clearReply() {
    replyToCommentId = null;
    replyToUsername = null;
    const indicator = document.getElementById('reply-indicator');
    if (indicator) indicator.style.display = 'none';
  }

  const replyCancelBtn = document.getElementById('reply-cancel-btn');
  if (replyCancelBtn) replyCancelBtn.addEventListener('click', clearReply);

  function renderCommentEl(c, canReply) {
    const el = document.createElement('div'); el.className = 'comment-item';
    el.dataset.commentId = c.id || '';
    const isAnonGuest = window.f0ckSession?.is_anonymized ?? (window.f0ckSession?.guest_anonymize && !window.f0ckSession?.logged_in);
    const av = isAnonGuest ? '/a/default.png' : (c.avatar_file ? `/a/${c.avatar_file}` : (c.avatar ? `/t/${c.avatar}.webp` : '/a/default.png'));
    const nc = (!isAnonGuest && c.username_color) ? `color:${esc(c.username_color)}` : '';
    const _i = window.f0ckI18n || {};
    const uname = isAnonGuest ? 'anonymous' : (c.display_name || c.username || 'anon');
    el.innerHTML = `
      <img class="comment-avatar" src="${esc(av)}" alt="" loading="lazy" data-on="error:img-fallback" data-fallback="/a/default.png">
      <div class="comment-body">
        <div class="comment-username" style="${nc}">${esc(uname)}</div>
        <div class="comment-content" data-raw="${esc(c.content || '')}">${renderCommentContent(c.content || '')}</div>
        <div class="comment-meta">
          <span class="comment-time">${c.created_at ? timeAgo(c.created_at) : (_i.ta_just_now || _i.just_now || 'just now')}</span>
          ${(!isAnonGuest && canReply) ? `
            <button class="comment-reply-btn" data-id="${c.id}" data-user="${esc(c.username || uname)}">${_i.reply || 'Reply'}</button>
            <button class="comment-quote-btn" data-id="${c.id}" data-user="${esc(c.username || uname)}">${_i.quote || 'Quote'}</button>
          ` : ''}
        </div>
      </div>`;
    // Wire buttons
    const replyBtn = el.querySelector('.comment-reply-btn');
    if (replyBtn) {
      replyBtn.addEventListener('click', () => {
        setReplyTo(replyBtn.dataset.id, replyBtn.dataset.user);
      });
    }
    const quoteBtn = el.querySelector('.comment-quote-btn');
    if (quoteBtn) {
      quoteBtn.addEventListener('click', () => {
        quoteComment(quoteBtn.dataset.id, quoteBtn.dataset.user);
      });
    }
    return el;
  }

  async function openComments(itemId) {
    commentsItemId = itemId;
    clearReply();
    commentsOpenLink.href = `/${itemId}`;
    commentsLoading.style.display = 'block'; commentsEmpty.style.display = 'none'; commentsCount.textContent = '';
    commentsList.querySelectorAll('.comment-item').forEach(el => el.remove());
    openPanel(commentsPanel, commentsBackdrop);
    // Desktop only: focus input immediately (skip on touch to avoid surprise keyboard)
    if (commentInput && !window.matchMedia('(pointer: coarse)').matches) {
      setTimeout(() => commentInput.focus(), 120);
    }
    try {
      const resp = await fetch(`/api/comments/${itemId}?sort=new`);
      const data = await resp.json();
      commentsLoading.style.display = 'none';
      const visible = data.comments ? data.comments.filter(c => !c.is_deleted) : [];
      commentsCount.textContent = `(${visible.length})`;
      // Sync the slide action button with the real count from the server
      const slide = feed.querySelector(`.scroll-slide[data-id="${itemId}"]`);
      if (slide) {
        const btn = slide.querySelector('.js-comments-btn .scroll-btn-count');
        if (btn) btn.textContent = visible.length;
      }
      if (!visible.length) { commentsEmpty.style.display = 'block'; return; }
      const canReply = !!window.scrollerLoggedIn;
      visible.forEach(c => commentsList.appendChild(renderCommentEl(c, canReply)));
    } catch {
      commentsLoading.style.display = 'none'; commentsEmpty.style.display = 'block';
      commentsEmpty.innerHTML = `<i class="fa-solid fa-triangle-exclamation"></i><br>${(window.f0ckI18n && window.f0ckI18n.failed_load_comments) || 'Failed to load'}`;
    }
  }

  // ── Tag bar ───────────────────────────────────────────────────────────────
  const tagBar      = document.getElementById('tag-bar');
  const tagBarClose = document.getElementById('tag-bar-close-btn');

  function openTagBar(itemId) {
    closeAllPanels();
    tagBarItemId = itemId;
    if (!tagBar) return;
    tagBar.classList.add('open');
    // Focus the input after transition
    const inp = document.getElementById('scroll-tag-input');
    if (inp) setTimeout(() => inp.focus(), 240);
  }
  function closeTagBar() {
    if (!tagBar) return;
    tagBar.classList.remove('open');
    closeSugg();
    tagBarItemId = null;
    const inp = document.getElementById('scroll-tag-input');
    if (inp) { inp.value = ''; inp.blur(); }
  }
  if (tagBarClose) tagBarClose.addEventListener('click', closeTagBar);

  // ── Share panel ────────────────────────────────────────────────────────────
  const sharePanel    = document.getElementById('share-panel');
  const shareBackdrop = document.getElementById('share-backdrop');
  const shareCopyRow  = document.getElementById('share-copy-row');
  const shareCopySub  = document.getElementById('share-copy-sub');
  const shareDmRow    = document.getElementById('share-dm-row');
  const shareDmSearch = document.getElementById('share-dm-search');
  const shareUserInput  = document.getElementById('share-user-input');
  const shareUserResults = document.getElementById('share-user-results');
  let sharePanelUrl   = '';
  let shareUserTimer  = null;
  const SHARE_RECENTS_KEY = 'scroller_share_recents';

  function getShareRecents() {
    try { return JSON.parse(localStorage.getItem(SHARE_RECENTS_KEY) || '[]'); } catch { return []; }
  }
  function saveShareRecent(u) {
    const list = getShareRecents().filter(r => r.user !== u.user);
    list.unshift({ user: u.user, display_name: u.display_name || '', avatar: u.avatar || '', avatar_file: u.avatar_file || '' });
    localStorage.setItem(SHARE_RECENTS_KEY, JSON.stringify(list.slice(0, 5)));
  }

  function renderShareRow(u, container, label) {
    const row = document.createElement('div');
    row.className = 'share-user-row';
    const av = u.avatar_file ? `/a/${u.avatar_file}` : (u.avatar ? `/t/${u.avatar}.webp` : '/a/default.png');
    row.innerHTML = `<img src="${esc(av)}" data-on="error:img-fallback" data-fallback="/a/default.png" alt=""><span class="share-user-row-name">${esc(u.display_name || u.user)}</span>`;
    if (label) {
      const lbl = document.createElement('span');
      lbl.className = 'share-user-row-label'; lbl.textContent = label;
      row.appendChild(lbl);
    }
    row.addEventListener('click', async () => {
      row.style.opacity = '0.5'; row.style.pointerEvents = 'none';
      try {
        await sendDmSilent(u.user, sharePanelUrl);
        saveShareRecent(u);
        closeSharePanel();
        showShareToast(`✓ Sent to ${u.display_name || u.user}`);
      } catch (err) {
        row.style.opacity = ''; row.style.pointerEvents = '';
        showShareToast(err.message, true);
      }
    });
    container.appendChild(row);
    return row;
  }

  function showShareRecents() {
    if (!shareUserResults) return;
    const recents = getShareRecents();
    shareUserResults.innerHTML = '';
    if (!recents.length) { shareUserResults.classList.remove('show'); return; }
    const hdr = document.createElement('div');
    hdr.className = 'share-recents-label';
    hdr.textContent = (window.f0ckI18n && window.f0ckI18n.recent) || 'Recent';
    shareUserResults.appendChild(hdr);
    recents.forEach(u => renderShareRow(u, shareUserResults));
    shareUserResults.classList.add('show');
  }

  function openSharePanel(url) {
    sharePanelUrl = url || '';
    // Reset state
    if (shareCopySub) shareCopySub.textContent = (window.f0ckI18n && window.f0ckI18n.copy_clipboard) || 'Copy to clipboard';
    if (shareDmSearch) shareDmSearch.classList.remove('show');
    if (shareUserInput) shareUserInput.value = '';
    if (shareUserResults) { shareUserResults.innerHTML = ''; shareUserResults.classList.remove('show'); }
    if (sharePanel) sharePanel.classList.add('open');
    if (shareBackdrop) shareBackdrop.classList.add('show');
  }

  function closeSharePanel() {
    if (sharePanel) sharePanel.classList.remove('open');
    if (shareBackdrop) shareBackdrop.classList.remove('show');
    if (shareDmSearch) shareDmSearch.classList.remove('show');
    if (shareUserResults) { shareUserResults.innerHTML = ''; shareUserResults.classList.remove('show'); }
    if (shareUserInput) shareUserInput.value = '';
  }

  if (shareBackdrop) shareBackdrop.addEventListener('click', closeSharePanel);

  // Copy link row
  if (shareCopyRow) {
    shareCopyRow.addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(sharePanelUrl);
        if (shareCopySub) {
          shareCopySub.textContent = (window.f0ckI18n && window.f0ckI18n.copied) || 'Copied ✓';
          setTimeout(() => { if (shareCopySub) shareCopySub.textContent = (window.f0ckI18n && window.f0ckI18n.copy_clipboard) || 'Copy to clipboard'; }, 1800);
        }
      } catch {
        window.prompt('Copy link:', sharePanelUrl);
      }
    });
  }

  // Send via DM row — toggle user search
  if (shareDmRow) {
    shareDmRow.addEventListener('click', () => {
      if (!shareDmSearch) return;
      const showing = shareDmSearch.classList.toggle('show');
      if (showing) {
        showShareRecents(); // show recent recipients immediately
        if (shareUserInput) shareUserInput.focus();
      }
    });
  }

  // User search for DM
  // ── Silent DM send with Web Crypto (E2E, same scheme as messages.js) ────────
  const DM_KEY_NAME = 'f0ck_dm_privkey';

  function b64u(buf) {
    const u8 = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
    let s = ''; for (const b of u8) s += String.fromCharCode(b);
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
  }

  async function sendDmSilent(recipientUsername, text) {
    const subtle = crypto.subtle;
    // 1. Resolve user → user_id
    const resolveResp = await fetch(`/api/dm/resolve/${encodeURIComponent(recipientUsername)}`);
    const resolveData = await resolveResp.json();
    if (!resolveData.success) throw new Error(resolveData.msg || 'User not found');
    const recipientId = resolveData.user_id;
    // 2. Load own private key
    const privJwk = localStorage.getItem(DM_KEY_NAME);
    if (!privJwk) throw new Error('No encryption key — open Messages to set one up first');
    // 3. Fetch recipient public key
    const pubResp = await fetch(`/api/dm/pubkey/${recipientId}`);
    const pubData = await pubResp.json();
    if (!pubData.success) throw new Error("Recipient hasn't set up their encryption key yet");
    // 4. ECDH derive shared key
    const privKey = await subtle.importKey('jwk', JSON.parse(privJwk), { name: 'ECDH', namedCurve: 'P-256' }, true, ['deriveKey', 'deriveBits']);
    const pubKey  = await subtle.importKey('jwk', JSON.parse(pubData.pubkey), { name: 'ECDH', namedCurve: 'P-256' }, true, []);
    const sharedKey = await subtle.deriveKey({ name: 'ECDH', public: pubKey }, privKey, { name: 'AES-GCM', length: 256 }, false, ['encrypt']);
    // 5. Encrypt
    const iv  = crypto.getRandomValues(new Uint8Array(12));
    const enc = await subtle.encrypt({ name: 'AES-GCM', iv }, sharedKey, new TextEncoder().encode(text));
    // 6. Send
    const csrf = window.f0ckSession?.csrf_token || '';
    const sendResp = await fetch(`/api/dm/send/${recipientId}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'X-CSRF-Token': csrf },
      body: new URLSearchParams({ ciphertext: b64u(enc), iv: b64u(iv) }).toString()
    });
    const sendData = await sendResp.json();
    if (!sendData.success) throw new Error(sendData.msg || 'Failed to send');
  }

  function showShareToast(msg, isError = false) {
    let toast = document.getElementById('share-toast');
    if (!toast) {
      toast = document.createElement('div');
      toast.id = 'share-toast';
      toast.style.cssText = 'position:fixed;bottom:80px;left:50%;transform:translateX(-50%) translateY(20px);'
        + 'background:rgba(20,20,28,.95);color:#fff;padding:10px 18px;border-radius:22px;'
        + 'font-size:.82rem;font-weight:600;z-index:2000;pointer-events:none;'
        + 'box-shadow:0 4px 20px rgba(0,0,0,.5);opacity:0;transition:opacity .18s,transform .18s;';
      document.body.appendChild(toast);
    }
    toast.textContent = msg;
    toast.style.background = isError ? 'rgba(200,40,40,.95)' : 'rgba(20,20,28,.95)';
    toast.style.opacity = '1'; toast.style.transform = 'translateX(-50%) translateY(0)';
    clearTimeout(toast._t);
    toast._t = setTimeout(() => { toast.style.opacity = '0'; toast.style.transform = 'translateX(-50%) translateY(20px)'; }, 2400);
  }

  async function fetchShareUsers(q) {
    try {
      const r = await fetch(`/api/v2/users/suggest?q=${encodeURIComponent(q)}`);
      const data = await r.json();
      if (!shareUserResults) return;
      const users = data.suggestions || [];
      shareUserResults.innerHTML = '';
      if (!users.length) { shareUserResults.classList.remove('show'); return; }
      users.forEach(u => renderShareRow(u, shareUserResults));
      shareUserResults.classList.add('show');
    } catch {}
  }

  if (shareUserInput) {
    shareUserInput.addEventListener('input', () => {
      const q = shareUserInput.value.trim();
      clearTimeout(shareUserTimer);
      if (!q) { showShareRecents(); return; }
      shareUserTimer = setTimeout(() => fetchShareUsers(q), 230);
    });
  }

  // ── Sitewide emoji picker ─────────────────────────────────────────────────
  // Loads custom emojis from /api/v2/emojis (same source as comments.js).
  // Wires the ☺ trigger button to show an image picker grid,
  // AND updates the :shortcode: inline autocomplete to prefer custom emojis.
  let customEmojis = {};  // { name: url }
  let emojiPickerEl = null;
  let emojiPickerCloseHandler = null;

  async function loadCustomEmojis() {
    // Reuse cache from comments.js if already loaded
    if (window.CommentSystem?.emojiCache) {
      customEmojis = window.CommentSystem.emojiCache;
      return;
    }
    try {
      const res = await fetch('/api/v2/emojis');
      const data = await res.json();
      if (data.emojis) data.emojis.forEach(e => { customEmojis[e.name] = e.url; });
    } catch {}
  }

  /**
   * Render comment text: escapes HTML then replaces :emoji_name: with <img> tags
   * for any custom (site) emoji that's loaded. Unknown shortcodes stay as text.
   */
  function renderCommentContent(text) {
    if (!text) return '';

    // Build allowed-image host regex (same origin + f0ckAllowedImages list)
    const escH = s => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const siteHost = escH(window.location.host);
    const allowedHosts = [siteHost];
    if (window.f0ckAllowedImages && Array.isArray(window.f0ckAllowedImages)) {
      window.f0ckAllowedImages.forEach(h => allowedHosts.push(`(?:[a-z0-9-]+\\.)*${escH(h)}`));
    }
    const hostsPart = allowedHosts.join('|');
    const safeS = '(?:(?!https?:\\/\\/)\\S)';
    const domainOrRel = `(?:(?:https?:\\/\\/|\\/\\/)?(?:${hostsPart})|(?=\\/[a-zA-Z0-9_\\-]))`;
    const imageRe = new RegExp(
      `(?<![\\(\\[\\"])(${domainOrRel}(?:\\/${safeS}+\\.(?:jpg|jpeg|png|gif|webp)(?:\\?${safeS}+)?))(?![\\)\\]])`,
      'gi'
    );

    // Escape HTML first, then process line by line
    const normalized = text.replace(/<br\s*\/?>/gi, '\n');
    const escaped = esc(normalized);
    const lines = escaped.split('\n').map(line => {
      const trimmed = line.trimStart();
      // Greentext / quote: lines starting with >
      // Exclude only the numeric context links (>>ID) so they can be handled as interactive links.
      if (trimmed.startsWith('&gt;') && !trimmed.match(/^&gt;&gt;\d+/)) {
        const after = line.substring(line.indexOf('&gt;') + 4);
        const withEmoji = after.replace(/:([a-z0-9_]+):/g, (m, name) => {
          const url = customEmojis[name];
          return url ? `<img src="${esc(url)}" alt=":${esc(name)}:" title=":${esc(name)}:" style="height:1.6em;vertical-align:middle;display:inline-block;margin:0 1px">` : m;
        });
        return `<span class="scroller-greentext">&gt;${withEmoji}</span>`;
      }
      // Blank lines → small spacer
      if (!trimmed) return '<span style="display:block;height:.3em"></span>';
      // 1. Replace image URLs from allowed hosts FIRST (before emoji, so we never
      //    accidentally re-match the .png inside an already-inserted <img> src)
      let out = line.replace(imageRe, (_, url) => {
        const fullUrl = url.startsWith('http') || url.startsWith('//') || url.startsWith('/') ? url : '//' + url;
        return `<img src="${fullUrl}" alt="" style="max-width:100%;max-height:320px;border-radius:6px;display:block;margin:4px 0" loading="lazy">`;
      });
      // 2. Replace custom emoji shortcodes with images (runs on raw text so it
      //    won't see the <img> tags inserted above)
      out = out.replace(/:([a-z0-9_]+):/g, (m, name) => {
        const url = customEmojis[name];
        return url ? `<img src="${esc(url)}" alt=":${esc(name)}:" title=":${esc(name)}:" style="height:1.8em;vertical-align:middle;display:inline-block;margin:0 2px">` : m;
      });

      // 3. Replace >>ID patterns with context links
      out = out.replace(/(?<!\w)&gt;&gt;(\d+)/g, (match, id) => {
        return `<a href="#c${id}" class="comment-context-link" data-id="${id}">>>${id}</a>`;
      });

      return out;
    });
    let html = lines.map((line, i) => {
      if (i === lines.length - 1) return line;
      if (line.includes('scroller-greentext') || line.includes('display:block')) return line;
      return line + '<br>';
    }).join('');
    // [spoiler]…[/spoiler] — iterative to handle nesting
    const spoilerRe = /\[spoiler\]((?:(?!\[spoiler\])[\s\S])*?)\[\/spoiler\]/gi;
    let prev, n = 0;
    do { prev = html; html = html.replace(spoilerRe, (_, c) => `<span class="scroller-spoiler">${c}</span>`); } while (html !== prev && ++n < 10);
    // [blur]…[/blur] — iterative
    const blurRe = /\[blur\]((?:(?!\[blur\])[\s\S])*?)\[\/blur\]/gi;
    n = 0;
    do { prev = html; html = html.replace(blurRe, (_, c) => `<span class="scroller-blur">${c}</span>`); } while (html !== prev && ++n < 10);
    return html;
  }

  // Track insert position across emoji clicks (without needing input focused)
  let emojiInsertPos = null;

  function buildEmojiPickerEl() {
    const picker = document.createElement('div');
    picker.className = 'emoji-picker'; // reuse comments.css class if present
    picker.style.cssText = [
      'position:fixed','z-index:9999','bottom:80px','left:12px','right:12px',
      'max-height:220px','overflow-y:auto',
      'background:rgba(10,10,14,.97)','border:1px solid rgba(255,255,255,.12)',
      'border-radius:14px','padding:10px','display:flex','flex-wrap:wrap','gap:6px',
      'box-shadow:0 8px 40px rgba(0,0,0,.7)'
    ].join(';');
    Object.keys(customEmojis).forEach(name => {
      const img = document.createElement('img');
      img.src = customEmojis[name]; img.title = ':' + name + ':';
      img.alt = name; img.loading = 'lazy';
      img.style.cssText = 'width:40px;height:40px;object-fit:contain;cursor:pointer;border-radius:6px;transition:transform .1s';
      img.addEventListener('mouseenter', () => { img.style.transform = 'scale(1.2)'; });
      img.addEventListener('mouseleave', () => { img.style.transform = 'scale(1)'; });
      img.addEventListener('click', e => {
        e.preventDefault(); e.stopPropagation();
        if (!commentInput) return;
        const val = commentInput.value;
        const code = ':' + name + ':';
        // Use tracked position; fall back to end
        const pos = (emojiInsertPos !== null && emojiInsertPos <= val.length)
          ? emojiInsertPos : val.length;
        commentInput.value = val.slice(0, pos) + code + val.slice(pos);
        emojiInsertPos = pos + code.length; // advance for next insert
        // Update send button state — no focus(), picker stays open
        if (commentSendBtn) commentSendBtn.disabled = !commentInput.value.trim();
      });
      picker.appendChild(img);
    });
    if (!Object.keys(customEmojis).length) {
      picker.innerHTML = `<div style="color:rgba(255,255,255,.4);font-size:.8rem;padding:8px">${(window.f0ckI18n && window.f0ckI18n.no_custom_emojis) || 'No custom emojis'}</div>`;
    }
    return picker;
  }

  function hideEmojiPicker() {
    if (emojiPickerEl?.parentNode) emojiPickerEl.parentNode.removeChild(emojiPickerEl);
    emojiPickerEl = null;
    if (emojiPickerCloseHandler) { document.removeEventListener('click', emojiPickerCloseHandler); emojiPickerCloseHandler = null; }
  }

  const emojiTriggerBtn = document.getElementById('comment-emoji-trigger');
  if (emojiTriggerBtn) {
    // Load emojis once (don't wait)
    loadCustomEmojis();
    emojiTriggerBtn.addEventListener('click', e => {
      e.preventDefault(); e.stopPropagation();
      if (emojiPickerEl) { hideEmojiPicker(); return; }
      emojiPickerEl = buildEmojiPickerEl();
      document.body.appendChild(emojiPickerEl);
      emojiPickerEl.scrollIntoView?.({ behavior: 'smooth', block: 'nearest' });
      emojiPickerCloseHandler = ev => {
        if (!emojiPickerEl?.contains(ev.target) && ev.target !== emojiTriggerBtn) hideEmojiPicker();
      };
      setTimeout(() => document.addEventListener('click', emojiPickerCloseHandler), 0);
    });
  }

  // ── Comment input: @mention autocomplete ─────────────────────────────────
  function insertMention(username) {
    if (!commentInput) return;
    const val   = commentInput.value;
    const atIdx = val.lastIndexOf('@', commentInput.selectionStart - 1);
    if (atIdx === -1) return;
    const before = val.slice(0, atIdx);
    const after  = val.slice(commentInput.selectionStart);
    commentInput.value = before + '@' + username + ' ' + after;
    const pos = before.length + username.length + 2;
    commentInput.setSelectionRange(pos, pos);
    commentInput.focus();
    closeMentionDropdown();
    commentInput.dispatchEvent(new Event('input'));
  }

  function closeMentionDropdown() {
    if (mentionDropdown) { mentionDropdown.classList.remove('show'); mentionDropdown.innerHTML = ''; }
    mentionResults = []; mentionIndex = -1;
  }

  function renderMentionDropdown(users) {
    mentionResults = users;
    mentionIndex = -1;
    if (!mentionDropdown) return;
    if (!users.length) { closeMentionDropdown(); return; }
    mentionDropdown.innerHTML = '';
    users.forEach((u, i) => {
      const item = document.createElement('div'); item.className = 'mention-item'; item.dataset.idx = i;
      const av = u.avatar_file ? `/a/${u.avatar_file}` : (u.avatar ? `/t/${u.avatar}.webp` : '/a/default.png');
      item.innerHTML = `<img class="mention-avatar" src="${esc(av)}" data-on="error:img-fallback" data-fallback="/a/default.png">${esc(u.display_name || u.user)}`;
      item.addEventListener('mousedown', e => { e.preventDefault(); insertMention(u.user); });
      mentionDropdown.appendChild(item);
    });
    mentionDropdown.classList.add('show');
  }

  async function fetchMentions(q) {
    try {
      const r = await fetch(`/api/v2/users/suggest?q=${encodeURIComponent(q)}`);
      const data = await r.json();
      if (data.success) renderMentionDropdown(data.suggestions || []);
    } catch {}
  }

  // ── Emoji autocomplete (:shortcode:) ─────────────────────────────────────
  function closeAcDropdown() {
    if (mentionDropdown) { mentionDropdown.classList.remove('show'); mentionDropdown.innerHTML = ''; }
    emojiMode = false; emojiResults = []; mentionResults = []; acIndex = -1;
    mentionIndex = -1; // keep compat
  }

  function renderEmojiDropdown(matches) {
    emojiMode = true; emojiResults = matches; acIndex = -1;
    if (!mentionDropdown || !matches.length) { closeAcDropdown(); return; }
    mentionDropdown.innerHTML = '';
    matches.forEach(([code, char], i) => {
      const item = document.createElement('div'); item.className = 'mention-item'; item.dataset.idx = i;
      item.innerHTML = `<span style="font-size:1.2rem;line-height:1">${char}</span><span style="font-size:.8rem;color:rgba(255,255,255,.65)">:${code}:</span>`;
      item.addEventListener('mousedown', e => { e.preventDefault(); insertEmoji(char, code); });
      mentionDropdown.appendChild(item);
    });
    mentionDropdown.classList.add('show');
  }

  // Render custom (site) emoji suggestions: show img + name
  function renderCustomEmojiAC(matches) {
    emojiMode = true; emojiResults = matches; acIndex = -1;
    if (!mentionDropdown || !matches.length) { closeAcDropdown(); return; }
    mentionDropdown.innerHTML = '';
    matches.forEach(([, name, url], i) => {
      const item = document.createElement('div'); item.className = 'mention-item'; item.dataset.idx = i;
      item.innerHTML = `<img src="${esc(url)}" style="width:24px;height:24px;object-fit:contain;border-radius:4px" alt="${esc(name)}"><span style="font-size:.8rem;color:rgba(255,255,255,.65)">:${esc(name)}:</span>`;
      item.addEventListener('mousedown', e => { e.preventDefault(); insertCustomEmoji(name); });
      mentionDropdown.appendChild(item);
    });
    mentionDropdown.classList.add('show');
  }

  function insertCustomEmoji(name) {
    if (!commentInput) return;
    const val = commentInput.value;
    const cursor = commentInput.selectionStart;
    const textBefore = val.slice(0, cursor);
    const colonIdx = textBefore.lastIndexOf(':');
    if (colonIdx === -1) return;
    const code = ':' + name + ':';
    const before = val.slice(0, colonIdx);
    const after  = val.slice(cursor);
    commentInput.value = before + code + after;
    const pos = colonIdx + code.length;
    commentInput.setSelectionRange(pos, pos);
    commentInput.focus(); closeAcDropdown();
    commentInput.dispatchEvent(new Event('input'));
  }

  function insertEmoji(char, code) {
    if (!commentInput) return;
    const val = commentInput.value;
    const cursor = commentInput.selectionStart;
    const textBefore = val.slice(0, cursor);
    // Find the opening colon
    const colonIdx = textBefore.lastIndexOf(':');
    if (colonIdx === -1) return;
    const before = val.slice(0, colonIdx);
    const after  = val.slice(cursor);
    commentInput.value = before + char + after;
    const pos = colonIdx + char.length;
    commentInput.setSelectionRange(pos, pos);
    commentInput.focus();
    closeAcDropdown();
    commentInput.dispatchEvent(new Event('input'));
  }

  if (commentInput) {
    // Keep emojiInsertPos in sync with the real cursor whenever the user
    // types or moves the caret themselves (tap/click into the input, arrow keys, etc.)
    const syncInsertPos = () => { emojiInsertPos = commentInput.selectionStart; };
    commentInput.addEventListener('click', syncInsertPos);
    commentInput.addEventListener('keyup', syncInsertPos);

    commentInput.addEventListener('input', () => {
      // Sync position after typing
      emojiInsertPos = commentInput.selectionStart;
      // Resize
      commentInput.style.height = 'auto';
      commentInput.style.height = Math.min(commentInput.scrollHeight, 100) + 'px';
      if (commentSendBtn) commentSendBtn.disabled = !commentInput.value.trim();

      const cursor = commentInput.selectionStart;
      const textBefore = commentInput.value.slice(0, cursor);

      // 1. :emoji: shortcode detection — custom site emojis only
      const emojiMatch = textBefore.match(/:([a-z0-9_]{1,})$/);
      if (emojiMatch) {
        const q = emojiMatch[1];
        const customHits = Object.keys(customEmojis)
          .filter(n => n.startsWith(q)).slice(0, 12)
          .map(n => ['__custom__', n, customEmojis[n]]);
        if (customHits.length) {
          renderCustomEmojiAC(customHits);
          return;
        }
        closeAcDropdown(); return;
      }

      // 2. @mention detection
      const mentionMatch = textBefore.match(/@([a-zA-Z0-9_.\-]*)$/);
      if (mentionMatch) {
        mentionQuery = mentionMatch[1];
        emojiMode = false;
        clearTimeout(mentionTimer);
        if (mentionQuery.length >= 1) {
          mentionTimer = setTimeout(() => fetchMentions(mentionQuery), 250);
        } else {
          closeAcDropdown();
        }
      } else {
        closeAcDropdown();
      }
    });

    commentInput.addEventListener('keydown', e => {
      // Ctrl+Enter → submit comment (ignore if autocomplete is open with selection)
      if (e.key === 'Enter' && e.ctrlKey && !(mentionDropdown?.classList.contains('show') && acIndex >= 0)) {
        e.preventDefault();
        if (commentSendBtn && !commentSendBtn.disabled) commentSendBtn.click();
        return;
      }
      if (!mentionDropdown?.classList.contains('show')) return;
      const listLen = emojiMode ? emojiResults.length : mentionResults.length;
      if (e.key === 'ArrowDown') {
        e.preventDefault(); acIndex = Math.min(acIndex + 1, listLen - 1); mentionIndex = acIndex;
        mentionDropdown.querySelectorAll('.mention-item').forEach((el, i) => el.classList.toggle('selected', i === acIndex));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault(); acIndex = Math.max(acIndex - 1, 0); mentionIndex = acIndex;
        mentionDropdown.querySelectorAll('.mention-item').forEach((el, i) => el.classList.toggle('selected', i === acIndex));
      } else if ((e.key === 'Enter' || e.key === 'Tab') && acIndex >= 0) {
        e.preventDefault();
        if (emojiMode) {
          if (emojiResults[acIndex][0] === '__custom__') {
            const [,name,] = emojiResults[acIndex]; insertCustomEmoji(name);
          } else {
            const [code, char] = emojiResults[acIndex]; insertEmoji(char, code);
          }
        }
        else insertMention(mentionResults[acIndex].user);
      } else if (e.key === 'Escape') {
        closeAcDropdown();
      }
    });

    commentInput.addEventListener('blur', () => { setTimeout(closeAcDropdown, 200); });
  }

  // ── Post comment ──────────────────────────────────────────────────────────
  if (commentSendBtn) {
    commentSendBtn.addEventListener('click', async () => {
      const content = commentInput ? commentInput.value.trim() : '';
      if (!content || !commentsItemId || commentsPosting) return;
      commentsPosting = true; commentSendBtn.disabled = true;
      try {
        // Captcha first when the site requires one for comments (cap.comments); cancelling throws
        if (window.f0ckCaptchaGate) await window.f0ckCaptchaGate('comment');
        const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
        let postBody = `item_id=${commentsItemId}&content=${encodeURIComponent(content)}`;
        if (replyToCommentId) postBody += `&parent_id=${replyToCommentId}`;
        if (csrfToken) postBody += `&csrf_token=${encodeURIComponent(csrfToken)}`;
        const resp = await fetch('/api/comments', {
          method: 'POST',
          headers: {
            'Content-Type': 'application/x-www-form-urlencoded',
            ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
          },
          body: postBody
        });
        const data = await resp.json();
        if (data.success) {
          if (commentInput) { commentInput.value = ''; commentInput.style.height = 'auto'; }
          commentSendBtn.disabled = true; commentsEmpty.style.display = 'none';
          const displayName = window.scrollerDisplayName || window.scrollerUsername || 'you';
          const el = renderCommentEl({
            id: data.comment?.id,
            avatar_file: null,
            avatar: null,
            username: window.scrollerUsername,
            display_name: displayName,
            content: data.comment?.content ?? content,
            created_at: null
          }, !!window.scrollerLoggedIn);
          // Set avatar from global
          const avImg = el.querySelector('.comment-avatar');
          if (avImg) avImg.src = window.scrollerUserAvatar || '/a/default.png';
          commentsList.appendChild(el);
          commentsList.scrollTop = commentsList.scrollHeight;
          clearReply();
          // Update panel header count
          const cur = parseInt(commentsCount.textContent.replace(/\D/g, '') || '0');
          commentsCount.textContent = `(${cur + 1})`;
          // Update the slide action button count
          const slide = feed.querySelector(`.scroll-slide[data-id="${commentsItemId}"]`);
          if (slide) {
            const btn = slide.querySelector('.js-comments-btn .scroll-btn-count');
            if (btn) btn.textContent = cur + 1;
          }
        }
      } catch {
        // Failed or captcha cancelled: let the user send again
        commentSendBtn.disabled = !(commentInput && commentInput.value.trim());
      } finally { commentsPosting = false; }
    });
  }

  // ── Tag input ──────────────────────────────────────────────────────────────
  const addTagInput   = document.getElementById('scroll-tag-input');
  const addTagSendBtn = document.getElementById('scroll-tag-send-btn');
  const addTagSuggBox = document.getElementById('scroll-tag-suggestions');
  let tagAddSuggIdx   = -1;
  let tagAddSuggItems = [];
  let tagAddSuggTimer = null;
  let tagPosting      = false;

  function closeSugg() {
    if (addTagSuggBox) { addTagSuggBox.classList.remove('show'); addTagSuggBox.innerHTML = ''; }
    tagAddSuggItems = []; tagAddSuggIdx = -1;
  }

  function renderTagSugg(rows) {
    if (!addTagSuggBox || !rows.length) { closeSugg(); return; }
    addTagSuggBox.innerHTML = '';
    tagAddSuggItems = rows;
    tagAddSuggIdx = -1;
    rows.forEach((r, i) => {
      const el = document.createElement('div');
      el.className = 'scroll-tag-sugg-item'; el.dataset.idx = i;
      el.innerHTML = `<i class="fa-solid fa-tag" style="font-size:.65rem;color:rgba(255,255,255,.3)"></i>${esc(r.tag)}<span class="scroll-tag-sugg-count">${r.uses ?? ''}</span>`;
      
      el.addEventListener('mouseup', (ev) => {
        const sel = window.getSelection?.()?.toString().trim();
        if (!sel || sel === r.tag) return;
        el.addEventListener('click', (e) => e.stopImmediatePropagation(), { once: true, capture: true });
        ev.stopPropagation();
        window._showSelTagPopover?.(sel, el, (confirmed) => {
          window.getSelection?.()?.removeAllRanges();
          addTagInput.value = confirmed;
          closeSugg();
          submitTag();
        });
      });

      el.addEventListener('click', e => { 
        const sel = window.getSelection?.()?.toString().trim();
        if (sel && sel !== r.tag) {
           e.preventDefault();
           e.stopPropagation();
           return;
        }
        addTagInput.value = r.tag; 
        closeSugg(); 
        submitTag();
      });
      addTagSuggBox.appendChild(el);
    });
    addTagSuggBox.classList.add('show');
  }

  async function fetchTagSugg(q) {
    try {
      const r = await fetch(`/api/v2/scroller/tags?q=${encodeURIComponent(q)}`);
      renderTagSugg(await r.json());
    } catch {}
  }

  async function submitTag() {
    if (!addTagInput || tagPosting) return;
    // Always tag the currently active slide, regardless of which slide opened the bar
    const targetId = currentSlide?.dataset?.localId || currentSlide?.dataset?.id || tagBarItemId;
    if (!targetId || !/^\d+$/.test(targetId)) { showShareToast('Add to site first to tag'); return; }
    const tag = addTagInput.value.trim();
    if (!tag) return;
    tagPosting = true;
    if (addTagSendBtn) addTagSendBtn.disabled = true;
    closeSugg();
    try {
      const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
      const resp = await fetch(`/api/v2/tags/${targetId}`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/x-www-form-urlencoded',
          ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
        },
        body: `tagname=${encodeURIComponent(tag)}${csrfToken ? `&csrf_token=${encodeURIComponent(csrfToken)}` : ''}`
      });
      const data = await resp.json();
      if (data.success) {
        addTagInput.value = '';
        addTagInput.focus(); // stay focused for next tag
        // Apply full fresh tag list from response
        const targetSlide = currentSlide || feed.querySelector(`.scroll-slide[data-id="${targetId}"]`);
        if (targetSlide) applyFreshTags(targetSlide, (data.tags || []).filter(t => t.id > 2).map(t => t.tag).join(', '));
        // ✓ flash on send button
        if (addTagSendBtn) {
          addTagSendBtn.innerHTML = '<i class="fa-solid fa-check"></i>';
          setTimeout(() => { addTagSendBtn.innerHTML = '<i class="fa-solid fa-plus"></i>'; }, 900);
        }
      } else {
        addTagInput.placeholder = data.msg || 'Error adding tag';
        addTagInput.value = '';
        addTagInput.focus();
        setTimeout(() => { addTagInput.placeholder = 'Add a tag to this item…'; }, 2500);
      }
    } catch (err) {
      addTagInput.placeholder = 'Network error — try again';
      setTimeout(() => { addTagInput.placeholder = 'Add a tag to this item…'; }, 2500);
    } finally { tagPosting = false; if (addTagSendBtn) addTagSendBtn.disabled = false; }
  }

  if (addTagInput) {
    addTagInput.addEventListener('input', () => {
      const q = addTagInput.value.trim();
      clearTimeout(tagAddSuggTimer);
      if (q.length >= 2) tagAddSuggTimer = setTimeout(() => fetchTagSugg(q), 220);
      else closeSugg();
    });
    addTagInput.addEventListener('keydown', e => {
      if (addTagSuggBox?.classList.contains('show') && tagAddSuggItems.length) {
        if (e.key === 'ArrowDown') {
          e.preventDefault(); tagAddSuggIdx = Math.min(tagAddSuggIdx + 1, tagAddSuggItems.length - 1);
          addTagSuggBox.querySelectorAll('.scroll-tag-sugg-item').forEach((el, i) => el.classList.toggle('selected', i === tagAddSuggIdx));
        } else if (e.key === 'ArrowUp') {
          e.preventDefault(); tagAddSuggIdx = Math.max(tagAddSuggIdx - 1, 0);
          addTagSuggBox.querySelectorAll('.scroll-tag-sugg-item').forEach((el, i) => el.classList.toggle('selected', i === tagAddSuggIdx));
        } else if ((e.key === 'Enter' || e.key === 'Tab') && tagAddSuggIdx >= 0) {
          e.preventDefault(); addTagInput.value = tagAddSuggItems[tagAddSuggIdx].tag; closeSugg(); return;
        } else if (e.key === 'Escape') { closeSugg(); return; }
      }
      if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submitTag(); }
    });
    addTagInput.addEventListener('blur', () => setTimeout(closeSugg, 200));
  }
  if (addTagSendBtn) addTagSendBtn.addEventListener('click', submitTag);

  // ── Filter panel ──────────────────────────────────────────────────────────
  const modeLabels = { 0: 'SFW', 1: 'NSFW', 2: 'Unrated', 3: 'All', 4: 'NSFL' };

  filterOpenBtn.addEventListener('click', () => {
    pending = { ...applied, tags: [...applied.tags] }; syncPanelUI();
    tagInput.value = ''; tagClear.classList.remove('show'); tagSuggestEl.innerHTML = ''; renderActiveTags();
    openPanel(filterPanel, filterBackdrop);
  });

  function syncPanelUI() {
    const isGuest = getIsScrollerGuest();
    if (isGuest) {
      pending.mode = 0;
      applied.mode = 0;
    } else {
      document.querySelectorAll('#mode-pills .filter-pill').forEach(pill => {
        pill.classList.remove('locked');
        pill.disabled = false;
        pill.style.cursor = '';
        pill.style.opacity = '';
        pill.querySelectorAll('.fa-lock').forEach(i => i.remove());
      });
    }
    if (scrollerSingleMime) {
      pending.mime = scrollerSingleMime;
      applied.mime = scrollerSingleMime;
    }
    document.querySelectorAll('#mode-pills .filter-pill').forEach(p => p.classList.toggle('active', +p.dataset.mode === pending.mode));
    document.querySelectorAll('#mime-pills .filter-pill').forEach(p => p.classList.toggle('active', p.dataset.mime === pending.mime));
    document.querySelectorAll('#order-pills .filter-pill').forEach(p => p.classList.toggle('active', p.dataset.order === pending.order));
    if (externalUrlInput) externalUrlInput.value = pending.externalUrl || '';
    renderActiveTags();
  }

  function makePillListener(groupId, key, transform) {
    const el = document.getElementById(groupId);
    if (!el) return;
    el.querySelectorAll('.filter-pill').forEach(pill => {
      pill.addEventListener('click', (e) => {
        if (groupId === 'mode-pills' && (getIsScrollerGuest() || pill.classList.contains('locked') || pill.disabled)) {
          e.preventDefault();
          return;
        }
        if (groupId === 'mime-pills' && scrollerSingleMime) {
          e.preventDefault();
          return;
        }
        el.querySelectorAll('.filter-pill').forEach(p => p.classList.remove('active'));
        pill.classList.add('active'); pending[key] = transform ? transform(pill.dataset[key]) : pill.dataset[key];
      });
    });
  }

  window.addEventListener('f0ck:anon_session_ready', () => {
    if (window.f0ckSession) {
      window.f0ckSession.user = 'anonymous';
      window.f0ckSession.is_anon = true;
      window.f0ckSession.logged_in = true;
    }
    syncPanelUI();
  });
  makePillListener('mode-pills', 'mode', v => +v);
  makePillListener('mime-pills', 'mime', v => v);
  makePillListener('order-pills', 'order', v => v);

  filterApplyBtn.addEventListener('click', () => { applied = { ...pending, tags: [...pending.tags] }; closePanel(filterPanel, filterBackdrop); reloadFeed(); });

  if (externalUrlBtn) {
    externalUrlBtn.addEventListener('click', () => {
      const val = externalUrlInput.value.trim();
      if (!val) {
        applied.externalUrl = null;
      } else {
        if (!val.includes('boards.4chan.org')) {
           alert((window.f0ckI18n && window.f0ckI18n.invalid_chan_url) || 'Please enter a valid 4chan thread URL');
           return;
        }
        applied.externalUrl = val;
        applied.order = 'oldest';
      }
      closePanel(filterPanel, filterBackdrop);
      reloadFeed();
    });
  }

  // ── Auto-load 4chan thread from path or hash (e.g. /abyss/wsg/6211653 or #wsg/6211653) ────────────────
  let chanHashPending = null; // async promise if we need server lookup
  {
    const hid = hashId();
    const chanMatch = hid.match(/^([a-z0-9]+)\/(\d+)$/);
    if (chanMatch && !applied.externalUrl) {
      const board = chanMatch[1];
      const postno = chanMatch[2];
      // Look up thread ID from the post→thread mapping
      let tid = null;
      try {
        const postMap = JSON.parse(localStorage.getItem('4chan_post_threads') || '{}');
        tid = postMap[hid]; // hid is "board/postno"
      } catch(_) {}

      const effectiveTid = tid || postno;
      applied.externalUrl = `https://boards.4chan.org/${board}/thread/${effectiveTid}`;
      applied.order = 'oldest';
      unlock4chan();

      if (!tid) {
        // In case postno was a reply post rather than thread OP, query find in background
        chanHashPending = fetch(`/api/v2/scroller/external/4chan/${board}/find/${postno}`)
          .then(r => r.json())
          .then(data => {
            window.f0ckDebug && window.f0ckDebug('[CHAN] Find result:', data);
            if (data.success && data.tid) {
              try {
                const postMap = JSON.parse(localStorage.getItem('4chan_post_threads') || '{}');
                postMap[hid] = data.tid;
                localStorage.setItem('4chan_post_threads', JSON.stringify(postMap));
              } catch(_) {}
              if (String(data.tid) !== String(effectiveTid)) {
                applied.externalUrl = `https://boards.4chan.org/${board}/thread/${data.tid}`;
                applied.order = 'oldest';
                reloadFeed();
              }
            }
          })
          .catch(err => { console.error('[CHAN] Find error:', err); });
      }
    }
  }

  filterResetBtn.addEventListener('click', () => { pending = { mode: defaultMode, mime: scrollerSingleMime || '', order: 'random', tags: [] }; syncPanelUI(); tagInput.value = ''; tagClear.classList.remove('show'); tagSuggestEl.innerHTML = ''; lastSugg = []; renderActiveTags(); });

  function clearActiveFilters() {
    applied = { mode: defaultMode, mime: scrollerSingleMime || '', order: 'random', tags: [], externalUrl: null };
    pending = { ...applied, tags: [] };
    if (externalUrlInput) externalUrlInput.value = '';
    if (galleryOpen) toggleGallery();
    if (chanGalleryBtn) chanGalleryBtn.style.display = 'none';
    if (tagInput) tagInput.value = '';
    if (tagClear) tagClear.classList.remove('show');
    if (tagSuggestEl) tagSuggestEl.innerHTML = '';
    lastSugg = [];
    renderActiveTags();
    syncPanelUI();
    renderPresets();
    reloadFeed();
  }

  function setupFilterSummary() {
    if (!filterSummary) return;
    if (!filterSummaryText || !filterClearBtn) {
      if (!filterSummary.querySelector('#filter-active-summary-text')) {
        const txt = document.createElement('span');
        txt.id = 'filter-active-summary-text';
        const btn = document.createElement('button');
        btn.id = 'filter-active-clear';
        btn.type = 'button';
        btn.title = (window.f0ckI18n && window.f0ckI18n.clear_filter) || 'Clear filter';
        btn.setAttribute('aria-label', (window.f0ckI18n && window.f0ckI18n.clear_filter) || 'Clear filter');
        btn.innerHTML = '<i class="fa-solid fa-xmark"></i>';
        filterSummary.innerHTML = '';
        filterSummary.appendChild(txt);
        filterSummary.appendChild(btn);
      }
      filterSummaryText = document.getElementById('filter-active-summary-text');
      filterClearBtn = document.getElementById('filter-active-clear');
    }
    if (filterClearBtn && !filterClearBtn._hasListener) {
      filterClearBtn._hasListener = true;
      filterClearBtn.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        clearActiveFilters();
      });
    }
  }
  setupFilterSummary();

  function updateFilterSummary() {
    if (!filterSummary) return;
    setupFilterSummary();
    const is4chan = !!applied.externalUrl;
    const isDef = applied.mode === defaultMode && applied.mime === (scrollerSingleMime || '') && applied.order === 'random' && applied.tags.length === 0 && !is4chan;
    if (filterOpenBtn) filterOpenBtn.classList.toggle('has-filter', !isDef);
    filterSummary.classList.toggle('show', !isDef);
    if (!isDef) {
      // Check if current filters exactly match a saved preset
      const tagsKey = t => [...t].map(s => s.toLowerCase()).sort().join(',');
      const matchedPreset = getPresets().find(p =>
        p.mode === applied.mode &&
        (p.mime || '') === applied.mime &&
        p.order === applied.order &&
        tagsKey(p.tags) === tagsKey(applied.tags)
      );
      let summaryText = '';
      if (matchedPreset && !is4chan) {
        summaryText = matchedPreset.name;
      } else {
        const parts = [];
        if (is4chan) parts.push('4chan');
        if (applied.mode !== defaultMode) parts.push(modeLabels[applied.mode]);
        if (applied.mime) parts.push(applied.mime);
        if (applied.order !== 'random') parts.push(applied.order);
        parts.push(...applied.tags);
        summaryText = parts.join(' · ');
      }
      if (filterSummaryText) {
        filterSummaryText.textContent = summaryText;
      } else {
        filterSummary.textContent = summaryText;
      }
    }
  }

  function renderActiveTags() {
    activeTagsEl.innerHTML = '';
    pending.tags.forEach(tag => {
      const p = document.createElement('span'); p.className = 'active-tag-pill';
      p.innerHTML = `${esc(tag)}<i class="fa-solid fa-xmark"></i>`;
      p.addEventListener('click', () => { pending.tags = pending.tags.filter(t => t !== tag); renderActiveTags(); renderSugg(lastSugg); });
      activeTagsEl.appendChild(p);
    });
  }
  function renderSugg(sug) {
    lastSugg = sug; tagSuggestEl.innerHTML = '';
    sug.forEach(s => {
      const el = document.createElement('span'); el.className = 'tag-suggestion' + (pending.tags.includes(s.normalized) ? ' active' : '');
      el.innerHTML = `${esc(s.tag)}<span class="uses">${s.uses}</span>`;
      el.addEventListener('click', () => { if (pending.tags.includes(s.normalized)) pending.tags = pending.tags.filter(t => t !== s.normalized); else pending.tags.push(s.normalized); renderActiveTags(); renderSugg(lastSugg); });
      tagSuggestEl.appendChild(el);
    });
  }
  async function fetchTagSugg(q) {
    if (tagCache[q]) { renderSugg(tagCache[q]); return; }
    try { const r = await fetch(`/api/v2/scroller/tags?q=${encodeURIComponent(q)}`); tagCache[q] = await r.json(); renderSugg(tagCache[q]); } catch {}
  }
  tagInput.addEventListener('input', () => {
    const v = tagInput.value.trim();
    tagClear.classList.toggle('show', !!v);
    clearTimeout(tagTimer);
    tagSuggIdx = -1; // reset keyboard nav on new input
    if (!v) { tagSuggestEl.innerHTML = ''; return; }
    tagTimer = setTimeout(() => fetchTagSugg(v), 260);
  });
  tagClear.addEventListener('click', () => { tagInput.value = ''; tagClear.classList.remove('show'); tagSuggestEl.innerHTML = ''; tagSuggIdx = -1; tagInput.focus(); });

  function setSuggFocus(idx) {
    const items = tagSuggestEl.querySelectorAll('.tag-suggestion');
    items.forEach((el, i) => el.classList.toggle('keyboard-focus', i === idx));
    if (items[idx]) items[idx].scrollIntoView({ block: 'nearest' });
  }

  tagInput.addEventListener('keydown', e => {
    const items = tagSuggestEl.querySelectorAll('.tag-suggestion');
    if (!items.length) return;
    if (e.key === 'ArrowRight') {
      e.preventDefault();
      tagSuggIdx = (tagSuggIdx + 1) % items.length;
      setSuggFocus(tagSuggIdx);
    } else if (e.key === 'ArrowLeft') {
      e.preventDefault();
      tagSuggIdx = (tagSuggIdx <= 0 ? items.length : tagSuggIdx) - 1;
      setSuggFocus(tagSuggIdx);
    } else if (e.key === 'Enter' && tagSuggIdx >= 0) {
      e.preventDefault();
      items[tagSuggIdx]?.click(); // toggle the highlighted suggestion
      tagSuggIdx = -1;
    } else if (e.key === 'Escape') {
      tagSuggIdx = -1;
      setSuggFocus(-1);
      tagSuggestEl.innerHTML = '';
    }
  });

  // ── Keyboard ──────────────────────────────────────────────────────────────
  document.addEventListener('keydown', e => {
    if (!document.body.classList.contains('scroller-active')) return;
    const active = document.activeElement;
    const isTyping = active && (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA' || active.isContentEditable);

    // 'c' toggles comments — only when NOT typing (so it never fires inside the comment input)
    if ((e.key === 'c' || e.key === 'C') && !isTyping && !tagBar?.classList.contains('open')) {
      e.preventDefault();
      if (commentsPanel.classList.contains('open')) closeAllPanels();
      else if (currentSlide) openComments(currentSlide.dataset.localId || currentSlide.dataset.id);
      return;
    }

    // Tag bar open or typing: only Escape passes through
    if (tagBar?.classList.contains('open') || isTyping) {
      if (e.key === 'Escape') {
        if (tagBar?.classList.contains('open')) { closeTagBar(); e.preventDefault(); }
        else closeAllPanels();
      }
      return;
    }

    // Filter or comments panel open: only Escape passes through
    if (filterPanel.classList.contains('open') || commentsPanel.classList.contains('open')) {
      if (e.key === 'Escape') closeAllPanels(); return;
    }

    const slides = Array.from(feed.querySelectorAll('.scroll-slide'));
    const idx = currentSlide ? slides.indexOf(currentSlide) : 0;
    if (e.key === 'ArrowDown' || e.key === 'j') { e.preventDefault(); const n = slides[idx + 1]; if (n) n.scrollIntoView({ behavior: 'smooth' }); }
    else if (e.key === 'ArrowUp' || e.key === 'k') { e.preventDefault(); const p = slides[idx - 1]; if (p) p.scrollIntoView({ behavior: 'smooth' }); }
    else if (e.key === 'm') { e.preventDefault(); isMuted = !isMuted; if (!isMuted && volume === 0) volume = 0.5; syncVolumeUI(); applyVolumeToAll(); saveVolume(); prefs.startUnmuted = !isMuted; savePrefs(prefs); applyStartUnmuted(!isMuted); showVolumePopup(); }
    else if (e.key === ' ') { e.preventDefault(); if (currentMedia) currentMedia.paused ? currentMedia.play().catch(() => {}) : currentMedia.pause(); }
    else if (e.key === 'f' || e.key === 'F') { e.preventDefault(); pending = { ...applied, tags: [...applied.tags] }; syncPanelUI(); renderPresets(); openPanel(filterPanel, filterBackdrop); }
    else if (e.key === 'g' || e.key === 'G') { e.preventDefault(); if (applied.externalUrl && chanGalleryBtn) toggleGallery(); }
    else if (e.key === 'r' || e.key === 'R') {
      e.preventDefault();
      e.stopImmediatePropagation(); // prevent f0ckm.js global 'r' random shortcut from firing
      // Fetch a truly random item from the server with current filters
      (async () => {
        try {
          const params = new URLSearchParams({ limit: 1, mode: applied.mode, orderby: 'random' });
          if (applied.mime) params.set('mime', applied.mime);
          if (applied.tags.length > 0) params.set('tag', applied.tags.join(','));
          if (seenIds.size > 0) {
            const recent = [...seenIds].slice(-200);
            params.set('exclude', recent.join(','));
          }
          const resp = await fetch(`/api/v2/scroller/feed?${params}`);
          const data = await resp.json();
          if (data.items && data.items.length > 0) {
            const item = data.items[0];
            seenIds.add(item.id);
            const slide = createSlide(item);
            // Insert right after current slide — smooth scroll only travels 1 slide
            if (currentSlide && currentSlide.nextSibling) {
              feed.insertBefore(slide, currentSlide.nextSibling);
            } else {
              feed.appendChild(slide);
            }
            if (currentSlide) deactivateSlide(currentSlide);
            scrollingToRandom = slide;
            slide.scrollIntoView({ behavior: 'smooth', block: 'start' });
            // Safety: activate after 1.5s if observer didn't fire
            setTimeout(() => {
              if (scrollingToRandom === slide) {
                scrollingToRandom = null;
                activateSlide(slide);
              }
            }, 1500);
          }
        } catch (err) { console.warn('[scroller] random fetch failed', err); }
      })();
    }
    else if (e.key === 'p' || e.key === 'P') {
      e.preventDefault();
      if (!currentSlide || !window.scrollerLoggedIn) return;
      const itemId = currentSlide.dataset.localId || currentSlide.dataset.id;
      if (!itemId) return;
      const badge = currentSlide.querySelector('.scroll-rating[data-item-id]');
      if (badge) {
        cycleRatingOptimistic(badge, itemId, true);
      }
    }
    else if (e.key === 'l' || e.key === 'L') { e.preventDefault(); if (currentSlide) toggleFav(currentSlide); }
    else if (e.key === 'i' || e.key === 'I') { e.preventDefault(); if (currentSlide) openTagBar(currentSlide.dataset.id); }
    else if (e.key === 'e' || e.key === 'E') { e.preventDefault(); e.stopImmediatePropagation(); } // suppress upload modal shortcut in abyss
    else if (e.key === 's' || e.key === 'S') { e.preventDefault(); e.stopImmediatePropagation(); } // suppress flash yank shortcut in scroller
    else if (e.key === 'Escape') { window.location.href = '/'; }
  }, true); // capture phase — fires before f0ckm.js bubble listeners

  // ── 4chan thread loading: auto-show for mods, secret '4444' for everyone else
  function unlock4chan() {
    const section = document.getElementById('external-source-section');
    if (section) section.style.display = '';
    if (chanOpenBtn) chanOpenBtn.style.display = 'flex';
  }
  if (window.scrollerIsMod) {
    unlock4chan();
  } else {
    let fourCount = 0;
    let fourTimer = null;
    document.addEventListener('keydown', (e) => {
      if (!document.body.classList.contains('scroller-active')) return;
      if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;
      if (e.key === '4') {
        fourCount++;
        clearTimeout(fourTimer);
        fourTimer = setTimeout(() => { fourCount = 0; }, 1000);
        if (fourCount >= 4) {
          fourCount = 0;
          unlock4chan();
          showShareToast('4chan mode unlocked 🔓');
        }
      } else {
        fourCount = 0;
      }
    });
  }

  // ── 4chan Panel ──────────────────────────────────────────────────────────
  let chanCurrentBoard = 'wsg';
  let chanCatalogCache = {};

  if (chanOpenBtn && chanPanel && chanBackdrop) {
    chanOpenBtn.addEventListener('click', () => {
      openPanel(chanPanel, chanBackdrop);
      if (!chanCatalogCache[chanCurrentBoard]) loadCatalog(chanCurrentBoard);
    });
    chanBackdrop.addEventListener('click', () => closePanel(chanPanel, chanBackdrop));

    // Board pills
    document.querySelectorAll('#chan-board-pills .filter-pill').forEach(p => {
      p.addEventListener('click', () => {
        document.querySelectorAll('#chan-board-pills .filter-pill').forEach(b => b.classList.remove('active'));
        p.classList.add('active');
        chanCurrentBoard = p.dataset.board;
        if (chanCatalogCache[chanCurrentBoard]) {
          renderCatalog(chanCurrentBoard, chanCatalogCache[chanCurrentBoard]);
        } else {
          loadCatalog(chanCurrentBoard);
        }
      });
    });

    // Custom board input
    const chanCustomBoard = document.getElementById('chan-custom-board');
    const chanCustomBoardBtn = document.getElementById('chan-custom-board-btn');
    function loadCustomBoard() {
      const code = chanCustomBoard?.value.trim().toLowerCase().replace(/\//g, '');
      if (!code || !/^[a-z0-9]{1,6}$/.test(code)) { showShareToast('Invalid board code'); return; }
      document.querySelectorAll('#chan-board-pills .filter-pill').forEach(b => b.classList.remove('active'));
      chanCustomBoardBtn.classList.add('active');
      chanCurrentBoard = code;
      if (chanCatalogCache[code]) {
        renderCatalog(code, chanCatalogCache[code]);
      } else {
        loadCatalog(code);
      }
    }
    if (chanCustomBoardBtn) chanCustomBoardBtn.addEventListener('click', loadCustomBoard);
    if (chanCustomBoard) chanCustomBoard.addEventListener('keydown', e => { if (e.key === 'Enter') loadCustomBoard(); });

    // URL load button
    if (chanUrlLoadBtn && chanUrlInput) {
      chanUrlLoadBtn.addEventListener('click', () => {
        const val = chanUrlInput.value.trim();
        if (!val || !val.includes('boards.4chan.org')) {
          showShareToast('Invalid 4chan URL');
          return;
        }
        applied.externalUrl = val;
        closePanel(chanPanel, chanBackdrop);
        reloadFeed();
      });
      chanUrlInput.addEventListener('keydown', e => { if (e.key === 'Enter') chanUrlLoadBtn.click(); });
    }
  }

  async function loadCatalog(board) {
    if (chanLoading) chanLoading.style.display = 'block';
    if (chanGrid) chanGrid.innerHTML = '';
    try {
      const resp = await fetch(`/api/v2/scroller/external/4chan/${board}/catalog`);
      const data = await resp.json();
      if (data.success && data.threads) {
        chanCatalogCache[board] = data.threads;
        renderCatalog(board, data.threads);
      }
    } catch (err) {
      console.error('[CHAN] Catalog error:', err);
      if (chanGrid) chanGrid.innerHTML = `<div style="text-align:center;padding:20px;color:rgba(255,255,255,.4)">${(window.f0ckI18n && window.f0ckI18n.chan_catalog_failed) || 'Failed to load catalog'}</div>`;
    } finally {
      if (chanLoading) chanLoading.style.display = 'none';
    }
  }

  function renderCatalog(board, threads) {
    if (!chanGrid) return;
    chanGrid.innerHTML = '';
    const searchInput = document.getElementById('chan-catalog-search');
    if (searchInput) searchInput.value = '';

    threads.forEach(t => {
      const card = document.createElement('div');
      card.className = 'chan-thread-card' + (t.sticky ? ' sticky' : '');
      card.dataset.search = `${(t.sub || '')} ${(t.com || '')}`.toLowerCase();
      const thumbUrl = t.tim ? `/api/v2/scroller/external/4chan/${board}/media/${t.tim}s.jpg` : '';
      card.innerHTML = `
        ${thumbUrl ? `<img class="chan-thread-thumb" src="${thumbUrl}" loading="lazy" alt="" data-on="error:hide">` : '<div class="chan-thread-thumb" style="background:rgba(255,255,255,.06)"></div>'}
        <div class="chan-thread-info">
          <div class="chan-thread-sub">${esc(t.sub || t.com || 'No subject')}</div>
          ${t.com && t.sub ? `<div class="chan-thread-com">${esc(t.com)}</div>` : ''}
          <div class="chan-thread-stats">
            <span><i class="fa-solid fa-comment"></i>${t.replies}</span>
            <span><i class="fa-solid fa-image"></i>${t.images}</span>
            ${t.sticky ? '<span><i class="fa-solid fa-thumbtack"></i>Pinned</span>' : ''}
          </div>
        </div>`;
      card.addEventListener('click', () => {
        applied.externalUrl = `https://boards.4chan.org/${board}/thread/${t.no}`;
        applied.order = 'oldest';
        closePanel(chanPanel, chanBackdrop);
        reloadFeed();
      });
      chanGrid.appendChild(card);
    });
  }

  // Catalog search filter
  const chanCatalogSearch = document.getElementById('chan-catalog-search');
  if (chanCatalogSearch) {
    chanCatalogSearch.addEventListener('input', () => {
      const q = chanCatalogSearch.value.toLowerCase().trim();
      if (!chanGrid) return;
      chanGrid.querySelectorAll('.chan-thread-card').forEach(card => {
        card.style.display = !q || card.dataset.search.includes(q) ? '' : 'none';
      });
    });
  }

  // ── Thread Gallery Sidebar ──────────────────────────────────────────────
  let galleryOpen = false;

  function toggleGallery() {
    galleryOpen = !galleryOpen;
    if (chanGallerySidebar) chanGallerySidebar.classList.toggle('open', galleryOpen);
    if (chanGalleryBtn) chanGalleryBtn.classList.toggle('active', galleryOpen);
    document.body.classList.toggle('gallery-open', galleryOpen);
    if (galleryOpen) populateGallery();
  }

  function populateGallery() {
    if (!chanGallerySidebar) return;
    chanGallerySidebar.innerHTML = '';
    const slides = feed.querySelectorAll('.scroll-slide');
    slides.forEach((slide, i) => {
      const thumbSrc = slide._thumbnail || '';
      if (!thumbSrc) return;
      const wrap = document.createElement('div');
      wrap.className = 'gallery-thumb-wrap';
      const img = document.createElement('img');
      img.className = 'gallery-thumb';
      img.src = thumbSrc;
      img.loading = 'lazy';
      img.alt = '';
      if (slide === currentSlide) img.classList.add('active');
      img.dataset.slideId = slide.dataset.id || '';
      img.addEventListener('click', () => {
        slide.scrollIntoView({ behavior: 'smooth', block: 'start' });
      });
      const idx = document.createElement('span');
      idx.className = 'gallery-thumb-idx';
      idx.textContent = i + 1;
      wrap.appendChild(img);
      wrap.appendChild(idx);
      chanGallerySidebar.appendChild(wrap);
    });
    // Sync sidebar scroll to active thumbnail
    const activeTh = chanGallerySidebar.querySelector('.gallery-thumb.active');
    if (activeTh) activeTh.scrollIntoView({ block: 'center' });
  }

  function updateGalleryActive(slideId) {
    if (!chanGallerySidebar || !galleryOpen) return;
    chanGallerySidebar.querySelectorAll('.gallery-thumb').forEach(th => {
      const isActive = th.dataset.slideId === slideId;
      th.classList.toggle('active', isActive);
      if (isActive) th.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    });
  }

  if (chanGalleryBtn) {
    chanGalleryBtn.addEventListener('click', toggleGallery);
  }

  // ── Touch: let native CSS scroll-snap handle everything on mobile ─────────
  // (Removing custom touchstart/touchend handlers that caused jank by fighting
  //  the browser's native momentum scrolling and snap behaviour.)
  // The IntersectionObserver + fallback scroll listener are sufficient.

  // ── Notification badge in topbar ──────────────────────────────────────────
  const notifBadge = document.getElementById('scroller-notif-badge');
  if (notifBadge && window.scrollerLoggedIn) {
    const updateScrollerNotifBadge = (count) => {
      if (count > 0) {
        notifBadge.textContent = count > 99 ? '99+' : count;
        notifBadge.style.display = 'block';
      } else {
        notifBadge.style.display = 'none';
      }
    };

    // Fetch unread count from the API
    const pollNotifCount = async () => {
      try {
        const res = await fetch('/api/notifications');
        const data = await res.json();
        if (data.success) {
          updateScrollerNotifBadge((data.notifications || []).filter(n => !n.is_read).length);
        }
      } catch {}
    };

    // ── Lightweight live socket for instant badge updates ──────────
    const tabId = Math.random().toString(36).slice(2);
    let liveWs = null;
    let liveRetryCount = 0;
    let liveRetryTimer = null;
    const LIVE_MAX_RETRIES = 8;

    const closeScrollerSocket = () => {
      clearTimeout(liveRetryTimer);
      if (liveWs) { liveWs.onclose = null; liveWs.close(); liveWs = null; }
    };

    const initScrollerSocket = () => {
      closeScrollerSocket();
      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      let wsUrl = `${proto}//${location.host}/api/notifications/ws?tabId=${tabId}`;
      try {
        const fp = localStorage.getItem('f0ck_anon_ssh_fp');
        if (fp) wsUrl += `&fp=${encodeURIComponent(fp)}`;
        const hw = localStorage.getItem('f0ck_anon_hw_fp');
        if (hw) wsUrl += `&hw=${encodeURIComponent(hw)}`;
      } catch (e) {}
      const ws = new WebSocket(wsUrl);
      liveWs = ws;
      ws.onopen = () => { liveRetryCount = 0; };
      ws.onmessage = (e) => {
        try {
          const data = JSON.parse(e.data);
          if (data.type === 'banned') {
            try {
              const tombstoneData = { banned: true, reason: data.data?.reason, expires: data.data?.expires };
              document.cookie = `f0ck_banned=${encodeURIComponent(JSON.stringify(tombstoneData))}; Path=/; Max-Age=31536000; SameSite=Lax`;
            } catch (err) {}
            window.location.href = data.data?.redirect || '/banned';
            return;
          }
          if (data.type === 'notify') {
            pollNotifCount(); // instant re-fetch on live push
            if (navigator.vibrate) navigator.vibrate([200, 80, 200]);
          }
        } catch {}
      };
      ws.onclose = () => {
        if (liveWs !== ws) return;
        liveWs = null;
        if (document.hidden || liveRetryCount >= LIVE_MAX_RETRIES) return;
        liveRetryTimer = setTimeout(initScrollerSocket, Math.min(Math.pow(2, liveRetryCount++) * 1000, 30000));
      };
    };

    // Signal server we're active
    fetch(`/api/notifications/active?tabId=${tabId}`).catch(() => {});
    initScrollerSocket();

    // Fallback poll every 60s in case the socket drops
    const notifPollTimer = setInterval(pollNotifCount, 60000);
    pollNotifCount(); // initial count on load

    // Also honor live hook from f0ckm.js if navigated via PJAX
    window._scrollerNotifHook = updateScrollerNotifBadge;

    // ── Scroller Notification Dropdown ──────────────────────────────────
    const sNotifBtn = document.getElementById('scroller-notif-btn');
    const sNotifDropdown = document.getElementById('scroller-notif-dropdown');
    const sNotifList = document.getElementById('scroller-notif-list');
    const sMarkAll = document.getElementById('scroller-mark-all-read');

    // Tab type arrays
    const SCROLLER_USER_TYPES = ['comment_reply', 'subscription', 'mention', 'upload_comment'];
    const sActiveTabEl = sNotifDropdown ? sNotifDropdown.querySelector('.notif-tab.active') : null;
    let sActiveTab = sActiveTabEl ? sActiveTabEl.dataset.tab : ((window.f0ckEnableComments === false) ? 'system' : 'user');
    let sCachedNotifs = [];

    if (sNotifBtn && sNotifDropdown) {
      let sNotifOpen = false;

      function positionDropdown() {
        const rect = sNotifBtn.getBoundingClientRect();
        const dw = sNotifDropdown.offsetWidth || 340;
        let left = rect.left + rect.width / 2 - dw / 2;
        // Clamp to viewport
        if (left + dw > window.innerWidth - 8) left = window.innerWidth - dw - 8;
        if (left < 8) left = 8;
        sNotifDropdown.style.top = (rect.bottom + 6) + 'px';
        sNotifDropdown.style.left = left + 'px';
        sNotifDropdown.style.right = 'auto';
      }

      function toggleScrollerNotifDropdown() {
        sNotifOpen = !sNotifOpen;
        if (sNotifOpen) {
          sNotifDropdown.style.display = 'block';
          positionDropdown();
          sNotifDropdown.classList.add('visible');
          fetchScrollerNotifs();
        } else {
          sNotifDropdown.classList.remove('visible');
          sNotifDropdown.style.display = 'none';
        }
      }

      // Reposition on resize / layout shift
      const ro = new ResizeObserver(() => { if (sNotifOpen) positionDropdown(); });
      ro.observe(document.getElementById('topbar') || document.body);
      window.addEventListener('resize', () => { if (sNotifOpen) positionDropdown(); });

      function isUserType(type) { return SCROLLER_USER_TYPES.includes(type); }
      function isSystemType(type) { return SCROLLER_SYSTEM_TYPES.includes(type); }

      function filterByTab(notifs, tab) {
        if (tab === 'user') return notifs.filter(n => isUserType(n.type));
        if (tab === 'system') return notifs.filter(n => isSystemType(n.type));
        return notifs;
      }

      function updateScrollerTabBadges(notifs) {
        const userUnread = notifs.filter(n => isUserType(n.type) && !n.is_read).length;
        const sysUnread = notifs.filter(n => isSystemType(n.type) && !n.is_read).length;
        const userBadge = document.getElementById('scroller-notif-tab-badge-user');
        const sysBadge = document.getElementById('scroller-notif-tab-badge-system');
        if (userBadge) {
          userBadge.textContent = userUnread;
          userBadge.style.display = userUnread > 0 ? 'inline-block' : 'none';
        }
        if (sysBadge) {
          sysBadge.textContent = sysUnread;
          sysBadge.style.display = sysUnread > 0 ? 'inline-block' : 'none';
        }
      }

      async function fetchScrollerNotifs() {
        try {
          const res = await fetch('/api/notifications');
          const data = await res.json();
          if (data.success) {
            sCachedNotifs = data.notifications || [];
            updateScrollerTabBadges(sCachedNotifs);
            renderScrollerNotifs(filterByTab(sCachedNotifs, sActiveTab));
          }
        } catch (e) { console.warn('[SCROLLER] Notif fetch failed:', e); }
      }

      function renderScrollerNotifs(notifs) {
        if (!sNotifList) return;
        if (!notifs.length) {
          const emptyMsg = (window.f0ckI18n && window.f0ckI18n.no_notifications) || 'No new notifications';
          sNotifList.innerHTML = `<div class="notif-empty">${emptyMsg}</div>`;
          return;
        }
        const i18n = window.f0ckI18n || {};
        sNotifList.innerHTML = notifs.map(n => {
          let link = `/${n.item_id}`, msg = '', user = n.from_display_name || n.from_user || 'System';
          if (n.type === 'approve') {
            msg = i18n.notif_upload_approved || 'Your Upload has been approved';
            user = i18n.notif_system || 'System';
          } else if (n.type === 'upload_success') {
            msg = i18n.notif_upload_success || 'Background upload finished';
            user = i18n.notif_system || 'System';
          } else if (n.type === 'upload_error') {
            msg = i18n.notif_upload_error || 'Background upload failed';
            user = i18n.notif_system || 'System';
            link = n.item_id ? `/${n.item_id}` : '#';
          } else if (n.type === 'deny' || n.type === 'item_deleted') {
            msg = n.type === 'item_deleted'
              ? (i18n.notif_upload_deleted || `Upload #${n.item_id} deleted`)
              : (i18n.notif_upload_denied || `Upload #${n.item_id} denied`);
            user = i18n.notif_moderation || 'Moderation';
          } else if (n.type === 'admin_pending') {
            link = '/mod/approve'; user = i18n.notif_admin || 'Admin';
            msg = i18n.notif_upload_pending || 'New upload needs approval';
          } else if (n.type === 'report') {
            link = '/mod/reports'; user = i18n.notif_moderation || 'Moderator';
            msg = i18n.notif_new_report || 'New user report';
          } else if (n.type === 'warning') {
            link = `/notifications?tab=system#notif-${n.id}`; user = i18n.notif_system || 'System';
            msg = (i18n.account_warning && i18n.account_warning.title) || 'Account Warning';
          } else {
            link = `/${n.item_id}#c${n.reference_id}`;
            if (n.type === 'comment_reply') msg = i18n.notif_replied || 'replied to you';
            else if (n.type === 'subscription') msg = i18n.notif_subscribed || 'commented in a thread you follow';
            else if (n.type === 'mention') msg = i18n.notif_mentioned || 'highlighted you';
            else msg = i18n.notif_commented || 'commented';
          }
          let thumb;
          if (n.type === 'warning') {
            thumb = `<div class="notif-thumb" style="display:flex;align-items:center;justify-content:center;background:var(--bg-lighter);color:var(--danger);font-size:1.5em;"><i class="fa-solid fa-triangle-exclamation"></i></div>`;
          } else {
            // Pending items aren't in /t/ yet: fall back to /pending/t/ (uploader + staff only)
            const thumbSrc = n.type === 'admin_pending' ? `/pending/t/${n.item_id}.webp` : `/t/${n.item_id}.webp`;
            thumb = n.item_id ? `<div class="notif-thumb"><img src="${thumbSrc}" alt="" data-on="error:img-fallback" data-fallback="/pending/t/${n.item_id}.webp" data-fallback-hide></div>` : '';
          }
          return `<a href="${link}" target="_blank" class="notif-item ${n.is_read ? '' : 'unread'} notif-with-thumb" data-id="${n.id}">
            ${thumb}
            <div class="notif-content">
              <div class="notif-user"><strong ${n.username_color ? `style="color:${n.username_color}"` : ''}>${user}</strong></div>
              <div class="notif-msg">${msg}</div>
              <small class="notif-time">${new Date(n.created_at).toLocaleString()}</small>
            </div>
          </a>`;
        }).join('');
      }

      // Tab switching
      sNotifDropdown.querySelectorAll('.notif-tab').forEach(tab => {
        tab.addEventListener('click', () => {
          sActiveTab = tab.dataset.tab;
          sNotifDropdown.querySelectorAll('.notif-tab').forEach(t => t.classList.toggle('active', t.dataset.tab === sActiveTab));
          if (sNotifList) sNotifList.dataset.activeTab = sActiveTab;
          renderScrollerNotifs(filterByTab(sCachedNotifs, sActiveTab));
        });
      });

      sNotifBtn.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        toggleScrollerNotifDropdown();
      });

      // Close on outside click
      document.addEventListener('click', (e) => {
        if (!document.body.classList.contains('scroller-active')) return;
        if (sNotifOpen && !sNotifDropdown.contains(e.target) && !sNotifBtn.contains(e.target)) {
          sNotifOpen = false;
          sNotifDropdown.classList.remove('visible');
          sNotifDropdown.style.display = 'none';
        }
      });

      // Mark all read (current tab only)
      if (sMarkAll) {
        sMarkAll.addEventListener('click', async () => {
          try {
            const tab = sActiveTab || 'user';
            const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
            await fetch('/api/notifications/read?tab=' + encodeURIComponent(tab), {
              method: 'POST',
              headers: { ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {}) }
            });
            // Mark only the active tab's cached notifications as read
            const tabCheck = tab === 'user' ? isUserType : (tab === 'system' ? isSystemType : () => false);
            sCachedNotifs = sCachedNotifs.map(n => tabCheck(n.type) ? { ...n, is_read: true } : n);
            updateScrollerTabBadges(sCachedNotifs);
            renderScrollerNotifs(filterByTab(sCachedNotifs, sActiveTab));
            // Update overall badge
            const totalUnread = sCachedNotifs.filter(n => !n.is_read).length;
            updateScrollerNotifBadge(totalUnread);
          } catch (e) {}
        });
      }

      // Single notif click → mark read
      sNotifDropdown.addEventListener('click', (e) => {
        const item = e.target.closest('.notif-item');
        if (!item) return;
        const nid = item.dataset.id;
        if (nid && item.classList.contains('unread')) {
          const csrfToken = window.f0ckSession?.csrf_token || window.scrollerCsrf || '';
          fetch(`/api/notifications/${nid}/read`, {
            method: 'POST',
            keepalive: true,
            headers: { ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {}) }
          }).catch(() => {});
          item.classList.remove('unread');
          // Update cache
          const cached = sCachedNotifs.find(n => String(n.id) === String(nid));
          if (cached) cached.is_read = true;
          updateScrollerTabBadges(sCachedNotifs);
          const badge = document.getElementById('scroller-notif-badge');
          if (badge) {
            let c = parseInt(badge.textContent) || 0;
            if (c > 0) { badge.textContent = --c; if (c === 0) badge.style.display = 'none'; }
          }
        }
      });
    }


    // Cleanup on PJAX navigation away
    document.addEventListener('pjax:send', () => {
      clearInterval(notifPollTimer);
      closeScrollerSocket();
    }, { once: true });
  }

  // ── Init ──────────────────────────────────────────────────────────────────
  // Bridge theme.js 't' key toast → scroller toast
  window._scrollerThemeToast = (themeName) => showShareToast(`🎨 ${themeName}`);

  volumeSlider.value = volume;
  syncVolumeUI();
  updateFilterSummary();

  const restored = tryRestoreFromCache();
  if (!restored) {
    if (chanHashPending && !applied.externalUrl) {
      chanHashPending.then(() => fetchItems());
    } else {
      fetchItems();
    }
  }

})();
