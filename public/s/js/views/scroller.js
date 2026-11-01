// Scroller page config: publishes the server values from the #scroller-data JSON block as the
// window.* globals scroller.js reads. Wrapped in a function so a re-run (PJAX into /abyss)
// doesn't redeclare anything.
(function() {
  var D = JSON.parse(document.getElementById('scroller-data').textContent);
  window.f0ckThemes = D.themes;
  window.f0ckDomain = D.domain;
  window.scrollerMode = D.mode;
  window.scrollerLoggedIn = D.logged_in;
  window.scrollerIsMod = D.is_mod;
  window.scrollerCsrf = D.csrf;
  window.f0ckSession = {
    logged_in: window.scrollerLoggedIn,
    user: D.user,
    is_anon: D.is_anon,
    anon_permissions: D.anon_permissions,
    csrf_token: window.scrollerCsrf,
    guest_anonymize: D.guest_anonymize,
    anon_anonymize: D.anon_anonymize,
    is_anonymized: D.is_anonymized,
    excluded_tags: D.excluded_tags,
    development: D.development
  };
  window.f0ckDebug = window.f0ckSession.development ? console.log.bind(console) : () => {};
  window.scrollerEnableNsfl = D.enable_nsfl;
  window.scrollerEnableSwf  = D.enable_swf;
  // The DB value arrives as a numeric string (it used to be emitted as a bare JS number literal)
  window.scrollerRuffleVolume = Number(D.ruffle_volume);
  window.f0ckAllowedImages = D.allowed_images;
  window.scrollerPublic = D.public;
  window.scrollerMimeCats = D.mime_cats;
  window.f0ckI18n = D.i18n;
  if (D.session) {
    window.scrollerUsername = D.session.username;
    window.scrollerDisplayName = D.session.display_name;
    window.scrollerUserAvatar = D.session.avatar;
    Object.assign(window.f0ckI18n, D.session.i18n);
  }
})();
