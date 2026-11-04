// The admin page's no-JavaScript stand-ins (the Upload / Save buttons next to "Choose image",
// "Choose icon" and the glitch checkboxes) do nothing here: picking a file uploads it and the
// checkboxes save on change. CSS hides them too (html:not(.no-js) .adm-nojs-only); removed here
// as well, so no frontend's CSS can bring them back.
document.querySelectorAll('.adm .adm-nojs-only').forEach((el) => el.remove());

F0ckActions.define({
  saveAdminSettings: () => saveAdminSettings(),
  saveBrandText: (form) => saveBrandText(form),
  saveBrandGlitch: (el) => saveBrandGlitch(el.closest('form')),
  uploadBrandImage: (el) => uploadBrandImage(el.closest('form')),
  removeBrandImage: (form) => removeBrandImage(form),
  uploadFavicon: (el) => uploadFavicon(el.closest('form')),
  removeFavicon: (form) => removeFavicon(form),
  regenerateTagImages: () => regenerateTagImages(),
});

async function saveAdminSettings() {
  const status = document.getElementById('settings-status');
  const approvalToggle = document.getElementById('manual_approval_toggle');
  const registrationToggle = document.getElementById('registration_open_toggle');
  const minTagsInput = document.getElementById('min_tags_input');
  const trustedUploadsInput = document.getElementById('trusted_uploads_input');

  status.textContent = 'Saving...';
  status.style.color = 'var(--accent)';

  try {
    const csrfToken = (window.f0ckSession && window.f0ckSession.csrf_token) || document.getElementById('settings-status').dataset.csrf;
    if (!csrfToken || csrfToken.startsWith('{')) {
      status.textContent = 'CSRF token missing. Please refresh the page.';
      status.style.color = '#d9534f';
      return;
    }
    const res = await fetch('/admin/settings', {
      method: 'POST',
      headers: {
        'X-Requested-With': 'XMLHttpRequest',
        'Content-Type': 'application/x-www-form-urlencoded',
        'X-CSRF-Token': csrfToken
      },
      body: new URLSearchParams({
        manual_approval: approvalToggle ? (approvalToggle.checked ? 'on' : 'off') : 'off',
        ...(registrationToggle ? { registration_open: registrationToggle.checked ? 'on' : 'off' } : {}),
        min_tags: minTagsInput ? minTagsInput.value : '0',
        trusted_uploads: trustedUploadsInput ? trustedUploadsInput.value : '0',
        csrf_token: csrfToken
      }).toString()
    });

    if (!res.ok) {
      if (res.status === 403) {
        throw new Error('CSRF validation failed or session expired. Please reload the page.');
      }
      throw new Error('Server returned ' + res.status);
    }

    const data = await res.json();
    if (data.success) {
      status.textContent = 'Saved!';
      status.style.color = '#28a745';
      setTimeout(() => { status.textContent = ''; }, 2000);
    } else {
      throw new Error(data.msg || 'Save failed');
    }
  } catch (err) {
    console.error('Settings save error:', err);
    status.textContent = 'Error: ' + err.message;
    status.style.color = '#d9534f';
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
function brandStatus(text, kind) {
  const status = document.getElementById('brand-status');
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

async function saveBrandText(form) {
  brandStatus('Saving…', 'busy');
  try {
    const data = await brandPost(form);
    const input = form.querySelector('input[name="text"]');
    if (input) input.value = data.text || '';
    if (window.updateBrandText) window.updateBrandText(data.text || '');
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
