(function() {
  function getCsrf() { return (window.f0ckSession && window.f0ckSession.csrf_token) || ''; }
  var i18n = window.f0ckI18n || {};
  var grid = document.getElementById('hall-manager-grid');

  function toSlug(s) {
    return s.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
  }

  // ── Create Hall ─────────────────────────────────────────────────────────────
  // ── Config from data attributes ────────────────────────────────────────────
  var cfgEl = document.getElementById('uh-config');
  var isAdmin = cfgEl.dataset.isAdmin === 'true';
  var ownerUserId = cfgEl.dataset.ownerId;
  var ownerUser = cfgEl.dataset.ownerUser;
  var isOwner = cfgEl.dataset.isOwner === 'true';
  var enableUpload = cfgEl.dataset.enableUpload === 'true';
  var i18n_posts = cfgEl.dataset.i18nHallPosts;
  
  // If admin is managing another user, append user_id query param
  var qs = (isAdmin && !isOwner) ? '?user_id=' + ownerUserId : '';

  var newNameInput = document.getElementById('uh-new-name');
  var createStatus = document.getElementById('uh-create-status');

  async function createHall() {
    var name = newNameInput.value.trim();
    if (!name) { createStatus.textContent = i18n.hall_enter_name_error || "Name is required"; createStatus.style.color = '#f55'; return; }
    var slug = toSlug(name);
    createStatus.textContent = i18n.hall_creating || "Creating…"; createStatus.style.color = '#888';
    try {
      var r = await fetch('/api/v2/me/halls' + qs, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'x-csrf-token': getCsrf() },
        body: JSON.stringify({ name: name, slug: slug })
      });
      var d = await r.json();
      if (d.success) {
        createStatus.textContent = i18n.hall_created || '✓ Created!'; createStatus.style.color = 'var(--accent)';
        newNameInput.value = '';
        var card = buildCard(name, slug, d.hall);
        grid.appendChild(card);
        wireCard(card);
        card.scrollIntoView({ behavior: 'smooth', block: 'center' });
        setTimeout(function() { createStatus.textContent = ''; }, 2000);
      } else {
        createStatus.textContent = '✗ ' + (d.msg || d.message || 'Error'); createStatus.style.color = '#f55';
      }
    } catch(e) { createStatus.textContent = '✗ Network error'; createStatus.style.color = '#f55'; }
  }

  document.getElementById('btn-create-uh').addEventListener('click', createHall);
  newNameInput.addEventListener('keydown', function(e) { if (e.key === 'Enter') createHall(); });

  // ── Card builder (for newly created halls) ──────────────────────────────────
  function buildCard(name, slug, hall) {
    var div = document.createElement('div');
    div.className = 'hall-manager-card';
    div.id = 'uh-card-' + slug;
    div.dataset.slug = slug;
    div.dataset.ownerId = ownerUserId;
    var imgHtml = 
      '<div class="hall-manager-image" title="' + (enableUpload ? (i18n.hall_click_upload_hint || 'Click to upload custom image') : (i18n.hall_view || 'View hall')) + '">' +
        '<img src="/user_hall_image/' + ownerUserId + '/' + slug + '" alt="' + name + '" class="hall-card-img">' +
        '<span class="hall-card-name">' + name + '</span>';
    
    if (enableUpload) {
      imgHtml += '<input type="file" class="uh-img-upload" accept="image/*">';
    }
    imgHtml += '</div>';

    div.innerHTML = imgHtml + 
      '<div class="hall-card-body">' +
        '<div class="hall-card-field"><label class="hall-card-label">' + (i18n.common_name || 'Name') + '</label>' +
          '<input type="text" class="uh-name-input hall-card-input" value="' + name + '"></div>' +
        '<div class="hall-card-field"><label class="hall-card-label">' + (i18n.common_description || 'Description') + '</label>' +
          '<textarea class="uh-desc-input hall-card-input hall-card-textarea"></textarea></div>' +
        '<div class="hall-card-row hall-card-row-private">' +
          '<label class="hall-card-private-label"><input type="checkbox" class="uh-private-toggle"> ' + (i18n.common_private || 'Private') + '</label>' +
        '</div>' +
        '<div class="hall-card-row">' +
          '<button class="uh-btn-save hm-btn hall-card-btn-save">' + (i18n.common_save || 'Save') + '</button>' +
          '<a href="/user/' + ownerUser + '/hall/' + slug + '" class="hm-btn hall-card-btn-view">' + (i18n.common_view || 'View') + ' →</a>' +
          '<span class="hall-card-posts">' + 
            i18n_posts.replace('{count}', (hall && hall.total_items || 0)) + 
          '</span>' +
          '<button class="uh-btn-delete hm-btn hall-card-btn-delete">🗑 ' + (i18n.common_delete || 'Delete') + '</button>' +
        '</div>' +
        '<div class="uh-card-status"></div>' +
      '</div>';
    return div;
  }

  // ── Wire a card ─────────────────────────────────────────────────────────────
  function wireCard(card) {
    var slug = card.dataset.slug;
    var status = card.querySelector('.uh-card-status');
    var nameInput = card.querySelector('.uh-name-input');
    var descInput = card.querySelector('.uh-desc-input');
    var imgEl = card.querySelector('.hall-manager-image img');
    var imgContainer = card.querySelector('.hall-manager-image');
    var fileInput = card.querySelector('.uh-img-upload');
    var nameOverlay = imgContainer && imgContainer.querySelector('span');
    var privToggle = card.querySelector('.uh-private-toggle');

    function setStatus(msg, color) {
      if (!status) return;
      status.textContent = msg;
      status.style.color = color || '#aaa';
    }

    // Live name overlay
    if (nameInput && nameOverlay) {
      nameInput.addEventListener('input', function() { nameOverlay.textContent = nameInput.value; });
    }

    // Click image → open file picker or go to hall
    if (imgContainer) {
      if (enableUpload) {
        imgContainer.title = i18n.hall_click_upload_hint || 'Click to upload custom image';
        imgContainer.addEventListener('click', function(e) {
          if (e.target.classList.contains('uh-btn-del-img')) return;
          if (fileInput) fileInput.click();
        });
      } else {
        imgContainer.title = i18n.hall_view || 'View hall';
        imgContainer.addEventListener('click', function() {
          var url = '/user/' + ownerUser.toLowerCase() + '/hall/' + slug;
          if (window.loadPageAjax) window.loadPageAjax(url, true);
          else window.location.href = url;
        });
      }
    }

    // Upload image
    if (fileInput) {
      fileInput.addEventListener('change', async function(e) {
        var file = e.target.files[0];
        if (!file) return;
        setStatus(i18n.uploading || 'Uploading…');
        var fd = new FormData();
        fd.append('file', file);
        try {
          var r = await fetch('/api/v2/me/halls/' + encodeURIComponent(slug) + '/image' + qs, {
            method: 'POST',
            headers: { 'x-csrf-token': getCsrf() },
            body: fd
          });
          var d = await r.json();
          if (d.success) {
            if (imgEl) imgEl.src = '/user_hall_image/' + (card.dataset.ownerId || '') + '/' + slug + '?v=' + Date.now();
            setStatus('✓ ' + (i18n.hall_image_uploaded || 'Image uploaded'), 'var(--accent)');
            // Add remove button if not present
            if (!imgContainer.querySelector('.uh-btn-del-img')) {
              var xBtn = document.createElement('button');
              xBtn.className = 'uh-btn-del-img';
              xBtn.title = 'Remove custom image';
              xBtn.textContent = '✕';
              imgContainer.appendChild(xBtn);
              wireDelBtn(xBtn);
            }
          } else {
            setStatus('✗ ' + (d.msg || 'Upload failed'), '#f55');
          }
        } catch(e2) { setStatus('✗ Network error', '#f55'); }
        e.target.value = '';
      });
    }

    // Remove custom image
    function wireDelBtn(btn) {
      btn.addEventListener('click', async function(e) {
        e.stopPropagation();
        setStatus(i18n.hall_removing || 'Removing…');
        try {
          var r = await fetch('/api/v2/me/halls/' + encodeURIComponent(slug) + '/image' + qs, {
            method: 'DELETE',
            headers: { 'x-csrf-token': getCsrf() }
          });
          var d = await r.json();
          if (d.success) {
            if (imgEl) imgEl.src = '/user_hall_image/' + (card.dataset.ownerId || '') + '/' + slug + '?v=' + Date.now();
            btn.remove();
            setStatus('✓ ' + (i18n.hall_image_removed || 'Image removed'), 'var(--accent)');
          } else { setStatus('✗ ' + (d.msg || 'Error'), '#f55'); }
        } catch(e2) { setStatus('✗ Network error', '#f55'); }
      });
    }
    var existingDelBtn = card.querySelector('.uh-btn-del-img');
    if (existingDelBtn) wireDelBtn(existingDelBtn);

    // Private toggle
    if (privToggle) {
      privToggle.addEventListener('change', function() {
        fetch('/api/v2/me/halls/' + encodeURIComponent(slug) + qs, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json', 'x-csrf-token': getCsrf() },
          body: JSON.stringify({ is_private: privToggle.checked })
        }).then(function(r) { return r.json(); }).then(function(d) {
          if (d.success) setStatus(privToggle.checked ? '🔒 Private' : '🌐 Public', 'var(--accent)');
          else { privToggle.checked = !privToggle.checked; setStatus('✗ ' + (d.msg || 'Error'), '#f55'); }
        }).catch(function() { privToggle.checked = !privToggle.checked; setStatus('✗ Network error', '#f55'); });
      });
    }

    // Save
    var saveBtn = card.querySelector('.uh-btn-save');
    if (saveBtn) {
      saveBtn.addEventListener('click', async function() {
        setStatus(i18n.hall_saving || 'Saving…');
        var newName = nameInput ? nameInput.value.trim() : null;
        var newDesc = descInput ? descInput.value.trim() : null;
        var isPrivate = privToggle ? privToggle.checked : undefined;
        if (!newName) { setStatus('✗ ' + (i18n.hall_name_empty || 'Name cannot be empty'), '#f55'); return; }
        try {
          var r = await fetch('/api/v2/me/halls/' + encodeURIComponent(slug) + qs, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json', 'x-csrf-token': getCsrf() },
            body: JSON.stringify({ name: newName, description: newDesc || null, is_private: isPrivate })
          });
          var d = await r.json();
          if (d.success) {
            if (nameOverlay && newName) nameOverlay.textContent = newName;
            setStatus('✓ ' + (i18n.saved || 'Saved'), 'var(--accent)');
          } else { setStatus('✗ ' + (d.msg || 'Error'), '#f55'); }
        } catch(e2) { setStatus('✗ Network error', '#f55'); }
      });
    }

    // Delete
    var delBtn = card.querySelector('.uh-btn-delete');
    if (delBtn) {
      delBtn.addEventListener('click', async function() {
        if (!confirm(i18n.hall_delete_confirm || 'Are you sure you want to delete this hall?')) return;
        setStatus(i18n.hall_deleting || 'Deleting…');
        try {
          var r = await fetch('/api/v2/me/halls/' + encodeURIComponent(slug) + qs, {
            method: 'DELETE',
            headers: { 'x-csrf-token': getCsrf() }
          });
          var d = await r.json();
          if (d.success) {
            card.style.transition = 'opacity 0.3s';
            card.style.opacity = '0';
            setTimeout(function() { card.remove(); }, 300);
          } else { setStatus('✗ ' + (d.msg || 'Error'), '#f55'); }
        } catch(e2) { setStatus('✗ Network error', '#f55'); }
      });
    }
  }

  // Wire all existing cards
  document.querySelectorAll('.hall-manager-card').forEach(wireCard);
})();
