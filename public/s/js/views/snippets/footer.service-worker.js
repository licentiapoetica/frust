if ('serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    navigator.serviceWorker.getRegistrations().then((registrations) => {
      let rootFound = false;
      let scopedFound = false;

      for (let r of registrations) {
        try {
          const scopePath = new URL(r.scope).pathname;
          if (scopePath === '/') {
            r.unregister().then(() => {
              console.log('[SW] Root ServiceWorker cleaned up.');
            });
            rootFound = true;
          } else if (scopePath === '/api/v2/export/') {
            scopedFound = true;
          }
        } catch (e) {}
      }

      // Register the scoped service worker so that exports still work on settings page
      // without intercepting normal site requests.
      if (!scopedFound || rootFound) {
        navigator.serviceWorker.register('/sw.js', { scope: '/api/v2/export/' }).then((registration) => {
          window.f0ckDebug('ServiceWorker registered with scoped path: ', registration.scope);
        }).catch((err) => {
          window.f0ckDebug('ServiceWorker registration failed: ', err);
        });
      }
    });
  });
}
