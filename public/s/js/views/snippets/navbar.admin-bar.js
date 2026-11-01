(function() {
  'use strict';

  /* ── Admin bar: Sidebar Width Awareness & Hotkey Toggle ────────────────── */
  const adminBar    = document.getElementById('admin-bar');
  const closeBtn    = document.getElementById('admin-bar-close');
  const closeImpBtn = document.getElementById('admin-bar-close-imp');

  function syncAdminBarSidebars() {
    if (!adminBar) return;

    // Right sidebar detection
    const rightSb = document.querySelector('.global-sidebar-right, .item-sidebar-right, .index-sidebar-right');
    const isRightHidden = !rightSb ||
                          document.body.classList.contains('sidebar-right-hidden') ||
                          window.getComputedStyle(rightSb).display === 'none' ||
                          window.getComputedStyle(rightSb).visibility === 'hidden';

    let rw = 0;
    if (!isRightHidden && rightSb) {
      rw = rightSb.offsetWidth || (window.innerWidth <= 999 ? Math.min(280, Math.round(window.innerWidth * 0.8)) : 300);
    }

    // Left sidebar detection (modern layout item page)
    const leftSb = document.querySelector('.item-sidebar-left');
    const isLeftHidden = !leftSb ||
                         document.body.classList.contains('sidebar-left-hidden') ||
                         document.body.classList.contains('comments-hidden') ||
                         window.innerWidth < 1000 ||
                         window.getComputedStyle(leftSb).display === 'none' ||
                         window.getComputedStyle(leftSb).visibility === 'hidden' ||
                         !!document.querySelector('.item-layout-container.sidebar-hidden');

    let lw = 0;
    if (!isLeftHidden && leftSb) {
      lw = leftSb.offsetWidth || Math.min(350, Math.round(window.innerWidth * 0.3));
    }

    adminBar.style.setProperty('--admin-bar-right', rw + 'px');
    adminBar.style.setProperty('--admin-bar-left', lw + 'px');
    adminBar.style.right = rw + 'px';
    adminBar.style.left = lw + 'px';
  }

  function openAdminBar() {
    if (!adminBar) return;
    syncAdminBarSidebars();
    adminBar.classList.add('open');
    document.body.classList.add('admin-bar-open');
  }

  function closeAdminBar() {
    if (!adminBar) return;
    adminBar.classList.remove('open');
    document.body.classList.remove('admin-bar-open');
  }

  function toggleAdminBar() {
    if (!adminBar) return;
    if (adminBar.classList.contains('open')) {
      closeAdminBar();
    } else {
      openAdminBar();
    }
  }

  if (closeBtn) closeBtn.addEventListener('click', closeAdminBar);
  if (closeImpBtn) closeImpBtn.addEventListener('click', closeAdminBar);

  window.addEventListener('keydown', (e) => {
    // Never trigger if focus is inside an input, textarea, select or contenteditable
    const target = e.target;
    const tag = (target && target.tagName) ? target.tagName.toUpperCase() : '';
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || (target && target.isContentEditable)) {
      return;
    }

    // Hotkey: '^' or '~' (also match Backquote key without modifier keys)
    const isHotkey = (
      e.key === '^' ||
      e.key === '~' ||
      (e.code === 'Backquote' && !e.ctrlKey && !e.metaKey && !e.altKey)
    );

    if (isHotkey) {
      e.preventDefault();
      toggleAdminBar();
      return;
    }

    // Escape closes admin bar if open
    if (e.key === 'Escape' && adminBar && adminBar.classList.contains('open')) {
      closeAdminBar();
    }
  });

  // Watch body class changes for sidebar toggles
  if (window.MutationObserver) {
    const bodyObserver = new MutationObserver(() => {
      syncAdminBarSidebars();
    });
    bodyObserver.observe(document.body, { attributes: true, attributeFilter: ['class'] });
  }

  window.addEventListener('resize', syncAdminBarSidebars, { passive: true });
  document.addEventListener('f0ck:contentLoaded', syncAdminBarSidebars);
  document.addEventListener('transitionend', (e) => {
    if (e.target && (e.target.classList.contains('global-sidebar-right') ||
                     e.target.classList.contains('item-sidebar-right') ||
                     e.target.classList.contains('item-sidebar-left'))) {
      syncAdminBarSidebars();
    }
  });

  /* ── Admin bar: View-As (impersonation) ────────────────────────────────── */
  const input      = document.getElementById('admin-viewas-input');
  const suggestBox = document.getElementById('admin-viewas-suggestions');
  const viewAsBtn  = document.getElementById('admin-viewas-btn');
  const exitBtn    = document.getElementById('admin-bar-exit-impersonate');

  let _suggestTimer = null;
  let _suggestions  = [];
  let _activeIdx    = -1;

  function closeSuggestions() {
    if (suggestBox) { suggestBox.innerHTML = ''; suggestBox.classList.remove('open'); }
    _suggestions = [];
    _activeIdx   = -1;
  }

  function renderSuggestions(list) {
    if (!suggestBox) return;
    suggestBox.innerHTML = '';
    if (!list || list.length === 0) { suggestBox.classList.remove('open'); return; }
    list.forEach((u, i) => {
      const el = document.createElement('div');
      el.className = 'admin-viewas-suggestion';
      el.innerHTML = '<i class="fa-solid fa-user"></i>' + u.user;
      el.dataset.username = u.user;
      el.addEventListener('mousedown', (e) => { e.preventDefault(); selectSuggestion(u.user); });
      el.addEventListener('mouseenter', () => { setActive(i); });
      suggestBox.appendChild(el);
    });
    _suggestions = list;
    _activeIdx = -1;
    suggestBox.classList.add('open');
  }

  function setActive(idx) {
    const items = suggestBox ? suggestBox.querySelectorAll('.admin-viewas-suggestion') : [];
    items.forEach((el, i) => el.classList.toggle('active', i === idx));
    _activeIdx = idx;
  }

  function selectSuggestion(username) {
    if (input) input.value = username;
    closeSuggestions();
  }

  function fetchSuggestions(q) {
    if (!q || q.length < 1) { closeSuggestions(); return; }
    fetch('/api/v2/admin/users/search?q=' + encodeURIComponent(q), {
      headers: { 'X-Requested-With': 'XMLHttpRequest' }
    }).then(r => r.json()).then(data => {
      if (Array.isArray(data)) renderSuggestions(data);
    }).catch(() => {});
  }

  if (input) {
    input.addEventListener('input', () => {
      clearTimeout(_suggestTimer);
      const q = input.value.trim();
      if (!q) { closeSuggestions(); return; }
      _suggestTimer = setTimeout(() => fetchSuggestions(q), 200);
    });

    input.addEventListener('keydown', (e) => {
      const items = suggestBox ? suggestBox.querySelectorAll('.admin-viewas-suggestion') : [];
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setActive(Math.min(_activeIdx + 1, items.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setActive(Math.max(_activeIdx - 1, -1));
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (_activeIdx >= 0 && _suggestions[_activeIdx]) {
          selectSuggestion(_suggestions[_activeIdx].user);
        } else {
          startImpersonation();
        }
      } else if (e.key === 'Escape') {
        closeSuggestions();
        input.blur();
      }
    });

    input.addEventListener('blur', () => {
      setTimeout(closeSuggestions, 150);
    });
  }

  function startImpersonation() {
    const username = input ? input.value.trim() : '';
    if (!username) return;
    if (viewAsBtn) { viewAsBtn.disabled = true; }

    const csrf = (typeof window.f0ckSession !== 'undefined' && window.f0ckSession.csrf_token)
      ? window.f0ckSession.csrf_token
      : (document.querySelector('meta[name="csrf-token"]')?.content || '');

    const params = new URLSearchParams();
    params.append('username', username);
    if (csrf) params.append('csrf_token', csrf);

    fetch('/api/v2/admin/impersonate', {
      method: 'POST',
      headers: { 'X-Requested-With': 'XMLHttpRequest', 'Content-Type': 'application/x-www-form-urlencoded' },
      body: params
    }).then(r => r.json()).then(data => {
      if (data.success) {
        window.location.reload();
      } else {
        alert('Impersonation failed: ' + (data.msg || 'Unknown error'));
        if (viewAsBtn) viewAsBtn.disabled = false;
      }
    }).catch(e => {
      alert('Network error: ' + e.message);
      if (viewAsBtn) viewAsBtn.disabled = false;
    });
  }

  if (viewAsBtn) {
    viewAsBtn.addEventListener('click', startImpersonation);
  }

  /* ── Exit impersonation ─────────────────────────────────────────────────── */
  if (exitBtn) {
    exitBtn.addEventListener('click', () => {
      exitBtn.disabled = true;
      exitBtn.innerHTML = '<i class="fa-solid fa-circle-notch fa-spin"></i> Exiting…';

      const csrf = (typeof window.f0ckSession !== 'undefined' && window.f0ckSession.csrf_token)
        ? window.f0ckSession.csrf_token
        : (document.querySelector('meta[name="csrf-token"]')?.content || '');

      const params = new URLSearchParams();
      if (csrf) params.append('csrf_token', csrf);

      fetch('/api/v2/admin/stop-impersonate', {
        method: 'POST',
        headers: { 'X-Requested-With': 'XMLHttpRequest', 'Content-Type': 'application/x-www-form-urlencoded' },
        body: params
      }).then(() => {
        window.location.reload();
      }).catch(() => {
        window.location.reload();
      });
    });
  }

  /* ── Admin bar: Preview As role toggle ──────────────────────────────────── */
  // The role is applied server-side (cookie preview_role, see src/index.mjs): templates, f0ckSession and
  // admin/mod-guarded routes all behave as that role. Switching sets the cookie and reloads.
  (function() {
    const COOKIE = 'preview_role';
    const pills       = document.querySelectorAll('.admin-bar-preview-pill');
    const exitPill    = document.getElementById('admin-preview-exit-pill');
    const exitLabel   = document.getElementById('admin-preview-exit-label');
    const exitBtn     = document.getElementById('admin-preview-exit-btn');

    const roleLabels = { admin: 'Admin', mod: 'Moderator', user: 'User' };

    function applyRole(role) {
      document.body.classList.remove('preview-as-user', 'preview-as-mod');
      if (role === 'user') document.body.classList.add('preview-as-user');
      if (role === 'mod')  document.body.classList.add('preview-as-mod');

      // Update active pill
      pills.forEach(p => p.classList.toggle('active', p.dataset.role === role));

      // Show/hide exit pill
      if (exitPill) {
        if (role !== 'admin') {
          if (exitLabel) exitLabel.textContent = 'Previewing as ' + roleLabels[role];
          exitPill.style.display = 'flex';
        } else {
          exitPill.style.display = 'none';
        }
      }
    }

    function switchRole(role) {
      const secure = location.protocol === 'https:' ? '; Secure' : '';
      // No template literals here: the view engine would evaluate the placeholders server-side
      document.cookie = role === 'admin'
        ? COOKIE + '=; path=/; max-age=0; SameSite=Lax' + secure
        : COOKIE + '=' + role + '; path=/; max-age=86400; SameSite=Lax' + secure;
      location.reload();
    }

    // The server tells which role this page was rendered as
    try { localStorage.removeItem('f0ck_preview_role'); } catch {} // old client-only preview
    const current = (adminBar && adminBar.dataset.previewRole) || 'admin';
    applyRole(roleLabels[current] ? current : 'admin');

    pills.forEach(pill => {
      pill.addEventListener('click', () => {
        const role = pill.dataset.role;
        if (role !== current) switchRole(role);
      });
    });

    if (exitBtn) exitBtn.addEventListener('click', () => switchRole('admin'));
  })();

})();
