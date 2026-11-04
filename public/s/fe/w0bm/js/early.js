/*
 * w0bm frontend, loaded in <head> before the stylesheets apply: drops the no-js class and
 * activates the theme picked in the Theme menu (port of w0bm's styleswitcher.js).
 */
(() => {
  document.documentElement.classList.remove('w0bm-nojs');

  const readCookie = (name) => {
    const m = document.cookie.match(new RegExp('(?:^|; )' + name + '=([^;]*)'));
    return m ? decodeURIComponent(m[1]) : null;
  };

  const sheets = () => Array.from(document.querySelectorAll('link[rel~="stylesheet"][title]'));

  // Active theme as <html data-w0bm-theme>, so CSS can follow it (the shared pages' palette,
  // public/s/fe/w0bm/css/w0bm-shell.css)
  const markTheme = (title) => { document.documentElement.dataset.w0bmTheme = title || 'w0bmcustom'; };

  function setActiveStyleSheet(title) {
    markTheme(title);
    for (const link of sheets()) {
      link.disabled = true;
      if (link.getAttribute('title') === title) link.disabled = false;
    }
    document.querySelectorAll('#switcher button[data-style]').forEach((b) => {
      b.classList.toggle('active', b.dataset.style === (title || 'w0bmcustom'));
    });
  }

  function chooseStyle(title) {
    setActiveStyleSheet(title);
    const d = new Date();
    d.setTime(d.getTime() + 365 * 24 * 3600 * 1000);
    document.cookie = 'style=' + encodeURIComponent(title) + '; expires=' + d.toUTCString() + '; path=/; SameSite=Lax';
  }

  window.w0bmStyle = { set: chooseStyle };
  markTheme(readCookie('style'));

  // Comments sidebar hidden with C / the comments button: set before the page is drawn, so it
  // doesn't flash on the next item (w0bm.js keeps the class in sync when toggling)
  try {
    if (localStorage.getItem('comments') === 'false') document.documentElement.classList.add('w0bm-comments-off');
  } catch (e) { /* storage blocked */ }

  // The <link>s are parsed after this script: apply once they exist
  const apply = () => setActiveStyleSheet(readCookie('style') || 'w0bmcustom');
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', apply);
    // Alternate stylesheets start disabled anyway; enabling the chosen one early avoids a flash
    const obs = new MutationObserver(() => {
      const want = readCookie('style');
      if (!want) return;
      const link = sheets().find((l) => l.getAttribute('title') === want);
      if (link) {
        link.disabled = false;
        obs.disconnect();
      }
    });
    obs.observe(document.documentElement, { childList: true, subtree: true });
    document.addEventListener('DOMContentLoaded', () => obs.disconnect());
  } else {
    apply();
  }
})();
