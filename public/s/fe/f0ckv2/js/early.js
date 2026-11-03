// f0ckv2 frontend: runs in <head> before the page renders.
// JavaScript is on: drop the no-js class (CSS hides JS-only controls under html.no-js).
// Guests keep fullscreen in a cookie the templates can't read, so apply it here.
(() => {
  const html = document.documentElement;
  html.classList.remove('no-js');
  const m = document.cookie.match(/(?:^|;\s*)fullscreen=([^;]*)/);
  if (m) html.setAttribute('res', m[1] === '1' ? 'fullscreen' : '');
})();

// f0ckm's invite links and /register point at /#register[:token] (a modal there): show f0ckv2's page
(() => {
  const m = location.hash.match(/^#register(?::(.+))?$/);
  if(m && location.pathname === '/')
    location.replace('/?register=1' + (m[1] ? '&token=' + encodeURIComponent(decodeURIComponent(m[1])) : ''));
})();
