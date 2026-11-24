// Scraper trap (views/scraper-trap.html, server/src/scraper_guard.rs): a proof of work that never
// finishes. Hashes challenge + nonce with SHA-256 looking for `difficulty` leading zero hex digits
// (16^difficulty tries: never), and shows the counters a real check would. Nothing is ever sent
// back; the page's meta refresh brings the visitor back once the block runs out.
(function () {
  'use strict';
  document.documentElement.classList.remove('no-js');

  var root = document.getElementById('st-trap');
  if (!root) return;
  var challenge = root.getAttribute('data-challenge') || '';
  var difficulty = parseInt(root.getAttribute('data-difficulty'), 10) || 20;
  var progress = root.querySelector('.st-progress');
  var hashesEl = document.getElementById('st-hashes');
  var speedEl = document.getElementById('st-speed');
  var bestEl = document.getElementById('st-best');

  var subtle = window.crypto && window.crypto.subtle;
  var encoder = window.TextEncoder ? new TextEncoder() : null;
  var nonce = 0;
  var best = 0;
  var started = Date.now();
  var BATCH = 512;

  function leadingZeroNibbles(bytes) {
    var n = 0;
    for (var i = 0; i < bytes.length; i++) {
      if (bytes[i] === 0) { n += 2; continue; }
      if (bytes[i] < 16) n += 1;
      break;
    }
    return n;
  }

  function note(zeros) {
    if (zeros > best) best = zeros;
  }

  function human(n) {
    if (n >= 1e9) return (n / 1e9).toFixed(2) + 'G';
    if (n >= 1e6) return (n / 1e6).toFixed(2) + 'M';
    if (n >= 1e3) return (n / 1e3).toFixed(1) + 'k';
    return String(n);
  }

  function show() {
    var secs = Math.max((Date.now() - started) / 1000, 0.001);
    hashesEl.textContent = human(nonce);
    speedEl.textContent = human(Math.round(nonce / secs)) + 'H/s';
    bestEl.textContent = best + '/' + difficulty;
    progress.value = best;
  }

  // Secure pages: the browser's SHA-256
  function batchSubtle() {
    var jobs = [];
    for (var i = 0; i < BATCH; i++) jobs.push(subtle.digest('SHA-256', encoder.encode(challenge + (nonce++))));
    return Promise.all(jobs).then(function (digests) {
      digests.forEach(function (d) { note(leadingZeroNibbles(new Uint8Array(d))); });
    });
  }

  // Plain http (no crypto.subtle): a 32-bit FNV-1a stand-in, hashed in rounds so it costs about as much
  function batchPlain() {
    for (var i = 0; i < BATCH; i++) {
      var s = challenge + (nonce++);
      var h = 0x811c9dc5;
      for (var round = 0; round < 64; round++) {
        for (var j = 0; j < s.length; j++) {
          h ^= s.charCodeAt(j);
          h = Math.imul(h, 0x01000193) >>> 0;
        }
      }
      var bytes = [h >>> 24, (h >>> 16) & 255, (h >>> 8) & 255, h & 255];
      note(Math.min(leadingZeroNibbles(bytes), 7));
    }
    return Promise.resolve();
  }

  var batch = subtle && encoder ? batchSubtle : batchPlain;

  function loop() {
    batch().then(function () {
      show();
      setTimeout(loop, 0);
    }, function () {
      batch = batchPlain;
      setTimeout(loop, 0);
    });
  }
  loop();
})();
