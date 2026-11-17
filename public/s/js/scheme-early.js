// Colour scheme of the frontends with a light and a dark look (f0ckgle, f0ckyou;
// views/frontends/README.md "Light and dark"). Loaded in <head>, before the frontend's CSS paints.
//
// The page comes with <html data-scheme="light|dark|auto" data-scheme-default="light|dark"> and
// the class scheme-light / scheme-dark (scheme-auto for "the device's", which CSS without
// JavaScript follows with a prefers-color-scheme query). Here "auto" is resolved to scheme-light /
// scheme-dark and follows the device while the page is open; on the shared pages (html.f0ck-shell)
// f0ckm's light theme (theme="light") goes along.
//
// Links a[data-scheme-set="light|dark|auto|default"] (plain links to /scheme/<x>, which work without
// JavaScript) switch it in place: the cookie is set with fetch, the classes change, the links of
// the current choice get aria-current, and `f0ck:scheme` is sent ({ detail: { scheme, resolved } }).
(() => {
  const html = document.documentElement;
  const media = window.matchMedia ? window.matchMedia('(prefers-color-scheme: dark)') : null;
  const fallback = () => (html.dataset.schemeDefault === 'dark' ? 'dark' : 'light');
  const choice = () => {
    const s = html.dataset.scheme;
    return s === 'light' || s === 'dark' || s === 'auto' ? s : fallback();
  };
  const resolve = (s) => (s === 'auto' ? (media && media.matches ? 'dark' : 'light') : s);

  const apply = (scheme, announce) => {
    const s = scheme === 'light' || scheme === 'dark' || scheme === 'auto' ? scheme : fallback();
    const resolved = resolve(s);
    html.dataset.scheme = s;
    html.classList.remove('scheme-light', 'scheme-dark', 'scheme-auto');
    html.classList.add(`scheme-${resolved}`);
    if (html.classList.contains('f0ck-shell')) {
      if (resolved === 'light') html.setAttribute('theme', 'light');
      else html.removeAttribute('theme');
    }
    const meta = document.querySelector('meta[name="theme-color"][data-light][data-dark]');
    if (meta) meta.setAttribute('content', resolved === 'dark' ? meta.dataset.dark : meta.dataset.light);
    document.querySelectorAll('[data-scheme-set]').forEach(a => {
      if (a.dataset.schemeSet === s) a.setAttribute('aria-current', 'true');
      else a.removeAttribute('aria-current');
    });
    if (announce) document.dispatchEvent(new CustomEvent('f0ck:scheme', { detail: { scheme: s, resolved } }));
  };

  apply(choice(), false);
  if (media) {
    const follow = () => { if (html.dataset.scheme === 'auto') apply('auto', true); };
    if (media.addEventListener) media.addEventListener('change', follow);
    else if (media.addListener) media.addListener(follow);
  }
  // the links' aria-current once they exist
  document.addEventListener('DOMContentLoaded', () => apply(html.dataset.scheme, false));

  document.addEventListener('click', (e) => {
    const a = e.target.closest && e.target.closest('a[data-scheme-set]');
    if (!a || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    const set = a.dataset.schemeSet;
    apply(set === 'default' ? fallback() : set, true);
    fetch(`/scheme/${encodeURIComponent(set)}`, { credentials: 'same-origin', headers: { Accept: 'application/json' } }).catch(() => {});
  });

  window.f0ckScheme = {
    get: choice,
    resolved: () => resolve(choice()),
    set: (s) => {
      apply(s, true);
      return fetch(`/scheme/${encodeURIComponent(s)}`, { credentials: 'same-origin', headers: { Accept: 'application/json' } }).catch(() => {});
    },
  };
})();
