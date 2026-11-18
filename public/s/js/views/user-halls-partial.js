// A user's halls (views/user-halls-partial.html), for whoever may manage them. The page is plain forms
// and :target dialogs that work without JavaScript; here the dialogs open in place (class is-open),
// the forms go through fetch, and [data-uh-page] is swapped from a fresh render of the page, so the
// markup stays in the template. Delegated listeners, bound once: the SPA runs this file again on
// every visit of the page.
(function () {
  // A dialog the server sent us back to (the no-JS #anchor), on every visit of the page
  function openFromHash() {
    var id = decodeURIComponent((location.hash || '').slice(1));
    if (/^uh-(edit|del)-/.test(id) && window.__f0ckUserHalls) window.__f0ckUserHalls.open(id);
  }
  if (window.__f0ckUserHalls) { openFromHash(); return; }

  var i18n = window.f0ckI18n || {};
  function csrf() { return (window.f0ckSession && window.f0ckSession.csrf_token) || ''; }
  function page() { return document.querySelector('[data-uh-page]'); }
  function flash(msg, type) {
    if (window.flashMessage) window.flashMessage(msg, 3000, type || 'success');
    else if (window.showFlash) window.showFlash(msg, type || 'success');
  }

  function openDialog(id) {
    var root = page();
    if (!root) return;
    root.querySelectorAll('.uh-dialog.is-open').forEach(function (d) { d.classList.remove('is-open'); });
    var d = id && document.getElementById(id);
    if (!d || !root.contains(d)) return;
    d.classList.add('is-open');
    document.body.classList.add('modal-open');
    var first = d.querySelector('input[type="text"], button[type="submit"]');
    if (first) first.focus({ preventScroll: true });
  }
  function closeDialogs() {
    var root = page();
    if (root) root.querySelectorAll('.uh-dialog.is-open').forEach(function (d) { d.classList.remove('is-open'); });
    document.body.classList.remove('modal-open');
  }

  // Fresh render of the page (the SPA partial), swapped in; `reopen` is a dialog to show again
  async function refresh(reopen, bust) {
    var r = await fetch(location.pathname, { headers: { 'X-Requested-With': 'XMLHttpRequest' }, credentials: 'same-origin' });
    if (!r.ok) throw new Error('HTTP ' + r.status);
    var doc = new DOMParser().parseFromString(await r.text(), 'text/html');
    var fresh = doc.querySelector('[data-uh-page]');
    var old = page();
    if (!fresh || !old) return;
    if (bust) {
      // The same picture URL after a change: make the browser load it again
      var v = Date.now();
      fresh.querySelectorAll('img[src^="/user_list_image/"]').forEach(function (img) {
        if (img.getAttribute('src').split('?')[0].endsWith('/' + encodeURIComponent(bust))) img.setAttribute('src', img.getAttribute('src').split('?')[0] + '?v=' + v);
      });
    }
    old.replaceWith(document.importNode(fresh, true));
    document.body.classList.remove('modal-open');
    if (reopen) openDialog(reopen);
  }

  document.addEventListener('click', function (e) {
    if (!page()) return;
    var open = e.target.closest('[data-uh-page] [data-uh-open]');
    if (open) {
      e.preventDefault();
      openDialog(open.getAttribute('data-uh-open'));
      return;
    }
    if (e.target.closest('[data-uh-page] [data-uh-close]')) {
      e.preventDefault();
      closeDialogs();
    }
  });

  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && page() && page().querySelector('.uh-dialog.is-open')) {
      e.stopPropagation();
      closeDialogs();
    }
  }, true);

  // Picking a picture uploads it at once
  document.addEventListener('change', function (e) {
    var input = e.target.closest('[data-uh-page] .uh-file');
    if (input && input.files && input.files.length) input.form.requestSubmit();
  });

  document.addEventListener('submit', async function (e) {
    var form = e.target.closest('[data-uh-page] form[data-uh-form]');
    if (!form) return;
    e.preventDefault();
    if (form.classList.contains('is-busy')) return;
    var kind = form.getAttribute('data-uh-form');
    var dialog = form.closest('.uh-dialog');
    var slug = dialog ? dialog.id.replace(/^uh-(edit|del)-/, '') : '';
    var body;
    var headers = { 'Accept': 'application/json', 'X-CSRF-Token': csrf() };
    if (kind === 'picture') {
      body = new FormData(form);
      body.delete('back');
    } else {
      body = new URLSearchParams(new FormData(form));
      body.delete('back');
      headers['Content-Type'] = 'application/x-www-form-urlencoded';
    }
    form.classList.add('is-busy');
    form.querySelectorAll('button').forEach(function (b) { b.disabled = true; });
    try {
      var r = await fetch(form.getAttribute('action'), { method: 'POST', headers: headers, body: body, credentials: 'same-origin' });
      var d = await r.json().catch(function () { return {}; });
      if (!r.ok || !d.success) throw new Error(d.msg || d.message || (i18n.hall_error_label || 'Error'));
      if (kind === 'create') {
        var created = d.hall && d.hall.slug;
        await refresh(null);
        flash(i18n.hall_created || 'Created');
        var card = created && document.getElementById('hall-' + created);
        if (card) {
          card.classList.add('uh-card-new');
          card.scrollIntoView({ behavior: 'smooth', block: 'center' });
        }
      } else if (kind === 'picture' || kind === 'picture-remove') {
        await refresh('uh-edit-' + slug, slug);
        flash(kind === 'picture' ? (i18n.hall_image_uploaded || 'Picture uploaded') : (i18n.hall_image_removed || 'Picture removed'));
      } else if (kind === 'delete') {
        await refresh(null);
        flash(i18n.hall_deleted || 'List deleted');
      } else {
        await refresh(null);
        flash(i18n.hall_saved || 'Saved');
      }
    } catch (err) {
      flash(err.message, 'error');
      form.classList.remove('is-busy');
      form.querySelectorAll('button').forEach(function (b) { b.disabled = false; });
      var file = form.querySelector('.uh-file');
      if (file) file.value = '';
    }
  });

  window.__f0ckUserHalls = { open: openDialog };
  openFromHash();
})();
