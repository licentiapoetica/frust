// f0ckv2 frontend: upload page (f0ckm's /api/v2/upload, multipart with the X-CSRF-Token header).
(() => {
  const form = document.getElementById('v2-upload-form');
  if(!form) return;
  const progress = document.getElementById('v2-upload-progress');
  const result = document.getElementById('v2-upload-result');
  const csrf = () => document.querySelector('meta[name="csrf-token"]')?.content || '';
  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  // tag suggestions for the last tag in the list
  const tagInput = form.tags;
  const datalist = document.getElementById('v2-upload-tags');
  let tt = null, last = '';
  tagInput.addEventListener('input', () => {
    clearTimeout(tt);
    tt = setTimeout(async () => {
      const parts = tagInput.value.split(',');
      const q = parts.pop().trim();
      if(q.length < 2 || q === last) return;
      last = q;
      try {
        const res = await (await fetch('/api/v2/tags/suggest?q=' + encodeURIComponent(q))).json();
        const prefix = parts.length ? parts.map(p => p.trim()).join(', ') + ', ' : '';
        datalist.innerHTML = '';
        (res.suggestions || []).forEach(s => {
          const o = document.createElement('option');
          o.value = prefix + s.tag;
          datalist.appendChild(o);
        });
      } catch {}
    }, 300);
  });

  form.addEventListener('submit', async e => {
    e.preventDefault();
    const file = form.file.files[0];
    const url = form.url ? form.url.value.trim() : '';
    if(!file && !url) return alert('choose a file or paste a link');
    if(window.f0ckCaptchaGate) {
      try { await window.f0ckCaptchaGate('upload'); } catch { return; }
    }
    const fd = new FormData();
    if(file) fd.append('file', file);
    else fd.append('file', url);
    fd.append('rating', form.querySelector('input[name="rating"]:checked')?.value || 'sfw');
    fd.append('tags', tagInput.value.split(',').map(t => t.trim()).filter(Boolean).join(','));
    const titleEl = form.elements.namedItem('title');
    if(titleEl && titleEl.value.trim()) fd.append('title', titleEl.value.trim());
    if(form.comment && form.comment.value.trim()) fd.append('comment', form.comment.value.trim());
    fd.append('is_oc', form.is_oc && form.is_oc.checked ? 'true' : 'false');
    if(form.visibility) fd.append('visibility', form.visibility.value);

    const btn = form.querySelector('button[type="submit"]');
    btn.disabled = true;
    result.textContent = '';
    const xhr = new XMLHttpRequest();
    xhr.open('POST', '/api/v2/upload');
    xhr.setRequestHeader('X-CSRF-Token', csrf());
    xhr.upload.addEventListener('progress', ev => {
      if(ev.lengthComputable) progress.textContent = Math.round(ev.loaded / ev.total * 100) + '%';
    });
    xhr.addEventListener('load', () => {
      btn.disabled = false;
      progress.textContent = '';
      let res = {};
      try { res = JSON.parse(xhr.responseText); } catch {}
      if(res.success) {
        const link = res.redirect || res.url;
        result.className = 'v2-upload-result ok';
        result.innerHTML = `${esc(res.msg || 'f0cked!')}${link ? ` <a href="${esc(link)}">${esc(res.slug || res.itemid || 'view')}</a>` : ''}`;
        form.reset();
        if(res.redirect) setTimeout(() => { location.href = res.redirect; }, 800);
      }
      else {
        result.className = 'v2-upload-result error';
        result.textContent = res.msg || res.message || `upload failed (HTTP ${xhr.status})`;
      }
    });
    xhr.addEventListener('error', () => {
      btn.disabled = false;
      result.className = 'v2-upload-result error';
      result.textContent = 'connection error';
    });
    xhr.send(fd);
  });
})();
