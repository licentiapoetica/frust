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

  function setActiveStyleSheet(title) {
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
