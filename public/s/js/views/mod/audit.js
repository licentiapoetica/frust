// Audit log (views/mod/audit.html). The page works without JavaScript (links, GET filter form,
// pager, plain admin forms); here:
// - the filter form applies as soon as a choice changes, through the site's page navigation;
// - older entries load while scrolling (the "Older entries" link, from the next page's HTML);
// - new entries appear at the top live (audit_entry events) while on the first page;
// - the admin forms (retention, delete old entries) save in place.
// The markup lives in the template only: new rows come from a fresh copy of the page.
(function () {
  'use strict';

  const page = document.getElementById('audit-page');
  if (!page || page.dataset.auditBound) return;
  page.dataset.auditBound = '1';

  const firstPage = (Number(page.dataset.page) || 1) === 1;

  function csrf() {
    return (window.f0ckSession && window.f0ckSession.csrf_token) || page.dataset.csrf || '';
  }

  async function fetchDoc(url) {
    const res = await fetch(url, { headers: { 'Accept': 'text/html' }, credentials: 'same-origin' });
    if (!res.ok) throw new Error('Loading failed (' + res.status + ')');
    return new DOMParser().parseFromString(await res.text(), 'text/html');
  }

  function navigate(url) {
    if (typeof window.loadPageAjax === 'function') window.loadPageAjax(url);
    else location.assign(url);
  }

  function emojify(root) {
    if (typeof window.f0ckEmojify === 'function') {
      try { window.f0ckEmojify(root); } catch (e) { /* plain text then */ }
    }
  }

  // ── Filters ────────────────────────────────────────────────────────────────
  const filters = document.getElementById('audit-filters');
  if (filters) {
    const go = () => {
      const params = new URLSearchParams();
      new FormData(filters).forEach((v, k) => {
        const s = String(v).trim();
        if (s) params.set(k, s);
      });
      const qs = params.toString();
      navigate('/mod/audit' + (qs ? '?' + qs : ''));
    };
    filters.addEventListener('submit', (e) => {
      e.preventDefault();
      go();
    });
    filters.addEventListener('change', (e) => {
      if (e.target.matches('select, input[type="date"]')) go();
    });
  }

  // ── Older entries ──────────────────────────────────────────────────────────
  const list = document.getElementById('audit-list');
  let loadingMore = false;
  let observer = null;

  function knownIds() {
    return new Set(Array.from(list.querySelectorAll(':scope > li[data-id]'), (li) => li.dataset.id));
  }

  async function loadMore(link) {
    if (loadingMore || !link) return;
    loadingMore = true;
    link.classList.add('is-loading');
    link.textContent = 'Loading…';
    try {
      const doc = await fetchDoc(link.getAttribute('href'));
      const seen = knownIds();
      const rows = Array.from(doc.querySelectorAll('#audit-list > li[data-id]')).filter((li) => !seen.has(li.dataset.id));
      rows.forEach((li) => {
        const row = document.importNode(li, true);
        list.appendChild(row);
        emojify(row);
      });
      const next = doc.querySelector('[data-audit-more]');
      const pager = doc.querySelector('.audit-pager');
      const curPager = document.querySelector('#audit-results .audit-pager');
      if (curPager && pager) curPager.replaceWith(document.importNode(pager, true));
      if (next) {
        link.setAttribute('href', next.getAttribute('href'));
        link.textContent = 'Older entries';
        link.classList.remove('is-loading');
      } else {
        if (observer) observer.disconnect();
        link.remove();
      }
    } catch (err) {
      link.textContent = 'Older entries (loading failed, click to retry)';
      link.classList.remove('is-loading');
    } finally {
      loadingMore = false;
    }
  }

  function bindMore() {
    const link = document.querySelector('#audit-results [data-audit-more]');
    if (observer) observer.disconnect();
    if (!link) return;
    if ('IntersectionObserver' in window) {
      observer = new IntersectionObserver((seen) => {
        if (seen.some((s) => s.isIntersecting)) loadMore(link);
      }, { rootMargin: '600px 0px' });
      observer.observe(link);
    }
  }

  document.getElementById('audit-results').addEventListener('click', (e) => {
    const link = e.target.closest('[data-audit-more]');
    if (!link) return;
    e.preventDefault();
    e.stopPropagation();
    loadMore(link);
  });
  bindMore();

  // ── Live: new entries at the top ───────────────────────────────────────────
  let liveTimer = null;
  let liveBusy = false;

  async function addNewEntries() {
    if (liveBusy || !document.body.contains(page)) return;
    liveBusy = true;
    try {
      const doc = await fetchDoc(location.pathname + location.search);
      const seen = knownIds();
      const top = Math.max(0, ...Array.from(seen, Number));
      const fresh = Array.from(doc.querySelectorAll('#audit-list > li[data-id]'))
        .filter((li) => !seen.has(li.dataset.id) && Number(li.dataset.id) > top);
      if (!fresh.length) return;
      const empty = list.querySelector(':scope > .audit-empty');
      if (empty) empty.remove();
      fresh.reverse().forEach((li) => {
        const row = document.importNode(li, true);
        row.classList.add('is-new');
        list.prepend(row);
        emojify(row);
      });
      // Counts and choices follow
      ['.audit-summary', '.audit-toolbar .audit-range'].forEach((sel) => {
        const a = doc.querySelector('#audit-results ' + sel);
        const b = document.querySelector('#audit-results ' + sel);
        if (a && b) b.replaceWith(document.importNode(a, true));
      });
      const info = doc.getElementById('audit-manage-info');
      const curInfo = document.getElementById('audit-manage-info');
      if (info && curInfo) curInfo.replaceWith(document.importNode(info, true));
    } catch (err) {
      /* the next event tries again */
    } finally {
      liveBusy = false;
    }
  }

  // One socket handler for every visit of the page (the page's navigation runs this file again)
  window.f0ckAuditLive = firstPage ? () => {
    clearTimeout(liveTimer);
    liveTimer = setTimeout(addNewEntries, 400);
  } : null;
  if (!window.f0ckAuditLiveBound && window.F0ckLive && typeof window.F0ckLive.onMessage === 'function') {
    window.f0ckAuditLiveBound = true;
    window.F0ckLive.onMessage((d) => {
      if (d && d.type === 'audit_entry' && typeof window.f0ckAuditLive === 'function') window.f0ckAuditLive(d.data);
    });
  }

  // ── Admin: retention, delete old entries ───────────────────────────────────
  const status = document.getElementById('audit-status');
  let statusTimer = null;

  function setStatus(text, state) {
    if (!status) return;
    clearTimeout(statusTimer);
    status.textContent = text;
    status.classList.toggle('is-busy', state === 'busy');
    status.classList.toggle('is-ok', state === 'ok');
    status.classList.toggle('is-err', state === 'err');
    if (state === 'ok') statusTimer = setTimeout(() => setStatus('', ''), 5000);
  }

  // Entries, counts and the info line from a fresh copy of the page
  async function refresh() {
    const doc = await fetchDoc(location.pathname + location.search);
    const fresh = doc.getElementById('audit-results');
    const cur = document.getElementById('audit-results');
    if (fresh && cur) {
      cur.replaceChildren(...Array.from(fresh.childNodes, (n) => document.importNode(n, true)));
      emojify(cur);
    }
    const info = doc.getElementById('audit-manage-info');
    const curInfo = document.getElementById('audit-manage-info');
    if (info && curInfo) curInfo.replaceWith(document.importNode(info, true));
    bindMore();
  }

  page.addEventListener('submit', async (e) => {
    const form = e.target.closest('form[data-audit-admin]');
    if (!form) return;
    e.preventDefault();
    if (form.classList.contains('is-busy')) return;
    const body = new URLSearchParams(new FormData(form));
    body.delete('back');
    body.set('csrf_token', csrf());
    form.classList.add('is-busy');
    setStatus('Saving…', 'busy');
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
      if (!data.success) throw new Error(data.msg || 'Saving failed');
      const confirm = form.querySelector('input[name="confirm"]');
      if (confirm) confirm.checked = false;
      await refresh();
      setStatus(data.msg || 'Saved', 'ok');
    } catch (err) {
      setStatus(err.message || 'Saving failed', 'err');
    } finally {
      form.classList.remove('is-busy');
    }
  });
})();
