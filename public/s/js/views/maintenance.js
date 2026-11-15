// Maintenance page (views/maintenance.html). Loaded in <head>: drops the no-js class (the CSS-only
// glitch stand-in is for visitors without JavaScript, glitchfx.js takes over here), turns the
// video's sound on, and asks the server every 30 s whether maintenance is over, then loads the
// page the visitor wanted.
(function () {
  'use strict';
  document.documentElement.classList.remove('no-js');

  var INTERVAL = 30000;

  function check() {
    if (document.hidden) return;
    fetch(location.href, { headers: { 'X-Requested-With': 'XMLHttpRequest' }, credentials: 'same-origin', cache: 'no-store' })
      .then(function (r) { if (r.status !== 503) location.reload(); })
      .catch(function () { /* server restarting: ask again next time */ });
  }

  // The video plays with sound. Browsers block that until the visitor has clicked or pressed a key
  // somewhere on the site, so: try with sound, fall back to muted with a hint, and turn the sound on
  // at the first click / key / touch.
  function sound(video) {
    var hint = video.parentNode.querySelector('.mt-sound');
    if (!video.dataset.controls) video.removeAttribute('controls');
    var events = ['pointerdown', 'keydown', 'touchend'];

    function unmuted() {
      if (hint) hint.hidden = true;
      events.forEach(function (ev) { document.removeEventListener(ev, onGesture, true); });
    }
    function tryUnmute(then) {
      video.muted = false;
      var p = video.play();
      if (p && p.then) p.then(unmuted, then); else unmuted();
    }
    function muted() {
      video.muted = true;
      if (hint) hint.hidden = false;
      var p = video.play();
      if (p && p.catch) p.catch(function () {});
    }
    function onGesture() { tryUnmute(muted); }

    events.forEach(function (ev) { document.addEventListener(ev, onGesture, true); });
    tryUnmute(muted);
  }

  document.addEventListener('DOMContentLoaded', function () {
    var video = document.querySelector('video.mt-video');
    if (video) sound(video);
    if (document.body.dataset.preview) return;
    setInterval(check, INTERVAL);
    document.addEventListener('visibilitychange', check);
  });
})();
