// Scraper guard admin page (views/admin/scrapers.html): sends the settings and lift forms with fetch
// and refreshes the page's live parts ([data-sg-live]) in place, after an action and every 15 s.
(function () {
  'use strict';
  var root = document.getElementById('adm-sg');
  if (!root) return;
  var busy = false;

  function flash(msg, type) {
    if (typeof window.flashMessage === 'function') window.flashMessage(msg, 3000, type || 'info');
  }

  function csrf(form) {
    return (typeof window.getCsrfToken === 'function' && window.getCsrfToken()) ||
      (form.querySelector('input[name="csrf_token"]') || {}).value || '';
  }

  // Swaps each [data-sg-live] region for the fresh page's
  function refresh() {
    if (busy) return Promise.resolve();
    busy = true;
    return fetch(location.pathname + location.search, { credentials: 'same-origin', cache: 'no-store' })
      .then(function (r) { if (!r.ok) throw new Error(r.status); return r.text(); })
      .then(function (html) {
        var doc = new DOMParser().parseFromString(html, 'text/html');
        root.querySelectorAll('[data-sg-live]').forEach(function (el) {
          var fresh = doc.querySelector('[data-sg-live="' + el.getAttribute('data-sg-live') + '"]');
          if (fresh) el.replaceWith(document.importNode(fresh, true));
        });
      })
      .catch(function () { /* server restarting: next time */ })
      .then(function () { busy = false; });
  }

  root.addEventListener('submit', function (e) {
    var form = e.target.closest('form.adm-sg-form');
    if (!form) return;
    e.preventDefault();
    var button = form.querySelector('button[type="submit"]');
    if (button) button.disabled = true;
    fetch(form.getAttribute('action'), {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        'Accept': 'application/json',
        'X-Requested-With': 'XMLHttpRequest',
        'X-CSRF-Token': csrf(form),
        'Content-Type': 'application/x-www-form-urlencoded'
      },
      body: new URLSearchParams(new FormData(form))
    })
      .then(function (r) { return r.json().catch(function () { return { success: false, msg: 'Error ' + r.status }; }); })
      .then(function (data) {
        flash(data.msg || (data.success ? 'Done' : 'Failed'), data.success ? 'success' : 'error');
        if (data.success) return refresh();
      })
      .catch(function () { flash('Network error', 'error'); })
      .then(function () { if (button && button.isConnected) button.disabled = false; });
  });

  setInterval(function () { if (!document.hidden) refresh(); }, 15000);
})();
