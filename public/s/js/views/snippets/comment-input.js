// Same action comments.js registers for renderGuestInput(); defined here too so the server copy works before comments.js (deferred) has run
F0ckActions.define('comments-guest-login', () => { const b = document.getElementById('nav-login-btn'); if (b) b.click(); });
