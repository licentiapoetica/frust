// Maintenance page (views/maintenance.html). Loaded in <head>: drops the no-js class (the CSS-only
// glitch stand-in is for visitors without JavaScript, glitchfx.js takes over here), and asks the
// server every 30 s whether maintenance is over, then loads the page the visitor wanted.
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

  document.addEventListener('DOMContentLoaded', function () {
    if (document.body.dataset.preview) return;
    setInterval(check, INTERVAL);
    document.addEventListener('visibilitychange', check);
  });
})();
