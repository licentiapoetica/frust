/**
 * danmaku.js — NicoNico/弾幕-style flying comments for v0ck
 *
 * Usage:
 *   const d = new Danmaku(playerEl, videoEl);
 *   d.load(commentsArray);          // feed comments from API
 *   d.fire('hello!', 'user', '#f0f'); // fire one immediately
 *   d.toggle();                     // toggle on/off
 *   d.destroy();                    // cleanup
 */
(function () {

const PILL_MIN_MS      = 6000;   // Fastest a pill can cross the screen
const PILL_MAX_MS      = 12000;  // Slowest a pill can cross the screen
const LOOKAHEAD_SEC    = 0.25;   // How far ahead of currentTime we look when scanning
const MIN_RANDOM_SECS  = 2;      // Random timecode lower bound (avoid very start)
const RANDOM_SPREAD    = 0.85;   // Use 85% of duration for random spread

const DEFAULT_DANMAKU_TUNING = {
    fontSize: 35,
    opacity: 1.0,
    speedMultiplier: 1.0,
    laneCount: 10,
    laneCoverage: 100,
    fontWeight: 700,
    outlineStyle: 2, // 0=None, 1=Subtle, 2=Default Outline, 3=Neon Glow
    useCustomColor: 0,
    customColor: '#ffffff',
    pillBackground: 0, // 0=None, 1=Glass Card, 2=Solid Capsule
    allowMediaEmbeds: 1,
    mediaMaxHeight: 80,
    showGreentext: 1,
    densityLimit: 35,
    flashInterval: 2.5,
    showLaneGuides: 0,
    showDebugHUD: 0
};

const loadDanmakuTuning = () => {
    try {
        const raw = localStorage.getItem('f0ck_danmaku_tuning');
        return raw ? Object.assign({}, DEFAULT_DANMAKU_TUNING, JSON.parse(raw)) : Object.assign({}, DEFAULT_DANMAKU_TUNING);
    } catch (e) {
        return Object.assign({}, DEFAULT_DANMAKU_TUNING);
    }
};

window.DEFAULT_DANMAKU_TUNING = DEFAULT_DANMAKU_TUNING;
window.danmakuTuning = window.danmakuTuning || loadDanmakuTuning();

/**
 * SyntheticClock — emulates a <video> element's time API for non-video items
 * (Flash/Ruffle). Ticks at 4 Hz so Danmaku's timeupdate handler fires normally.
 */
class SyntheticClock {
    constructor() {
        this._currentTime = 0;
        this._paused = false;
        this._listeners = { timeupdate: [], seeked: [] };
        this._timer = this._startTimer();
    }
    _startTimer() {
        return setInterval(() => {
            if (this._paused) return;
            this._currentTime += 0.25;
            this._listeners.timeupdate.forEach(fn => fn());
        }, 250);
    }
    get currentTime() { return this._currentTime; }
    get duration()    { return Infinity; }
    get paused()      { return this._paused; }
    pause() {
        // For Flash/Ruffle: never pause the clock — let pills and time advance freely.
        // Ruffle's is_playing is unreliable and would stall danmaku if respected.
        this._paused = true;
    }
    resume() {
        this._paused = false;
    }
    addEventListener(type, fn, opts) {
        if (this._listeners[type]) this._listeners[type].push(fn);
    }
    removeEventListener(type, fn) {
        if (this._listeners[type])
            this._listeners[type] = this._listeners[type].filter(f => f !== fn);
    }
    /** Reset the clock to zero (used for looping in Flash/Ruffle mode). */
    reset() {
        this._currentTime = 0;
        this._listeners.seeked.forEach(fn => fn());
    }
    destroy() { clearInterval(this._timer); }
}

class Danmaku {
    /**
     * @param {HTMLElement} playerEl  — the .v0ck wrapper element
     * @param {HTMLVideoElement|HTMLAudioElement} mediaEl — the <video> or <audio>
     */
    constructor(playerEl, mediaEl) {
        this.player      = playerEl;
        this.media       = mediaEl;
        this._synthClock = (mediaEl instanceof SyntheticClock) ? mediaEl : null;
        this.overlay     = null;
        this.items       = [];
        this._lastTime   = -1;
        this._paused     = false;

        const initialLanes = Math.max(2, Math.min(30, Number(window.danmakuTuning?.laneCount) || 10));
        this._laneUntil  = new Array(initialLanes).fill(0);

        // Site-wide config default
        const configDefault = (window.f0ckSession && window.f0ckSession.enable_danmaku !== undefined)
            ? !!window.f0ckSession.enable_danmaku
            : true;

        // User preference
        const savedPref = localStorage.getItem('danmaku');

        if (savedPref !== null) {
            // User has explicitly chosen ON or OFF in the past
            this._enabled = savedPref !== 'false';
        } else {
            // No user preference yet — use the site-wide factory default
            this._enabled = configDefault;
        }
        this._bound_onTime  = this._onTimeUpdate.bind(this);
        this._bound_onSeek  = this._onSeeked.bind(this);
        this._bound_onPause = this._onPause.bind(this);
        this._bound_onPlay  = this._onPlay.bind(this);

        // Own emoji cache — populated via CommentSystem, event, or independent fetch
        this._emojiCache = {};
        this._initEmojiCache();

        this._createOverlay();
        this._applyTuning();

        this._bound_onTuningChange = () => this._applyTuning();
        window.addEventListener('f0ck:danmaku_tuning_changed', this._bound_onTuningChange);

        this.media.addEventListener('timeupdate', this._bound_onTime,  { passive: true });
        this.media.addEventListener('seeked',     this._bound_onSeek,  { passive: true });
        this.media.addEventListener('pause',      this._bound_onPause, { passive: true });
        this.media.addEventListener('play',       this._bound_onPlay,  { passive: true });

        // Comment markers on the progress bar need the duration
        this._bound_onDuration = () => this._renderMarkers();
        this._bound_onMarker   = this._onMarkerClick.bind(this);
        this.media.addEventListener('loadedmetadata', this._bound_onDuration, { passive: true });
        this.media.addEventListener('durationchange', this._bound_onDuration, { passive: true });
    }

    /**
     * Initialise the emoji cache from whichever source resolves first:
     *   1. CommentSystem.emojiCache already populated (fast path — browser was already on a page)
     *   2. f0ck:emojis_ready event (CommentSystem finishes loading after us)
     *   3. Independent fetch (Ruffle/Flash items where CommentSystem may never fire the event)
     */
    _initEmojiCache() {
        // Fast path: CommentSystem already populated (AJAX nav / second page view)
        const tryCs = () => {
            const cs = (typeof CommentSystem !== 'undefined') ? CommentSystem.emojiCache : null;
            return (cs && Object.keys(cs).length > 0) ? cs : null;
        };

        const cs0 = tryCs();
        if (cs0) { this._emojiCache = cs0; return; }

        // Listen for CommentSystem's own event
        this._bound_onEmojis = (e) => {
            const map = e.detail || tryCs() || {};
            if (map && Object.keys(map).length > 0) {
                this._emojiCache = map;
                this._reRenderEmojis();
            }
        };
        window.addEventListener('f0ck:emojis_ready', this._bound_onEmojis);

        // Aggressive retry: try every 500 ms for up to 30 attempts.
        let attempts = 0;
        let fetched  = false;
        const retry = () => {
            if (this._emojiCache && Object.keys(this._emojiCache).length > 0) return;
            if (++attempts > 30) return;

            const cs = tryCs();
            if (cs) {
                this._emojiCache = cs;
                this._reRenderEmojis();
                return;
            }

            if (!fetched) {
                fetched = true;
                fetch('/api/v2/emojis')
                    .then(r => {
                        if (!r.ok) throw new Error(`emoji fetch ${r.status}`);
                        return r.json();
                    })
                    .then(data => {
                        if (!data.success || !Array.isArray(data.emojis)) return;
                        const map = {};
                        data.emojis.forEach(e => { map[e.name] = e.url; });
                        if (Object.keys(map).length > 0) {
                            this._emojiCache = map;
                            this._reRenderEmojis();
                        }
                    })
                    .catch(err => {
                        console.warn('[Danmaku] emoji fetch failed:', err.message);
                        fetched = false;
                    });
            }

            if (!this._destroyed) setTimeout(retry, 500);
        };
        setTimeout(retry, 200);
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /**
     * Load (or reload) comments. Comments with video_time=null get a random time.
     * @param {Array} comments — raw comment objects from /api/comments
     */
    load(comments) {
        if (!Array.isArray(comments) || comments.length === 0) return;

        const duration = this.media.duration;
        const hasDuration = isFinite(duration) && duration > 0;

        // Build the prepared item list
        const mapped = comments
            .filter(c => !c.is_deleted && c.content)
            .map(c => ({
                id:       c.id || null,
                text:     this._prepareText(c.content),
                raw:      c.content,
                username: c.display_name || c.username || '?',
                color:    c.username_color || null,
                raw_time: (c.video_time != null) ? parseFloat(c.video_time) : null,
                fired:    false,
                video_time: 0
            }));

        if (this._synthClock) {
            this._flashPool = mapped;
            this._startFlashLoop();
            return;
        }

        this.items = mapped.map(item => {
            if (item.raw_time !== null) {
                item.video_time = item.raw_time;
            } else if (hasDuration) {
                const spread = duration * RANDOM_SPREAD;
                item.video_time = MIN_RANDOM_SECS + Math.random() * Math.max(0, spread - MIN_RANDOM_SECS);
            } else {
                item.video_time = MIN_RANDOM_SECS + Math.random() * 598;
            }
            return item;
        }).sort((a, b) => a.video_time - b.video_time);

        this._resetFiredState(this.media.currentTime);
        this._lastTime = this.media.currentTime;
        this._renderMarkers();
    }

    /**
     * Random continuous loop for Flash/Ruffle.
     */
    _startFlashLoop() {
        if (this._flashTimer) clearTimeout(this._flashTimer);
        this._flashTimer = null;

        if (!this._flashPool || this._flashPool.length === 0) return;

        const shuffle = arr => {
            for (let i = arr.length - 1; i > 0; i--) {
                const j = Math.floor(Math.random() * (i + 1));
                [arr[i], arr[j]] = [arr[j], arr[i]];
            }
            return arr;
        };

        let queue = shuffle([...this._flashPool]);
        let idx = 0;

        const tick = () => {
            if (this._destroyed || !this._enabled) return;

            if (idx >= queue.length) {
                queue = shuffle([...this._flashPool]);
                idx = 0;
            }

            const item = queue[idx++];
            this._spawnPill(item.text, item.username, item.color);

            const cfg = window.danmakuTuning || DEFAULT_DANMAKU_TUNING;
            const baseInterval = Math.max(0.4, Number(cfg.flashInterval) || 2.5);
            const delay = (baseInterval * 1000) + Math.random() * (baseInterval * 1200);
            this._flashTimer = setTimeout(tick, delay);
        };

        this._flashTimer = setTimeout(tick, 800);
    }

    /**
     * Immediately fire a single comment pill (e.g. user's own new comment).
     */
    fire(text, username, color) {
        if (!this._enabled) return;
        this._spawnPill(this._prepareText(text), username, color);
    }

    /**
     * Add a new comment to the timeline so it loops back in future playback.
     */
    addItem(comment) {
        if (!comment || !comment.content) return;
        if (comment.id && this.items && this.items.some(i => i.id === comment.id)) return;

        const duration    = this.media.duration;
        const hasDuration = isFinite(duration) && duration > 0;

        let t = (comment.video_time != null) ? parseFloat(comment.video_time) : null;

        if (t === null) {
            const now = this.media.currentTime || 0;
            if (hasDuration) {
                const remaining = duration - now;
                const spread    = Math.max(remaining * 0.9, MIN_RANDOM_SECS);
                t = now + MIN_RANDOM_SECS + Math.random() * spread;
                if (t > duration) t = MIN_RANDOM_SECS + Math.random() * duration * RANDOM_SPREAD;
            } else {
                t = (this.media.currentTime || 0) + MIN_RANDOM_SECS + Math.random() * 300;
            }
        }

        const item = {
            id: comment.id || null,
            video_time: t,
            text: this._prepareText(comment.content),
            raw: comment.content,
            raw_time: (comment.video_time != null) ? parseFloat(comment.video_time) : null,
            username: comment.display_name || comment.username || '?',
            color: comment.username_color || null,
            fired: true
        };

        if (this._synthClock && this._flashPool) {
            if (!this._flashPool.some(i => (comment.id && i.id === comment.id) || (i.text === item.text && i.username === item.username))) {
                this._flashPool.push(item);
            }
            return;
        }

        const idx = this.items.findIndex(i => i.video_time > t);
        if (idx === -1) this.items.push(item);
        else            this.items.splice(idx, 0, item);
        this._renderMarkers();
    }

    /** Toggle danmaku on/off. */
    toggle() {
        this._enabled = !this._enabled;
        localStorage.setItem('danmaku', this._enabled ? 'true' : 'false');
        if (this.overlay) this.overlay.style.display = this._enabled ? '' : 'none';
        // Comment markers on the progress bar belong to danmaku: hidden while it is off
        if (this._markerLayer) this._markerLayer.style.display = this._enabled ? '' : 'none';
        if (!this._enabled) this._hideMarkerPreview();

        const sw = this.player.querySelector('#toggledanmaku');
        if (sw) sw.classList.toggle('active', this._enabled);
    }

    setEnabled(val) {
        if (this._enabled === !!val) return;
        this.toggle();
    }

    isEnabled() { return this._enabled; }

    toggleLoop(forcedVal, options = {}) {
        this._loopMode = (forcedVal !== undefined) ? !!forcedVal : !this._loopMode;
        if (this._loopMode) {
            this.items.forEach(i => { i.fired = false; });
            this._startDebugContinuousLoop(options.interval || 220, options.customText, options.username, options.color);
        } else {
            this._stopDebugContinuousLoop();
        }
        return this._loopMode;
    }

    isLooping() {
        return !!this._loopMode;
    }

    _getSampleComments() {
        return [
            { text: 'Danmaku burst test! :kreygasm:', username: 'BurstUser', color: '#ffcc00' },
            { text: '>be me\n>browsing f0ck\n>feels good man', username: 'Anon', color: '#78b87a' },
            { text: '[spoiler]Secret Classified Spoiler[/spoiler]', username: 'Agent007', color: '#ff5577' },
            { text: '[blur]Sensitive content blur reveal[/blur]', username: 'ModUser', color: '#bb77ff' },
            { text: 'Testing flight speed & collision avoidance 🚀', username: 'DevTester', color: '#00e5ff' },
            { text: '弾幕 Nico Nico style flying comment', username: 'Otaku', color: '#ff88aa' },
            { text: 'Super high density bullet stream! ⚡⚡', username: 'HyperUser', color: '#99ff00' },
            { text: 'FeelsGoodMan :feelsgood:', username: 'Pepe', color: '#55cc55' },
            { text: 'Nice visualizer and danmaku sync! 🔥', username: 'Vibes', color: '#ff6600' },
            { text: '>mfw danmaku is running infinitely :dance_fart:', username: 'F0cker', color: '#78b87a' }
        ];
    }

    _startDebugContinuousLoop(interval = 220, customText = null, username = null, color = null) {
        if (this._debugLoopTimer) clearInterval(this._debugLoopTimer);

        let idx = 0;
        const tick = () => {
            if (this._destroyed || !this._enabled || !this._loopMode) {
                this._stopDebugContinuousLoop();
                return;
            }

            if (customText) {
                this._spawnPill(customText, username || 'LoopTester', color || '#00ffcc');
            } else {
                let pool = (this.items && this.items.length > 0) ? this.items : (this._flashPool || []);
                if (!pool || pool.length === 0) {
                    pool = this._getSampleComments();
                }

                if (idx >= pool.length) {
                    idx = 0;
                    pool.forEach(i => { i.fired = false; });
                }
                const item = pool[idx++];
                if (item) {
                    this._spawnPill(item.text, item.username || 'Tester', item.color || null);
                }
            }
        };

        tick();
        this._debugLoopTimer = setInterval(tick, Math.max(40, interval));
    }

    _stopDebugContinuousLoop() {
        if (this._debugLoopTimer) {
            clearInterval(this._debugLoopTimer);
            this._debugLoopTimer = null;
        }
    }

    clearActivePills() {
        if (!this.overlay) return;
        this.overlay.querySelectorAll('.danmaku-pill').forEach(p => p.remove());
    }

    resetTimeline() {
        if (this._synthClock) {
            this._loopSynth();
        } else {
            this._resetFiredState(this.media ? this.media.currentTime : 0);
            this._lastTime = this.media ? this.media.currentTime : 0;
        }
    }

    fireBurst(count = 10, options = {}) {
        const isFlood = count > 15;
        const interval = isFlood ? 100 : 160;

        if (this._loopMode) {
            this._startDebugContinuousLoop(interval, options.text, options.username, options.color);
            return;
        }

        const samples = this._getSampleComments();
        for (let i = 0; i < count; i++) {
            setTimeout(() => {
                if (this._destroyed || !this._enabled) return;
                const sample = samples[i % samples.length];
                const text = options.text ? `${options.text} #${i + 1}` : `${sample.text} #${i + 1}`;
                const user = options.username || sample.username;
                const col = options.color || sample.color;
                this.fire(text, user, col);
            }, i * interval);
        }
    }

    destroy() {
        this._destroyed = true;
        this._stopDebugContinuousLoop();
        this.media.removeEventListener('timeupdate', this._bound_onTime);
        this.media.removeEventListener('seeked',     this._bound_onSeek);
        this.media.removeEventListener('pause',      this._bound_onPause);
        this.media.removeEventListener('play',       this._bound_onPlay);
        this.media.removeEventListener('loadedmetadata', this._bound_onDuration);
        this.media.removeEventListener('durationchange', this._bound_onDuration);
        if (this._markerLayer && this._markerLayer.parentNode) this._markerLayer.parentNode.removeChild(this._markerLayer);
        this._markerLayer = null;
        if (this._markerTip && this._markerTip.parentNode) this._markerTip.parentNode.removeChild(this._markerTip);
        this._markerTip = null;
        if (this._bound_onEmojis) window.removeEventListener('f0ck:emojis_ready', this._bound_onEmojis);
        if (this._bound_onTuningChange) window.removeEventListener('f0ck:danmaku_tuning_changed', this._bound_onTuningChange);
        if (this._laneGuidesTimer) clearInterval(this._laneGuidesTimer);
        if (this._hudTimer) clearInterval(this._hudTimer);
        if (this._rufflePoller) clearInterval(this._rufflePoller);
        if (this._flashTimer)   clearTimeout(this._flashTimer);
        if (this._synthClock) this._synthClock.destroy();
        if (this.overlay && this.overlay.parentNode) this.overlay.parentNode.removeChild(this.overlay);
        this.overlay = null;
    }

    // ── Private ───────────────────────────────────────────────────────────────

    _createOverlay() {
        this.overlay = document.createElement('div');
        this.overlay.className = 'danmaku-overlay';
        if (!this._enabled) this.overlay.style.display = 'none';
        this.player.appendChild(this.overlay);
    }

    _applyTuning() {
        if (!this.overlay) return;
        const cfg = window.danmakuTuning || DEFAULT_DANMAKU_TUNING;

        this.overlay.style.setProperty('--danmaku-font-size', (cfg.fontSize || 35) + 'px');
        this.overlay.style.setProperty('--danmaku-opacity', cfg.opacity !== undefined ? cfg.opacity : 1);
        this.overlay.style.setProperty('--danmaku-font-weight', cfg.fontWeight || 700);
        this.overlay.style.setProperty('--danmaku-color', Number(cfg.useCustomColor) === 1 ? (cfg.customColor || '#ffffff') : '#ffffff');
        this.overlay.style.setProperty('--danmaku-media-max-h', (cfg.mediaMaxHeight || 80) + 'px');

        this.overlay.classList.toggle('danmaku-bg-glass', Number(cfg.pillBackground) === 1);
        this.overlay.classList.toggle('danmaku-bg-capsule', Number(cfg.pillBackground) === 2);

        this.overlay.classList.toggle('danmaku-glow-none', Number(cfg.outlineStyle) === 0);
        this.overlay.classList.toggle('danmaku-glow-subtle', Number(cfg.outlineStyle) === 1);
        this.overlay.classList.toggle('danmaku-glow-neon', Number(cfg.outlineStyle) === 3);

        this.overlay.classList.toggle('danmaku-no-greentext', Number(cfg.showGreentext) === 0);

        const targetLanes = Math.max(2, Math.min(30, Number(cfg.laneCount) || 10));
        if (this._laneUntil.length !== targetLanes) {
            const old = this._laneUntil;
            this._laneUntil = new Array(targetLanes).fill(0);
            for (let i = 0; i < Math.min(old.length, targetLanes); i++) {
                this._laneUntil[i] = old[i];
            }
        }

        this._updateLaneGuides();
        this._updateDebugHUD();
    }

    _updateLaneGuides() {
        if (!this.overlay) return;
        const cfg = window.danmakuTuning || DEFAULT_DANMAKU_TUNING;
        let guides = this.overlay.querySelector('.danmaku-lane-guides-container');

        if (!cfg.showLaneGuides) {
            if (guides) guides.remove();
            if (this._laneGuidesTimer) {
                clearInterval(this._laneGuidesTimer);
                this._laneGuidesTimer = null;
            }
            return;
        }

        if (!guides) {
            guides = document.createElement('div');
            guides.className = 'danmaku-lane-guides-container';
            this.overlay.appendChild(guides);
        }

        const laneCount = this._laneUntil.length || 10;
        const coverage  = Math.min(100, Math.max(10, Number(cfg.laneCoverage) || 100)) / 100;
        const laneH     = (100 * coverage) / laneCount;

        let html = '';
        const now = Date.now();
        for (let i = 0; i < laneCount; i++) {
            const topPct = i * laneH;
            const isOccupied = (this._laneUntil[i] || 0) > now;
            const remainingSec = isOccupied ? (((this._laneUntil[i] - now) / 1000).toFixed(1) + 's') : 'FREE';
            html += `
              <div class="danmaku-lane-guide ${isOccupied ? 'occupied' : ''}" style="top: ${topPct}%; height: ${laneH}%;">
                <span class="danmaku-lane-badge">L${i} (${topPct.toFixed(0)}%)</span>
                <span class="danmaku-lane-status">${remainingSec}</span>
              </div>
            `;
        }
        guides.innerHTML = html;

        if (!this._laneGuidesTimer) {
            this._laneGuidesTimer = setInterval(() => {
                if (this._destroyed || !this.overlay || !window.danmakuTuning?.showLaneGuides) {
                    if (this._laneGuidesTimer) clearInterval(this._laneGuidesTimer);
                    this._laneGuidesTimer = null;
                    return;
                }
                this._updateLaneGuides();
            }, 300);
        }
    }

    _updateDebugHUD() {
        if (!this.overlay) return;
        const cfg = window.danmakuTuning || DEFAULT_DANMAKU_TUNING;
        let hud = this.overlay.querySelector('.danmaku-debug-hud');

        if (!cfg.showDebugHUD) {
            if (hud) hud.remove();
            if (this._hudTimer) {
                clearInterval(this._hudTimer);
                this._hudTimer = null;
            }
            return;
        }

        if (!hud) {
            hud = document.createElement('div');
            hud.className = 'danmaku-debug-hud';
            this.overlay.appendChild(hud);
        }

        const activePills = this.overlay.querySelectorAll('.danmaku-pill').length;
        const totalItems = this.items.length || (this._flashPool ? this._flashPool.length : 0);
        const firedItems = this.items.filter(i => i.fired).length;
        const curTime = (this.media ? this.media.currentTime : 0).toFixed(1);
        const emojiCount = Object.keys(this._emojiCache || {}).length;

        hud.innerHTML = `
          <div class="hud-title"><i class="fa-solid fa-bolt"></i> Danmaku Debug HUD</div>
          <div class="hud-row"><span>Active Pills:</span><span class="hud-val accent">${activePills} / ${cfg.densityLimit || 35}</span></div>
          <div class="hud-row"><span>Loaded Comments:</span><span class="hud-val">${totalItems} (fired: ${firedItems})</span></div>
          <div class="hud-row"><span>Clock Time:</span><span class="hud-val">${curTime}s ${this._synthClock ? '(Synth)' : ''}</span></div>
          <div class="hud-row"><span>Lanes:</span><span class="hud-val">${this._laneUntil.length} (${cfg.laneCoverage || 100}%)</span></div>
          <div class="hud-row"><span>Speed Multiplier:</span><span class="hud-val">${cfg.speedMultiplier || 1.0}x</span></div>
          <div class="hud-row"><span>Emoji Cache:</span><span class="hud-val">${emojiCount} emojis</span></div>
        `;

        if (!this._hudTimer) {
            this._hudTimer = setInterval(() => {
                if (this._destroyed || !this.overlay || !window.danmakuTuning?.showDebugHUD) {
                    if (this._hudTimer) clearInterval(this._hudTimer);
                    this._hudTimer = null;
                    return;
                }
                this._updateDebugHUD();
            }, 250);
        }
    }

    // ── Progress-bar comment markers ─────────────────────────────────────────
    // One marker per timeline comment on .v0ck_progress. Comments with their own timestamp are solid;
    // the others got a random time on load and are drawn fainter. Clicking jumps there and shows the pill.

    _renderMarkers() {
        if (this._destroyed || this._synthClock) return;
        const track = this.player.querySelector('.v0ck_progress');
        if (!track) return;
        const dur = this.media.duration;
        const ok = isFinite(dur) && dur > 0 && this.items.length > 0;

        let layer = this._markerLayer;
        if (!ok) { if (layer) layer.textContent = ''; return; }
        if (!layer || !layer.isConnected) {
            layer = document.createElement('div');
            layer.className = 'danmaku-markers';
            if (!this._enabled) layer.style.display = 'none';
            // Keep the bar's own scrub handlers (mousedown/pointerdown/click on the track) out of marker clicks
            const stop = (e) => e.stopPropagation();
            layer.addEventListener('pointerdown', stop);
            layer.addEventListener('mousedown', stop);
            layer.addEventListener('touchstart', stop, { passive: true });
            layer.addEventListener('click', this._bound_onMarker);
            layer.addEventListener('mouseover', (e) => {
                const b = e.target.closest && e.target.closest('.danmaku-marker');
                if (b) this._showMarkerPreview(b);
            });
            layer.addEventListener('mouseout', (e) => {
                const to = e.relatedTarget;
                if (!to || !to.closest || !to.closest('.danmaku-marker')) this._hideMarkerPreview();
            });
            track.appendChild(layer);
            this._markerLayer = layer;
        }

        const fmt = (t) => {
            const m = Math.floor(t / 60), sec = Math.floor(t % 60);
            return m + ':' + String(sec).padStart(2, '0');
        };
        const frag = document.createDocumentFragment();
        this.items.forEach((item, i) => {
            if (!(item.video_time >= 0) || item.video_time > dur) return;
            const b = document.createElement('button');
            b.type = 'button';
            b.tabIndex = -1;
            b.className = 'danmaku-marker' + (item.raw_time == null ? ' is-random' : '');
            b.style.left = (item.video_time / dur * 100) + '%';
            b.dataset.idx = String(i);
            const txt = String(item.raw || '').replace(/\s+/g, ' ').trim();
            b.setAttribute('aria-label', fmt(item.video_time) + ' · ' + item.username + ': ' + (txt.length > 80 ? txt.slice(0, 80) + '…' : txt));
            frag.appendChild(b);
        });
        this._hideMarkerPreview();
        layer.textContent = '';
        layer.appendChild(frag);
    }

    // Plain-text preview of a comment above its marker. Built with textContent only (no HTML from comments).
    _showMarkerPreview(b) {
        const item = this.items[+b.dataset.idx];
        if (!item) return;
        let tip = this._markerTip;
        if (!tip || !tip.isConnected) {
            tip = document.createElement('div');
            tip.className = 'danmaku-marker-preview';
            tip.innerHTML = '<div class="dmp-head"><span class="dmp-time"></span><span class="dmp-user"></span></div><div class="dmp-text"></div>';
            this.player.appendChild(tip);
            this._markerTip = tip;
        }
        const t = item.video_time;
        tip.querySelector('.dmp-time').textContent = Math.floor(t / 60) + ':' + String(Math.floor(t % 60)).padStart(2, '0');
        const user = tip.querySelector('.dmp-user');
        user.textContent = item.username;
        user.style.color = item.color || '';
        // Same renderer as the flying pills: emojis, images/media embeds, greentext; spoiler and blur
        // stay hidden. Builds DOM nodes (no HTML from the comment); size is capped by CSS.
        const body = tip.querySelector('.dmp-text');
        body.textContent = '';
        const liveCache = (this._emojiCache && Object.keys(this._emojiCache).length > 0)
            ? this._emojiCache
            : ((typeof CommentSystem !== 'undefined' && CommentSystem.emojiCache) || null);
        if (liveCache && liveCache !== this._emojiCache) this._emojiCache = liveCache;
        body.appendChild(this._renderContent(item.raw || ''));
        if (!body.firstChild) body.textContent = '…';
        // Reposition once images have their size
        body.querySelectorAll('img').forEach(img => {
            if (!img.complete) img.addEventListener('load', () => { if (this._markerTip === tip && tip.classList.contains('is-visible')) this._showMarkerPreview(b); }, { once: true });
        });

        // Position: centred over the marker, just above the progress bar, kept inside the player
        const pr = this.player.getBoundingClientRect();
        const mr = b.getBoundingClientRect();
        tip.style.visibility = 'hidden';
        tip.classList.add('is-visible');
        const w = tip.offsetWidth;
        let left = mr.left + mr.width / 2 - pr.left - w / 2;
        left = Math.max(6, Math.min(left, pr.width - w - 6));
        tip.style.left = left + 'px';
        tip.style.bottom = (pr.bottom - mr.top + 8) + 'px';
        tip.style.visibility = '';
    }

    _hideMarkerPreview() {
        if (this._markerTip) this._markerTip.classList.remove('is-visible');
    }

    _onMarkerClick(e) {
        const b = e.target.closest && e.target.closest('.danmaku-marker');
        if (!b) return;
        e.preventDefault();
        e.stopPropagation();
        const item = this.items[+b.dataset.idx];
        if (!item) return;
        // Land just past the comment: the seek reset then counts it as fired, so the timeline won't
        // spawn it a second time; it is shown right here instead (also while paused)
        const dur = this.media.duration;
        const t = Math.min(isFinite(dur) ? dur : Infinity, item.video_time + LOOKAHEAD_SEC + 0.05);
        try { this.media.currentTime = t; } catch (_) {}
        item.fired = true;
        this._hideMarkerPreview();
        this._spawnPill(item.text, item.username, item.color, true);
    }

    _onPause() {
        this._paused = true;
        if (this._synthClock) this._synthClock.pause();
    }

    _onPlay() {
        this._paused = false;
        if (this._synthClock) this._synthClock.resume();
    }

    _checkRuffleState() {
        const rp = document.querySelector('f0ck-flash');
        if (!rp) return;
        const isPlaying = rp.is_playing !== undefined ? !!rp.is_playing : true;
        if (isPlaying && this._paused)  this._onPlay();
        if (!isPlaying && !this._paused) this._onPause();
    }

    _onTimeUpdate() {
        if (this._paused) return;
        const now = this.media.currentTime;
        if (!this._enabled || this.items.length === 0) { this._lastTime = now; return; }

        const prev = this._lastTime;
        this._lastTime = now;

        if (now < prev - 0.5) {
            this._resetFiredState(now);
            return;
        }

        // Forward jump (seek, marker click, scrubbing): the timeupdate at the new position arrives before
        // 'seeked', and treating the gap as played would fire every comment in between. Resync instead,
        // so only comments from here on appear. Normal playback advances ~0.25s per event (x playbackRate).
        const maxStep = Math.max(1.5, 1.5 * (this.media.playbackRate || 1));
        if (this.media.seeking || now > prev + maxStep) {
            this._resetFiredState(now);
            return;
        }

        const from = prev;
        const to   = now + LOOKAHEAD_SEC;

        for (const item of this.items) {
            if (item.fired) continue;
            if (item.video_time < from) { item.fired = true; continue; }
            if (item.video_time > to) break;
            item.fired = true;
            this._spawnPill(item.text, item.username, item.color);
        }

        if (this._synthClock && this.items.length > 0 && this.items.every(i => i.fired)) {
            this._loopSynth();
        }
    }

    _loopSynth() {
        this.items.forEach(i => { i.fired = false; });
        this._synthClock.reset();
        this._lastTime = 0;
    }

    _onSeeked() {
        this._resetFiredState(this.media.currentTime);
        this._lastTime = this.media.currentTime;
    }

    _resetFiredState(currentTime) {
        for (const item of this.items) {
            item.fired = item.video_time < currentTime - LOOKAHEAD_SEC;
        }
    }

    _pickLane() {
        const now = Date.now();
        const laneCount = this._laneUntil.length || 10;
        let best = 0;
        let bestFree = this._laneUntil[0] || 0;
        for (let i = 1; i < laneCount; i++) {
            if ((this._laneUntil[i] || 0) < bestFree) {
                bestFree = this._laneUntil[i];
                best = i;
            }
        }
        const cfg = window.danmakuTuning || DEFAULT_DANMAKU_TUNING;
        const speedMult = Math.max(0.2, Number(cfg.speedMultiplier) || 1.0);
        const maxMs = PILL_MAX_MS / speedMult;
        this._laneUntil[best] = now + maxMs;
        return best;
    }

    /** @param {boolean} [force] show even while paused (marker click on the progress bar) */
    _spawnPill(text, username, color, force = false) {
        if (!this.overlay || !this._enabled || (this._paused && !force)) return;
        const cfg = window.danmakuTuning || DEFAULT_DANMAKU_TUNING;

        const limit = Number(cfg.densityLimit) || 35;
        const currentPills = this.overlay.querySelectorAll('.danmaku-pill');
        if (currentPills.length >= limit) {
            if (currentPills[0]) currentPills[0].remove();
        }

        const pill = document.createElement('div');
        pill.className = 'danmaku-pill';

        const lane       = this._pickLane();
        const laneCount  = this._laneUntil.length || 10;
        const coverage   = Math.min(100, Math.max(10, Number(cfg.laneCoverage) || 100)) / 100;
        const laneH      = (100 * coverage) / laneCount;
        const topPct     = lane * laneH + (laneH * 0.1);
        pill.style.top   = topPct + '%';

        if (Number(cfg.useCustomColor) === 1 && cfg.customColor) {
            pill.style.color = cfg.customColor;
        }

        const msg = document.createElement('span');
        msg.className = 'dpill-text';
        msg.dataset.rawText = text;

        const liveCache = (this._emojiCache && Object.keys(this._emojiCache).length > 0)
            ? this._emojiCache
            : ((typeof CommentSystem !== 'undefined' && CommentSystem.emojiCache) || null);
        if (liveCache && liveCache !== this._emojiCache) this._emojiCache = liveCache;

        msg.appendChild(this._renderContent(text));

        if (!this._emojiCache || Object.keys(this._emojiCache).length === 0) {
            if (!this._pendingPills) this._pendingPills = new Set();
            this._pendingPills.add(msg);
        }

        pill.appendChild(msg);
        this.overlay.appendChild(pill);

        const charCount = text.length;
        const speedMult = Math.max(0.2, Number(cfg.speedMultiplier) || 1.0);
        const baseDuration = Math.min(Math.max(5000 + charCount * 25, PILL_MIN_MS), 45_000);
        const duration  = baseDuration / speedMult;

        const overlayW  = this.overlay.offsetWidth || window.innerWidth || 1920;
        const contentW  = pill.scrollWidth || pill.offsetWidth || 200;
        const startX    = overlayW + contentW;
        const endX      = -(contentW + 200);

        const anim = pill.animate(
            [
                { transform: `translateX(${startX}px)` },
                { transform: `translateX(${endX}px)` }
            ],
            { duration, easing: 'linear', fill: 'none' }
        );

        anim.addEventListener('finish', () => {
            if (pill.parentNode) pill.parentNode.removeChild(pill);
        }, { once: true });

        pill._timeoutId = setTimeout(() => { if (pill.parentNode) pill.parentNode.removeChild(pill); }, duration + 1000);
    }

    /** Re-render pending pills once emojis are available. */
    _reRenderEmojis() {
        if (!this.overlay) return;
        const pending = this._pendingPills;
        if (pending && pending.size > 0) {
            pending.forEach(msg => {
                if (!msg.parentNode) { pending.delete(msg); return; }
                const raw = msg.dataset.rawText;
                if (!raw) return;
                msg.textContent = '';
                msg.appendChild(this._renderContent(raw));
            });
            pending.clear();
        }
        this.overlay.querySelectorAll('.dpill-text[data-raw-text]').forEach(msg => {
            const raw = msg.dataset.rawText;
            if (!raw) return;
            if (!msg.querySelector('.dpill-emoji')) {
                msg.textContent = '';
                msg.appendChild(this._renderContent(raw));
            }
        });
    }

    /**
     * Build a DocumentFragment from raw comment text.
     * Handles [spoiler], [blur], :emoji:, and >greentext lines.
     */
    _renderContent(rawText) {
        if (!rawText) return document.createDocumentFragment();

        const prepared   = this._prepareText(rawText);
        const frag       = document.createDocumentFragment();
        const cache      = this._emojiCache;

        const lines = prepared.split('\n').filter(l => l.trim() !== '');
        lines.forEach((line) => {
            const isQuote = /^>\s?/.test(line);
            const text    = isQuote ? '> ' + line.replace(/^>\s?/, '') : line;

            const span = document.createElement('span');
            span.className = isQuote ? 'dpill-greentext' : 'dpill-line';
            this._renderInline(text, span, cache);
            frag.appendChild(span);
        });

        return frag;
    }

    /**
     * Renders inline content (spoiler/blur/emoji) into a parent node.
     */
    _renderInline(text, parent, emojiCache) {
        const combined = /\[spoiler\]([\s\S]*?)\[\/spoiler\]|\[blur\]([\s\S]*?)\[\/blur\]|:([a-z0-9_+\-]+):|\x04([^\x05]+)\x05/gi;
        let lastIndex = 0;
        let match;

        while ((match = combined.exec(text)) !== null) {
            if (match.index > lastIndex) {
                parent.appendChild(document.createTextNode(text.slice(lastIndex, match.index)));
            }

            if (match[1] !== undefined) {
                const span = document.createElement('span');
                span.className = 'dpill-spoiler';
                span.title = 'Click to reveal spoiler';
                span.addEventListener('click', (e) => { e.stopPropagation(); span.classList.toggle('revealed'); });
                this._renderInline(match[1], span, emojiCache);
                parent.appendChild(span);
            } else if (match[2] !== undefined) {
                const span = document.createElement('span');
                span.className = 'dpill-blur';
                span.title = 'Click to reveal';
                span.addEventListener('click', (e) => { e.stopPropagation(); span.classList.toggle('revealed'); });
                this._renderInline(match[2], span, emojiCache);
                parent.appendChild(span);
            } else if (match[3]) {
                const code = match[3];
                const url  = emojiCache[code];
                if (url) {
                    if (url.endsWith('.webm')) {
                        const vid = document.createElement('video');
                        vid.src = url;
                        vid.className = 'dpill-emoji';
                        vid.muted = true;
                        vid.loop = true;
                        vid.autoplay = true;
                        vid.playsInline = true;
                        vid.play().catch(() => {
                            vid.addEventListener('canplay', () => vid.play().catch(() => {}), { once: true });
                        });
                        parent.appendChild(vid);
                    } else {
                        const img = document.createElement('img');
                        img.src = url;
                        img.alt = `:${code}:`;
                        img.className = 'dpill-emoji';
                        parent.appendChild(img);
                    }
                } else {
                    parent.appendChild(document.createTextNode(match[0]));
                }
            } else if (match[4]) {
                const mediaUrl = match[4];
                const cleanUrl = mediaUrl.replace(/#gif$/, '');
                const videoExts = /\.(?:mp4|webm|ogv|mov)$/i;
                const audioExts = /\.(?:mp3|ogg|wav|flac|aac|opus|m4a)$/i;
                const cfg = window.danmakuTuning || DEFAULT_DANMAKU_TUNING;

                if (Number(cfg.allowMediaEmbeds) === 0) {
                    const span = document.createElement('span');
                    span.className = 'dpill-media-placeholder';
                    span.textContent = ' [attachment] ';
                    parent.appendChild(span);
                } else if (videoExts.test(cleanUrl)) {
                    const vid = document.createElement('video');
                    vid.src = cleanUrl;
                    vid.className = 'dpill-video';
                    vid.muted = true;
                    vid.loop = true;
                    vid.autoplay = true;
                    vid.playsInline = true;
                    vid.play().catch(() => { vid.addEventListener('canplay', () => vid.play().catch(() => {}), { once: true }); });
                    parent.appendChild(vid);
                } else if (audioExts.test(cleanUrl)) {
                    const span = document.createElement('span');
                    span.className = 'dpill-audio';
                    span.textContent = '🔊 ';
                    parent.appendChild(span);
                } else {
                    const img = document.createElement('img');
                    img.src = cleanUrl;
                    img.className = 'dpill-img';
                    parent.appendChild(img);
                }
            }

            lastIndex = match.index + match[0].length;
        }

        if (lastIndex < text.length) {
            parent.appendChild(document.createTextNode(text.slice(lastIndex)));
        }
    }

    /** Strips markdown and character limits; preserves > and newlines for _renderContent. */
    _prepareText(text) {
        if (!text) return '';

        // Idempotent: pills prepare the text on load and _renderContent prepares it again. Media already
        // wrapped as \x04url\x05 is kept as one token; re-matching the URL inside it used to double-wrap
        // it and leave a stray \x05 that browsers draw as a little control-character box behind the image.
        const imgTokenUrls = [];
        const emojiTokens = [];
        let protected_ = text
            .replace(/\x04([^\x04\x05]+)\x05/g, (_, url) => {
                imgTokenUrls.push(url);
                return `\x04${imgTokenUrls.length - 1}\x05`;
            })
            .replace(/:([a-z0-9_+\-]+):/gi, (match) => {
                emojiTokens.push(match);
                return `\x02${emojiTokens.length - 1}\x03`;
            });

        const allowedHosts = Array.isArray(window.f0ckAllowedImages) ? window.f0ckAllowedImages : [];
        const siteHost = window.location.hostname;
        const isAllowedImg = (url) => {
            try {
                const h = new URL(url).hostname;
                return h === siteHost || allowedHosts.some(a => h === a || h.endsWith('.' + a));
            } catch { return false; }
        };
        protected_ = protected_
            .replace(/\/c\/[a-f0-9]+\.(?:png|jpg|jpeg|gif|webp|svg|avif|mp4|webm|ogv|mov|mp3|ogg|wav|flac|aac|opus|m4a)(?:#gif)?/gi, (url) => {
                imgTokenUrls.push(url);
                return `\x04${imgTokenUrls.length - 1}\x05`;
            })
            .replace(/https?:\/\/(?:(?!https?:\/\/)\S)+\.(?:png|jpg|jpeg|gif|webp|svg|avif)(\?(?:(?!https?:\/\/)\S)*)?/gi, (url) => {
                if (!isAllowedImg(url)) return url;
                imgTokenUrls.push(url);
                return `\x04${imgTokenUrls.length - 1}\x05`;
            })
            .replace(/```[\s\S]*?```/g, '[code]')
            .replace(/`[^`]+`/g, match => match.slice(1, -1))
            .replace(/!\[[^\]]*\]\([^)]+\)/g, '[img]')
            .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
            .replace(/^#{1,6}\s+/gm, '')
            .replace(/[*_]{1,3}([^*_]+)[*_]{1,3}/g, '$1')
            .replace(/\r\n?/g, '\n')
            .replace(/\n{3,}/g, '\n\n')
            .trim();

        // Any other control characters (from the comment itself) would render as visible boxes; strays
        // of the token markers too. Valid media tokens are parked on \x0E/\x0F (already stripped) meanwhile.
        return protected_
            .replace(/[\x00-\x01\x06-\x08\x0B-\x1F\x7F]/g, '')
            .replace(/\x02(\d+)\x03/g, (_, i) => emojiTokens[+i] || '')
            .replace(/\x04(\d+)\x05/g, (_, i) => imgTokenUrls[+i] ? '\x0E' + i + '\x0F' : '')
            .replace(/[\x02-\x05]/g, '')
            .replace(/\x0E(\d+)\x0F/g, (_, i) => '\x04' + imgTokenUrls[+i] + '\x05');
    }
}

Danmaku.applyGlobalTuning = function(newCfg) {
    if (newCfg) {
        Object.assign(window.danmakuTuning, newCfg);
        try {
            localStorage.setItem('f0ck_danmaku_tuning', JSON.stringify(window.danmakuTuning));
        } catch (e) {}
    }
    window.dispatchEvent(new CustomEvent('f0ck:danmaku_tuning_changed', { detail: window.danmakuTuning }));
};

window.Danmaku       = Danmaku;
window.SyntheticClock = SyntheticClock;

})();
