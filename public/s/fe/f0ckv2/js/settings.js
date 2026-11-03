// f0ckv2 frontend: settings page (f0ckm's /api/v2/settings/* endpoints).
(() => {
  const root = document.getElementById('v2-settings');
  if(!root) return;
  const msg = document.getElementById('v2-settings-msg');
  const csrf = () => document.querySelector('meta[name="csrf-token"]')?.content || '';
  let msgTimer;
  const say = (text, ok = true) => {
    if(!msg) return;
    msg.textContent = text;
    msg.classList.toggle('error', !ok);
    msg.classList.add('show');
    clearTimeout(msgTimer);
    msgTimer = setTimeout(() => msg.classList.remove('show'), 3000);
  };
  const api = async (method, url, body) => {
    try {
      const res = await fetch(url, {
        method,
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf() },
        body: body ? JSON.stringify({ ...body, csrf_token: csrf() }) : undefined
      });
      const data = await res.json().catch(() => ({}));
      if(!res.ok && data.success === undefined) data.success = false;
      return data;
    } catch(err) {
      return { success: false, msg: 'network error' };
    }
  };
  const report = (res, okText = 'saved') => say(res.success ? (res.msg || okText) : (res.msg || res.message || 'error'), !!res.success);

  // avatar = id of a f0ck (f0ckv2's original setting)
  const avatarInput = root.querySelector('input[name="i_avatar"]');
  const saveAvatar = async e => {
    e.preventDefault();
    const avatar = +avatarInput.value;
    const res = await api('PUT', '/api/v2/settings/setAvatar', { avatar });
    report(res);
    if(res.success) {
      const img = document.querySelector('#img_avatar');
      if(img) img.src = `/t/${avatar}.webp`;
      const nav = document.querySelector('img.avatar');
      if(nav) nav.src = `/t/${avatar}.webp`;
    }
  };
  root.querySelector('input#s_avatar')?.addEventListener('click', saveAvatar);
  avatarInput?.addEventListener('keyup', e => { if(e.key === 'Enter') saveAvatar(e); });

  // text fields: display_name, description, email, min_xd_score
  root.querySelectorAll('[data-save]').forEach(btn => btn.addEventListener('click', async () => {
    const key = btn.dataset.save;
    const field = root.querySelector(`[name="${key}"]`);
    const value = field.type === 'number' ? +field.value : field.value;
    report(await api('PUT', `/api/v2/settings/${key}`, { [key]: value }));
  }));

  // checkboxes
  root.querySelectorAll('input[data-pref]').forEach(cb => cb.addEventListener('change', async () => {
    const res = await api('PUT', `/api/v2/settings/${cb.dataset.pref}`, { [cb.dataset.key]: cb.checked });
    report(res);
    if(!res.success) cb.checked = !cb.checked;
  }));

  // language
  root.querySelectorAll('select[data-select]').forEach(sel => sel.addEventListener('change', async () => {
    const res = await api('PUT', `/api/v2/settings/${sel.dataset.select}`, { [sel.dataset.select]: sel.value });
    report(res);
    if(res.success) location.reload();
  }));

  // excluded tags
  const exTable = document.getElementById('v2-excluded');
  const renderExcluded = tags => {
    exTable.innerHTML = '';
    if(!tags.length) {
      exTable.innerHTML = '<tr class="v2-none"><td colspan="2">none</td></tr>';
      return;
    }
    tags.forEach(t => {
      const norm = t.normalized || t.tag;
      const tr = document.createElement('tr');
      tr.dataset.tag = norm;
      tr.innerHTML = `<td><a></a></td><td><a href="#">remove</a></td>`;
      tr.firstChild.firstChild.href = `/tag/${encodeURIComponent(norm)}`;
      tr.firstChild.firstChild.textContent = t.tag;
      tr.lastChild.firstChild.dataset.unexclude = norm;
      exTable.appendChild(tr);
    });
  };
  exTable?.addEventListener('click', async e => {
    const a = e.target.closest('[data-unexclude]');
    if(!a) return;
    e.preventDefault();
    const res = await api('DELETE', `/api/v2/settings/excluded_tags/${encodeURIComponent(a.dataset.unexclude)}`);
    report(res, 'removed');
    if(res.success) {
      if(Array.isArray(res.tags)) renderExcluded(res.tags);
      else a.closest('tr').remove();
    }
  });
  const exForm = document.getElementById('v2-exclude-form');
  exForm?.addEventListener('submit', async e => {
    e.preventDefault();
    const res = await api('POST', '/api/v2/settings/excluded_tags', { tagname: exForm.tagname.value.trim() });
    report(res, 'excluded');
    if(res.success) {
      exForm.tagname.value = '';
      if(Array.isArray(res.tags)) renderExcluded(res.tags);
    }
  });

  // password
  const pwForm = document.getElementById('v2-password-form');
  pwForm?.addEventListener('submit', async e => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(pwForm).entries());
    const res = await api('PUT', '/api/v2/settings/password', body);
    report(res);
    if(res.success) pwForm.reset();
  });
})();
