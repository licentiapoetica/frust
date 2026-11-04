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

// Page lifetime: pages change without reloading (f0ck.js <navigation>). What a page's scripts hang on
// document / window gets { signal: v2Page.signal } and goes when the page is left; other clean-ups
// go through v2Page.onLeave(fn), intervals through v2Page.every(fn, ms). v2Page.onPage(fn): the
// setup of a script that stays loaded (f0ck.js, user.js), run now and for every page shown later.
// Defined here, in <head>, so the scripts of the page itself (comments.js) can use it too.
window.v2Page = (() => {
  let ac = new AbortController();
  let cleanups = [];
  const setups = [];
  const run = fn => { try { fn(); } catch(e) { console.error('[v2Page]', e); } };
  return {
    get signal() { return ac.signal; },
    onLeave(fn) { cleanups.push(fn); },
    every(fn, ms) { const id = setInterval(fn, ms); cleanups.push(() => clearInterval(id)); return id; },
    onPage(fn) { setups.push(fn); run(fn); },
    leave() { ac.abort(); cleanups.splice(0).forEach(run); ac = new AbortController(); },
    show() { setups.forEach(run); }
  };
})();
