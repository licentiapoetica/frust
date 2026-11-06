/*
 * f0ck-core.js: the shared part of the f0ckm client, used by every frontend.
 *
 * The default frontend loads it right before f0ckm.js; the alternative frontends (views/frontends/*)
 * load it on every page through their shell. It holds what the shared page scripts (messages.js,
 * admin.js, settings.js, comments.js, ...) expect from the page, so they work in any frontend:
 *
 *   getCsrfToken, escHTML, f0ckTimeAgo, f0ckFormatDateFull, updateGlobalTimestamps, ModAction,
 *   f0ckPasskeyLogin (WebAuthn login of registered users),
 *   F0ckLive (the live WebSocket /api/notifications/ws: tab id, reconnect, active pings)
 *
 * f0ckCommentFx (comment text effects [rb] [reich] [krebs] [glitch], see the end of the file),
 * f0ckLoadEmojis / f0ckEmojify (custom emojis :name: in comments, for the frontends' renderers),
 * updateBrandImage / updateBrandText / updateBrandStyle / applyBrandEvent (the navbar brand, see below),
 * and fallbacks for what f0ckm.js provides on the default frontend (showFlash, flashMessage,
 * loadPageAjax, updateMotdUI). A fallback is only set when nothing defined the
 * name yet; f0ckm.js (loaded after this file) and the frontends' own scripts replace them. The
 * fallbacks call the frontend's hooks when it registered them:
 *
 *   window.f0ckFrontend = { flash(message, type), motd(text), brand(url, text) }
 *
 * Frontends get live events with F0ckLive.onMessage(data => ...) and start the connection with
 * F0ckLive.start() (connect + reconnect when the tab becomes visible or focused + active pings).
 * f0ckm.js registers its own handlers and calls F0ckLive.connect()/ping() itself.
 *
 * Contract for frontends: docs in views/frontends/README.md.
 */

if (typeof window.f0ckDebug !== 'function') {
  window.f0ckDebug = (...args) => {
    if (window.f0ckSession?.development) console.log(...args);
  };
}

window.getCsrfToken = () => {
  return (window.f0ckSession && window.f0ckSession.csrf_token) ||
         document.querySelector('meta[name="csrf-token"]')?.getAttribute('content') ||
         document.querySelector('input[name="csrf_token"]')?.value ||
         '';
};

window.escHTML = (str) => {
  if (!str) return '';
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
};

(() => {
  // Global Timeago — uses Intl.RelativeTimeFormat for native, correctly-localized output
  const timeAgo = (date) => {
      const i18n = window.f0ckI18n || {};
      const lang = i18n.lang || navigator.language || 'en';
      const seconds = Math.floor((new Date() - new Date(date)) / 1000);

      if (seconds < 5) return i18n.timeago_just_now || 'just now';

      const intervals = [
          { unit: 'year',   seconds: 31536000 },
          { unit: 'month',  seconds: 2592000  },
          { unit: 'week',   seconds: 604800   },
          { unit: 'day',    seconds: 86400    },
          { unit: 'hour',   seconds: 3600     },
          { unit: 'minute', seconds: 60       },
          { unit: 'second', seconds: 1        }
      ];

      for (const interval of intervals) {
          const count = Math.floor(seconds / interval.seconds);
          if (count >= 1) {
              try {
                  // Force fallback for custom/unrecognized locales like 'zange'
                  if (lang === 'zange') throw new Error('Force fallback');
                  const rtf = new Intl.RelativeTimeFormat(lang, { numeric: 'auto' });
                  return rtf.format(-count, interval.unit);
              } catch (e) {
                  // Intl not available — fall back to i18n strings
                  const key = count === 1 ? `timeago_${interval.unit}` : `timeago_${interval.unit}s`;
                  const tpl = i18n[key] || `{n} ${interval.unit}${count !== 1 ? 's' : ''}`;
                  const timeStr = tpl.replace('{n}', count).replace('{s}', count !== 1 ? 's' : '');
                  const agoTpl = i18n.timeago_ago || '{t} ago';
                  return agoTpl.replace('{t}', timeStr);
              }
          }
      }
      return i18n.timeago_just_now || 'just now';
  };

  // Expose globally so comments.js, user_comments.js, messages.js etc. can use the same i18n-aware implementation
  window.f0ckTimeAgo = timeAgo;

  // Format an ISO date string as DD.MM.YYYY - HH:MM:SS in local time
  const formatDateFull = (dateStr) => {
      const d = new Date(dateStr);
      if (isNaN(d.getTime())) return dateStr;
      const pad = n => String(n).padStart(2, '0');
      return `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.${d.getFullYear()} - ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  };
  window.f0ckFormatDateFull = formatDateFull;

  const updateGlobalTimestamps = () => {
      document.querySelectorAll('.timeago').forEach(el => {
          // On first run, seed data-iso from the server-rendered tooltip (raw ISO string)
          // so we always have a reliable source for time calculations even after the
          // tooltip is rewritten to the human-readable DD.MM.YYYY format.
          if (!el.dataset.iso && el.hasAttribute('tooltip')) {
              el.dataset.iso = el.getAttribute('tooltip');
          }

          // Use data-iso (seeded above) → data-ts (sidebar) → tooltip as fallback
          const isoStr = el.dataset.iso || el.getAttribute('data-ts');
          if (isoStr) {
              const newTime = timeAgo(isoStr);
              if (el.textContent !== newTime) {
                  el.textContent = newTime;
              }
              // Update tooltip to show nicely formatted local date instead of raw ISO string
              if (el.hasAttribute('tooltip')) {
                  const niceFull = formatDateFull(isoStr);
                  if (el.getAttribute('tooltip') !== niceFull) {
                      el.setAttribute('tooltip', niceFull);
                  }
              }
          }
      });

      // Also reformat tooltip on static date elements (e.g. .stat-joined on profiles)
      // that carry a data-iso attribute but are NOT .timeago (text content not updated).
      document.querySelectorAll('[data-iso][tooltip]:not(.timeago)').forEach(el => {
          const isoStr = el.dataset.iso;
          if (isoStr) {
              const niceFull = formatDateFull(isoStr);
              if (el.getAttribute('tooltip') !== niceFull) {
                  el.setAttribute('tooltip', niceFull);
              }
          }
      });
  };
  // Expose globally so AJAX content hooks can re-stamp immediately after injection
  window.updateGlobalTimestamps = updateGlobalTimestamps;
})();

// Mod Action Modal
class ModAction {
  static confirm(title, promptHtml, callback, options = {}) {
    const modal = document.getElementById('mod-action-modal');
    if (!modal) return window.flashMessage('Error: Mod modal not found', 3000, 'error');
    const titleEl = document.getElementById('mod-action-title');
    const contentEl = document.getElementById('mod-action-content');
    const reasonEl = document.getElementById('mod-reason');
    const confirmBtn = document.getElementById('mod-action-confirm');
    const cancelBtn = document.getElementById('mod-action-cancel');
    const errorEl = document.getElementById('mod-action-error');

    const singleLine = options.singleLine || false;
    const hideReason = options.hideReason || false;
    const allowEmpty = options.allowEmpty || false;
    const i18n = window.f0ckI18n || {};

    titleEl.innerText = title;
    if (options.unsafeContent) {
      contentEl.innerHTML = promptHtml; // Trusted admin-only content — skip sanitizer
    } else {
      contentEl.innerHTML = Sanitizer.clean(promptHtml);
    }

    reasonEl.value = '';
    errorEl.style.display = 'none';
    modal.style.display = 'flex';

    // Apply single-line mode: style the textarea to look and act like a text input
    if (singleLine) {
      reasonEl.rows = 1;
      reasonEl.style.resize = 'none';
      reasonEl.style.overflow = 'hidden';
      reasonEl.style.height = 'auto';
    } else {
      reasonEl.rows = 3;
      reasonEl.style.resize = '';
      reasonEl.style.overflow = '';
      reasonEl.style.height = '';
    }

    if (hideReason) {
      reasonEl.style.display = 'none';
    } else {
      reasonEl.style.display = 'block';
      reasonEl.placeholder = options.placeholder ||
        (allowEmpty ? (i18n.reason_optional || 'Reason (optional)') : (i18n.reason_required_label || 'Reason (required)'));
      reasonEl.focus();
    }

    confirmBtn.innerText = options.confirmText || (hideReason ? (i18n.confirm_yes || 'Yes') : (i18n.confirm_btn || 'Confirm'));
    cancelBtn.innerText = options.cancelText || (hideReason ? (i18n.confirm_no || 'No') : (i18n.cancel_btn || 'Cancel'));

    confirmBtn.style.display = options.hideConfirm ? 'none' : '';
    cancelBtn.style.display = options.hideCancel ? 'none' : '';

    const close = () => {
      modal.style.display = 'none';
      cleanup();
    };

    const onConfirm = async () => {
      const reason = reasonEl.value.trim();
      if (!hideReason && !allowEmpty && !reason) {
        errorEl.innerText = (window.f0ckI18n && window.f0ckI18n.reason_required) || 'Reason is required.';
        errorEl.style.display = 'block';
        return;
      }
      errorEl.style.display = 'none';
      confirmBtn.disabled = true;
      confirmBtn.innerText = (window.f0ckI18n && window.f0ckI18n.processing) || 'Processing...';

      try {
        await callback(hideReason ? null : reason);
        close();
      } catch (e) {
        errorEl.innerText = e.message || 'Error occurred';
        errorEl.style.display = 'block';
        confirmBtn.disabled = false;
        confirmBtn.innerText = options.confirmText || (hideReason ? (i18n.confirm_yes || 'Yes') : (i18n.confirm_btn || 'Confirm'));
      }
    };

    // Block Enter from inserting newlines in single-line mode; instead submit
    const enterHandler = singleLine ? (e) => {
      if (e.key === 'Enter') { e.preventDefault(); onConfirm(); }
    } : null;
    if (enterHandler) reasonEl.addEventListener('keydown', enterHandler);

    const cleanup = () => {
      confirmBtn.onclick = null;
      cancelBtn.onclick = null;
      confirmBtn.style.display = '';
      cancelBtn.style.display = '';
      if (enterHandler) reasonEl.removeEventListener('keydown', enterHandler);
      confirmBtn.disabled = false;
    };

    confirmBtn.onclick = onConfirm;
    cancelBtn.onclick = close;

    modal.onclick = (e) => {
      if (e.target === modal) close();
    };
  }
}
window.ModAction = ModAction;

// ── Passkey login (registered users) ─────────────────────────────────────────────────────
// The WebAuthn round trip of /api/v2/settings/passkeys/login/{begin,finish}. Resolves when the
// session is set (reload the page then), rejects with an Error whose message can be shown.
// Each frontend has its own button for it.
window.f0ckPasskeyLogin = async () => {
  if (!window.PublicKeyCredential || !navigator.credentials) throw new Error('Passkeys are not supported in this browser.');
  const b64urlToArr = (s) => Uint8Array.from(atob(s.split('-').join('+').split('_').join('/')), (c) => c.charCodeAt(0));
  const arrToB64url = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf))).split('+').join('-').split('/').join('_').replace(/=+$/, '');
  const beginRes = await fetch('/api/v2/settings/passkeys/login/begin', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
  const beginData = await beginRes.json();
  if (!beginData.success) throw new Error(beginData.msg || 'Server error');
  const opts = beginData.options;
  let cred;
  try {
    cred = await navigator.credentials.get({ publicKey: {
      challenge: b64urlToArr(opts.challenge),
      rpId: opts.rpId,
      allowCredentials: (opts.allowCredentials || []).map((c) => ({ type: c.type, id: b64urlToArr(c.id) })),
      userVerification: opts.userVerification || 'preferred',
      timeout: opts.timeout || 60000,
    } });
  } catch (e) {
    if (e && e.name === 'NotAllowedError') {
      throw new Error('Passkey request was cancelled or blocked. If Bitwarden or another password manager is locked, please unlock it or check extension permissions.');
    }
    throw e;
  }
  const finishRes = await fetch('/api/v2/settings/passkeys/login/finish', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      challenge: opts.challenge,
      credentialId: arrToB64url(cred.rawId),
      clientDataJSON: arrToB64url(cred.response.clientDataJSON),
      authenticatorData: arrToB64url(cred.response.authenticatorData),
      signature: arrToB64url(cred.response.signature),
      userHandle: cred.response.userHandle ? arrToB64url(cred.response.userHandle) : null,
    }),
  });
  const finishData = await finishRes.json();
  if (!finishData.success) throw new Error(finishData.msg || 'Authentication failed');
};

// ── Live connection (/api/notifications/ws) ───────────────────────────────────────────────
// One socket per tab. The server knows the tab by tabId (kept per tab in sessionStorage) and uses
// the connection for notifications, DMs, presence and live page updates.
window.F0ckLive = (() => {
  const messageHandlers = [];
  const openHandlers = [];
  const maxRetries = 20;
  let ws = null;
  let retryCount = 0;
  let retryTimer = null;
  let started = false;

  let tabId = null;
  try {
    tabId = sessionStorage.getItem('f0ck_tab_id');
    if (!tabId) {
      tabId = Math.random().toString(36).slice(2);
      sessionStorage.setItem('f0ck_tab_id', tabId);
    }
  } catch (e) {
    tabId = Math.random().toString(36).slice(2);
  }

  const url = () => {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    let u = `${proto}//${location.host}/api/notifications/ws?tabId=${tabId}`;
    try {
      const fp = localStorage.getItem('f0ck_anon_ssh_fp');
      if (fp) u += `&fp=${encodeURIComponent(fp)}`;
      const hw = localStorage.getItem('f0ck_anon_hw_fp') || window.f0ckAnonSSH?.hwFingerprint;
      if (hw) u += `&hw=${encodeURIComponent(hw)}`;
    } catch (e) {}
    return u;
  };

  const connect = () => {
    if (ws) {
      // Detach first: close() fires onclose, which would schedule a retry
      ws.onclose = null;
      ws.close();
    }
    clearTimeout(retryTimer);
    window.f0ckDebug(`[F0ckLive] Opening live socket (tabId: ${tabId})...`);
    const sock = new WebSocket(url());
    ws = sock;
    sock.onopen = () => {
      window.f0ckDebug('[F0ckLive] Live socket connected');
      retryCount = 0;
      document.documentElement.dataset.sseReady = '1';
      document.dispatchEvent(new CustomEvent('f0ck:sse_ready'));
      openHandlers.forEach((fn) => { try { fn(); } catch (err) { console.error('[F0ckLive] open handler', err); } });
    };
    sock.onmessage = (e) => {
      let data;
      try { data = JSON.parse(e.data); } catch (err) { console.error('[F0ckLive] bad message', err); return; }
      window.f0ckDebug('[F0ckLive] Received message:', data.type);
      messageHandlers.forEach((fn) => { try { fn(data); } catch (err) { console.error('[F0ckLive] handler', err); } });
    };
    // onerror is always followed by onclose, so reconnecting is handled there only
    sock.onclose = (ev) => {
      if (ws !== sock) return; // replaced by a newer socket
      console.warn('Notification socket closed', ev.code, ev.reason);
      ws = null;
      // Tab hidden: don't retry now, ensure() reconnects when it is visible again
      if (document.hidden) {
        window.f0ckDebug('[F0ckLive] Tab hidden, deferring socket retry until visible.');
        return;
      }
      // Exponential backoff, capped at 30s; past maxRetries keep trying every 30s
      const delay = Math.min(Math.pow(2, retryCount) * 1000, 30000);
      if (retryCount < maxRetries) {
        window.f0ckDebug(`[F0ckLive] Reconnecting socket in ${delay}ms... (attempt ${retryCount + 1}/${maxRetries})`);
        retryTimer = setTimeout(connect, delay);
        retryCount++;
      } else {
        console.warn('[F0ckLive] Max socket retries reached, retrying every 30s.');
        retryCount = 0;
        retryTimer = setTimeout(connect, 30000);
      }
    };
  };

  const isOpen = () => !!ws && ws.readyState < WebSocket.CLOSING;

  // Reconnect if the socket died (e.g. while the tab was hidden)
  const ensure = () => {
    if (!isOpen()) {
      retryCount = 0;
      connect();
    }
  };

  // Tell the server this tab is the active one (online status, where to deliver) and revive the socket
  const ping = () => {
    if (document.hidden) return;
    fetch(`/api/notifications/active?tabId=${tabId}`).catch(() => {});
    ensure();
  };

  // For frontends without f0ckm.js: connect and keep the tab marked active
  const start = () => {
    if (started) return;
    started = true;
    connect();
    document.addEventListener('visibilitychange', () => { if (!document.hidden) ping(); });
    window.addEventListener('focus', ping);
  };

  return {
    get tabId() { return tabId; },
    get ws() { return ws; },
    onMessage(fn) { messageHandlers.push(fn); },
    onOpen(fn) { openHandlers.push(fn); },
    connect, ensure, ping, isOpen, start,
  };
})();

// ── Fallbacks for frontends without f0ckm.js ──────────────────────────────────────────────
(() => {
  const fe = () => window.f0ckFrontend || {};
  const def = (name, fn) => { if (typeof window[name] === 'undefined') window[name] = fn; };

  // A plain toast when the frontend has no flash hook (styled in /s/css/f0ck-shell.css)
  const toast = (message, type) => {
    let box = document.getElementById('f0ck-core-flash');
    if (!box) {
      box = document.createElement('div');
      box.id = 'f0ck-core-flash';
      document.body.appendChild(box);
    }
    const el = document.createElement('div');
    el.className = `f0ck-core-toast f0ck-core-toast-${type || 'info'}`;
    el.textContent = String(message);
    box.appendChild(el);
    setTimeout(() => el.remove(), 4000);
  };
  const flash = (message, type) => {
    if (typeof fe().flash === 'function') fe().flash(message, type || 'info');
    else toast(message, type);
  };

  def('showFlash', (message, type = 'success') => flash(message, type));
  def('flashMessage', (message, _duration, type = 'info') => flash(message, type));
  // No AJAX navigation outside the default frontend: a normal page load
  def('loadPageAjax', (url) => { location.assign(url || location.href); return Promise.resolve(); });
  def('updateMotdUI', (text) => { if (typeof fe().motd === 'function') fe().motd(text); });
})();

// ── Navbar brand ──────────────────────────────────────────────────────────────────────────
// The image and the text set in the admin panel (live event brand_image, admin.js after a save).
// Every frontend that shows them marks them inside .navbar-brand: <img data-brand-img> and
// <span data-brand-text data-text="admin text" data-fallback="site name">. The text sits next to
// the image; with no image and no text the site name shows. The frontend's brand hook, if any, is
// called as well.
(() => {
  const each = (fn) => document.querySelectorAll('.navbar-brand').forEach((a) => {
    const img = a.querySelector('[data-brand-img]');
    const txt = a.querySelector('[data-brand-text]');
    if (img && txt) fn(img, txt);
  });
  const showText = (img, txt) => {
    const text = txt.dataset.text || (img.hidden ? (txt.dataset.fallback || '') : '');
    txt.textContent = text;
    txt.hidden = !text;
  };
  const hook = (data) => {
    const fe = window.f0ckFrontend;
    if (fe && typeof fe.brand === 'function') fe.brand(data.url, data.text);
  };

  window.updateBrandImage = (url) => {
    window.f0ckBrandImages = url ? [url] : [];
    each((img, txt) => {
      if (url) img.src = url;
      else img.removeAttribute('src');
      img.hidden = !url;
      showText(img, txt);
    });
    window.syncNavbarHeight?.();
    hook({ url: url || null });
  };
  window.updateBrandText = (text) => {
    each((img, txt) => {
      txt.dataset.text = text || '';
      img.alt = text || txt.dataset.fallback || '';
      showText(img, txt);
    });
    window.syncNavbarHeight?.();
    hook({ text: text || '' });
  };
  // Case ("" / "upper" / "lower"), font (font_css: the @font-face stylesheet, "" for the
  // frontend's own font), size (a percent, null: the frontend's own; left alone when undefined)
  // and weight (one of the font's weights, null: the frontend's own; left alone when undefined)
  // of the brand text: classes from the frontend's CSS
  window.updateBrandStyle = (kase, fontCss, size, weight) => {
    let link = document.getElementById('brand-font-css');
    if (fontCss) {
      if (!link) {
        link = document.createElement('link');
        link.rel = 'stylesheet';
        link.id = 'brand-font-css';
        document.head.appendChild(link);
      }
      if (link.getAttribute('href') !== fontCss) link.setAttribute('href', fontCss);
    } else if (link) link.remove();
    each((img, txt) => {
      txt.classList.toggle('brand-case-upper', kase === 'upper');
      txt.classList.toggle('brand-case-lower', kase === 'lower');
      txt.classList.toggle('brand-font', !!fontCss);
      if (size !== undefined) {
        Array.from(txt.classList).filter((c) => c.startsWith('brand-size-')).forEach((c) => txt.classList.remove(c));
        if (size) txt.classList.add('brand-size-' + size);
      }
      if (weight !== undefined) {
        Array.from(txt.classList).filter((c) => c.startsWith('brand-weight-')).forEach((c) => txt.classList.remove(c));
        if (weight && fontCss) txt.classList.add('brand-weight-' + weight);
      }
    });
    window.syncNavbarHeight?.();
  };
  // Live event payload: only the changed parts
  window.applyBrandEvent = (data) => {
    if (!data) return;
    if ('url' in data) window.updateBrandImage(data.url || null);
    if ('text' in data) window.updateBrandText(data.text || '');
    if ('case' in data || 'font_css' in data || 'size' in data || 'weight' in data) {
      window.updateBrandStyle(data.case || '', data.font_css || '', 'size' in data ? data.size : undefined, 'weight' in data ? data.weight : undefined);
    }
  };
})();

// ── Comment text effects ────────────────────────────────────────────────────────────────────
// [rb] [reich] [krebs] [glitch]: w0bm's text modifiers, so a comment looks the same in every
// frontend. Runs on the renderer's already escaped HTML; only these spans are added. Styles:
// /s/css/comment-fx.css; [glitch] carries data-glitch, which /s/js/glitchfx.js animates (it picks up
// comments added later by itself). Server copy for the no-JavaScript comments: comment_fx in
// server/src/routes/comments.rs. The w0bm frontend keeps its own classes for the first three.
window.F0CK_COMMENT_FX = {
  rb: 'class="cfx-rb"',
  reich: 'class="cfx-reich"',
  krebs: 'class="cfx-krebs"',
  glitch: 'class="cfx-glitch" data-glitch="0.5"',
};
window.f0ckCommentFx = (html) => {
  let s = String(html ?? '');
  for (const [tag, attrs] of Object.entries(window.F0CK_COMMENT_FX)) {
    if (!s.toLowerCase().includes('[/' + tag + ']')) continue;
    // innermost first, so nested tags work (up to 10 levels)
    const re = new RegExp('\\[' + tag + '\\]((?:(?!\\[' + tag + '\\])[\\s\\S])*?)\\[\\/' + tag + '\\]', 'gi');
    for (let i = 0; i < 10; i++) {
      const n = s.replace(re, '<span ' + attrs + '>$1</span>');
      if (n === s) break;
      s = n;
    }
  }
  return s;
};

// ── Links to this site ──────────────────────────────────────────────────────────────────────
// Comment/message renderers show a full URL to this site as its path: https://f0ck.me/123-slug
// becomes /123-slug (link and text). "This site" is the host being browsed or the configured
// domain, with or without www. and any scheme. f0ckSitePath(href) is that path or null;
// f0ckSiteLink(href, text) gives { href, text, internal } for a renderer's link, shortening the
// text only when it is the URL itself (not for [custom text](url)). Server copy for visitors
// without JavaScript: site_path in server/src/routes/comments.rs.
window.f0ckSitePath = (href) => {
  const m = /^(?:https?:)?\/\/([^/?#\s]+)([^\s]*)$/i.exec(String(href || ''));
  if (!m) return null;
  const host = (h) => String(h || '').toLowerCase().replace(/^www\./, '');
  const h = host(m[1]);
  if (!h || ![location.host, location.hostname, window.f0ckDomain].some(o => o && host(o) === h)) return null;
  const path = m[2] || '/';
  return path.startsWith('/') ? path : '/' + path;
};
window.f0ckSiteLink = (href, text) => {
  href = String(href || '');
  const path = window.f0ckSitePath(href);
  if (path) {
    const bare = href.replace(/^(?:https?:)?\/\//i, '');
    return { href: path, text: (text === href || text === bare) ? path : text, internal: true };
  }
  return { href, text, internal: href.startsWith('/') && !href.startsWith('//') };
};

// ── Custom emojis ───────────────────────────────────────────────────────────────────────────
// :name: in comments, for the alternative frontends' comment renderers (f0ckm.js has its own).
// f0ckLoadEmojis() loads /api/v2/emojis once and resolves to { name: url }; it reloads after the
// live event emojis_updated. f0ckEmojify(html, map) swaps known names in escaped HTML for <img>.
(() => {
  let loading = null;
  window.f0ckLoadEmojis = () => {
    if (!loading) {
      loading = fetch('/api/v2/emojis', { credentials: 'same-origin' })
        .then((r) => r.json())
        .then((d) => {
          const map = {};
          (d.emojis || []).forEach((e) => { if (e.name && e.url) map[e.name] = e.url; });
          return map;
        })
        .catch(() => { loading = null; return {}; });
    }
    return loading;
  };
  window.addEventListener('f0ck:emojis_updated', () => { loading = null; });
  window.f0ckEmojify = (html, map) => {
    if (!map) return html;
    return String(html ?? '').replace(/:([a-z0-9_\-]+):/g, (m, n) => (map[n]
      ? `<img class="f0ck-emoji" src="${window.escHTML(map[n]).replace(/"/g, '&quot;')}" alt=":${n}:" title=":${n}:" loading="lazy">`
      : m));
  };
})();

// ── Page swaps of the alternative frontends ─────────────────────────────────────────────────────
// w0bm.js and f0ckv2's f0ck.js change pages without reloading: they fetch the next page, swap the
// page part of <body> (everything before their anchor: #w0bm-flash / #footer-data) and use these for
// what differs between their own pages and the shared pages in their shell (views/frontends/README.md):
//   styles(doc, anchor) the stylesheets of <head>: adds the new page's (f0ck-shell.css and the
//                     shell's CSS on a shared page), waits for them; returns a function that removes
//                     the ones the new page doesn't have, and the tail elements it doesn't have (call
//                     it after the swap: no unstyled moment). Tail elements: what tail() added, and
//                     the [data-swap-tail] ones the page loaded first has after the anchor (a shared
//                     page loaded first brings the mod dialog; it mustn't stay, unstyled, on the
//                     frontend's own pages). Not the rest: scripts add their own elements there.
//   scripts(list)     runs the scripts of the swapped-in part in order: page scripts every time
//                     (/s/js/views/*, data-v2-page, anything inside a shared page, like f0ckm.js does
//                     with #main), libraries only the first time
//   tail(doc, anchor) what the new page has after the anchor and this one lacks (a shared page's mod
//                     dialog, admin.js, mention_autocomplete.js, ...): elements added (and removed
//                     again on a page without them, see styles), scripts run once
//   shown()           the shared scripts' re-init after a swap, as after f0ckm's AJAX loads
//                     (initMessagesPage, f0ck:contentLoaded)
window.F0ckPageSwap = (() => {
  // scripts that ran, by path (a library whose ?v= changed after a deploy still mustn't run twice).
  // The page's own scripts count once it is parsed: the ones after f0ck-core.js (upload.js,
  // admin.js, ...) aren't in the document yet when this runs.
  const srcKey = (src) => { try { return new URL(src, location.href).pathname; } catch { return src; } };
  const loaded = new Set();
  const known = () => { for (const s of document.scripts) if (s.src) loaded.add(srcKey(s.src)); };
  known();
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', known, { once: true });
  // elements tail() added (and the first page's tail, see styles): they go again on a page that
  // doesn't have them
  let extras = [];
  let adopted = false;
  const adoptTail = (anchorId) => {
    if (adopted || !anchorId) return;
    const anchor = document.getElementById(anchorId);
    if (!anchor) return;
    adopted = true;
    for (let n = anchor.nextElementSibling; n; n = n.nextElementSibling) {
      if (n.id && n.hasAttribute('data-swap-tail')) extras.push(n);
    }
  };
  const sheetKey = (l) => { try { return new URL(l.getAttribute('href'), location.href).pathname; } catch { return l.getAttribute('href') || ''; } };
  const sheets = (root) => [...root.querySelectorAll('link[rel~="stylesheet"][href]')];
  const waitSheet = (l) => new Promise((res) => {
    l.addEventListener('load', res, { once: true });
    l.addEventListener('error', res, { once: true });
    setTimeout(res, 2500);
  });

  const styles = async (doc, anchorId) => {
    adoptTail(anchorId);
    const want = sheets(doc.head);
    const have = sheets(document.head);
    const haveKeys = new Set(have.map(sheetKey));
    const wantKeys = new Set(want.map(sheetKey));
    const waits = [];
    want.forEach((l) => {
      if (haveKeys.has(sheetKey(l))) return;
      const n = document.createElement('link');
      [...l.attributes].forEach((a) => n.setAttribute(a.name, a.value));
      document.head.appendChild(n);
      if (!n.disabled && !/alternate/i.test(n.rel)) waits.push(waitSheet(n));
    });
    await Promise.all(waits);
    return () => {
      have.forEach((l) => { if (!wantKeys.has(sheetKey(l))) l.remove(); });
      const docSheets = new Set(sheets(doc).map(sheetKey));
      extras = extras.filter((el) => {
        const keep = el.tagName === 'LINK' ? docSheets.has(sheetKey(el)) : !!(el.id && doc.getElementById(el.id));
        if (!keep) el.remove();
        return keep;
      });
    };
  };

  const executable = (s) => {
    const type = (s.getAttribute('type') || '').trim().toLowerCase();
    return !type || ['text/javascript', 'application/javascript', 'module'].includes(type);
  };
  const isPageScript = (s) => s.hasAttribute('data-v2-page') || !!s.closest?.('.f0ck-shell-page')
    || /^\/s\/js\/views\//.test(new URL(s.src, location.href).pathname);
  const run = (old) => new Promise((res) => {
    const s = document.createElement('script');
    [...old.attributes].forEach((a) => { if (a.name !== 'nonce') s.setAttribute(a.name, a.value); });
    s.async = false;
    s.addEventListener('load', res, { once: true });
    s.addEventListener('error', res, { once: true });
    old.replaceWith(s);
    loaded.add(srcKey(s.src));
  });
  // (only scripts with a src: the templates have no inline scripts)
  const scripts = async (list) => {
    for (const old of list) {
      if (!old.isConnected || !executable(old) || !old.src) continue;
      const page = isPageScript(old);
      if (loaded.has(srcKey(old.src)) && !page) { old.remove(); continue; }
      await run(old);
    }
  };

  const tail = async (doc, anchorId) => {
    const anchor = doc.getElementById(anchorId);
    if (!anchor) return;
    const after = [];
    for (let n = anchor.nextSibling; n; n = n.nextSibling) after.push(n);
    const haveSheets = new Set(sheets(document).map(sheetKey));
    const todo = [];
    after.forEach((n) => {
      if (n.nodeType !== 1) return;
      if (n.tagName === 'SCRIPT') {
        if (executable(n) && n.src && !loaded.has(srcKey(n.src))) {
          const s = document.importNode(n, false);
          document.body.appendChild(s);
          todo.push(s);
        }
        return;
      }
      if (n.tagName === 'LINK') {
        if (/stylesheet/i.test(n.rel) && !haveSheets.has(sheetKey(n))) {
          const el = document.importNode(n, true);
          document.body.appendChild(el);
          extras.push(el);
        }
        return;
      }
      if (n.id && !document.getElementById(n.id)) {
        const el = document.importNode(n, true);
        el.querySelectorAll('script').forEach((x) => x.remove());
        document.body.appendChild(el);
        extras.push(el);
      }
    });
    for (const s of todo) await run(s);
  };

  const shown = () => {
    if ((document.getElementById('dm-inbox-list') || document.getElementById('dm-thread')) && typeof window.initMessagesPage === 'function') {
      try { window.initMessagesPage(); } catch (e) { console.error('[F0ckPageSwap] initMessagesPage', e); }
    }
    window.F0ckActions?.scan?.(document);
    document.dispatchEvent(new Event('f0ck:contentLoaded'));
  };

  return { styles, scripts, tail, shown, loaded };
})();
