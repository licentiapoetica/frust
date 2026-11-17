// Settings → Excluded tags (views/settings.html, the frontends that list "excluded_tags" in their
// settings-prefs.html): the plain forms are sent with fetch and the list is rebuilt from the reply
// (the endpoint answers with all excluded tags), its rows from the page's <template>. Without
// JavaScript the forms post and come back (nojs.rs).
(() => {
  const box = document.getElementById('settings-excluded');
  if (!box) return;
  const list = box.querySelector('.settings-excluded-list');
  const empty = box.querySelector('.settings-excluded-empty');
  const rowTpl = document.getElementById('settings-excluded-row');
  const t = (key, fallback) => (window.f0ckI18n && window.f0ckI18n[key]) || fallback;
  const say = (text, kind) => {
    if (typeof window.flashMessage === 'function') window.flashMessage(text, 2500, kind);
  };

  const render = (tags) => {
    const rows = (tags || []).slice().sort((a, b) => String(a.tag).localeCompare(String(b.tag)));
    list.replaceChildren(...rows.map((tag) => {
      const li = rowTpl.content.firstElementChild.cloneNode(true);
      li.dataset.tag = tag.tag;
      const a = li.querySelector('a');
      a.href = `/tag/${encodeURIComponent(tag.normalized || tag.tag)}`;
      a.textContent = `#${tag.tag}`;
      li.querySelector('input[name="tag_id"]').value = String(tag.id);
      const btn = li.querySelector('button');
      btn.title = `Remove #${tag.tag}`;
      btn.setAttribute('aria-label', btn.title);
      return li;
    }));
    empty.hidden = rows.length > 0;
    // the exclude buttons elsewhere on the page and the session's list (f0ck-core.js)
    window.f0ckSyncExcluded?.(tags || []);
  };

  box.addEventListener('submit', async (e) => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement)) return;
    const adding = form.classList.contains('settings-excluded-add');
    if (!adding && !form.classList.contains('settings-excluded-remove')) return;
    e.preventDefault();
    if (form.dataset.busy) return;
    const body = {};
    for (const [k, v] of new FormData(form)) if (k !== 'back') body[k] = v;
    if (adding && !String(body.tagname || '').trim()) return;
    const name = adding ? String(body.tagname).trim() : (form.closest('[data-tag]')?.dataset.tag || '');
    form.dataset.busy = '1';
    form.querySelectorAll('button').forEach((b) => { b.disabled = true; });
    try {
      const res = await fetch(form.getAttribute('action'), {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json', Accept: 'application/json', 'X-CSRF-Token': body.csrf_token || '' },
        body: JSON.stringify(body),
      });
      const data = await res.json().catch(() => ({}));
      if (!data.success) {
        say(data.msg || data.message || 'Failed to update excluded tags', 'error');
        return;
      }
      render(data.tags);
      if (adding) form.elements.tagname.value = '';
      const tag = (data.tag && data.tag.tag) || name;
      say((adding ? t('tag_excluded_msg', 'Tag "{tag}" added to excluded tags') : t('tag_unexcluded_msg', 'Tag "{tag}" removed from excluded tags')).replace('{tag}', tag), 'info');
    } catch (err) {
      console.error(err);
      say('Failed to update excluded tags', 'error');
    } finally {
      delete form.dataset.busy;
      if (form.isConnected) form.querySelectorAll('button').forEach((b) => { b.disabled = false; });
      if (adding) form.elements.tagname.focus();
    }
  });
})();
