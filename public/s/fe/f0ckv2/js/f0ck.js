// f0ckv2 frontend: player, keybindings, wheel/swipe paging, image zoom, visualizer, themes, and
// pages changing without reloading (<navigation> at the end).
// Port of f0ckv2's f0ck.js + theme.js to the f0ckm server (no inline styles: classes in f0ckv2.css).
// The page setup runs for every page shown (v2Page.onPage, early.js): listeners on document /
// window go through on() and are removed when the page is left.

window.requestAnimFrame = (function(){
  return window.requestAnimationFrame
          || window.webkitRequestAnimationFrame
          || window.mozRequestAnimationFrame
          || function(callback) { window.setTimeout(callback, 1000 / 60);};
})();

const Cookie = {
  get: name => {
    const c = document.cookie.match(`(?:(?:^|.*; *)${name} *= *([^;]*).*$)|^.*$`)[1];
    if(c) return decodeURIComponent(c);
  },
  set: (name, value, opts = {}) => {
    if(opts.days) {
      opts['max-age'] = opts.days * 60 * 60 * 24;
      delete opts.days;
    }
    opts.SameSite = 'Lax';
    opts = Object.entries(opts).reduce((accumulatedStr, [k, v]) => `${accumulatedStr}; ${k}=${v}`, '');
    document.cookie = name + '=' + encodeURIComponent(value) + opts;
  }
};

const isTyping = e => ["INPUT", "TEXTAREA", "SELECT"].includes(e.target.tagName) || e.target.isContentEditable;

// a listener for as long as the page is shown
const v2On = (target, type, fn, opts) => target.addEventListener(type, fn,
  Object.assign(typeof opts === 'boolean' ? { capture: opts } : { ...(opts || {}) }, { signal: window.v2Page.signal }));

window.v2Page.onPage(() => {
  // Onara viewer: the item over its grid page (index.html), closed by a link to that page
  const onara = document.getElementById('v2-onara');
  let video;
  let elem = document.querySelector("#my-video");
  if(elem) {
    video = new v0ck(elem);
    v2On(document, "keydown", e => {
      if(e.key === " " && !isTyping(e)) {
        e.preventDefault();
        video[video.paused ? 'play' : 'pause']();
        document.querySelector('.v0ck_overlay').classList[video.paused ? 'remove' : 'add']('v0ck_hidden');
      }
    });

    if(localStorage.getItem('background') == undefined)
      localStorage.setItem('background', 'true');

    let background = localStorage.getItem('background') === 'true';
    const canvas = document.getElementById('bg');
    const context = canvas ? canvas.getContext('2d') : null;
    const cw = canvas ? (canvas.width = canvas.clientWidth | 0) : 0;
    const ch = canvas ? (canvas.height = canvas.clientHeight | 0) : 0;

    const animationLoop = () => {
      if(!context || elem.tagName !== "VIDEO" || video.paused || video.ended || !background)
        return;
      context.drawImage(video, 0, 0, cw, ch);
      window.requestAnimFrame(animationLoop);
    };

    // The player's settings menu (f0ckm's v0ck) has the background and autonext switches; it calls
    // these when they exist (f0ckm.js has its own). State in localStorage, as before.
    const syncSwitches = () => {
      document.querySelectorAll('#togglebg').forEach(el => el.classList.toggle('active', background));
      const autonext = localStorage.getItem('autoplay') === 'true';
      document.querySelectorAll('#toggleautoplay').forEach(el => el.classList.toggle('active', autonext));
    };
    window.toggleBackground = () => {
      background = !background;
      localStorage.setItem('background', background.toString());
      if(canvas) {
        canvas.classList.toggle('fader-in', background);
        canvas.classList.toggle('fader-out', !background);
      }
      syncSwitches();
      animationLoop();
    };
    window.toggleAutoplay = () => {
      localStorage.setItem('autoplay', (localStorage.getItem('autoplay') !== 'true').toString());
      syncSwitches();
    };
    syncSwitches();

    elem.addEventListener('play', animationLoop);
  }

  let tt = false;
  const stimeout = 500;
  const changePage = (e, pbwork = true) => {
    pbwork && document.querySelector("nav.navbar").classList.add("pbwork");
    !tt && (tt = setTimeout(() => e.click(), stimeout));
  };

  // <themes>
  const themelinks = [...document.querySelectorAll("nav li#themes a[data-theme]")];
  const themes = themelinks.map(t => t.dataset.theme);
  // Switch in place: every theme is in the loaded f0ck.css (html[theme=...]). The cookie is the one
  // /theme/<name> sets, so the server renders the next page in it (and saves it for the user).
  const setTheme = t => {
    if(!t) return;
    document.documentElement.setAttribute("theme", t);
    document.cookie = `theme=${encodeURIComponent(t)}; Path=/; Max-Age=31536000; SameSite=Lax${window.isSecureContext ? "; Secure" : ""}`;
    themelinks.forEach(a => a.classList.toggle("active", a.dataset.theme === t));
    document.querySelectorAll('a.v2-chip[href^="/theme/"]').forEach(a => a.classList.toggle("active", a.getAttribute("href") === "/theme/" + t));
  };
  // The /theme/<name> links stay the no-JS fallback
  v2On(document, "click", e => {
    const a = e.target.closest('a[href^="/theme/"]:not([data-toggle])');
    if(!a || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey) return;
    e.preventDefault();
    setTheme(a.dataset.theme || decodeURIComponent(a.getAttribute("href").slice(7)));
    if(document.activeElement && document.activeElement.blur) document.activeElement.blur();
  });
  // </themes>

  // <keybindings>
  const clickOnElementBinding = selector => () => { const el = document.querySelector(selector); if(el) el.click(); };
  const keybindings = {
    "ArrowLeft": clickOnElementBinding("#next"),
    "a": clickOnElementBinding("#next"),
    "ArrowRight": clickOnElementBinding("#prev"),
    "d": clickOnElementBinding("#prev"),
    "r": clickOnElementBinding("#random"),
    " ": clickOnElementBinding("#f0ck-image"),
    "t": () => {
      if(!themes.length) return;
      let i = themes.indexOf(document.documentElement.getAttribute("theme"));
      if(++i >= themes.length) i = 0;
      setTheme(themes[i]);
    }
  };
  v2On(document, "keydown", e => {
    if(e.key in keybindings && !isTyping(e)) {
      if(e.shiftKey || e.ctrlKey || e.metaKey || e.altKey)
        return;
      if(e.key === " " && !document.querySelector("#f0ck-image"))
        return;
      e.preventDefault();
      keybindings[e.key]();
    }
  });
  // </keybindings>

  // <fullscreen>
  const tfull = document.querySelector('#a_tfull');
  if(tfull) {
    tfull.addEventListener('click', e => {
      e.preventDefault();
      const full = document.documentElement.getAttribute('res') === 'fullscreen';
      Cookie.set('fullscreen', full ? 0 : 1, { path: '/', days: 360 });
      document.documentElement.setAttribute('res', full ? '' : 'fullscreen');
      const use = tfull.querySelector('use');
      if(use) use.setAttribute('href', use.getAttribute('href').replace(/#.*$/, '#window-' + (full ? 'maximize' : 'minimize')));
    });
  }
  // </fullscreen>

  // <image-responsive>
  const imgSize = e => new Promise(res => {
    const i = new Image();
    i.addEventListener('load', function() {
      res({ width: this.width, height: this.height });
    });
    i.src = e.src;
  });

  // <wheeler>
  // Same rules as f0ckm (f0ckm.js <wheeler>): Shift+wheel anywhere, the wheel over the prev/next
  // arrows, or over the media only with "Scroll wheel navigation on items" on in the settings
  // (localStorage wheelNavEnabled, shared with f0ckm).
  const wheelNavOn = () => { try { return localStorage.getItem('wheelNavEnabled') === 'true'; } catch(e) { return false; } };
  let lastWheelNav = 0;
  const wheelEventListener = function(event) {
    if(event.target.closest('input, textarea, [contenteditable="true"], .album-strip')) return;
    const shift = event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey;
    const onArrow = !!event.target.closest('a#next, a#prev');
    const onMedia = !!event.target.closest('.media-object');
    if(!shift && !onArrow && !(onMedia && wheelNavOn())) return;
    const delta = Math.abs(event.deltaY) >= Math.abs(event.deltaX) ? event.deltaY : event.deltaX;
    const el = document.getElementById(delta < 0 ? 'next' : 'prev');
    if(delta === 0 || !el || el.getAttribute('href') === '#') return;
    event.preventDefault();
    const now = Date.now();
    if(now - lastWheelNav < 250) return;
    lastWheelNav = now;
    el.click();
  };
  v2On(window, 'wheel', wheelEventListener, { passive: false });
  // </wheeler>

  const f0ckimage = document.querySelector("img#f0ck-image");
  if(f0ckimage) {
    const f0ckimagescroll = document.querySelector("#image-scroll");
    let isImageExpanded = false;

    f0ckimage.addEventListener("click", async e => {
      e.preventDefault();
      const img = await imgSize(f0ckimage);
      if(isImageExpanded) {
        isImageExpanded = false;
        f0ckimagescroll.classList.remove("v2-scroll");
        f0ckimage.classList.remove("v2-expanded");
        v2On(window, 'wheel', wheelEventListener, { passive: false });
      }
      else {
        if(img.width > img.height) return;
        isImageExpanded = true;
        window.removeEventListener('wheel', wheelEventListener);
        f0ckimagescroll.classList.add("v2-scroll");
        f0ckimage.classList.add("v2-expanded");
      }
    });
  }
  // </image-responsive>

  // <scroller>
  let tts = 0;
  const scroll_treshold = 1;
  const footbar = document.querySelector("div#footbar");
  const navbar = document.querySelector("nav.navbar");
  // Wheel past the bottom / top of a grid turns the page: only with the wheel navigation setting on
  // (see <wheeler>); otherwise the grid scrolls like any page and the page numbers turn it.
  if([...document.querySelectorAll("div.posts")].length === 1 && footbar) {
    v2On(document, "wheel", e => {
      if(!wheelNavOn() || onara) return;
      const main = document.querySelector('#main');
      if(main && Math.ceil(window.innerHeight + window.scrollY) >= main.offsetHeight && e.deltaY > 0) { // down
        const el = document.querySelector(".pagination > .next:not(.disabled)");
        if(el) {
          if(tts < scroll_treshold) {
            footbar.classList.add("v2-armed");
            tts++;
          }
          else
            changePage(el);
        }
      }
      else if(window.scrollY <= 0 && e.deltaY < 0) { // up
        const el = document.querySelector(".pagination > .prev:not(.disabled)");
        if(el) {
          if(tts < scroll_treshold) {
            navbar.classList.add("v2-armed");
            tts++;
          }
          else
            changePage(el);
        }
      }
      else {
        tts = 0;
        footbar.classList.remove("v2-armed");
        navbar.classList.remove("v2-armed");
      }
    });
  }
  // </scroller>

  // <swipe>
  const swipeRT = { xDown: null, yDown: null, xDiff: null, yDiff: null, timeDown: null, startEl: null };
  const swipeOpt = { treshold: 20, timeout: 500 };

  v2On(document, 'touchstart', e => {
    swipeRT.startEl = e.target;
    swipeRT.timeDown = Date.now();
    swipeRT.xDown = e.touches[0].clientX;
    swipeRT.yDown = e.touches[0].clientY;
    swipeRT.xDiff = 0;
    swipeRT.yDiff = 0;
  }, false);

  v2On(document, 'touchmove', e => {
    if(!swipeRT.xDown || !swipeRT.yDown)
      return;
    swipeRT.xDiff = swipeRT.xDown - e.touches[0].clientX;
    swipeRT.yDiff = swipeRT.yDown - e.touches[0].clientY;
  }, false);

  v2On(document, 'touchend', e => {
    if(swipeRT.startEl !== e.target)
      return;
    const timeDiff = Date.now() - swipeRT.timeDown;
    let el;
    if(Math.abs(swipeRT.xDiff) > Math.abs(swipeRT.yDiff)) {
      if(Math.abs(swipeRT.xDiff) > swipeOpt.treshold && timeDiff < swipeOpt.timeout) {
        // in the Onara viewer the navbar pagination is the grid's: swipe through the items instead
        if(onara)
          el = document.querySelector(swipeRT.xDiff > 0 ? "a#prev:not([href='#'])" : "a#next:not([href='#'])");
        else if(swipeRT.xDiff > 0) // left
          el = document.querySelector(".pagination > .next:not(.disabled)");
        else // right
          el = document.querySelector(".pagination > .prev:not(.disabled)");
      }
    }
    swipeRT.xDown = null;
    swipeRT.yDown = null;
    swipeRT.timeDown = null;
    if(el)
      changePage(el);
  }, false);
  // </swipe>

  // <visualizer>
  const audioElement = document.querySelector("audio#my-video");
  if(audioElement && window.AudioContext) {
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");
    canvas.width = 1920;
    canvas.height = 1080;
    canvas.classList.add("v2-visualizer");

    setTimeout(() => {
      const player = document.querySelector(".v0ck");
      if(player) player.insertAdjacentElement("afterbegin", canvas);
    }, 400);

    let audioCtx, analyser, data;
    const draw = data => {
      data = [ ...data ];
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.fillStyle = getComputedStyle(document.body).getPropertyValue("--accent");
      data.forEach((value, i) => {
        const percent = value / 256;
        const height = (canvas.height * percent / 2) - 40;
        const offset = canvas.height - height - 1;
        const barWidth = canvas.width / analyser.frequencyBinCount;
        ctx.fillRect(i * barWidth, offset, barWidth, height);
      });
    };
    const loopingFunction = () => {
      requestAnimationFrame(loopingFunction);
      analyser.getByteFrequencyData(data);
      draw(data);
    };
    audioElement.addEventListener('play', () => {
      if(!audioCtx) {
        try {
          audioCtx = new AudioContext();
          analyser = audioCtx.createAnalyser();
          analyser.fftSize = 2048;
          const source = audioCtx.createMediaElementSource(audioElement);
          source.connect(analyser);
          source.connect(audioCtx.destination);
          data = new Uint8Array(analyser.frequencyBinCount);
          requestAnimationFrame(loopingFunction);
        } catch(err) {
          return;
        }
      }
      audioCtx.resume();
    });
  }
  // </visualizer>

  // <mediakeys>
  if(video && "mediaSession" in navigator) {
    const playpauseEvent = () => {
      video[video.paused ? 'play' : 'pause']();
      document.querySelector('.v0ck_overlay').classList[video.paused ? 'remove' : 'add']('v0ck_hidden');
    };
    navigator.mediaSession.setActionHandler('play', playpauseEvent);
    navigator.mediaSession.setActionHandler('pause', playpauseEvent);
    navigator.mediaSession.setActionHandler('stop', playpauseEvent);
    navigator.mediaSession.setActionHandler('previoustrack', () => {
      const link = document.querySelector("#next:not([href='#'])");
      if(link) changePage(link);
    });
    navigator.mediaSession.setActionHandler('nexttrack', () => {
      const link = document.querySelector("#prev:not([href='#'])");
      if(link) changePage(link);
    });
  }
  // </mediakeys>

  // <flash> SWFs play in f0ckm's sandboxed Ruffle frame (flash-embed.js)
  const ruffle = document.getElementById('ruffle-container');
  if(ruffle && window.customElements && customElements.get('f0ck-flash')) {
    const player = document.createElement('f0ck-flash');
    ruffle.appendChild(player);
    player.addEventListener('loadedmetadata', () => {
      player.volume = +(localStorage.getItem('volume') ?? 0.5);
    });
    player.load({ url: ruffle.dataset.swf, autoplay: true, background: true }).catch(() => {
      ruffle.textContent = 'SWF playback failed';
    });
  }
  // </flash>

  // <report> the box is a :target modal and a real form; with JavaScript it answers in place
  const reportForm = document.getElementById('v2-report-form');
  if(reportForm) {
    const out = reportForm.querySelector('.v2-report-msg');
    reportForm.addEventListener('submit', async e => {
      e.preventDefault();
      const fd = new FormData(reportForm);
      const body = {
        item_id: fd.get('item_id'),
        reason: fd.get('reason') || '',
        categories: fd.getAll('categories[]'),
        'cap-token': fd.get('cap-token') || undefined,
        csrf_token: fd.get('csrf_token')
      };
      const csrf = document.querySelector('meta[name="csrf-token"]')?.content || '';
      try {
        const res = await (await fetch('/api/v2/report', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf },
          body: JSON.stringify(body)
        })).json();
        out.classList.toggle('error', !res.success);
        out.textContent = res.success ? 'thanks, a mod will look at it' : (res.msg || 'could not send report');
        if(res.success) {
          reportForm.reset();
          setTimeout(() => { if(location.hash === '#report') location.hash = '_'; }, 1500);
        }
        else window.f0ckCapReset?.();
      } catch {
        out.classList.add('error');
        out.textContent = 'could not send report';
      }
    });
  }
  // </report>

  // <touch dropdowns> no hover on touch screens: the first tap on a navbar dropdown opens it
  // (.v2-open), a second tap follows its link; a tap anywhere else closes it
  const touch = window.matchMedia ? window.matchMedia('(hover: none)') : { matches: false };
  v2On(document, 'click', e => {
    const open = [...document.querySelectorAll('.navbar .nav-item.v2-open')];
    const t = e.target.closest('.navbar .nav-item.dropdown > [data-toggle="dropdown"]');
    if(!t || !touch.matches) {
      if(!e.target.closest('.navbar .nav-item.v2-open')) open.forEach(li => li.classList.remove('v2-open'));
      return;
    }
    const li = t.parentElement;
    if(li.classList.contains('v2-open')) return;
    e.preventDefault();
    open.forEach(o => o.classList.remove('v2-open'));
    li.classList.add('v2-open');
  });
  // </touch dropdowns>

  // <soft view switch> filter and mode change the listing in place, without reloading the page.
  // Listings load again (<navigation>); on an item page a mode change only refreshes prev/next and
  // the pagination (the player keeps playing).
  // The links stay plain links without JavaScript (/mode/<n> redirects, the filter is a listing URL).
  const softSwap = (doc, sels) => sels.forEach(sel => {
    const cur = document.querySelector(sel), next = doc.querySelector(sel);
    if(cur && next) cur.replaceWith(next);
  });
  const fetchDoc = async url => {
    const res = await fetch(url, { credentials: 'same-origin' });
    if(!res.ok) throw new Error(res.status);
    return new DOMParser().parseFromString(await res.text(), 'text/html');
  };
  // a listing: the whole page again (<navigation>)
  const softLoad = (url, push) => window.v2Nav.go(url, push ? {} : { replace: true, keepScroll: true });
  v2On(document, 'click', async e => {
    const a = e.target.closest('#view-settings a[href]');
    if(!a || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || a.dataset.theme) return;
    const href = a.getAttribute('href');
    const isMode = /^\/mode\/\d$/.test(href);
    const isFilter = !isMode && a.closest('.v2-mm-col') && a.closest('.v2-mm-col').querySelector('.v2-mm-head')?.textContent.trim() === 'filter';
    if(!isMode && !isFilter) return;
    e.preventDefault();
    if(document.activeElement && document.activeElement.blur) document.activeElement.blur();
    try {
      if(isMode) {
        const res = await fetch(href, { credentials: 'same-origin', headers: { 'X-Requested-With': 'XMLHttpRequest', Accept: 'application/json' } });
        const data = await res.json().catch(() => ({}));
        if(!res.ok || data.success === false) { fe.flash?.(data.msg || 'mode not allowed', 'error'); return; }
        if(document.querySelector('.pagewrapper > #main')) await softLoad(location.href, false);
        else softSwap(await fetchDoc(location.href), ['.pagination-wrapper', '#view-settings', 'a#prev', 'a#next']);
      } else {
        await softLoad(href, true);
      }
    } catch(err) {
      location.href = href;
    }
  });
  // </soft view switch>

  // <frontend hooks> for the shared client (f0ck-core.js / f0ck-frontend.js, views/frontends/README.md)
  const fe = window.f0ckFrontend = window.f0ckFrontend || {};
  const itemBox = document.querySelector('.container[data-item-id]');
  if(itemBox) fe.itemId = itemBox.dataset.itemId;
  // toasts in f0ckv2's #flash colours (showFlash / flashMessage of the shared scripts end up here)
  fe.flash = (message, type) => {
    let box = document.getElementById('v2-toasts');
    if(!box) {
      box = document.createElement('div');
      box.id = 'v2-toasts';
      box.className = 'v2-toasts f0ck-shell-chrome';
      document.body.appendChild(box);
    }
    const el = document.createElement('div');
    const kind = ({ success: 'success', error: 'error', warning: 'warn', warn: 'warn' })[type] || 'info';
    el.className = 'v2-toast v2-toast-' + kind;
    const text = message == null ? '' : String(message);
    if(text.includes('<a ') && window.Sanitizer?.clean) el.innerHTML = Sanitizer.clean(text);
    else el.textContent = text;
    el.addEventListener('click', ev => { if(ev.target.tagName !== 'A') el.remove(); });
    box.appendChild(el);
    requestAnimationFrame(() => el.classList.add('v2-toast-in'));
    setTimeout(() => {
      el.classList.remove('v2-toast-in');
      setTimeout(() => el.remove(), 300);
    }, kind === 'error' ? 6000 : 3500);
  };
  // the MOTD box (item page, between the player and the metadata)
  fe.motd = (text) => {
    document.querySelectorAll('.v2-motd').forEach(box => {
      box.textContent = text || '';
      box.hidden = !String(text || '').trim();
    });
  };
  // the shared settings page lists every site theme: keep the ones f0ckv2's CSS has
  const themeSelect = document.getElementById('website_theme_select');
  if(themeSelect && themes.length) {
    [...themeSelect.options].forEach(o => { if(!themes.includes(o.value)) o.remove(); });
    themeSelect.value = document.documentElement.getAttribute('theme');
  }
  // </frontend hooks>

  // <onara> Escape closes the viewer (an open report / delete box first); the item's thumbnail
  // behind it is scrolled into view
  if(onara) {
    const active = document.querySelector('div.posts > a.v2-onara-active');
    if(active) active.scrollIntoView({ block: 'center' });
    v2On(document, 'keydown', e => {
      if(e.key !== 'Escape' || isTyping(e) || e.defaultPrevented) return;
      if(location.hash && location.hash !== '#_' && document.querySelector(`.v2-modal#${CSS.escape(location.hash.slice(1))}`)) return;
      e.preventDefault();
      window.v2Nav.go(onara.dataset.close || '/');
    });
  }
  // </onara>

  // <timeago> click toggles relative / full date
  document.querySelectorAll("time.timeago[data-full]").forEach(t => {
    t.addEventListener("click", () => {
      const full = t.dataset.full, rel = t.dataset.rel || t.textContent;
      t.dataset.rel = rel;
      t.textContent = t.textContent === rel ? new Date(full).toLocaleString() : rel;
    });
  });
  // </timeago>

  // item page: the neighbours load in the background, like f0ckm (600 ms after the page shows)
  if(itemBox) {
    const t = setTimeout(() => ['#next', '#prev'].forEach(sel => {
      const a = document.querySelector(sel);
      if(a && window.v2Nav?.navigable(a)) window.v2Nav.prefetch(a.href);
    }), 600);
    window.v2Page.onLeave(() => clearTimeout(t));
  }
});

// <navigation> pages change without reloading, like f0ckm: links and GET forms fetch the page and swap
// the page part of <body> (everything before #footer-data: navbar, content, the page data
// #f0ckv2-data), for f0ckv2's own pages and the shared pages in its shell (settings, messages,
// admin, mod, ...). The page's own scripts run first (libraries like Cap, danmaku, flash-embed once;
// data-v2-page scripts, /s/js/views/* and everything of a shared page every time), then the setups
// of the scripts that stay (v2Page.onPage), then the footer extras of a shared page. Stylesheets of
// <head> follow the page (f0ck-shell.css, f0ckv2-shell.css): F0ckPageSwap, f0ck-core.js.
// A full load instead for: login / logout / register (the session changes), another frontend,
// files, the upload page, a page that isn't f0ckv2's or a failed request.
// Without JavaScript all of this is plain links and forms.
(() => {
  const html = document.documentElement;
  const FULL_LOAD = /^\/(?:logout|login|register|frontend\/|theme\/|api\/|ajax\/|s\/|b\/|t\/|c\/|a\/|upload\/?$|mime(?:\/|$))/;
  const pathKey = href => { const u = new URL(href, location.href); return u.pathname + u.search; };
  let shownPath = pathKey(location.href);
  const readData = (doc = document) => {
    try { return JSON.parse(doc.getElementById('f0ckv2-data')?.textContent || 'null'); } catch { return null; }
  };

  const navigable = a => {
    if(!a || !a.href) return false;
    if(a.target && a.target !== '_self') return false;
    if(a.hasAttribute('download') || a.closest('[data-no-ajax]')) return false;
    const raw = a.getAttribute('href') || '';
    if(!raw || raw.startsWith('#') || /^(javascript|mailto|tel):/i.test(raw)) return false;
    let u;
    try { u = new URL(a.href, location.href); } catch { return false; }
    if(u.origin !== location.origin) return false;
    if(u.hash && pathKey(u.href) === pathKey(location.href)) return false; // a jump within the page
    if(FULL_LOAD.test(u.pathname)) return false;
    if(/\.[a-z0-9]{2,5}$/i.test(u.pathname)) return false; // files
    return true;
  };

  // ── fetched pages, kept briefly (prefetched neighbours, hovered links). Keyed by URL and the page
  // they are asked from (/random picks its context from the Referer); any POST/PUT/DELETE empties it.
  const prefetched = new Map();
  const TTL = 60000;
  const cacheKey = url => new URL(url, location.href).href + ' ' + location.href;
  const fetchPage = (url, { signal, prefetch = false } = {}) => {
    const headers = { Accept: 'text/html' };
    if(prefetch) Object.assign(headers, { 'X-Purpose': 'prefetch', Purpose: 'prefetch' });
    return fetch(url, { credentials: 'same-origin', headers, signal, priority: prefetch ? 'low' : 'high' }).then(async res => {
      if(!/text\/html/i.test(res.headers.get('content-type') || '')) return null;
      return { url: res.url || new URL(url, location.href).href, html: await res.text(), prefetched: prefetch };
    });
  };
  const prefetch = url => {
    const key = cacheKey(url);
    const hit = prefetched.get(key);
    if(hit && Date.now() - hit.at < TTL) return;
    prefetched.set(key, { at: Date.now(), page: fetchPage(url, { prefetch: true }).catch(() => null) });
    if(prefetched.size > 12) prefetched.delete(prefetched.keys().next().value);
  };
  const takePrefetched = url => {
    const key = cacheKey(url);
    const hit = prefetched.get(key);
    prefetched.delete(key);
    return hit && Date.now() - hit.at < TTL ? hit.page : null;
  };
  // a change sent to the server makes kept pages stale
  const nativeFetch = window.fetch.bind(window);
  window.fetch = (input, init) => {
    const method = String((init && init.method) || (input instanceof Request ? input.method : 'GET')).toUpperCase();
    if(method !== 'GET' && method !== 'HEAD') prefetched.clear();
    return nativeFetch(input, init);
  };

  // hovering a link for a moment loads its page already
  let hoverTimer = null;
  window.addEventListener('mouseover', e => {
    const a = e.target.closest?.('a[href]');
    if(!a || a === e.relatedTarget?.closest?.('a[href]')) return;
    clearTimeout(hoverTimer);
    if(navigable(a)) hoverTimer = setTimeout(() => prefetch(a.href), 120);
  });
  window.addEventListener('mouseout', e => {
    const a = e.target.closest?.('a[href]');
    if(a && !a.contains(e.relatedTarget)) clearTimeout(hoverTimer);
  });
  window.addEventListener('touchstart', e => {
    const a = e.target.closest?.('a[href]');
    if(navigable(a)) prefetch(a.href);
  }, { passive: true });

  // the navbar pulses (.pbwork, f0ck.css) while a page takes a moment
  let barTimer = null;
  const loading = onOff => {
    clearTimeout(barTimer);
    if(onOff) barTimer = setTimeout(() => document.querySelector('nav.navbar')?.classList.add('pbwork'), 150);
    else document.querySelector('nav.navbar')?.classList.remove('pbwork');
  };

  // one of f0ckv2's pages (its own or a shared one in its shell), for the same visitor
  const isOwnPage = doc => {
    const next = readData(doc), cur = readData() || {};
    return !!(next && doc.getElementById('footer-data') && (next.user || '') === (cur.user || ''));
  };

  // the page part of <body>: the nodes before #footer-data
  const swapBody = doc => {
    const anchor = document.getElementById('footer-data');
    const nextAnchor = doc.getElementById('footer-data');
    const old = [];
    for(const n of document.body.childNodes) { if(n === anchor) break; old.push(n); }
    // media of the page we leave stop (and stop downloading)
    old.forEach(n => n.querySelectorAll?.('video, audio').forEach(m => {
      try { m.pause(); m.removeAttribute('src'); m.querySelectorAll('source').forEach(x => x.remove()); m.load(); } catch { /* gone */ }
    }));
    const frag = document.createDocumentFragment();
    for(const n of [...doc.body.childNodes]) { if(n === nextAnchor) break; frag.appendChild(document.adoptNode(n)); }
    const scripts = [...frag.querySelectorAll('script')];
    old.forEach(n => n.remove());
    anchor.before(frag);
    [...document.body.attributes].forEach(a => document.body.removeAttribute(a.name));
    [...doc.body.attributes].forEach(a => document.body.setAttribute(a.name, a.value));
    ['v2-onara-open', 'f0ck-shell'].forEach(c => html.classList.toggle(c, doc.documentElement.classList.contains(c)));
    document.title = doc.title;
    const desc = doc.querySelector('meta[name="description"]');
    if(desc) document.querySelector('meta[name="description"]')?.setAttribute('content', desc.getAttribute('content') || '');
    return scripts;
  };

  let seq = 0;
  let abort = null;
  // opts: replace (no new history entry), pop (back / forward: scroll restored), keepScroll
  const go = async (url, opts = {}) => {
    const swap = window.F0ckPageSwap;
    if(!swap) { location.assign(url); return; }
    const target = new URL(url, location.href);
    const mine = ++seq;
    abort?.abort();
    abort = new AbortController();
    loading(true);
    let page = null;
    try {
      page = await (takePrefetched(target.href) || null);
      if(!page) page = await fetchPage(target.href, { signal: abort.signal });
    } catch(err) {
      if(err && err.name === 'AbortError') return;
      page = null;
    }
    if(mine !== seq) return;
    loading(false);
    const doc = page && new DOMParser().parseFromString(page.html, 'text/html');
    if(!doc || !isOwnPage(doc)) { location.assign(page ? page.url : target.href); return; }
    // the new page's stylesheets first (a shared page: f0ck-shell.css, f0ckv2-shell.css)
    const dropStyles = await swap.styles(doc);
    if(mine !== seq) return;

    if(!opts.pop) {
      try { history.replaceState({ ...(history.state || {}), v2Scroll: window.scrollY }, ''); } catch { /* ignore */ }
    }
    if('scrollRestoration' in history) history.scrollRestoration = 'manual';
    const final = new URL(page.url);
    if(!final.hash && target.hash) final.hash = target.hash;
    window.v2Page.leave();
    const scripts = swapBody(doc);
    dropStyles();
    if(opts.replace) history.replaceState({ v2: 1 }, '', final.href);
    else if(!opts.pop) history.pushState({ v2: 1 }, '', final.href);
    shownPath = pathKey(location.href);
    // the same order as a full load: the page's scripts (flash-embed.js, danmaku, comments.js) first,
    // then the setups of the scripts at the end of the page (f0ck.js starts the player, the Flash frame)
    await swap.scripts(scripts);
    window.v2Page.show();
    await swap.tail(doc, 'footer-data');
    swap.shown();
    // a prefetched item page didn't mark its notifications read (the server skips that for prefetches)
    const data = readData() || {};
    if(page.prefetched && data.itemId && data.loggedIn) {
      fetch(`/api/notifications/item/${data.itemId}/read`, { method: 'POST', credentials: 'same-origin', headers: { 'X-CSRF-Token': document.querySelector('meta[name="csrf-token"]')?.content || '' } })
        .then(() => window.f0ckFrontend?.notify?.()).catch(() => {});
    }
    const saved = opts.pop ? history.state?.v2Scroll : null;
    const hashEl = final.hash.length > 1 ? document.getElementById(decodeURIComponent(final.hash.slice(1))) : null;
    if(typeof saved === 'number') window.scrollTo(0, saved);
    else if(hashEl) hashEl.scrollIntoView();
    else if(!opts.keepScroll) window.scrollTo(0, 0);
    document.dispatchEvent(new CustomEvent('v2:page', { detail: { url: location.href } }));
  };

  window.v2Nav = { go, reload: () => go(location.href, { replace: true, keepScroll: true }), navigable, prefetch };

  window.addEventListener('click', e => {
    if(e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = e.target.closest?.('a[href]');
    if(!navigable(a)) return;
    e.preventDefault();
    go(a.href);
  });
  window.addEventListener('submit', e => {
    const f = e.target;
    if(e.defaultPrevented || !(f instanceof HTMLFormElement)) return;
    if((f.getAttribute('method') || 'get').toLowerCase() !== 'get' || (f.target && f.target !== '_self')) return;
    const u = new URL(f.action || location.href, location.href);
    if(u.origin !== location.origin || FULL_LOAD.test(u.pathname)) return;
    let fd;
    try { fd = new FormData(f, e.submitter || undefined); } catch { fd = new FormData(f); }
    u.search = new URLSearchParams(fd).toString();
    e.preventDefault();
    go(u.href);
  });
  window.addEventListener('popstate', () => {
    if(pathKey(location.href) === shownPath) return; // only the #hash changed
    go(location.href, { pop: true });
  });
})();
// </navigation>

