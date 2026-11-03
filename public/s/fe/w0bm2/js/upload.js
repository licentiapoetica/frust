/*
 * w0bm2 upload page (newscript.js "upload" part without jQuery): drag & drop, preview,
 * progress bar with speed, POST /api/v2/upload. Without JavaScript the form posts directly.
 */
(() => {
  'use strict';
  const form = document.getElementById('w2-upload-form');
  if (!form) return;
  const W2 = window.W2 || {};
  const flash = W2.flash || ((t, m) => window.alert(m));
  const box = document.getElementById('dragndrop');
  const fileInput = document.getElementById('w2-upload-file');
  const text = document.getElementById('dragndrop-text');
  const defaultPreview = text.innerHTML;
  const maxBytes = Number(form.dataset.maxBytes) || 0;
  const minTags = Number(form.dataset.minTags) || 0;
  let currentFile = null;
  let xhr = null;
  let counter = 0;

  const human = (size) => {
    if (!size) return '0 B';
    const i = Math.floor(Math.log(size) / Math.log(1024));
    return (size / Math.pow(1024, i)).toFixed(2) + ' ' + ['B', 'kB', 'MB', 'GB', 'TB'][i];
  };
  const tagsInput = () => {
    const map = W2.tagInputs;
    return map ? map.get('tags:w2-upload-form') : null;
  };

  function restore() {
    currentFile = null;
    text.innerHTML = defaultPreview;
    fileInput.value = '';
    fileInput.hidden = false;
  }

  function preview(file) {
    text.innerHTML = '';
    fileInput.hidden = true;
    const url = URL.createObjectURL(file);
    let media;
    if (file.type.startsWith('video')) {
      media = document.createElement('video');
      media.autoplay = true; media.loop = true; media.controls = true; media.muted = true;
    } else if (file.type.startsWith('image')) {
      media = document.createElement('img');
      media.alt = '';
    } else if (file.type.startsWith('audio')) {
      media = document.createElement('audio');
      media.controls = true;
    }
    if (media) {
      media.src = url;
      media.className = 'w2-upload-preview';
      media.id = 'video_preview';
      text.appendChild(media);
    }
    const info = document.createElement('span');
    info.className = 'upload-info';
    info.textContent = `${file.name} — ${human(file.size)} — `;
    const clear = document.createElement('a');
    clear.id = 'dragndrop-clear';
    clear.className = 'fa fa-times';
    clear.href = '#';
    clear.setAttribute('aria-label', 'Remove file');
    clear.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (xhr) { xhr.abort(); xhr = null; }
      restore();
    });
    info.appendChild(clear);
    text.appendChild(info);
    const progress = document.createElement('div');
    progress.className = 'progress progress-striped w2-progress';
    progress.hidden = true;
    progress.innerHTML = '<progress class="progress-bar progress-bar-custom w2-progress-bar" max="100" value="0" aria-label="Upload progress"></progress>';
    text.appendChild(progress);
    const stats = document.createElement('span');
    stats.className = 'upload-info';
    stats.innerHTML = '<span id="upload-stats"></span>';
    text.appendChild(stats);
  }

  function pick(file) {
    if (!file) return;
    if (maxBytes && file.size > maxBytes) {
      flash('error', `File too big. Max ${human(maxBytes)}`);
      return;
    }
    if (xhr) { xhr.abort(); xhr = null; }
    currentFile = file;
    preview(file);
  }

  fileInput.addEventListener('change', () => { if (fileInput.files.length) pick(fileInput.files[0]); });

  document.addEventListener('dragenter', (e) => {
    e.preventDefault();
    counter++;
    if (e.dataTransfer && Array.from(e.dataTransfer.types || []).includes('Files')) box.classList.add('w2-drag');
  });
  document.addEventListener('dragleave', () => { counter--; if (counter <= 0) { counter = 0; box.classList.remove('w2-drag'); } });
  document.addEventListener('dragover', (e) => e.preventDefault());
  document.addEventListener('drop', (e) => {
    e.preventDefault();
    counter = 0;
    box.classList.remove('w2-drag');
    if (e.dataTransfer && e.dataTransfer.files.length) pick(e.dataTransfer.files[0]);
  });

  form.addEventListener('submit', (e) => {
    e.preventDefault();
    if (!currentFile) { flash('error', 'No file selected'); return; }
    if (xhr) { flash('info', 'Already uploading'); return; }
    const rating = form.querySelector('input[name="rating"]:checked');
    if (!rating) { flash('error', 'Please select a rating (SFW/NSFW)'); return; }
    const ti = tagsInput();
    if (ti) ti.commit();
    const tags = ti ? ti.items : (form.elements.tags.value || '').split(',').map((t) => t.trim()).filter(Boolean);
    if (tags.length < minTags) { flash('error', `At least ${minTags} tags are required`); return; }

    const fd = new FormData();
    fd.append('file', currentFile, currentFile.name);
    fd.append('rating', rating.value);
    fd.append('tags', tags.join(','));
    ['title', 'visibility', 'expiry', 'comment'].forEach((n) => {
      const el = form.elements[n];
      if (el && el.value.trim()) fd.append(n, el.value.trim());
    });

    const progress = text.querySelector('.w2-progress');
    const bar = text.querySelector('.w2-progress-bar');
    const stats = document.getElementById('upload-stats');
    if (progress) progress.hidden = false;
    const started = Date.now();

    xhr = new XMLHttpRequest();
    xhr.upload.addEventListener('progress', (ev) => {
      if (!ev.lengthComputable || !bar) return;
      const pct = Math.floor((ev.loaded / ev.total) * 100);
      bar.value = pct;
      bar.textContent = pct + '%';
      const secs = (Date.now() - started) / 1000;
      if (stats && secs > 0) stats.textContent = `Speed: ${human(Math.floor(ev.loaded / secs))}/s Uploaded: ${human(ev.loaded)}`;
    });
    xhr.addEventListener('load', () => {
      let res = {};
      try { res = JSON.parse(xhr.responseText); } catch (err) { res = { success: false, msg: 'Upload failed' }; }
      xhr = null;
      if (res.success) {
        const target = res.redirect || ('/' + (res.slug || res.itemid || res.id || ''));
        flash('success', 'Upload successful. Redirect in 3 seconds...');
        setTimeout(() => { location.href = target; }, 3000);
      } else {
        if (bar) bar.classList.add('w2-failed');
        if (res.itemid || res.repost) flash('error', `${res.msg || 'Already exists'}`);
        else flash('error', res.msg || 'Upload failed');
      }
    });
    xhr.addEventListener('error', () => { xhr = null; flash('error', 'Upload failed'); if (bar) bar.classList.add('w2-failed'); });
    xhr.addEventListener('abort', () => { xhr = null; flash('info', 'Upload aborted'); });
    xhr.open('POST', '/api/v2/upload');
    xhr.setRequestHeader('X-CSRF-Token', (W2.data && W2.data.csrf) || '');
    xhr.setRequestHeader('X-Requested-With', 'XMLHttpRequest');
    xhr.send(fd);
  });
})();
