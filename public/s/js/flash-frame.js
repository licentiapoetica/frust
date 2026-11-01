/*
 * Runs inside the sandboxed Flash frame (views/flash-frame.html). The parent page's <f0ck-flash>
 * element (flash-embed.js) sends the SWF bytes and commands with postMessage; this file plays them
 * with Ruffle and reports state back. The frame has an opaque origin, so nothing here can use the
 * viewer's session, whatever the SWF (or a Ruffle bug) does.
 *
 * Messages from the parent: load {data, autoplay, volume, background, base}, play, pause,
 * volume {value}, snapshot {id, max, format, quality}, ambient {on}.
 * Messages to the parent: ready, loaded {width, height}, error {message}, state {playing, volume},
 * snapshot {id, blob}, frame {bitmap}, touch {phase, x, y}.
 */
(() => {
  const parentWin = window.parent;
  if (!parentWin || parentWin === window) return; // only works embedded

  const send = (msg, transfer) => parentWin.postMessage(msg, '*', transfer || []);
  const clamp01 = (v) => Math.max(0, Math.min(1, Number(v) || 0));

  // Keep WebGL frames readable after presentation, for snapshots and the ambient background
  const origGetContext = HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.getContext = function (type, attrs) {
    if (type === 'webgl' || type === 'webgl2' || type === 'experimental-webgl') {
      attrs = Object.assign({}, attrs || {}, { preserveDrawingBuffer: true });
    }
    return origGetContext.call(this, type, attrs);
  };

  let player = null;
  let lastState = '';

  const findCanvas = (root) => {
    if (!root) return null;
    const c = root.querySelector('canvas');
    if (c) return c;
    for (const el of root.querySelectorAll('*')) {
      if (el.shadowRoot) {
        const found = findCanvas(el.shadowRoot);
        if (found) return found;
      }
    }
    return null;
  };
  const playerCanvas = () => (player ? findCanvas(player.shadowRoot || player) : null);

  // Report when the user (or the SWF) pauses/resumes or the volume changes
  setInterval(() => {
    if (!player) return;
    const state = { playing: !!player.isPlaying, volume: player.volume };
    const key = `${state.playing}|${state.volume}`;
    if (key !== lastState) {
      lastState = key;
      send({ type: 'state', ...state });
    }
  }, 250);

  // Keep playing while the tab is in the background (the "Flash in background" setting)
  const keepAlive = () => {
    try {
      Object.defineProperty(document, 'visibilityState', { get: () => 'visible', configurable: true });
      Object.defineProperty(document, 'hidden', { get: () => false, configurable: true });
      document.addEventListener('visibilitychange', (e) => e.stopImmediatePropagation(), true);
      window.addEventListener('blur', (e) => e.stopImmediatePropagation(), true);
    } catch (e) {
      console.warn('[flash-frame] keep-alive failed', e);
    }
  };

  const ruffleReady = () => new Promise((resolve) => {
    const check = () => {
      if (window.RufflePlayer && typeof window.RufflePlayer.newest === 'function' && window.RufflePlayer.newest()) resolve();
      else setTimeout(check, 50);
    };
    check();
  });

  const load = async (m) => {
    if (player) return;
    if (!(m.data instanceof ArrayBuffer)) {
      send({ type: 'error', message: 'No SWF data' });
      return;
    }
    if (m.background) keepAlive();
    await ruffleReady();
    player = window.RufflePlayer.newest().createPlayer();
    document.body.appendChild(player);
    player.addEventListener('loadedmetadata', () => {
      const md = player.metadata || {};
      send({ type: 'loaded', width: md.width || 0, height: md.height || 0 });
    });
    try {
      await player.load({
        data: m.data,
        base: typeof m.base === 'string' ? m.base : undefined,
        autoplay: m.autoplay === false ? 'off' : 'on',
        volume: clamp01(m.volume ?? 0.5),
        unmuteOverlay: 'hidden',
        // Always fit the movie into the box. SWFs that set StageScaleMode.NO_SCALE
        // (0x40 Hues, most "app" style SWFs) would otherwise render at their native
        // size and get cropped whenever the box is smaller than the stage.
        scale: 'showAll',
        forceScale: true,
        letterbox: 'on',
        warnOnUnsupportedContent: false,
        contextMenu: 'on',
        splashScreen: true,
        // Defence in depth: the sandbox already denies these
        allowScriptAccess: false,
        allowNetworking: 'none',
        openUrlMode: 'deny',
      });
      player.volume = clamp01(m.volume ?? 0.5);
    } catch (err) {
      send({ type: 'error', message: String(err && err.message || err) });
    }
  };

  const snapshot = (m) => new Promise((resolve) => {
    const canvas = playerCanvas();
    if (!canvas || !canvas.width || !canvas.height) return resolve(null);
    const max = Math.max(16, Math.min(4096, Number(m.max) || 640));
    const ratio = Math.min(1, max / canvas.width, max / canvas.height);
    const out = document.createElement('canvas');
    out.width = Math.max(1, Math.round(canvas.width * ratio));
    out.height = Math.max(1, Math.round(canvas.height * ratio));
    out.getContext('2d').drawImage(canvas, 0, 0, out.width, out.height);
    const format = m.format === 'image/png' ? 'image/png' : 'image/jpeg';
    out.toBlob(resolve, format, Number(m.quality) || 0.92);
  });

  // Small frames for the parent's ambient background (about 8 per second)
  let ambientTimer = null;
  const setAmbient = (on) => {
    clearInterval(ambientTimer);
    ambientTimer = null;
    if (!on) return;
    ambientTimer = setInterval(async () => {
      const canvas = playerCanvas();
      if (!canvas || !canvas.width || !canvas.height) return;
      const w = 128;
      const h = Math.max(1, Math.round(w * canvas.height / canvas.width));
      try {
        const bitmap = await createImageBitmap(canvas, { resizeWidth: w, resizeHeight: h, resizeQuality: 'low' });
        send({ type: 'frame', bitmap }, [bitmap]);
      } catch {}
    }, 125);
  };

  // Touch gestures happen inside this frame; the parent draws the volume HUD. A vertical drag that
  // starts on the right half is the volume gesture, so it must not scroll here.
  let touchStart = null;
  const touchPos = (e) => e.touches[0] || e.changedTouches[0];
  document.addEventListener('touchstart', (e) => {
    const t = touchPos(e);
    touchStart = { x: t.clientX, y: t.clientY, right: t.clientX > window.innerWidth / 2 };
    send({ type: 'touch', phase: 'start', x: t.clientX, y: t.clientY });
  }, { passive: true });
  document.addEventListener('touchmove', (e) => {
    const t = touchPos(e);
    if (touchStart && touchStart.right && Math.abs(t.clientY - touchStart.y) > Math.abs(t.clientX - touchStart.x)) {
      e.preventDefault();
    }
    send({ type: 'touch', phase: 'move', x: t.clientX, y: t.clientY });
  }, { passive: false });
  document.addEventListener('touchend', () => {
    touchStart = null;
    send({ type: 'touch', phase: 'end', x: 0, y: 0 });
  }, { passive: true });

  window.addEventListener('message', (e) => {
    if (e.source !== parentWin) return;
    const m = e.data;
    if (!m || typeof m !== 'object') return;
    switch (m.type) {
      case 'load': load(m); break;
      case 'play': try { player?.play(); } catch {} break;
      case 'pause': try { player?.pause(); } catch {} break;
      case 'volume': if (player) player.volume = clamp01(m.value); break;
      case 'snapshot': snapshot(m).then((blob) => send({ type: 'snapshot', id: m.id, blob })); break;
      case 'ambient': setAmbient(!!m.on); break;
    }
  });

  send({ type: 'ready' });
})();
