// Session config, defaults and UI strings for the site scripts (values from the #footer-data JSON block in footer.html)
(function () {
  var D = JSON.parse(document.getElementById('footer-data').textContent);
  window.f0ckSession = D.session;
  window.f0ckDebug = window.f0ckSession.development ? console.log.bind(console) : () => {};
  window.f0ckDefaultThumbSize = D.default_thumb_size;
  if ('i18n' in D) window.f0ckI18n = D.i18n;
})();
