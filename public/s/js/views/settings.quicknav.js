      (function(){
        var nav = document.getElementById('settings-quicknav');
        var root = nav && nav.closest('.settings');
        if (!nav || !root) return;
        var GAP = 12; // keep in sync with --st-gap

        // Opened in the page modal (sidebar-footer style): its body scrolls, not the window, and the
        // quicknav sticks to the top of it instead of being fixed under the navbar
        var sc = nav.closest('.pgm-body');
        if (sc) root.classList.add('in-page-modal');

        // Full page (e.g. after F5 while the settings were open in the page modal): own title bar. Close
        // goes back to the page the modal was opened over (page-modal.js remembers it), else home.
        var header = sc ? null : root.querySelector('#settings-page-header');
        if (header) {
          header.querySelector('.sph-close').addEventListener('click', function() {
            var back = null;
            try { back = sessionStorage.getItem('f0ck_pgm_base'); sessionStorage.removeItem('f0ck_pgm_base'); } catch (e) {}
            if (!back || back.charAt(0) !== '/' || back.charAt(1) === '/' || /^\/settings\/?(?:[?#]|$)/.test(back)) back = '/';
            location.assign(back);
          });
        }
        function scrollTop() { return sc ? sc.scrollTop : window.scrollY; }
        function scrollToY(y) { if (sc) sc.scrollTop = y; else window.scrollTo({ top: y, behavior: 'instant' }); }
        function viewBottom() { return sc ? sc.getBoundingClientRect().bottom : window.innerHeight; }
        function atEnd() {
          return sc
            ? sc.scrollTop + sc.clientHeight >= sc.scrollHeight - 2
            : window.scrollY + window.innerHeight >= document.documentElement.scrollHeight - 2;
        }

        // The quicknav is fixed and wraps onto more lines on narrow screens, so its bottom edge is
        // measured instead of assumed; CSS uses it for the content offset and the scroll-margin.
        // Horizontal fit: the quicknav is fixed, so it has to end where the content does when the right
        // sidebar sits beside it (same rules as the page modal: small phones overlay, never make room)
        var rightOverride = null; // set by the sidebar toggle before/after its slide (f0ck:sidebar-right-layout)
        function sidebarEl() {
          return document.querySelector('.global-sidebar-right, .item-sidebar-right, .index-sidebar-right');
        }
        function contentRight() {
          var vw = window.innerWidth;
          if (vw <= 599) return 0;
          if (rightOverride && performance.now() < rightOverride.until) return rightOverride.right;
          var rs = sidebarEl();
          var open = !!rs && !document.body.classList.contains('sidebar-right-hidden')
            && getComputedStyle(rs).display !== 'none' && rs.offsetWidth > 0;
          return open ? Math.min(Math.round(rs.offsetWidth), Math.round(vw / 2)) : 0;
        }

        function measure() {
          if (!nav.isConnected) return;
          // In the page modal the modal already sits beside the sidebar and its header matches the tabs
          var right = sc ? 0 : contentRight();
          nav.style.right = sc ? '' : right + 'px';
          // Sidebar beside the content: exactly the height of its tab row, one continuous bar under the
          // navbar. On the full page that bar is the title bar (as in the page modal), the quicknav below it.
          var rs = right > 0 ? sidebarEl() : null;
          var tabs = rs ? rs.querySelector('.sidebar-tabs') : null;
          var tabsH = tabs ? tabs.getBoundingClientRect().height : 0;
          var bar = header || nav;
          if (header) header.style.right = right + 'px';
          if (tabsH > 0) {
            bar.style.height = tabsH + 'px';
            bar.classList.add(header ? 'sph-synced' : 'qn-synced');
          } else {
            bar.style.height = '';
            bar.classList.remove('sph-synced', 'qn-synced');
          }
          if (header) nav.style.top = Math.round(header.getBoundingClientRect().bottom) + 'px';
          var bottom = Math.ceil(nav.getBoundingClientRect().bottom);
          root.style.setProperty('--settings-qn-bottom', bottom + 'px');
        }

        function targetY(el) {
          return Math.max(0, Math.round(el.getBoundingClientRect().top + scrollTop() - nav.getBoundingClientRect().bottom - GAP));
        }

        var links = Array.prototype.slice.call(nav.querySelectorAll('a[href^="#"]'));
        var _active = null;
        function setActive(href) {
          if (href === _active) return;
          _active = href;
          links.forEach(function(a) { a.classList.toggle('active', a.getAttribute('href') === href); });
          // Single-line (synced) quicknav can overflow sideways: bring the active chip into view.
          // Adjusts only the nav's own scrollLeft, never the page.
          var link = nav.querySelector('a[href="' + href + '"]');
          if (link && nav.scrollWidth > nav.clientWidth) {
            var l = link.offsetLeft - 16, r = link.offsetLeft + link.offsetWidth + 16;
            if (l < nav.scrollLeft) nav.scrollTo({ left: l, behavior: 'smooth' });
            else if (r > nav.scrollLeft + nav.clientWidth) nav.scrollTo({ left: r - nav.clientWidth, behavior: 'smooth' });
          }
        }

        function highlightTarget(el) {
          root.querySelectorAll('.anchor-target').forEach(function(c) { c.classList.remove('anchor-target'); });
          if (el) el.classList.add('anchor-target');
        }

        // Scroll-spy: active tab = last section whose heading has reached the quicknav line.
        // At the very bottom the last section wins even if its heading can't reach the line.
        // After a click the clicked tab stays active until the user scrolls themselves, so a short
        // section right below can't take over the highlight
        var _lock = null;
        function spy() {
          if (_anim || _lock || !nav.isConnected) return;
          // A section counts as current once its heading is in the top 30% of the visible area
          // (below the quicknav), not only when it touches the quicknav
          var navBottom = nav.getBoundingClientRect().bottom;
          var line = navBottom + GAP + Math.max(2, (viewBottom() - navBottom) * 0.3);
          var current = links[0];
          for (var i = 0; i < links.length; i++) {
            var el = root.querySelector(links[i].getAttribute('href'));
            if (el && el.getBoundingClientRect().top <= line) current = links[i];
          }
          if (atEnd()) current = links[links.length - 1];
          if (current) setActive(current.getAttribute('href'));
        }

        // Smooth scroll driven per frame: the destination is re-read every frame, so sections that
        // change height mid-scroll (passkeys, invites, API key load async) are absorbed into the motion
        // and it ends exactly GAP px under the quicknav, with no correction snap afterwards.
        var _anim = null;
        function ease(t) { return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2; }
        function stopAnim() { if (_anim) { cancelAnimationFrame(_anim.raf); _anim = null; } }
        function scrollToSection(el, smooth) {
          stopAnim();
          measure();
          if (!smooth) { scrollToY(targetY(el)); return; }
          var startY = scrollTop();
          var dist = Math.abs(targetY(el) - startY);
          if (dist < 1) return;
          var duration = Math.min(900, Math.max(350, 250 + dist * 0.25));
          var t0 = null;
          var anim = _anim = { raf: 0 };
          function frame(now) {
            if (_anim !== anim || !el.isConnected) return;
            if (t0 === null) t0 = now;
            var p = Math.min(1, (now - t0) / duration);
            var y = startY + (targetY(el) - startY) * ease(p);
            scrollToY(y);
            if (p < 1) { anim.raf = requestAnimationFrame(frame); return; }
            _anim = null;
          }
          anim.raf = requestAnimationFrame(frame);
        }

        function go(hash, smooth) {
          if (!hash || !nav.querySelector('a[href="' + hash + '"]')) return false;
          var el = root.querySelector(hash);
          if (!el) return false;
          setActive(hash);
          highlightTarget(el);
          scrollToSection(el, smooth);
          return true;
        }

        measure();
        if (window.ResizeObserver) {
          var ro = new ResizeObserver(function() {
            if (!nav.isConnected) { ro.disconnect(); return; }
            measure();
          });
          ro.observe(nav);
          ro.observe(root);
        }
        // Sidebar open/close: the body class flips when the slide ends; the event announces the new width up front
        var bodyMo = new MutationObserver(function() {
          if (!nav.isConnected) { bodyMo.disconnect(); return; }
          measure();
        });
        bodyMo.observe(document.body, { attributes: true, attributeFilter: ['class'] });

        if (sc) {
          var scQueued = false;
          sc.addEventListener('scroll', function() {
            if (scQueued) return;
            scQueued = true;
            requestAnimationFrame(function() { scQueued = false; spy(); });
          }, { passive: true });
        }

        // Direct load with #hash: jump there (after a tick too: AJAX navigation sets the URL after this runs)
        if (go(location.hash, false)) _lock = location.hash; else spy();
        setTimeout(function() { if (location.hash && root.querySelector(location.hash) && !_anim) go(location.hash, false); else spy(); }, 50);

        nav.addEventListener('click', function(e) {
          var link = e.target.closest('a[href^="#"]');
          if (!link) return;
          e.preventDefault();
          // Keep the site's global link handler out: it would scroll same-page #links itself (centred),
          // which fights this scroll and, on the already active tab, lands on a different section
          e.stopPropagation();
          var href = link.getAttribute('href');
          if (location.hash !== href) history.replaceState(history.state, '', href);
          _lock = href;
          go(href, true);
        });

        // Window listeners are bound once (AJAX navigation re-runs this script) and delegate to the
        // current page instance. User input cancels a running animation so it never fights the user.
        window._settingsQn = {
          cancel: function(e) {
            // Input on the quicknav itself is the click that is about to start a new scroll
            if (e && e.target && e.target.closest && e.target.closest('#settings-quicknav')) return;
            var had = _anim || _lock;
            _lock = null;
            if (_anim) { stopAnim(); highlightTarget(null); }
            if (had) spy();
          },
          spy: spy,
          resize: measure,
          sidebarLayout: function(e) {
            var d = (e && e.detail) || {};
            rightOverride = {
              right: d.open && !d.overlay ? Math.min(Math.round(d.width || 0), Math.round(window.innerWidth / 2)) : 0,
              until: performance.now() + 800
            };
            measure();
          }
        };
        if (!window._settingsQnBound) {
          window._settingsQnBound = true;
          var q = function(fn) { return function(e) { var s = window._settingsQn; if (s && s[fn]) s[fn](e); }; };
          ['wheel', 'touchstart', 'keydown', 'mousedown'].forEach(function(ev) {
            window.addEventListener(ev, q('cancel'), { passive: true });
          });
          var spyQueued = false;
          window.addEventListener('scroll', function() {
            if (spyQueued) return;
            spyQueued = true;
            requestAnimationFrame(function() { spyQueued = false; q('spy')(); });
          }, { passive: true });
          window.addEventListener('resize', q('resize'), { passive: true });
          window.addEventListener('f0ck:sidebar-right-layout', function(e) { q('sidebarLayout')(e); });
        }
      })();
