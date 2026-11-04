// f0ckv2 frontend: player, keybindings, wheel/swipe paging, image zoom, visualizer, themes.
// Port of f0ckv2's f0ck.js + theme.js to the f0ckm server (no inline styles: classes in f0ckv2.css).

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

(() => {
  let video;
  let elem = document.querySelector("#my-video");
  if(elem) {
    video = new v0ck(elem);
    document.addEventListener("keydown", e => {
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

    const togglebg = document.getElementById('togglebg');
    if(togglebg) togglebg.addEventListener('click', e => {
      e.preventDefault();
      background = !background;
      localStorage.setItem('background', background.toString());
      if(canvas) {
        canvas.classList.toggle('fader-in', background);
        canvas.classList.toggle('fader-out', !background);
      }
      animationLoop();
    });

    elem.addEventListener('play', animationLoop);
  }

  let tt = false;
  const stimeout = 500;
  const changePage = (e, pbwork = true) => {
    pbwork && document.querySelector("nav.navbar").classList.add("pbwork");
    !tt && (tt = setTimeout(() => e.click(), stimeout));
  };

  // <themes>
  const themelinks = [...document.querySelectorAll("li#themes > ul.dropdown-menu > li > a")];
  const themes = themelinks.map(t => t.dataset.theme);
  // Switch in place: every theme is in the loaded f0ck.css (html[theme=...]). The cookie is the one
  // /theme/<name> sets, so the server renders the next page in it (and saves it for the user).
  const setTheme = t => {
    if(!t) return;
    document.documentElement.setAttribute("theme", t);
    document.cookie = `theme=${encodeURIComponent(t)}; Path=/; Max-Age=31536000; SameSite=Lax${window.isSecureContext ? "; Secure" : ""}`;
    document.querySelectorAll("li#themes > a.ddcontent").forEach(a => a.setAttribute("content", t));
    document.querySelectorAll('a.v2-chip[href^="/theme/"]').forEach(a => a.classList.toggle("active", a.getAttribute("href") === "/theme/" + t));
  };
  // The /theme/<name> links stay the no-JS fallback
  document.addEventListener("click", e => {
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
  document.addEventListener("keydown", e => {
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
      if(use) use.setAttribute('href', '/s/fe/f0ckv2/img/iconset.svg#window-' + (full ? 'maximize' : 'minimize'));
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
  const wheelEventListener = function(event) {
    if(event.target.closest('.media-object') && !event.target.closest('.album-strip')) {
      const el = document.getElementById(event.deltaY < 0 ? 'next' : 'prev');
      if(event.deltaY !== 0 && el && el.getAttribute('href') !== '#') {
        event.preventDefault();
        el.click();
      }
    }
  };
  window.addEventListener('wheel', wheelEventListener, { passive: false });
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
        window.addEventListener('wheel', wheelEventListener, { passive: false });
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
  if([...document.querySelectorAll("div.posts")].length === 1 && footbar) {
    document.addEventListener("wheel", e => {
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

  document.addEventListener('touchstart', e => {
    swipeRT.startEl = e.target;
    swipeRT.timeDown = Date.now();
    swipeRT.xDown = e.touches[0].clientX;
    swipeRT.yDown = e.touches[0].clientY;
    swipeRT.xDiff = 0;
    swipeRT.yDiff = 0;
  }, false);

  document.addEventListener('touchmove', e => {
    if(!swipeRT.xDown || !swipeRT.yDown)
      return;
    swipeRT.xDiff = swipeRT.xDown - e.touches[0].clientX;
    swipeRT.yDiff = swipeRT.yDown - e.touches[0].clientY;
  }, false);

  document.addEventListener('touchend', e => {
    if(swipeRT.startEl !== e.target)
      return;
    const timeDiff = Date.now() - swipeRT.timeDown;
    let el;
    if(Math.abs(swipeRT.xDiff) > Math.abs(swipeRT.yDiff)) {
      if(Math.abs(swipeRT.xDiff) > swipeOpt.treshold && timeDiff < swipeOpt.timeout) {
        if(swipeRT.xDiff > 0) // left
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
  // the MOTD box on the index page
  fe.motd = (text) => {
    const box = document.querySelector('.v2-motd');
    if(box) {
      box.textContent = text || '';
      box.hidden = !text;
    }
  };
  // the shared settings page lists every site theme: keep the ones f0ckv2's CSS has
  const themeSelect = document.getElementById('website_theme_select');
  if(themeSelect && themes.length) {
    [...themeSelect.options].forEach(o => { if(!themes.includes(o.value)) o.remove(); });
    themeSelect.value = document.documentElement.getAttribute('theme');
  }
  // </frontend hooks>

  // <timeago> click toggles relative / full date
  document.querySelectorAll("time.timeago[data-full]").forEach(t => {
    t.addEventListener("click", () => {
      const full = t.dataset.full, rel = t.dataset.rel || t.textContent;
      t.dataset.rel = rel;
      t.textContent = t.textContent === rel ? new Date(full).toLocaleString() : rel;
    });
  });
  // </timeago>
})();
