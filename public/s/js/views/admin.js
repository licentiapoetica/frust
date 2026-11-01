F0ckActions.define({
  saveAdminSettings: () => saveAdminSettings(),
  uploadBrandImage: (el) => uploadBrandImage(el),
  removeBrandImage: () => removeBrandImage(),
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

async function uploadBrandImage(input) {
  const file = input.files[0];
  if (!file) return;

  const status = document.getElementById('brand-status');
  const removeBtn = document.getElementById('brand-remove-btn');
  status.textContent = 'Uploading…';
  status.style.color = 'var(--accent)';

  const csrfToken = (window.f0ckSession && window.f0ckSession.csrf_token) || document.getElementById('settings-status').dataset.csrf;
  const fd = new FormData();
  fd.append('file', file);

  try {
    const res = await fetch('/admin/brand_image/upload', {
      method: 'POST',
      headers: {
        'X-Requested-With': 'XMLHttpRequest',
        'X-CSRF-Token': csrfToken
      },
      body: fd
    });
    const data = await res.json();
    if (!data.success) throw new Error(data.msg || 'Upload failed');

    status.textContent = '✓ Brand image updated!';
    status.style.color = '#28a745';

    // Update preview
    const wrap = document.getElementById('brand-preview-wrap');
    let preview = document.getElementById('brand-preview');
    if (!preview || preview.tagName !== 'IMG') {
      wrap.innerHTML = '';
      preview = document.createElement('img');
      preview.id = 'brand-preview';
      preview.alt = 'current brand';
      wrap.appendChild(preview);
    }
    preview.src = data.url;

    // Immediately update navbar brand logo
    if (window.updateBrandImage) window.updateBrandImage(data.url);

    removeBtn.disabled = false;

    setTimeout(() => { status.textContent = ''; }, 3000);
  } catch (err) {
    status.textContent = 'Error: ' + err.message;
    status.style.color = '#d9534f';
  } finally {
    input.value = '';
  }
}

async function removeBrandImage() {
  const status = document.getElementById('brand-status');
  const removeBtn = document.getElementById('brand-remove-btn');

  if (!confirm('Remove the custom brand image? The navbar will revert to text.')) return;

  status.textContent = 'Removing…';
  status.style.color = 'var(--accent)';

  const csrfToken = (window.f0ckSession && window.f0ckSession.csrf_token) || document.getElementById('settings-status').dataset.csrf;

  try {
    const res = await fetch('/admin/brand_image/delete', {
      method: 'POST',
      headers: {
        'X-Requested-With': 'XMLHttpRequest',
        'X-CSRF-Token': csrfToken
      }
    });
    const data = await res.json();
    if (!data.success) throw new Error(data.msg || 'Remove failed');

    status.textContent = '✓ Brand image removed.';
    status.style.color = '#28a745';

    // Reset preview
    const wrap = document.getElementById('brand-preview-wrap');
    wrap.innerHTML = '<span id="brand-preview" class="adm-muted">No image set</span>';

    // Immediately update navbar brand logo (revert to text)
    if (window.updateBrandImage) window.updateBrandImage(null);

    removeBtn.disabled = true;

    setTimeout(() => { status.textContent = ''; }, 3000);
  } catch (err) {
    status.textContent = 'Error: ' + err.message;
    status.style.color = '#d9534f';
  }
}
