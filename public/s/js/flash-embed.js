/*
 * <f0ck-flash>: plays an SWF in a sandboxed iframe (/flash-frame, see flash-frame.js).
 *
 * Uploaded SWFs are untrusted. Inside this page, Ruffle would run them with the viewer's session:
 * any request a SWF makes would carry the login cookie. The sandboxed frame has an opaque origin,
 * so it has no session. This element fetches the SWF itself (with the session, so private items
 * work) and hands only the bytes to the frame.
 *
 *   const el = document.createElement('f0ck-flash');
 *   container.appendChild(el);
 *   el.load({ url: '/b/x.swf' })           // or { data: ArrayBuffer | Blob }
 *     .then(...)                            // resolves when the movie is loaded
 *   el.play(); el.pause(); el.volume = 0.5; el.isPlaying / el.is_playing
 *   el.snapshot({ max: 640 })               // Promise<Blob|null>, JPEG of the current frame
 *   el.ambientCanvas                        // small canvas mirroring the movie (for the background)
 *   el.remove()                             // stops everything
 *
 * Events on the element: loadedmetadata, flash-error {message}, flash-state {playing, volume},
 * flash-touch {phase, clientX, clientY} (touches inside the frame, in page coordinates).
 *
 * Messages from the frame are treated as untrusted: only known types with checked values are used.
 */
(() => {
  if (window.customElements.get('f0ck-flash')) return;

  const style = document.createElement('style');
  style.textContent = 'f0ck-flash{display:block;position:relative;width:100%;height:100%}' +
    'f0ck-flash>iframe{display:block;width:100%;height:100%;border:0;background:transparent}';
  document.head.appendChild(style);

  const clamp01 = (v) => Math.max(0, Math.min(1, Number(v) || 0));

  class F0ckFlash extends HTMLElement {
    constructor() {
      super();
      this._ready = false;
      this._queue = [];
      this._volume = 0.5;
      this._playing = false;
      this._snapshots = new Map();
      this._snapshotSeq = 0;
      this._loading = null;
      this._ambient = null;
      this._onMessage = this._onMessage.bind(this);
    }

    connectedCallback() {
      window.addEventListener('message', this._onMessage);
      if (this._frame) return;
      const frame = document.createElement('iframe');
      // Same flags as the server's sandbox policy: scripts only, no same-origin, popups, forms or navigation
      frame.setAttribute('sandbox', 'allow-scripts allow-pointer-lock');
      frame.setAttribute('allow', 'autoplay; fullscreen');
      frame.setAttribute('allowfullscreen', '');
      frame.setAttribute('title', 'Flash');
      frame.src = '/flash-frame';
      this._frame = frame;
      this.appendChild(frame);
    }

    disconnectedCallback() {
      window.removeEventListener('message', this._onMessage);
      // Moving the element re-creates the iframe, so treat removal as the end
      this._frame?.remove();
      this._frame = null;
      this._ready = false;
      this._queue = [];
      for (const resolve of this._snapshots.values()) resolve(null);
      this._snapshots.clear();
    }

    _post(msg, transfer) {
      if (this._ready && this._frame?.contentWindow) this._frame.contentWindow.postMessage(msg, '*', transfer || []);
      else this._queue.push([msg, transfer]);
    }

    _onMessage(e) {
      if (!this._frame || e.source !== this._frame.contentWindow) return;
      const m = e.data;
      if (!m || typeof m !== 'object') return;
      switch (m.type) {
        case 'ready':
          this._ready = true;
          for (const [msg, transfer] of this._queue.splice(0)) this._post(msg, transfer);
          break;
        case 'loaded':
          this._loading?.resolve();
          this.dispatchEvent(new Event('loadedmetadata'));
          break;
        case 'error': {
          const message = String(m.message || 'Flash error').slice(0, 300);
          this._loading?.reject(new Error(message));
          this.dispatchEvent(new CustomEvent('flash-error', { detail: { message } }));
          break;
        }
        case 'state':
          this._playing = !!m.playing;
          if (typeof m.volume === 'number') this._volume = clamp01(m.volume);
          this.dispatchEvent(new CustomEvent('flash-state', { detail: { playing: this._playing, volume: this._volume } }));
          break;
        case 'snapshot': {
          const resolve = this._snapshots.get(m.id);
          if (!resolve) break;
          this._snapshots.delete(m.id);
          const ok = m.blob instanceof Blob && /^image\/(jpeg|png)$/.test(m.blob.type) && m.blob.size < 20 * 1024 * 1024;
          resolve(ok ? m.blob : null);
          break;
        }
        case 'frame':
          if (this._ambient && typeof ImageBitmap !== 'undefined' && m.bitmap instanceof ImageBitmap) {
            const c = this._ambient;
            if (c.width !== m.bitmap.width || c.height !== m.bitmap.height) {
              c.width = m.bitmap.width;
              c.height = m.bitmap.height;
            }
            c.getContext('2d').drawImage(m.bitmap, 0, 0);
            m.bitmap.close();
          }
          break;
        case 'touch': {
          if (!['start', 'move', 'end'].includes(m.phase)) break;
          const r = this._frame.getBoundingClientRect();
          this.dispatchEvent(new CustomEvent('flash-touch', {
            detail: { phase: m.phase, clientX: r.left + (Number(m.x) || 0), clientY: r.top + (Number(m.y) || 0) },
          }));
          break;
        }
      }
    }

    /** Load a movie: { url } (fetched here, with the session) or { data: ArrayBuffer|Blob }.
     *  Options: autoplay (default true), volume, background (keep playing in background tabs). */
    async load(opts) {
      if (this._loading) return this._loading.promise;
      let resolve, reject;
      const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
      this._loading = { promise, resolve, reject };
      try {
        let data = opts.data;
        let base;
        if (opts.url) {
          const url = new URL(opts.url, location.href);
          if (url.origin !== location.origin) throw new Error('SWF must come from this site');
          const res = await fetch(url, { credentials: 'same-origin' });
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
          data = await res.arrayBuffer();
          base = new URL('.', url).href;
        } else if (data instanceof Blob) {
          data = await data.arrayBuffer();
        }
        if (!(data instanceof ArrayBuffer)) throw new Error('No SWF data');
        if (opts.volume !== undefined) this._volume = clamp01(opts.volume);
        this._post({
          type: 'load',
          data,
          base,
          autoplay: opts.autoplay !== false,
          volume: this._volume,
          background: !!opts.background,
        }, [data]);
      } catch (err) {
        reject(err);
      }
      return promise;
    }

    play() { this._post({ type: 'play' }); }
    pause() { this._post({ type: 'pause' }); }

    get volume() { return this._volume; }
    set volume(v) {
      this._volume = clamp01(v);
      this._post({ type: 'volume', value: this._volume });
    }

    get isPlaying() { return this._playing; }
    // Name used by danmaku.js (Ruffle's old property)
    get is_playing() { return this._playing; }

    /** JPEG (or PNG) of the current frame, at most `max` px on the long side. */
    snapshot({ max = 640, type = 'image/jpeg', quality = 0.92 } = {}) {
      return new Promise((resolve) => {
        const id = ++this._snapshotSeq;
        this._snapshots.set(id, resolve);
        this._post({ type: 'snapshot', id, max, format: type, quality });
        setTimeout(() => {
          if (this._snapshots.delete(id)) resolve(null);
        }, 5000);
      });
    }

    /** Canvas kept in sync with the movie at low resolution (starts streaming on first use). */
    get ambientCanvas() {
      if (!this._ambient) {
        this._ambient = document.createElement('canvas');
        this._ambient.width = 1;
        this._ambient.height = 1;
        this._post({ type: 'ambient', on: true });
      }
      return this._ambient;
    }
  }

  window.customElements.define('f0ck-flash', F0ckFlash);
})();
