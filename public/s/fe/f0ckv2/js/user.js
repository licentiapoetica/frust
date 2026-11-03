// f0ckv2 frontend, logged-in visitors: favs, tags, subscriptions, delete, notifications.
// Port of f0ckv2's user.js/admin.js to the f0ckm API (/api/v2/togglefav, /api/v2/tags/:id, …).
(async () => {
  const data = JSON.parse(document.getElementById('f0ckv2-data')?.textContent || '{}');
  const csrf = () => document.querySelector('meta[name="csrf-token"]')?.content || '';

  const queryapi = async (url, body, method = 'GET') => {
    const opts = { method, credentials: 'same-origin', headers: { 'X-CSRF-Token': csrf() } };
    if(method === 'GET' && body) {
      url += '?' + new URLSearchParams(body).toString();
    }
    else if(body) {
      opts.headers['Content-Type'] = 'application/json';
      opts.body = JSON.stringify({ ...body, csrf_token: csrf() });
    }
    const res = await fetch(url, opts);
    try { return await res.json(); } catch { return { success: false, msg: `HTTP ${res.status}` }; }
  };
  const get  = (url, body) => queryapi(url, body, 'GET');
  const post = (url, body) => queryapi(url, body || {}, 'POST');

  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  // <notifications> unread count on the navbar, newest few in its dropdown
  const notifLink = document.getElementById('nav-notifs');
  if(notifLink) {
    const refresh = async () => {
      const res = await get('/api/notifications').catch(() => null);
      if(!res || !res.success) return;
      const list = res.notifications || [];
      notifLink.classList.toggle('ddcontent', list.length > 0);
      notifLink.classList.toggle('v2-has-notifs', list.length > 0);
      if(list.length) notifLink.setAttribute('content', list.length > 99 ? '99+' : list.length);
      else notifLink.removeAttribute('content');
      const menu = notifLink.parentElement.querySelector('.dropdown-menu');
      menu.querySelectorAll('.v2-notif').forEach(li => li.remove());
      list.slice(0, 8).reverse().forEach(n => {
        const li = document.createElement('li');
        li.className = 'v2-notif';
        const ref = n.item_slug || n.item_id;
        const who = n.from_display_name || n.from_user || 'system';
        const what = ({
          comment: 'commented', comment_reply: 'replied', mention: 'mentioned you', subscription: 'new comment',
          upload_comment: 'commented on your upload', approve: 'upload approved', deny: 'upload denied',
          item_deleted: 'upload deleted', upload_success: 'upload done', upload_error: 'upload failed',
          report: 'new report', admin_pending: 'pending upload', warning: 'warning'
        })[n.type] || n.type;
        const href = n.item_id ? `/${ref}${['comment', 'comment_reply', 'mention', 'subscription', 'upload_comment'].includes(n.type) && n.reference_id ? '#c' + n.reference_id : ''}` : '/notifications';
        li.innerHTML = `<a href="${esc(href)}" data-notif="${+n.id}">${esc(who)}: ${esc(what)}${n.item_id ? ' #' + esc(ref) : ''}</a>`;
        menu.insertAdjacentElement('afterbegin', li);
      });
      if(list.length && !menu.querySelector('.v2-notif-readall')) {
        const li = document.createElement('li');
        li.className = 'v2-notif-readall';
        li.innerHTML = '<a href="#">mark all read</a>';
        li.firstChild.addEventListener('click', async e => {
          e.preventDefault();
          await post('/api/notifications/read', {});
          li.remove();
          refresh();
        });
        menu.appendChild(li);
      }
    };
    notifLink.parentElement.addEventListener('click', e => {
      const a = e.target.closest('[data-notif]');
      if(a) post(`/api/notifications/${a.dataset.notif}/read`, {});
    });
    refresh();
    setInterval(() => { if(!document.hidden) refresh(); }, 60000);
  }
  // </notifications>

  // notifications page: mark all read
  document.getElementById('v2-readall')?.addEventListener('click', async e => {
    e.preventDefault();
    const tab = new URLSearchParams(location.search).get('tab') || '';
    await post('/api/notifications/read?tab=' + encodeURIComponent(tab), {});
    location.reload();
  });

  // <item>
  const container = document.querySelector('.container[data-item-id]');
  if(!container) return;
  const postid = +container.dataset.itemId;
  let tags = [...document.querySelectorAll('#tags > .badge[data-tag]')].map(t => t.dataset.tag);

  const deleteTagEvent = async e => {
    e.preventDefault();
    if(!confirm('Do you really want to delete this tag?'))
      return;
    const tagname = e.target.closest('.badge').dataset.tag;
    const res = await queryapi(`/api/v2/tags/${postid}/${encodeURIComponent(tagname)}`, {}, 'DELETE');
    if(!res.success)
      return alert(res.msg || 'uff');
    tags = res.tags.map(t => t.tag);
    renderTags(res.tags);
  };

  const renderTags = _tags => {
    [...document.querySelectorAll('#tags > .badge[data-tag]')].forEach(tag => tag.remove());
    const anchor = document.querySelector('#tags > form, #tags > .js-only');
    _tags.forEach(tag => {
      const span = document.createElement('span');
      span.classList.add('badge', 'mr-2', ...String(tag.badge || 'badge-light').split(' ').filter(Boolean));
      span.dataset.tag = tag.tag;
      if(tag.user) span.setAttribute('tooltip', tag.user);
      const a = document.createElement('a');
      a.href = `/tag/${encodeURIComponent(tag.normalized ?? tag.tag)}`;
      a.textContent = tag.tag;
      span.appendChild(a);
      if(data.mod) {
        span.insertAdjacentHTML('beforeend', '&nbsp;');
        const del = document.createElement('a');
        del.className = 'removetag';
        del.href = '#';
        del.innerHTML = '&#215;';
        del.addEventListener('click', deleteTagEvent);
        span.appendChild(del);
      }
      document.querySelector('#tags').insertBefore(span, anchor);
    });
  };
  document.querySelectorAll('#tags a.removetag').forEach(a => a.addEventListener('click', deleteTagEvent));

  const addtagClick = (ae = false) => {
    if(ae)
      ae.preventDefault();
    const insert = document.querySelector('a#a_addtag');
    if(!insert) return;
    const span = document.createElement('span');
    span.classList.add('badge', 'badge-light', 'mr-2');
    const input = document.createElement('input');
    input.size = 10;
    input.value = '';
    input.setAttribute('list', 'testlist');
    input.setAttribute('autocomplete', 'off');
    span.appendChild(input);
    insert.parentElement.insertAdjacentElement('beforebegin', span);
    input.focus();

    let tt = null;
    let lastInput = '';
    const testList = document.querySelector('#testlist');

    input.addEventListener('keyup', async e => {
      if(e.key === 'Enter') {
        const tmptag = input.value?.trim();
        if(!tmptag) return;
        if(tags.includes(tmptag))
          return alert('tag already exists');
        const res = await post(`/api/v2/tags/${postid}`, { tagname: tmptag });
        if(!res.success) {
          alert(res.msg || 'could not add tag');
          return false;
        }
        span.remove();
        tags = res.tags.map(t => t.tag);
        renderTags(res.tags);
        addtagClick();
        testList.innerText = '';
      }
      else if(e.key === 'Escape') {
        span.remove();
        testList.innerText = '';
      }
      else {
        if(tt != null)
          clearTimeout(tt);
        tt = setTimeout(async () => {
          tt = null;
          const tmptag = input.value?.trim();
          if(tmptag == lastInput || tmptag.length <= 1)
            return false;
          testList.innerText = '';
          lastInput = tmptag;
          const res = await get('/api/v2/tags/suggest', { q: tmptag });
          for(const entry of (res.suggestions || [])) {
            const option = document.createElement('option');
            option.value = entry.tag;
            if(!/fox/.test(navigator.userAgent) && entry.tagged != null)
              option.innerText = `tagged ${entry.tagged} times${entry.score != null ? ` (score: ${(+entry.score).toFixed(2)})` : ''}`;
            testList.appendChild(option);
          }
        }, 500);
      }
      return true;
    });

    input.addEventListener('focusout', () => {
      if(input.value.length === 0)
        span.remove();
    });
  };

  const toggleEvent = async (e = false) => {
    if(e)
      e.preventDefault();
    if(!data.mod) return;
    const res = await queryapi(`/api/v2/tags/${postid}/toggle`, {}, 'PUT');
    if(res.tags)
      renderTags(res.tags);
  };

  // fav (the heart is a real form; without JavaScript it posts to the API directly)
  const favform = document.querySelector('form#f_favo');
  const toggleFavEvent = async e => {
    if(e) e.preventDefault();
    if(!favform) return;
    const res = await post('/api/v2/togglefav', { postid, action: favform.action.value });
    if(!res.success)
      return alert(res.msg || 'could not fav');
    favform.action.value = res.favorited ? 'remove' : 'add';
    document.querySelector('svg#a_favo > use').setAttribute('href', '/s/fe/f0ckv2/img/iconset.svg#heart_' + (res.favorited ? 'solid' : 'regular'));
    const favcontainer = document.querySelector('span#favs');
    favcontainer.innerHTML = '';
    const visible = (res.favs || []).filter(f => !f.hide_fav_badge);
    favcontainer.hidden = visible.length === 0;
    visible.forEach(f => {
      const a = document.createElement('a');
      a.href = `/user/${encodeURIComponent(String(f.login || f.user || 'anonymous').toLowerCase())}/favs`;
      a.setAttribute('tooltip', f.display_name || f.user || 'anonymous');
      a.setAttribute('flow', 'up');
      const img = document.createElement('img');
      img.className = 'v2-favava';
      img.alt = '';
      img.src = f.avatar_file ? `/a/${encodeURIComponent(f.avatar_file)}` : (f.avatar ? `/t/${+f.avatar}.webp` : '/s/fe/f0ckv2/img/default.png');
      a.appendChild(img);
      favcontainer.appendChild(a);
      favcontainer.insertAdjacentHTML('beforeend', '&nbsp;');
    });
  };
  if(favform) favform.addEventListener('submit', toggleFavEvent);

  // comment subscription
  const subform = document.querySelector('form#f_subscribe');
  if(subform) subform.addEventListener('submit', async e => {
    e.preventDefault();
    const res = await post(subform.getAttribute('action'), {});
    if(!res.success) return;
    const btn = subform.querySelector('button');
    btn.classList.toggle('active', res.subscribed);
    btn.innerHTML = res.subscribed ? '&#128276;' : '&#128277;';
    btn.title = res.subscribed ? 'unsubscribe from comments' : 'subscribe to comments';
  });

  // delete (mods/admins)
  const deleteButtonEvent = async e => {
    if(e) e.preventDefault();
    if(!data.mod || !document.querySelector('#a_delete')) return;
    const reason = prompt(`Delete f0ck #${postid}? Reason:`, '');
    if(reason === null) return;
    const res = await post('/api/v2/admin/deletepost', { postid, reason });
    if(!res.success)
      return alert(res.msg || 'could not delete');
    const next = document.querySelector('#prev:not([href="#"])') || document.querySelector('#next:not([href="#"])');
    location.href = next ? next.href : '/';
  };
  document.querySelector('#a_delete')?.addEventListener('click', deleteButtonEvent);

  document.querySelector('a#a_addtag')?.addEventListener('click', addtagClick);
  document.querySelector('a#a_toggle')?.addEventListener('click', toggleEvent);

  document.addEventListener('keyup', e => {
    if(['INPUT', 'TEXTAREA', 'SELECT'].includes(e.target.tagName) || e.ctrlKey || e.metaKey || e.altKey)
      return;
    if(e.key === 'p')
      toggleEvent();
    else if(e.key === 'i')
      addtagClick();
    else if(e.key === 'x')
      deleteButtonEvent();
    else if(e.key === 'f')
      toggleFavEvent();
  });
  // </item>
})();
