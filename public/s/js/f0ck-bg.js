// f0ckm's blurred item background for the alternative frontends: the media on screen drawn into a
// <canvas id="bg"> behind the page, blurred, darkened and saturated, as f0ckm.js does it
// (initBackground / applyBackgroundOpacitySettings). Same tuning: f0ckm's defaults for an item,
// then the admin's (audio tuner, data-tuner on the canvas), then the visitor's own (f0ckm's tuner,
// localStorage f0ck_audio_tuning). Same switch: f0ckSession.show_background (saved for members,
// /api/v2/settings/background), else localStorage `background`; the player's settings menu
// (v0ck.js #togglebg) calls window.toggleBackground.
//
// Everything is drawn into the canvas, the opacity, zoom and tint included, so the page sets no
// styles: the frontend's CSS places the canvas (position: fixed behind the content) and fades it
// with the classes fader-in / fader-out.
//
//   f0ckBackground.start({ media, thumb })
//     media: <video>, <img>, <canvas> (Flash's ambientCanvas) or a function returning one; a
//            <video> is drawn every frame while it plays, the rest once (a canvas: every frame)
//     thumb: the item's thumbnail URL, drawn until a video has a frame (and for anything else)
(() => {
  'use strict';
  if (window.f0ckBackground) return;

  // f0ckm.js DEFAULT_AUDIO_TUNING, the background keys an item uses (later keys win there)
  const DEFAULTS = {
    bgBlurMethod: 1,
    bgCanvasFps: 60,
    bgCanvasBlur: 60,
    bgCanvasBrightness: 0.6,
    bgCanvasSaturate: 1.9,
    bgCanvasContrast: 1.15,
    bgCanvasZoom: 1,
    bgCanvasColor: '#000000',
    bgCanvasColorOpacity: 0.15,
    bgCanvasOpacity: 0.43,
  };
  const KEYS = Object.keys(DEFAULTS);
  const SCALE = 0.5; // the canvas is drawn at half the window size (f0ckm: SCALE)

  const canvas = document.getElementById('bg');
  if (!canvas || !canvas.getContext) return;
  const ctx = canvas.getContext('2d');
  if (!ctx) return;

  const pick = (o) => {
    const out = {};
    if (o && typeof o === 'object') KEYS.forEach(k => { if (o[k] !== undefined && o[k] !== null) out[k] = o[k]; });
    return out;
  };
  const tuning = () => {
    let server = null;
    try { server = JSON.parse(canvas.dataset.tuner || 'null'); } catch { server = null; }
    if (!server && window.f0ckServerAudioTuner) server = window.f0ckServerAudioTuner;
    let own = null;
    try { own = JSON.parse(localStorage.getItem('f0ck_audio_tuning') || 'null'); } catch { own = null; }
    return Object.assign({}, DEFAULTS, pick(server), pick(own));
  };
  const num = (v, d) => (Number.isFinite(Number(v)) ? Number(v) : d);

  // f0ckm's _computeBgCanvasFilter; the blur of methods 1 / 2 (a CSS filter on the canvas there)
  // is drawn here at the canvas scale, which looks the same
  const filterOf = (t) => {
    const parts = [];
    const blur = Math.round(Math.max(0, num(t.bgCanvasBlur, 60)) * SCALE);
    const bri = num(t.bgCanvasBrightness, 0.6);
    const sat = num(t.bgCanvasSaturate, 1.9);
    const con = num(t.bgCanvasContrast, 1.15);
    if (blur > 0) parts.push(`blur(${blur}px)`);
    if (bri !== 1) parts.push(`brightness(${bri})`);
    if (sat !== 1) parts.push(`saturate(${sat})`);
    if (con !== 1) parts.push(`contrast(${con})`);
    return parts.length ? parts.join(' ') : 'none';
  };

  // offscreen canvases: the frame scaled down before it is blurred, and the finished picture
  const scratch = {};
  const getScratch = (name, w, h) => {
    let s = scratch[name];
    if (!s) {
      const c = document.createElement('canvas');
      s = scratch[name] = { canvas: c, ctx: c.getContext('2d') };
    }
    if (s.canvas.width !== w || s.canvas.height !== h) {
      s.canvas.width = w;
      s.canvas.height = h;
    }
    return s;
  };

  const sizeOf = (src) => {
    if (src.videoWidth && src.videoHeight) return [src.videoWidth, src.videoHeight];
    if (src.naturalWidth && src.naturalHeight) return [src.naturalWidth, src.naturalHeight];
    if (src.width && src.height) return [src.width, src.height];
    return [0, 0];
  };
  const fit = () => {
    const w = Math.max(1, (canvas.clientWidth * SCALE) | 0);
    const h = Math.max(1, (canvas.clientHeight * SCALE) | 0);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    return [w, h];
  };

  // one picture: the source covering the canvas (times the zoom), filtered, the tint over it, all
  // of it at the background's opacity
  const paint = (src) => {
    const [sw, sh] = sizeOf(src);
    if (!sw || !sh) return false;
    const t = tuning();
    const [w, h] = fit();
    const zoom = Math.max(1, num(t.bgCanvasZoom, 1));
    const scale = Math.max(w / sw, h / sh) * zoom;
    const out = getScratch('out', w, h);
    if (!out.ctx) return false;
    const o = out.ctx;
    o.clearRect(0, 0, w, h);
    let source = src;
    let dx = (w - sw * scale) / 2;
    let dy = (h - sh * scale) / 2;
    let dw = sw * scale;
    let dh = sh * scale;
    // a big frame (4K video) is cut and scaled to the canvas first, unfiltered, so the blur runs on
    // the small copy only (f0ckm: drawCoverImage)
    if (scale < 1) {
      const cut = getScratch('cut', w, h);
      if (cut.ctx) {
        cut.ctx.clearRect(0, 0, w, h);
        cut.ctx.drawImage(src, (sw - w / scale) / 2, (sh - h / scale) / 2, w / scale, h / scale, 0, 0, w, h);
        source = cut.canvas;
        dx = 0;
        dy = 0;
        dw = w;
        dh = h;
      }
    }
    o.save();
    o.filter = filterOf(t);
    o.drawImage(source, dx, dy, dw, dh);
    o.restore();
    const tint = Math.min(1, Math.max(0, num(t.bgCanvasColorOpacity, 0)));
    if (tint > 0.001) {
      o.save();
      o.globalAlpha = tint;
      o.fillStyle = t.bgCanvasColor || '#000000';
      o.fillRect(0, 0, w, h);
      o.restore();
    }
    ctx.clearRect(0, 0, w, h);
    ctx.save();
    ctx.globalAlpha = Math.min(1, Math.max(0, num(t.bgCanvasOpacity, 0.43)));
    ctx.drawImage(out.canvas, 0, 0);
    ctx.restore();
    return true;
  };

  // ── on / off ──
  let enabled = (window.f0ckSession && window.f0ckSession.show_background !== undefined)
    ? !!window.f0ckSession.show_background
    : (() => { try { return localStorage.getItem('background') !== 'false'; } catch { return true; } })();
  const show = (on) => {
    canvas.classList.toggle('fader-in', on);
    canvas.classList.toggle('fader-out', !on);
  };
  const syncSwitches = () => {
    document.querySelectorAll('#togglebg').forEach(el => el.classList.toggle('active', enabled));
  };

  // ── what is drawn ──
  let current = null; // { media, thumbImg }
  let raf = 0;
  let lastTs = 0;
  let drawn = false;
  const mediaOf = () => {
    if (!current) return null;
    const m = typeof current.media === 'function' ? current.media() : current.media;
    return m || null;
  };
  const isLive = (m) => (m instanceof HTMLVideoElement && !m.paused && !m.ended) || m instanceof HTMLCanvasElement;
  const hasFrame = (m) => {
    if (m instanceof HTMLVideoElement) return m.readyState >= 2 && m.videoWidth > 0;
    if (m instanceof HTMLImageElement) return m.complete && m.naturalWidth > 0;
    if (m instanceof HTMLCanvasElement) return m.width > 0 && m.height > 0;
    return false;
  };

  const drawNow = () => {
    if (!enabled || !current) return;
    const m = mediaOf();
    try {
      if (m && hasFrame(m) && paint(m)) {
        drawn = true;
      } else if (current.thumbImg && current.thumbImg.complete && current.thumbImg.naturalWidth && paint(current.thumbImg)) {
        drawn = true;
      }
    } catch {
      // a frame that can't be read (cross-origin): keep what is drawn
    }
    if (drawn) show(true);
  };

  const loop = (ts) => {
    raf = 0;
    if (!enabled || !current || document.hidden) return;
    const m = mediaOf();
    if (!m || !isLive(m)) return;
    const fps = num(tuning().bgCanvasFps, 60);
    const frameMs = fps > 0 ? 1000 / fps : 0;
    if (!frameMs || ts - lastTs >= frameMs) {
      lastTs = ts;
      drawNow();
    }
    raf = requestAnimationFrame(loop);
  };
  const run = () => {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    drawNow();
    const m = mediaOf();
    if (enabled && m && isLive(m)) raf = requestAnimationFrame(loop);
  };

  const start = ({ media, thumb } = {}) => {
    current = { media, thumbImg: null };
    if (thumb) {
      const img = new Image();
      img.onload = () => { if (!drawn) drawNow(); };
      img.src = thumb;
      current.thumbImg = img;
    }
    const m = mediaOf();
    if (m instanceof HTMLVideoElement) {
      ['play', 'playing', 'seeked', 'loadeddata'].forEach(ev => m.addEventListener(ev, run));
      // paused: the frame on screen stays (f0ckm stops its loop the same way)
      m.addEventListener('pause', () => { if (raf) cancelAnimationFrame(raf); raf = 0; drawNow(); });
    } else if (m instanceof HTMLImageElement && !hasFrame(m)) {
      m.addEventListener('load', run, { once: true });
    }
    run();
  };

  let resizeTimer = 0;
  window.addEventListener('resize', () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(run, 150);
  }, { passive: true });
  document.addEventListener('visibilitychange', () => { if (!document.hidden) run(); });

  // the player's switch (v0ck.js), as f0ckm's toggleBackground: saved for members, else locally
  const toggle = async () => {
    enabled = !enabled;
    try { localStorage.setItem('background', enabled ? 'true' : 'false'); } catch { /* storage blocked */ }
    syncSwitches();
    if (enabled) run();
    else show(false);
    const s = window.f0ckSession;
    if (!s) return;
    s.show_background = enabled;
    if (!s.logged_in) return;
    try {
      await fetch('/api/v2/settings/background', {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': s.csrf_token || '' },
        body: JSON.stringify({ show_background: enabled }),
      });
    } catch { /* offline: the switch stays as it is */ }
  };
  if (!window.toggleBackground) window.toggleBackground = toggle;
  syncSwitches();

  window.f0ckBackground = {
    start,
    redraw: run,
    toggle,
    get enabled() { return enabled; },
  };
})();
