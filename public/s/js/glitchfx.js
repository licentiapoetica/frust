/*
 * glitchfx.js: datamosh glitch for any element (text, images, video, whole boxes), modelled on
 * w0bm_mosh_banner_by_marderchen.gif: per-frame RGB channel split, torn horizontal slices,
 * displaced and colour-corrupted macroblocks, dripping columns and stray streaks, mixed with
 * clean frames at ~20 fps.
 *
 * It is an SVG filter applied with CSS (`filter: url(#glitchfx-N)` via the classes in
 * glitchfx.css), so it works on any element and needs no canvas access to the content.
 * A small displacement map (canvas → data: URL) drives the tearing:
 *   red = x shift, green = y shift (128 = none), blue = colour-corruption mask.
 *
 * Usage:
 *   <link rel="stylesheet" href="/s/css/glitchfx.css">
 *   <script src="/s/js/glitchfx.js"></script>
 *
 *   Markup (auto-started):  <img src="logo.png" data-glitch>            default strength
 *                           <h1 data-glitch="0.9">w0bm.com</h1>          strength 0..1
 *                           <div data-glitch data-glitch-hover>…</div>   only while hovered/focused
 *                           <video data-glitch data-glitch-idle>…</video> random short bursts
 *   Script:                 const g = GlitchFX(el, { intensity: 0.7 });
 *                           g.stop(); g.start(); g.burst(800); g.set({ intensity: 1 }); g.destroy();
 *                           GlitchFX.all(".logo", { hover: true });
 *
 * Options: intensity (0..1, 0.6), fps (20), hover (false), idle (false: run all the time;
 * true or [minMs, maxMs]: quiet with random bursts), shift (max slice/block shift in px, auto),
 * split (max RGB split in px, auto), corrupt (colour corruption, true), reducedMotion
 * ("respect": still when the visitor prefers reduced motion; "ignore").
 *
 * Inline elements: give them display:inline-block (the filter works on the element's box).
 * Up to 32 elements glitch at the same time (CSS slots .glitchfx-0 … .glitchfx-31).
 */
(function (global) {
  "use strict";

  var SVGNS = "http://www.w3.org/2000/svg";
  var SLOTS = 32;
  var POOL = 24; // displacement maps kept per element, refreshed a few at a time
  var used = [];
  var defs = null;
  var instances = new Map();

  function rnd(a, b) { return a + Math.random() * (b - a); }
  function irnd(a, b) { return Math.floor(rnd(a, b + 1)); }
  function chance(p) { return Math.random() < p; }
  function pick(a) { return a[Math.floor(Math.random() * a.length)]; }
  function clamp(v, a, b) { return v < a ? a : v > b ? b : v; }

  function svgEl(name, attrs, parent) {
    var e = document.createElementNS(SVGNS, name);
    for (var k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  }

  function getDefs() {
    if (defs && defs.isConnected) return defs;
    var svg = svgEl("svg", { "class": "glitchfx-defs", "aria-hidden": "true", focusable: "false" });
    defs = svgEl("defs", {}, svg);
    document.body.appendChild(svg);
    return defs;
  }

  // Colour corruption presets (feColorMatrix rows), like the pink/green/inverted macroblocks
  var CORRUPT = [
    "0 0 1 0 0  0 1 0 0 0  1 0 0 0 0  0 0 0 1 0",      // swap red/blue
    "0 1 0 0 0  1 0 0 0 0  0 0 1 0 0  0 0 0 1 0",      // swap red/green
    "1 0 0 0 .3  0 .2 0 0 0  .6 0 .8 0 .25  0 0 0 1 0", // magenta
    "0 .3 0 0 0  .2 1 0 0 .2  0 .3 0 0 0  0 0 0 1 0",   // green
    "-1 0 0 0 1  0 -1 0 0 1  0 0 -1 0 1  0 0 0 1 0",    // inverted
    "0 1 0 0 0  0 0 1 0 0  1 0 0 0 0  0 0 0 1 0"       // rotate channels
  ];
  var ALPHA_FROM_BLUE = "0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 1 0 0";
  var ONLY = {
    r: "1 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0",
    g: "0 0 0 0 0  0 1 0 0 0  0 0 0 0 0  0 0 0 1 0",
    b: "0 0 0 0 0  0 0 0 0 0  0 0 1 0 0  0 0 0 1 0"
  };

  function buildFilter(id) {
    var f = svgEl("filter", {
      id: id,
      x: "-6%", y: "-25%", width: "112%", height: "150%",
      filterUnits: "objectBoundingBox",
      primitiveUnits: "userSpaceOnUse",
      // sRGB, so a map value of 128 is exactly "no shift"
      "color-interpolation-filters": "sRGB"
    }, getDefs());
    var n = {};
    n.map = svgEl("feImage", { x: 0, y: 0, width: 1, height: 1, preserveAspectRatio: "none", "image-rendering": "optimizeSpeed", result: "map" }, f);
    n.disp = svgEl("feDisplacementMap", { "in": "SourceGraphic", in2: "map", scale: 0, xChannelSelector: "R", yChannelSelector: "G", result: "d" }, f);
    n.corrupt = svgEl("feColorMatrix", { "in": "d", type: "matrix", values: CORRUPT[0], result: "c" }, f);
    svgEl("feColorMatrix", { "in": "map", type: "matrix", values: ALPHA_FROM_BLUE, result: "mask" }, f);
    svgEl("feComposite", { "in": "c", in2: "mask", operator: "in", result: "cm" }, f);
    svgEl("feComposite", { "in": "cm", in2: "d", operator: "over", result: "m" }, f);
    n.off = {};
    ["r", "g", "b"].forEach(function (ch) {
      svgEl("feColorMatrix", { "in": "m", type: "matrix", values: ONLY[ch], result: ch }, f);
      n.off[ch] = svgEl("feOffset", { "in": ch, dx: 0, dy: 0, result: ch + "o" }, f);
    });
    svgEl("feBlend", { "in": "ro", in2: "go", mode: "screen", result: "rg" }, f);
    svgEl("feBlend", { "in": "rg", in2: "bo", mode: "screen" }, f);
    return { el: f, n: n };
  }

  function reducedMotion() {
    return global.matchMedia && global.matchMedia("(prefers-reduced-motion: reduce)").matches;
  }

  function Glitch(el, opts) {
    this.el = el;
    this.o = Object.assign({ intensity: 0.6, fps: 20, hover: false, idle: false, shift: 0, split: 0, corrupt: true, reducedMotion: "respect" }, opts || {});
    this.slot = -1;
    this.running = false;
    this.wanted = false;
    this.visible = true;
    this.hovering = false;
    this.burstUntil = 0;
    this.nextIdle = 0;
    this.last = 0;
    this.pool = [];
    this.neutral = null;
    this.raf = 0;
    this.tick = this.tick.bind(this);

    var self = this;
    if (global.ResizeObserver) {
      this.ro = new ResizeObserver(function () { self.measure(); });
      this.ro.observe(el);
    }
    if (global.IntersectionObserver) {
      this.io = new IntersectionObserver(function (es) {
        self.visible = es[es.length - 1].isIntersecting;
        self.update();
      });
      this.io.observe(el);
    }
    if (this.o.hover) {
      this.onEnter = function () { self.hovering = true; self.update(); };
      this.onLeave = function () { self.hovering = false; self.update(); };
      ["mouseenter", "focusin"].forEach(function (t) { el.addEventListener(t, self.onEnter); });
      ["mouseleave", "focusout"].forEach(function (t) { el.addEventListener(t, self.onLeave); });
    }
    this.start();
  }

  Glitch.prototype.measure = function () {
    var r = this.el.getBoundingClientRect();
    var w = Math.max(1, Math.round(r.width)), h = Math.max(1, Math.round(r.height));
    if (w === this.w && h === this.h) return;
    this.w = w; this.h = h;
    // Map cells: blocky like codec macroblocks, at most ~320 cells wide
    this.cell = clamp(Math.round(Math.min(w, h * 4) / 160), 1, 8);
    this.cell = Math.max(this.cell, Math.ceil(w / 320));
    // The map covers the whole filter region (see buildFilter): the element plus neutral
    // margins. Outside the map the filter would read "full shift up-left" and ghost the content.
    this.ox = Math.ceil(w * 0.06 / this.cell) + 1;
    this.oy = Math.ceil(h * 0.25 / this.cell) + 1;
    this.ew = Math.ceil(w / this.cell);
    this.eh = Math.ceil(h / this.cell);
    this.mw = this.ew + 2 * this.ox;
    this.mh = this.eh + 2 * this.oy;
    this.pool = [];
    this.neutral = null;
    this.placeMap();
  };

  Glitch.prototype.placeMap = function () {
    if (!this.f) return;
    var m = this.f.n.map, c = this.cell;
    m.setAttribute("x", -this.ox * c);
    m.setAttribute("y", -this.oy * c);
    m.setAttribute("width", this.mw * c);
    m.setAttribute("height", this.mh * c);
  };

  Glitch.prototype.canvas = function () {
    if (!this.cv) {
      this.cv = document.createElement("canvas");
      this.cx = this.cv.getContext("2d");
    }
    this.cv.width = this.mw;
    this.cv.height = this.mh;
    var c = this.cx;
    c.fillStyle = "rgb(128,128,0)";
    c.fillRect(0, 0, this.mw, this.mh);
    return c;
  };

  // One torn frame. Shifts are in map units: 127 = full `shift` px.
  Glitch.prototype.makeMap = function () {
    var c = this.canvas(), W = this.ew, H = this.eh, k = this.o.intensity, ox = this.ox, oy = this.oy;
    var amt = function (s) { return clamp(Math.round(128 + s * 127), 0, 255); };
    var sgn = function () { return chance(0.5) ? -1 : 1; };
    function rect(x, y, w, h, dx, dy, corrupt) {
      c.fillStyle = "rgb(" + amt(dx) + "," + amt(dy) + "," + (corrupt ? 255 : 0) + ")";
      c.fillRect(ox + x, oy + y, Math.min(w, W - x), Math.min(h, H - y));
    }
    // torn horizontal slices
    var slices = irnd(0, 1 + Math.round(3 * k));
    for (var i = 0; i < slices; i++) {
      var sh = Math.max(1, Math.round(H * rnd(0.04, 0.3)));
      rect(0, irnd(0, H - 1), W, sh, sgn() * rnd(0.1, 0.6) * k, 0, chance(0.12 * k));
    }
    // displaced macroblocks, some colour-corrupted
    var bs = Math.max(2, Math.round(Math.min(W, H) / 6));
    var blocks = irnd(0, Math.round(6 * k));
    for (i = 0; i < blocks; i++) {
      var bw = bs * irnd(1, 4), bh = bs * irnd(1, 2);
      rect(irnd(0, W - 1), irnd(0, H - 1), bw, bh, sgn() * rnd(0, 0.5) * k, sgn() * rnd(0, 0.3) * k, this.o.corrupt && chance(0.45));
    }
    // dripping columns (pixels dragged down) and thin streak rows (stray dots)
    var drips = irnd(0, Math.round(3 * k));
    for (i = 0; i < drips; i++) {
      var dh = Math.round(H * rnd(0.2, 0.6));
      rect(irnd(0, W - 1), H - dh, irnd(1, 2), dh, 0, -rnd(0.2, 0.7), false);
    }
    var streaks = irnd(0, Math.round(2 * k));
    for (i = 0; i < streaks; i++) rect(0, irnd(0, H - 1), W, 1, sgn() * rnd(0.5, 1), 0, false);
    // now and then a big corrupted area (the bear going pink)
    if (this.o.corrupt && chance(0.08 * k)) rect(0, irnd(0, H >> 1), irnd(W >> 3, W >> 1), irnd(H >> 2, H), 0, 0, true);
    return this.cv.toDataURL();
  };

  Glitch.prototype.neutralMap = function () {
    if (!this.neutral) {
      this.canvas();
      this.neutral = this.cv.toDataURL();
    }
    return this.neutral;
  };

  Glitch.prototype.frame = function () {
    var n = this.f.n, k = this.o.intensity;
    var shift = this.o.shift || clamp(this.w * 0.08, 6, 80);
    var split = this.o.split || clamp(this.h * 0.03, 1.5, 8);
    var clean = chance(0.55 - 0.4 * k);
    if (clean) {
      n.map.setAttribute("href", this.neutralMap());
      n.disp.setAttribute("scale", 0);
    } else {
      if (this.pool.length < POOL) this.pool.push(this.makeMap());
      else if (chance(0.25)) this.pool[irnd(0, POOL - 1)] = this.makeMap();
      n.map.setAttribute("href", pick(this.pool));
      n.disp.setAttribute("scale", (shift * 2).toFixed(1));
      n.corrupt.setAttribute("values", pick(CORRUPT));
    }
    // RGB split: red one way, blue the other (green stays), sometimes a vertical kick
    var s = clean ? (chance(0.5) ? split * 0.35 : 0) : split * rnd(0.3, 1) * (0.5 + k);
    var dir = chance(0.5) ? 1 : -1;
    n.off.r.setAttribute("dx", (dir * s).toFixed(2));
    n.off.b.setAttribute("dx", (-dir * s * rnd(0.5, 1)).toFixed(2));
    n.off.r.setAttribute("dy", !clean && chance(0.2) ? (rnd(-1, 1) * split * 0.6).toFixed(2) : 0);
    n.off.b.setAttribute("dy", 0);
    n.off.g.setAttribute("dx", !clean && chance(0.15) ? (rnd(-1, 1) * split * 0.5).toFixed(2) : 0);
  };

  Glitch.prototype.idleWindow = function () {
    var i = this.o.idle;
    return Array.isArray(i) ? i : [1500, 6000];
  };

  // Whether frames should be drawn right now
  Glitch.prototype.active = function (now) {
    if (now < this.burstUntil) return true;
    if (this.o.hover) return this.hovering;
    if (this.o.idle) {
      if (now >= this.nextIdle) {
        var w = this.idleWindow();
        this.burstUntil = now + rnd(250, 900);
        this.nextIdle = this.burstUntil + rnd(w[0], w[1]);
        return true;
      }
      return false;
    }
    return true;
  };

  Glitch.prototype.attach = function () {
    if (this.slot >= 0) return;
    var slot = used.indexOf(false);
    if (slot < 0) {
      if (used.length >= SLOTS) return; // all slots busy: stays still
      slot = used.length;
      used.push(false);
    }
    used[slot] = true;
    this.slot = slot;
    var id = "glitchfx-" + slot;
    var old = document.getElementById(id);
    if (old) old.remove();
    this.f = buildFilter(id);
    this.w = 0;
    this.measure();
    this.el.classList.add("glitchfx", id);
  };

  Glitch.prototype.detach = function () {
    if (this.slot < 0) return;
    this.el.classList.remove("glitchfx", "glitchfx-" + this.slot);
    if (this.f) this.f.el.remove();
    this.f = null;
    used[this.slot] = false;
    this.slot = -1;
  };

  Glitch.prototype.tick = function (now) {
    this.raf = 0;
    if (!this.running) return;
    this.raf = requestAnimationFrame(this.tick);
    if (now - this.last < 1000 / this.o.fps) return;
    this.last = now;
    if (this.active(now)) {
      if (this.slot < 0) this.attach();
      if (this.slot >= 0) this.frame();
    } else if (this.slot >= 0) {
      this.detach(); // still between bursts / not hovered: no filter cost at all
    }
  };

  Glitch.prototype.update = function () {
    var still = this.o.reducedMotion !== "ignore" && reducedMotion();
    // Hover / idle modes keep the (cheap) loop running and only attach the filter when active
    var run = this.wanted && this.visible && !still;
    if (run && !this.running) {
      this.running = true;
      if (this.o.idle && !this.nextIdle) this.nextIdle = performance.now() + rnd(200, this.idleWindow()[1]);
      if (!this.raf) this.raf = requestAnimationFrame(this.tick);
    } else if (!run && this.running) {
      this.running = false;
      if (this.raf) cancelAnimationFrame(this.raf);
      this.raf = 0;
      this.detach();
    }
  };

  Glitch.prototype.start = function () { this.wanted = true; this.update(); return this; };
  Glitch.prototype.stop = function () { this.wanted = false; this.update(); return this; };
  Glitch.prototype.burst = function (ms) {
    this.burstUntil = performance.now() + (ms || 600);
    this.wanted = true;
    this.update();
    return this;
  };
  Glitch.prototype.set = function (opts) {
    Object.assign(this.o, opts || {});
    this.pool = [];
    this.update();
    return this;
  };
  Glitch.prototype.destroy = function () {
    this.stop();
    if (this.ro) this.ro.disconnect();
    if (this.io) this.io.disconnect();
    if (this.onEnter) {
      var el = this.el, self = this;
      ["mouseenter", "focusin"].forEach(function (t) { el.removeEventListener(t, self.onEnter); });
      ["mouseleave", "focusout"].forEach(function (t) { el.removeEventListener(t, self.onLeave); });
    }
    instances.delete(this.el);
  };

  function GlitchFX(el, opts) {
    if (typeof el === "string") el = document.querySelector(el);
    if (!el) return null;
    var g = instances.get(el);
    if (g) return g.set(opts);
    g = new Glitch(el, opts);
    instances.set(el, g);
    return g;
  }

  GlitchFX.all = function (sel, opts) {
    var list = typeof sel === "string" ? document.querySelectorAll(sel) : sel;
    return Array.prototype.map.call(list, function (el) { return GlitchFX(el, opts); });
  };

  GlitchFX.get = function (el) { return instances.get(el) || null; };

  // data-glitch="0.8" data-glitch-hover data-glitch-idle="1000,5000" data-glitch-fps="20"
  GlitchFX.scan = function (root) {
    (root || document).querySelectorAll("[data-glitch]").forEach(function (el) {
      if (instances.has(el)) return;
      var d = el.dataset, o = {};
      if (d.glitch !== "" && !isNaN(parseFloat(d.glitch))) o.intensity = clamp(parseFloat(d.glitch), 0, 1);
      if ("glitchHover" in d) o.hover = true;
      if ("glitchIdle" in d) {
        var p = (d.glitchIdle || "").split(",").map(Number).filter(function (x) { return x > 0; });
        o.idle = p.length === 2 ? p : true;
      }
      if (d.glitchFps) o.fps = clamp(parseFloat(d.glitchFps) || 20, 1, 60);
      if ("glitchNoCorrupt" in d) o.corrupt = false;
      GlitchFX(el, o);
    });
  };

  // Settings changes of the visitor (reduced motion) apply live
  if (global.matchMedia) {
    var mq = global.matchMedia("(prefers-reduced-motion: reduce)");
    var onMq = function () { instances.forEach(function (g) { g.update(); }); };
    if (mq.addEventListener) mq.addEventListener("change", onMq);
  }

  global.GlitchFX = GlitchFX;
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", function () { GlitchFX.scan(); });
  else GlitchFX.scan();
})(window);
