// The admin page's no-JavaScript stand-ins (the Upload / Save buttons next to "Choose image",
// "Choose icon" and the glitch checkboxes) do nothing here: picking a file uploads it and the
// checkboxes save on change. CSS hides them too (html:not(.no-js) .adm-nojs-only); removed here
// as well, so no frontend's CSS can bring them back.
document.querySelectorAll('.adm .adm-nojs-only').forEach((el) => el.remove());

F0ckActions.define({
  saveAdminSettings: () => saveAdminSettings(),
  saveBrandText: (form) => saveBrandText(form),
  fillBrandWeights: () => fillBrandWeights('brand-font-select', 'brand-weight-select'),
  saveBrandGlitch: (el) => saveBrandGlitch(el.closest('form')),
  uploadBrandImage: (el) => uploadBrandImage(el.closest('form')),
  removeBrandImage: (form) => removeBrandImage(form),
  saveFyBrand: (form) => saveFyBrand(form),
  fillFyBrandWeights: () => fillBrandWeights('fy-brand-font-select', 'fy-brand-weight-select'),
  uploadFyBrandImage: (el) => uploadFyBrandImage(el.closest('form')),
  removeFyBrandImage: (form) => removeFyBrandImage(form),
  uploadFavicon: (el) => uploadFavicon(el.closest('form')),
  removeFavicon: (form) => removeFavicon(form),
  regenerateTagImages: () => regenerateTagImages(),
});

function setSettingsStatus(text, state) {
  const status = document.getElementById('settings-status');
  if (!status) return;
  status.textContent = text;
  status.classList.toggle('is-busy', state === 'busy');
  status.classList.toggle('is-ok', state === 'ok');
  status.classList.toggle('is-err', state === 'err');
}

// The settings form (a plain form without JavaScript): every change is saved in place
async function saveAdminSettings() {
  const form = document.getElementById('admin-settings-form');
  if (!form) return;
  setSettingsStatus('Saving...', 'busy');
  try {
    const csrfToken = (window.f0ckSession && window.f0ckSession.csrf_token) || document.getElementById('settings-status').dataset.csrf;
    if (!csrfToken || csrfToken.startsWith('{')) {
      setSettingsStatus('CSRF token missing. Please refresh the page.', 'err');
      return;
    }
    // FormData keeps the hidden "off" before a checked switch's "on"; the server reads the last one
    const body = new URLSearchParams(new FormData(form));
    body.delete('back');
    body.set('csrf_token', csrfToken);
    const res = await fetch('/admin/settings', {
      method: 'POST',
      headers: {
        'X-Requested-With': 'XMLHttpRequest',
        'Content-Type': 'application/x-www-form-urlencoded',
        'X-CSRF-Token': csrfToken
      },
      body: body.toString()
    });
    if (res.status === 403) throw new Error('CSRF validation failed or session expired. Please reload the page.');
    const data = await res.json().catch(() => null);
    if (!data) throw new Error('Server returned ' + res.status);
    if (!data.success) throw new Error(data.msg || 'Save failed');
    const size = data.upload_size;
    if (size) {
      const now = document.getElementById('upload_limit_now');
      const large = document.getElementById('upload_limit_large_now');
      if (now) now.textContent = size.limit;
      if (large) large.textContent = size.limit_large;
    }
    setSettingsStatus('Saved!', 'ok');
    setTimeout(() => setSettingsStatus('', ''), 2000);
  } catch (err) {
    console.error('Settings save error:', err);
    setSettingsStatus('Error: ' + err.message, 'err');
  }
}

async function regenerateTagImages() {
  const btn = document.getElementById('regen_tag_btn');
  const status = document.getElementById('settings-status');

  if (!confirm('Are you sure you want to trigger a full regeneration of all tag images? This will run in the background.')) return;

  btn.disabled = true;
  btn.textContent = 'Processing...';
  status.textContent = 'Triggering regeneration...';
  status.style.color = 'var(--accent)';

  try {
    const res = await fetch('/admin/tag_image/regenerate_all', {
      headers: { 'X-Requested-With': 'XMLHttpRequest' }
    });
    const data = await res.json();

    if (data.success) {
      status.textContent = data.message;
      status.style.color = '#28a745';
      btn.textContent = 'Started!';
      setTimeout(() => {
        btn.disabled = false;
        btn.textContent = 'Regenerate all';
        status.textContent = '';
      }, 5000);
    } else {
      throw new Error(data.msg || 'Action failed');
    }
  } catch (err) {
    console.error('Regeneration error:', err);
    status.textContent = 'Error: ' + err.message;
    status.style.color = '#d9534f';
    btn.disabled = false;
    btn.textContent = 'Regenerate all';
  }
}

// ── Branding: brand text, brand image, favicon ──────────────────────────────────────────
// The forms post to the same endpoints without JavaScript; here they save in place.

let brandStatusTimer = null;
function brandStatus(text, kind, id = 'brand-status') {
  const status = document.getElementById(id);
  if (!status) return;
  clearTimeout(brandStatusTimer);
  status.textContent = text;
  status.classList.remove('is-busy', 'is-ok', 'is-err');
  if (kind) status.classList.add('is-' + kind);
  if (kind === 'ok') brandStatusTimer = setTimeout(() => { status.textContent = ''; }, 3000);
}

// POST a branding form (its fields minus the no-JS `back`) and return the JSON reply
async function brandPost(form) {
  const fd = new FormData(form);
  fd.delete('back');
  const csrfToken = (window.f0ckSession && window.f0ckSession.csrf_token) || fd.get('csrf_token') || '';
  const res = await fetch(form.action, {
    method: 'POST',
    headers: { 'X-Requested-With': 'XMLHttpRequest', 'X-CSRF-Token': csrfToken },
    body: form.enctype === 'multipart/form-data' ? fd : new URLSearchParams(fd),
  });
  let data = null;
  try { data = await res.json(); } catch (e) { /* not JSON */ }
  if (!data || !data.success) throw new Error((data && data.msg) || 'Request failed');
  return data;
}

// The brand weight select lists the chosen font family's weights (data-weights on its option)
// after "Default weight"; kept when the new family has it, hidden with fewer than two weights.
function fillBrandWeights(fontId, weightId) {
  const fontSelect = document.getElementById(fontId);
  const weightSelect = document.getElementById(weightId);
  if (!fontSelect || !weightSelect) return;
  let weights = [];
  try {
    weights = JSON.parse((fontSelect.selectedOptions[0] && fontSelect.selectedOptions[0].dataset.weights) || '[]');
  } catch (e) { /* no weights */ }
  if (!Array.isArray(weights)) weights = [];
  const keep = weightSelect.value;
  const def = document.createElement('option');
  def.value = '';
  def.textContent = 'Default weight';
  weightSelect.replaceChildren(def, ...weights.map((w) => {
    const o = document.createElement('option');
    o.value = String(w.weight);
    o.textContent = `${w.weight} ${w.name || ''}`.trim();
    return o;
  }));
  weightSelect.value = weights.some((w) => String(w.weight) === keep) ? keep : '';
  weightSelect.hidden = weights.length < 2;
}

async function saveBrandText(form) {
  brandStatus('Saving…', 'busy');
  try {
    const data = await brandPost(form);
    const input = form.querySelector('input[name="text"]');
    if (input) input.value = data.text || '';
    if (window.updateBrandText) window.updateBrandText(data.text || '');
    if (window.updateBrandStyle) window.updateBrandStyle(data.case || '', data.font_css || '', data.size || null, data.weight || null);
    const weightSelect = document.getElementById('brand-weight-select');
    if (weightSelect) weightSelect.value = data.weight ? String(data.weight) : '';
    brandStatus('✓ Brand text saved.', 'ok');
  } catch (err) {
    brandStatus('Error: ' + err.message, 'err');
  }
}

async function saveBrandGlitch(form) {
  brandStatus('Saving…', 'busy');
  try {
    const data = await brandPost(form);
    form.querySelector('#brand-glitch-text').checked = !!data.glitch_text;
    form.querySelector('#brand-glitch-image').checked = !!data.glitch_image;
    brandStatus('✓ Glitch effect saved (applies on the next page load).', 'ok');
  } catch (err) {
    brandStatus('Error: ' + err.message, 'err');
  }
}

async function uploadBrandImage(form) {
  const input = form.querySelector('input[type="file"]');
  if (!input || !input.files[0]) return;
  brandStatus('Uploading…', 'busy');
  try {
    const data = await brandPost(form);
    const preview = document.getElementById('brand-preview');
    preview.src = data.url;
    preview.hidden = false;
    document.getElementById('brand-preview-empty').hidden = true;
    document.getElementById('brand-remove-btn').disabled = false;
    if (window.updateBrandImage) window.updateBrandImage(data.url);
    brandStatus('✓ Brand image updated!', 'ok');
  } catch (err) {
    brandStatus('Error: ' + err.message, 'err');
  } finally {
    input.value = '';
  }
}

async function removeBrandImage(form) {
  if (!confirm('Remove the custom brand image? The navbar will show the brand text (or the site name).')) return;
  brandStatus('Removing…', 'busy');
  try {
    await brandPost(form);
    const preview = document.getElementById('brand-preview');
    preview.removeAttribute('src');
    preview.hidden = true;
    document.getElementById('brand-preview-empty').hidden = false;
    document.getElementById('brand-remove-btn').disabled = true;
    if (window.updateBrandImage) window.updateBrandImage(null);
    brandStatus('✓ Brand image removed.', 'ok');
  } catch (err) {
    brandStatus('Error: ' + err.message, 'err');
  }
}

// ── f0ckYou brand: the f0ckyou frontend's own logo (open f0ckYou pages change on the live event
//    fy_brand, this one too when it is shown in f0ckYou) ──
const fyStatus = (text, kind) => brandStatus(text, kind, 'fy-brand-status');

async function saveFyBrand(form) {
  fyStatus('Saving…', 'busy');
  try {
    const data = await brandPost(form);
    form.querySelector('#fy-brand-text-input').value = data.text || '';
    form.querySelector('#fy-brand-mark').checked = !!data.mark;
    form.querySelector('#fy-brand-site').checked = !!data.site;
    form.querySelector('#fy-brand-weight-select').value = data.weight ? String(data.weight) : '';
    fyStatus('✓ f0ckYou brand saved.', 'ok');
  } catch (err) {
    fyStatus('Error: ' + err.message, 'err');
  }
}

async function uploadFyBrandImage(form) {
  const input = form.querySelector('input[type="file"]');
  if (!input || !input.files[0]) return;
  fyStatus('Uploading…', 'busy');
  try {
    const data = await brandPost(form);
    const preview = document.getElementById('fy-brand-preview');
    preview.src = data.url;
    preview.hidden = false;
    document.getElementById('fy-brand-preview-empty').hidden = true;
    document.getElementById('fy-brand-remove-btn').disabled = false;
    fyStatus('✓ f0ckYou logo image updated!', 'ok');
  } catch (err) {
    fyStatus('Error: ' + err.message, 'err');
  } finally {
    input.value = '';
  }
}

async function removeFyBrandImage(form) {
  if (!confirm('Remove the f0ckYou logo image? The logo shows its text (and the finger mark) again.')) return;
  fyStatus('Removing…', 'busy');
  try {
    await brandPost(form);
    const preview = document.getElementById('fy-brand-preview');
    preview.removeAttribute('src');
    preview.hidden = true;
    document.getElementById('fy-brand-preview-empty').hidden = false;
    document.getElementById('fy-brand-remove-btn').disabled = true;
    fyStatus('✓ f0ckYou logo image removed.', 'ok');
  } catch (err) {
    fyStatus('Error: ' + err.message, 'err');
  }
}

// The favicon of this tab (the other pages pick it up on their next load)
function setPageFavicon(url) {
  document.querySelectorAll('link[rel~="icon"]').forEach((l) => {
    l.removeAttribute('type');
    l.href = url;
  });
}

async function uploadFavicon(form) {
  const input = form.querySelector('input[type="file"]');
  if (!input || !input.files[0]) return;
  brandStatus('Uploading…', 'busy');
  try {
    const data = await brandPost(form);
    document.getElementById('favicon-preview').src = data.url;
    document.getElementById('favicon-remove-btn').disabled = false;
    setPageFavicon(data.url);
    brandStatus('✓ Favicon updated!', 'ok');
  } catch (err) {
    brandStatus('Error: ' + err.message, 'err');
  } finally {
    input.value = '';
  }
}

async function removeFavicon(form) {
  if (!confirm('Remove the custom favicon? The config favicon is used again.')) return;
  brandStatus('Removing…', 'busy');
  try {
    const data = await brandPost(form);
    document.getElementById('favicon-preview').src = data.url;
    document.getElementById('favicon-remove-btn').disabled = true;
    setPageFavicon(data.url);
    brandStatus('✓ Favicon removed.', 'ok');
  } catch (err) {
    brandStatus('Error: ' + err.message, 'err');
  }
}
