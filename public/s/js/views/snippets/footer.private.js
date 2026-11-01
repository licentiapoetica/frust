(() => {
  const loginModal = document.getElementById('login-modal');
  const registerModal = document.getElementById('register-modal');
  const close = (m) => { if (m) m.style.display = 'none'; };
  const open = (m) => { if (m) m.style.display = 'flex'; };

  // View switching for login/forgot/reset within the login modal
  const switchModalView = (view) => {
    if (!loginModal) return;
    ['login', 'forgot', 'reset'].forEach(v => {
      const el = document.getElementById('modal-' + v + '-view');
      if (el) el.style.display = (v === view) ? 'block' : 'none';
    });
  };

  if (loginModal) {
    const loginClose = document.getElementById('login-modal-close');
    if (loginClose) loginClose.addEventListener('click', () => close(loginModal));
    loginModal.addEventListener('click', (e) => { if (e.target === loginModal) close(loginModal); });

    // Forgot Password link
    const modalForgotBtn = document.getElementById('modal-forgot-btn');
    if (modalForgotBtn) {
      modalForgotBtn.addEventListener('click', (e) => { e.preventDefault(); switchModalView('forgot'); });
    }
    const forgotToLogin = document.getElementById('forgot-to-login');
    if (forgotToLogin) {
      forgotToLogin.addEventListener('click', (e) => { e.preventDefault(); switchModalView('login'); });
    }

    // Check for reset token or login flag in URL
    const urlParams = new URLSearchParams(window.location.search);
    const resetToken = urlParams.get('token');
    if (resetToken) {
      const tokenInput = document.getElementById('reset-token');
      if (tokenInput) {
        tokenInput.value = resetToken;
        switchModalView('reset');
        loginModal.style.display = 'flex';
        const cleanUrl = window.location.pathname + window.location.search.replace(/[?&]token=[^&]+/, '').replace(/[?&]$/, '') + window.location.hash;
        window.history.replaceState({}, '', cleanUrl);
      }
    } else if (urlParams.get('login') === '1') {
      switchModalView('login');
      loginModal.style.display = 'flex';
      const cleanUrl = window.location.pathname + window.location.search.replace(/[?&]login=1/, '').replace(/[?&]$/, '') + window.location.hash;
      window.history.replaceState({}, '', cleanUrl);
    }

    // Login form AJAX
    const loginForm = loginModal.querySelector('.login-form');
    if (loginForm && loginForm.id !== 'forgot-password-form' && loginForm.id !== 'reset-password-form') {
      loginForm.addEventListener('submit', async (e) => {
        e.preventDefault();
        const formData = new FormData(loginForm);
        if (!formData.get('password')) {
          let errDiv = loginForm.querySelector('.flash-error');
          if (!errDiv) { errDiv = document.createElement('div'); errDiv.className = 'flash-error'; loginForm.insertBefore(errDiv, loginForm.firstChild); }
          errDiv.textContent = 'Invalid username or password.';
          return;
        }
        try {
          const res = await fetch('/login', {
            method: 'POST',
            headers: { 'X-Requested-With': 'XMLHttpRequest', 'Accept': 'application/json', 'Content-Type': 'application/x-www-form-urlencoded' },
            body: new URLSearchParams(new FormData(loginForm))
          });
          if (res.redirected) { window.location.href = res.url; return; }
          const json = await res.json();
          if (json && json.success === false) {
            let errDiv = loginForm.querySelector('.flash-error');
            if (!errDiv) { errDiv = document.createElement('div'); errDiv.className = 'flash-error'; loginForm.insertBefore(errDiv, loginForm.firstChild); }
            errDiv.textContent = json.msg;
          }
        } catch (err) { console.error('Login error:', err); }
      });
    }

    // Forgot Password form AJAX
    const forgotForm = document.getElementById('forgot-password-form');
    if (forgotForm) {
      forgotForm.addEventListener('submit', async (e) => {
        e.preventDefault();
        const email = document.getElementById('forgot-email').value;
        const status = document.getElementById('forgot-status');
        const btn = forgotForm.querySelector('button');
        btn.disabled = true; btn.textContent = 'Sending...';
        if (status) { status.textContent = ''; status.className = ''; }
        try {
          const res = await fetch('/forgot-password', {
            method: 'POST',
            headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'Accept': 'application/json' },
            body: new URLSearchParams({ email })
          });
          const data = await res.json();
          if (data.success) {
            if (status) { status.textContent = data.msg || 'Success! Check your email.'; status.className = 'flash-success'; }
            forgotForm.reset();
          } else {
            if (status) { status.textContent = data.msg || 'Error sending link.'; status.className = 'flash-error'; }
          }
        } catch (err) {
          if (status) { status.textContent = 'Network error.'; status.className = 'flash-error'; }
        } finally { btn.disabled = false; btn.textContent = 'Send Reset Link'; }
      });
    }

    // Reset Password form AJAX
    const resetForm = document.getElementById('reset-password-form');
    if (resetForm) {
      const resetToLogin = document.getElementById('reset-to-login');
      resetForm.addEventListener('submit', async (e) => {
        e.preventDefault();
        const token = document.getElementById('reset-token').value;
        const password = document.getElementById('reset-password').value;
        const password_confirm = document.getElementById('reset-password-confirm').value;
        const status = document.getElementById('reset-status');
        const btn = resetForm.querySelector('button');
        if (password !== password_confirm) {
          if (status) { status.textContent = 'Passwords do not match.'; status.className = 'flash-error'; }
          return;
        }
        if (password.length < 20) {
          if (status) { status.textContent = 'Password is too short (minimum 20 characters).'; status.className = 'flash-error'; }
          return;
        }
        btn.disabled = true; btn.textContent = 'Updating...';
        if (status) { status.textContent = ''; status.className = ''; }
        try {
          const res = await fetch('/reset-password', {
            method: 'POST',
            headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'Accept': 'application/json' },
            body: new URLSearchParams({ token, password, password_confirm })
          });
          const data = await res.json();
          if (data.success) {
            if (status) { status.textContent = data.msg || 'Password updated successfully!'; status.className = 'flash-success'; }
            resetForm.reset();
            btn.style.display = 'none';
            if (resetToLogin) resetToLogin.style.display = 'inline-block';
          } else {
            if (status) { status.textContent = data.msg || 'Error resetting password.'; status.className = 'flash-error'; }
          }
        } catch (err) {
          if (status) { status.textContent = 'Network error.'; status.className = 'flash-error'; }
        } finally { btn.disabled = false; btn.textContent = 'Update Password'; }
      });
      if (resetToLogin) {
        resetToLogin.addEventListener('click', (e) => {
          e.preventDefault();
          switchModalView('login');
          resetToLogin.style.display = 'none';
          resetForm.querySelector('button').style.display = 'inline-block';
        });
      }
    }
  }

  if (registerModal) {
    const regClose = document.getElementById('register-modal-close');
    if (regClose) regClose.addEventListener('click', () => close(registerModal));
    registerModal.addEventListener('click', (e) => { if (e.target === registerModal) close(registerModal); });
  }

  // ESC key closes modals
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { close(loginModal); close(registerModal); }
  });

  // Auto-open login modal if URL has #login hash
  if (window.location.hash === '#login' && loginModal) {
    switchModalView('login');
    loginModal.style.display = 'flex';
  }

  const toRegister = document.getElementById('login-to-register');
  if (toRegister) toRegister.addEventListener('click', (e) => { e.preventDefault(); close(loginModal); open(registerModal); });
  const toLogin = document.getElementById('register-to-login');
  if (toLogin) toLogin.addEventListener('click', (e) => { e.preventDefault(); close(registerModal); open(loginModal); });

  // Registration form AJAX
  const regForm = document.getElementById('modal-register-form');
  if (regForm) {
    regForm.addEventListener('submit', async (e) => {
      e.preventDefault();
      const status = document.getElementById('register-status');
      const formData = new FormData(regForm);
      const btn = regForm.querySelector('button[type="submit"]');
      const password = formData.get('password');
      const password_confirm = formData.get('password_confirm');

      if (password && password.length < 20) {
        if (status) {
          status.textContent = 'Password is too short (minimum 20 characters).';
          status.className = 'flash-error';
        }
        return;
      }

      if (password !== password_confirm) {
        if (status) {
          status.textContent = 'Passwords do not match.';
          status.className = 'flash-error';
        }
        return;
      }

      btn.disabled = true; btn.textContent = 'Creating...';
      try {
        const resp = await fetch('/register', {
          method: 'POST',
          headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'X-Requested-With': 'XMLHttpRequest' },
          body: new URLSearchParams(new FormData(regForm)),
          credentials: 'include'
        });
        const json = await resp.json();
        if (status) { status.textContent = json.msg || ''; status.className = json.success ? 'flash-success' : 'flash-error'; }
        if (json.success && json.upgraded) setTimeout(() => { location.reload(); }, 1500);
        else if (json.success) setTimeout(() => { close(registerModal); open(loginModal); }, 2000);
        else window.f0ckCapReset?.(regForm); // captcha tokens are single-use
      } catch (err) {
        if (status) { status.textContent = 'Network error.'; status.className = 'flash-error'; }
      } finally { btn.disabled = false; btn.textContent = 'Create Account'; }
    });
  }
})();
