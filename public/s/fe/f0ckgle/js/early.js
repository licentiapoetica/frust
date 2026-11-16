// f0ckgle frontend: runs in <head> before the page renders.
// JavaScript is on: drop the no-js class (CSS hides JS-only controls under html.no-js).
document.documentElement.classList.remove('no-js');

// f0ckm's invite links and /register point at /#register[:token] (a modal there): show f0ckgle's page
(() => {
  const m = location.hash.match(/^#register(?::(.+))?$/);
  if (m && location.pathname === '/')
    location.replace('/?register=1' + (m[1] ? '&token=' + encodeURIComponent(decodeURIComponent(m[1])) : ''));
})();
