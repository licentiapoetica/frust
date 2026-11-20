// Admin panel → Permissions (views/admin/permissions.html). Every form also works without
// JavaScript; here they save in place: a changed select saves its form at once, the group forms
// (new group, remove) refresh the groups section from a fresh copy of the page.
(function () {
  'use strict';

  const page = document.getElementById('perm-page');
  if (!page || page.dataset.permBound) return;
  page.dataset.permBound = '1';

  const status = document.getElementById('perm-status');
  let statusTimer = null;

  function setStatus(text, state) {
    if (!status) return;
    clearTimeout(statusTimer);
    status.textContent = text;
    status.classList.toggle('is-busy', state === 'busy');
    status.classList.toggle('is-ok', state === 'ok');
    status.classList.toggle('is-err', state === 'err');
    if (state === 'ok') statusTimer = setTimeout(() => setStatus('', ''), 2500);
  }

  function csrf() {
    return (window.f0ckSession && window.f0ckSession.csrf_token) || page.dataset.csrf || '';
  }

  // The groups section from a fresh copy of the page, keeping the open group open
  async function refreshGroups(openName) {
    const res = await fetch(location.pathname, { headers: { 'Accept': 'text/html' }, credentials: 'same-origin' });
    if (!res.ok) throw new Error('Reload failed (' + res.status + ')');
    const doc = new DOMParser().parseFromString(await res.text(), 'text/html');
    const fresh = doc.getElementById('perm-groups');
    const cur = document.getElementById('perm-groups');
    if (!fresh || !cur) return;
    cur.replaceWith(fresh);
    if (openName) {
      fresh.querySelectorAll('details.adm-perm-group').forEach((d) => {
        const n = d.querySelector('input[name="name"]');
        if (n && n.value === openName) d.open = true;
      });
    }
  }

  async function send(form) {
    const body = new URLSearchParams(new FormData(form));
    body.delete('back');
    body.set('csrf_token', csrf());
    setStatus('Saving...', 'busy');
    try {
      const res = await fetch(form.getAttribute('action'), {
        method: 'POST',
        headers: {
          'X-Requested-With': 'XMLHttpRequest',
          'Accept': 'application/json',
          'Content-Type': 'application/x-www-form-urlencoded',
          'X-CSRF-Token': csrf()
        },
        body: body.toString(),
        credentials: 'same-origin'
      });
      const data = await res.json().catch(() => null);
      if (!data) throw new Error('Server returned ' + res.status);
      if (!data.success) throw new Error(data.msg || 'Save failed');
      if (form.hasAttribute('data-perm-reload')) {
        await refreshGroups(data.created ? data.group : null);
      } else if (data.role_values) {
        // "Default (yes)" labels can change with the config; mark the explicitly set ones
        form.querySelectorAll('select.adm-perm-select').forEach((sel) => {
          sel.classList.toggle('is-set', sel.value !== 'default');
        });
      } else if (data.perms) {
        form.querySelectorAll('select.adm-perm-select').forEach((sel) => {
          sel.classList.toggle('is-set', sel.value !== 'inherit');
        });
        const details = form.closest('details.adm-perm-group');
        const chips = details ? details.querySelectorAll('.adm-perm-chip') : [];
        const n = Object.keys(data.perms).length;
        if (chips[1]) {
          chips[1].textContent = n + ' rule' + (n === 1 ? '' : 's');
          chips[1].classList.toggle('is-on', n > 0);
        }
      }
      setStatus(data.msg || 'Saved', 'ok');
    } catch (err) {
      setStatus('Error: ' + err.message, 'err');
    }
  }

  page.addEventListener('submit', (e) => {
    const form = e.target.closest('form.adm-perm-form');
    if (!form) return;
    e.preventDefault();
    send(form);
  });

  page.addEventListener('change', (e) => {
    const sel = e.target.closest('select.adm-perm-select');
    if (!sel) return;
    const form = sel.closest('form[data-perm-autosave]');
    if (form) send(form);
  });
})();
