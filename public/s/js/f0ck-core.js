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
 * the exclude-tag buttons of the frontends' tag lists (f0ckAddExcludeButton, f0ckSyncExcluded, see below),
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
// The WebAuthn round trip of /api/v2/settings/passkeys/{login,confirm}/{begin,finish}: resolves when
// the server accepted the passkey, rejects with an Error whose message can be shown.
const f0ckPasskeyAssert = async (base, headers) => {
  if (!window.PublicKeyCredential || !navigator.credentials) throw new Error('Passkeys are not supported in this browser.');
  const b64urlToArr = (s) => Uint8Array.from(atob(s.split('-').join('+').split('_').join('/')), (c) => c.charCodeAt(0));
  const arrToB64url = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf))).split('+').join('-').split('/').join('_').replace(/=+$/, '');
  const hdrs = Object.assign({ 'Content-Type': 'application/json' }, headers || {});
  const beginRes = await fetch(base + '/begin', { method: 'POST', headers: hdrs, body: '{}' });
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
  const finishRes = await fetch(base + '/finish', {
    method: 'POST',
    headers: hdrs,
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

// Login: the session is set when this resolves (reload the page then). Each frontend has its own
// button for it.
window.f0ckPasskeyLogin = () => f0ckPasskeyAssert('/api/v2/settings/passkeys/login');

// Confirm the signed-in owner with one of the account's passkeys (account changes of passkey-only
// accounts, which have no password to give).
window.f0ckPasskeyConfirm = () => {
  const csrf = (window.f0ckSession && window.f0ckSession.csrf_token) || document.querySelector('meta[name="csrf-token"]')?.content || '';
  return f0ckPasskeyAssert('/api/v2/settings/passkeys/confirm', { 'X-CSRF-Token': csrf });
};

// `doFetch()` returns a fetch promise. When the server answers `{ reauth: "passkey" }`, confirm with
// a passkey and send the request once more. Resolves with the JSON answer.
window.f0ckWithReauth = async (doFetch) => {
  let data = await (await doFetch()).json();
  if (data && data.reauth === 'passkey') {
    await window.f0ckPasskeyConfirm();
    data = await (await doFetch()).json();
  }
  return data;
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

// ── Exclude-tag buttons (alternative frontends) ───────────────────────────────────────────
// Next to each tag of an item: a plain form, so it also works without JavaScript (nojs.rs sends
// the visitor back). It posts the wanted state: `remove=1` when the tag is excluded already.
//   <span data-tag="…" class="… [f0ck-tag-excluded]">
//     <a>tag</a>
//     <form method="post" action="/api/v2/settings/excluded_tags" data-tag-id="…"
//           class="f0ck-exclude-form [is-excluded]">
//       csrf_token, back, tag_id, [remove=1]
//       <button type="submit" class="f0ck-exclude-btn"><… class="f0ck-excl-off"><… class="f0ck-excl-on"></button>
//     </form>
//   </span>
// With JavaScript the post goes through fetch and every such form on the page gets the new state.
// The frontend's CSS shows .f0ck-excl-off or .f0ck-excl-on (.is-excluded). Tag lists rebuilt by a
// frontend's script get the button from f0ckAddExcludeButton(span, tag, { off, on }).
(() => {
  const RATINGS = ['sfw', 'nsfw', 'nsfl'];
  const i18n = () => window.f0ckI18n || {};
  const title = (on) => (on
    ? (i18n().unexclude_tag || 'Excluded (click to unexclude)')
    : (i18n().exclude_tag || 'Exclude tag'));

  const setState = (form, on) => {
    form.classList.toggle('is-excluded', on);
    let rm = form.querySelector('input[name="remove"]');
    if (on && !rm) {
      rm = document.createElement('input');
      rm.type = 'hidden';
      rm.name = 'remove';
      rm.value = '1';
      form.appendChild(rm);
    } else if (!on && rm) {
      rm.remove();
    }
    const btn = form.querySelector('.f0ck-exclude-btn');
    if (btn) {
      btn.title = title(on);
      btn.setAttribute('aria-pressed', on ? 'true' : 'false');
    }
    form.closest('[data-tag]')?.classList.toggle('f0ck-tag-excluded', on);
  };

  const excludedIds = () => new Set((window.f0ckSession?.excluded_tags || []).map(Number));

  // tags: the excluded tags as the API returns them ([{id, tag, normalized}]). Other lists of them
  // (w0bm's filter dialog) listen for the `f0ck:excludedTags` event (detail: the tags).
  window.f0ckSyncExcluded = (tags) => {
    const ids = new Set((tags || []).map((t) => Number(t.id)));
    if (window.f0ckSession) window.f0ckSession.excluded_tags = [...ids];
    document.querySelectorAll('form.f0ck-exclude-form').forEach((f) => setState(f, ids.has(Number(f.dataset.tagId))));
    document.dispatchEvent(new CustomEvent('f0ck:excludedTags', { detail: tags || [] }));
  };

  // Appends the button to a tag badge the frontend built (tag: as /api/v2/tags returns it).
  // icons: HTML of the "exclude" and "excluded" symbols in the frontend's style.
  window.f0ckAddExcludeButton = (span, tag, icons) => {
    const norm = String(tag.normalized ?? tag.tag ?? '').toLowerCase();
    if (!span || !tag || !tag.id || !tag.can_exclude || RATINGS.includes(norm) || !window.f0ckSession?.logged_in) return null;
    const form = document.createElement('form');
    form.method = 'post';
    form.action = '/api/v2/settings/excluded_tags';
    form.className = 'f0ck-exclude-form';
    form.dataset.tagId = String(tag.id);
    const hidden = (name, value) => {
      const i = document.createElement('input');
      i.type = 'hidden';
      i.name = name;
      i.value = value;
      form.appendChild(i);
    };
    hidden('csrf_token', window.getCsrfToken());
    hidden('back', location.pathname + location.search);
    hidden('tag_id', String(tag.id));
    const btn = document.createElement('button');
    btn.type = 'submit';
    btn.className = 'f0ck-exclude-btn';
    btn.innerHTML = `<span class="f0ck-excl-off">${icons?.off || '&#8856;'}</span><span class="f0ck-excl-on">${icons?.on || '&#10003;'}</span>`;
    form.appendChild(btn);
    span.append(' ', form);
    setState(form, excludedIds().has(Number(tag.id)));
    return form;
  };

  document.addEventListener('submit', async (e) => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement) || !form.classList.contains('f0ck-exclude-form')) return;
    e.preventDefault();
    if (form.dataset.busy) return;
    form.dataset.busy = '1';
    const remove = form.classList.contains('is-excluded');
    const name = form.closest('[data-tag]')?.dataset.tag || '';
    const csrf = window.getCsrfToken();
    const headers = { 'X-CSRF-Token': csrf, 'X-Requested-With': 'XMLHttpRequest' };
    try {
      // removing goes through the DELETE endpoint (`remove=1` is for the plain form)
      const res = remove
        ? await fetch(`${form.getAttribute('action')}/${encodeURIComponent(form.dataset.tagId)}`, { method: 'DELETE', headers })
        : await fetch(form.getAttribute('action'), {
          method: 'POST',
          headers: { ...headers, 'Content-Type': 'application/json' },
          body: JSON.stringify({ tag_id: Number(form.dataset.tagId), csrf_token: csrf }),
        });
      const data = await res.json().catch(() => ({}));
      if (!data.success) {
        window.flashMessage(data.msg || 'Failed to update excluded tags', 3000, 'error');
        return;
      }
      window.f0ckSyncExcluded(data.tags);
      const tpl = remove
        ? (i18n().tag_unexcluded_msg || 'Tag "{tag}" removed from excluded tags')
        : (i18n().tag_excluded_msg || 'Tag "{tag}" added to excluded tags');
      window.flashMessage(tpl.replace('{tag}', name), 2500, 'info');
    } catch (err) {
      console.error(err);
      window.flashMessage('Failed to update excluded tags', 3000, 'error');
    } finally {
      delete form.dataset.busy;
    }
  });
})();

// ── Maintenance mode ──────────────────────────────────────────────────────────────────────
// Switched on in the admin panel (live event maintenance): everyone but admins reloads and gets
// the maintenance page right away instead of on the next click.
window.F0ckLive.onMessage((d) => {
  if (d.type === 'maintenance' && d.data && d.data.enabled && !(window.f0ckSession && window.f0ckSession.is_admin)) location.reload();
});

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
// f0ckLoadEmojiData() loads /api/v2/emojis once and resolves to { map: { name: url }, packs } (the
// packs as the API groups them, for the tabbed picker: emoji-picker.js); f0ckLoadEmojis() resolves to
// just the map. Both reload after the live event emojis_updated. f0ckEmojify(html, map) swaps known
// names in escaped HTML for <img>.
(() => {
  let loading = null;
  window.f0ckLoadEmojiData = () => {
    if (!loading) {
      loading = fetch('/api/v2/emojis', { credentials: 'same-origin' })
        .then((r) => r.json())
        .then((d) => {
          const map = {};
          (d.emojis || []).forEach((e) => { if (e.name && e.url) map[e.name] = e.url; });
          return { map, packs: Array.isArray(d.packs) ? d.packs : [] };
        })
        .catch(() => { loading = null; return { map: {}, packs: [] }; });
    }
    return loading;
  };
  window.f0ckLoadEmojis = () => window.f0ckLoadEmojiData().then((d) => d.map);
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

// ── User interest & behavioural recommendation engine (client tracking) ───────────────────────
// Learns the visitor's tags and creators (localStorage, also for guests) and reports views to
// /api/v2/track/*. getTopAffinities() feeds /api/v2/recommendations (session_tags /
// session_creators): f0ckm's recommendations tab (sidebar-activity.js) and the frontends' own
// recommendation lists. f0ckm.js starts the tracking on its item pages; a frontend calls
// startTrackingItem({ itemId, tags, creator }) on its item page itself.
(function() {
  const STORAGE_TAGS = 'f0ck_guest_tag_affinity';
  const STORAGE_CREATORS = 'f0ck_guest_creator_affinity';

  let currentTrackingItem = null;

  const loadStoredMap = (key) => {
    try {
      const data = localStorage.getItem(key);
      return data ? JSON.parse(data) : {};
    } catch (_) {
      return {};
    }
  };

  const saveStoredMap = (key, map) => {
    try {
      localStorage.setItem(key, JSON.stringify(map));
    } catch (_) {}
  };

  const updateLocalAffinity = (key, name, delta) => {
    if (!name || typeof name !== 'string') return;
    const cleanName = name.trim().toLowerCase();
    if (!cleanName) return;

    const map = loadStoredMap(key);
    const existing = map[cleanName] || { score: 0, count: 0, ts: Date.now() };
    existing.score = Math.max(-10, Math.min(500, (existing.score || 0) + delta));
    existing.count = (existing.count || 0) + 1;
    existing.ts = Date.now();
    map[cleanName] = existing;

    // Prune to top 60 to prevent storage bloat
    const entries = Object.entries(map);
    if (entries.length > 60) {
      entries.sort((a, b) => b[1].score - a[1].score);
      const pruned = Object.fromEntries(entries.slice(0, 50));
      saveStoredMap(key, pruned);
    } else {
      saveStoredMap(key, map);
    }
  };

  const f0ckInterestEngine = {
    getTopAffinities: () => {
      const tagsMap = loadStoredMap(STORAGE_TAGS);
      const creatorsMap = loadStoredMap(STORAGE_CREATORS);

      const topTags = Object.entries(tagsMap)
        .filter(([, v]) => v.score > 0)
        .sort((a, b) => b[1].score - a[1].score)
        .slice(0, 15)
        .map(([k]) => k);

      const topCreators = Object.entries(creatorsMap)
        .filter(([, v]) => v.score > 0)
        .sort((a, b) => b[1].score - a[1].score)
        .slice(0, 10)
        .map(([k]) => k);

      return { topTags, topCreators };
    },

    recordInteraction: ({ itemId, tags = [], creator = '', duration = 0, percent = 0, type = 'dwell' }) => {
      if (!itemId) return;

      // Behavioral weighting: time spent is a primary signal
      let delta = 1.0;
      if (type === 'skip' || (duration < 2.5 && percent < 15)) {
        delta = -1.5;
      } else if (type === 'click_suggestion') {
        delta = 2.5;
      } else if (type === 'finish' || percent >= 75 || duration >= 20) {
        delta = 3.0;
      } else if (duration >= 8 || percent >= 35) {
        delta = 1.5;
      }

      // Update local storage (for persistent guest learning across browser restarts)
      if (Array.isArray(tags)) {
        tags.forEach(t => updateLocalAffinity(STORAGE_TAGS, t, delta));
      }
      if (creator) {
        updateLocalAffinity(STORAGE_CREATORS, creator, delta);
      }

      // Send beacon to backend
      const payload = {
        item_id: itemId,
        duration: Math.round(duration * 10) / 10,
        percent: Math.round(percent),
        type
      };
      // sendBeacon can't set headers: logged-in users need the CSRF token in the body or get a 403
      const csrf = window.f0ckSession?.csrf_token;
      if (csrf) payload.csrf_token = csrf;

      try {
        if (navigator.sendBeacon) {
          const blob = new Blob([JSON.stringify(payload)], { type: 'application/json' });
          navigator.sendBeacon('/api/v2/track/interaction', blob);
        } else {
          fetch('/api/v2/track/interaction', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...(csrf ? { 'X-CSRF-Token': csrf } : {}) },
            body: JSON.stringify(payload),
            keepalive: true
          }).catch(() => {});
        }
      } catch (_) {}
    },

    recordSuggestionClick: (itemId, tags = [], creator = '') => {
      if (!itemId) return;
      f0ckInterestEngine.recordInteraction({ itemId, tags, creator, type: 'click_suggestion' });
    },

    recordTagSearch: (rawTag) => {
      if (!rawTag || typeof rawTag !== 'string') return;
      const tags = rawTag.split(',')
        .map(t => t.trim().toLowerCase())
        .filter(t => t && !t.startsWith('title:') && !t.startsWith('src:'));

      if (!tags.length) return;

      // Update local storage (for immediate in-session recommendations & guest learning)
      tags.forEach(t => updateLocalAffinity(STORAGE_TAGS, t, 2.0));

      // Dispatch tracking to backend
      const payload = { tag: tags.join(',') };
      // sendBeacon can't set headers: logged-in users need the CSRF token in the body or get a 403
      const csrf = window.f0ckSession?.csrf_token;
      if (csrf) payload.csrf_token = csrf;
      try {
        if (navigator.sendBeacon) {
          const blob = new Blob([JSON.stringify(payload)], { type: 'application/json' });
          navigator.sendBeacon('/api/v2/track/search', blob);
        } else {
          fetch('/api/v2/track/search', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', ...(csrf ? { 'X-CSRF-Token': csrf } : {}) },
            body: JSON.stringify(payload),
            keepalive: true
          }).catch(() => {});
        }
      } catch (_) {}
    },

    flushCurrentTracking: () => {
      if (!currentTrackingItem) return;
      const { itemId, startTime, maxPercent, tags, creator } = currentTrackingItem;
      currentTrackingItem = null;

      const duration = (Date.now() - startTime) / 1000;
      let type = 'dwell';
      if (duration < 2.5 && maxPercent < 15) {
        type = 'skip';
      } else if (maxPercent >= 75 || duration >= 20) {
        type = 'finish';
      }

      f0ckInterestEngine.recordInteraction({
        itemId,
        tags,
        creator,
        duration,
        percent: maxPercent,
        type
      });
    },

    // ctx: { itemId, tags, creator } from a frontend's page; without it (f0ckm.js calls it as an
    // event listener) the item is read from f0ckm's item page.
    startTrackingItem: (ctx) => {
      f0ckInterestEngine.flushCurrentTracking();

      const given = ctx && typeof ctx === 'object' && !(ctx instanceof Event) && ctx.itemId ? ctx : null;
      let itemId;
      let tags;
      let creator;
      if (given) {
        itemId = parseInt(given.itemId, 10);
        tags = Array.isArray(given.tags) ? given.tags.filter(Boolean) : [];
        creator = given.creator || '';
      } else {
        // Only track if on an item page
        const currentIdStr = typeof window.getCurrentItemId === 'function' ? window.getCurrentItemId() : null;
        itemId = currentIdStr ? parseInt(currentIdStr, 10) : null;
        tags = Array.from(document.querySelectorAll('#tags .tag-text, #tags .tag-badge, .tag-link'))
          .map(el => el.textContent.trim().replace(/^#/, ''))
          .filter(Boolean);
        const creatorEl = document.querySelector('#a_username, [data-username]');
        creator = creatorEl ? (creatorEl.dataset.username || creatorEl.textContent.trim()) : '';
      }
      if (!itemId) return;

      currentTrackingItem = {
        itemId,
        startTime: Date.now(),
        maxPercent: 0,
        tags,
        creator
      };

      // Attach media playback listeners
      const media = document.querySelector('video, audio');
      if (media) {
        const updatePercent = () => {
          if (!currentTrackingItem || !media.duration) return;
          const p = (media.currentTime / media.duration) * 100;
          if (p > currentTrackingItem.maxPercent) {
            currentTrackingItem.maxPercent = p;
          }
        };

        media.addEventListener('timeupdate', updatePercent, { passive: true });
        media.addEventListener('ended', () => {
          if (currentTrackingItem) currentTrackingItem.maxPercent = 100;
        }, { once: true });
      }
    }
  };

  window.f0ckInterestEngine = f0ckInterestEngine;

  window.addEventListener('beforeunload', f0ckInterestEngine.flushCurrentTracking);
  window.addEventListener('pagehide', f0ckInterestEngine.flushCurrentTracking);
})();

// ── Item views ────────────────────────────────────────────────────────────────────────────────
// A view counts when the item was actually seen: 3 seconds of a video or song played, or 3
// seconds on screen for anything else. Then POST /api/v2/view/<id> (routes/apiv2/index/views.rs
// decides whether it is a new one: one per viewer and item in 10 minutes, not the uploader's own, no
// bots). f0ckCountView(itemId, media) starts watching an item (again for the next item of a page
// that changes items without loading, f0ckm.js); pages with the view beacon (snippets/view-beacon.html)
// start for their item by themselves. Without JavaScript the beacon image counts the page load.
(() => {
  const NEED_MS = 3000;
  const counted = new Set();
  let stop = null;
  let watching = null; // [id, media] being watched

  // the page is still open after 3 seconds: fetch (the token in a header, an answer to read); a
  // counted view is announced as `f0ck:view-counted` ({ itemId }) for the page's view count
  const report = (id) => {
    counted.add(id);
    const csrf = window.f0ckSession?.csrf_token || document.querySelector('meta[name="csrf-token"]')?.content || '';
    const body = JSON.stringify(csrf ? { csrf_token: csrf } : {});
    fetch(`/api/v2/view/${id}`, {
      method: 'POST',
      credentials: 'same-origin',
      keepalive: true,
      headers: { 'Content-Type': 'application/json', Accept: 'application/json', ...(csrf ? { 'X-CSRF-Token': csrf } : {}) },
      body,
    })
      .then(res => res.json())
      .then(data => {
        if (data && data.counted) document.dispatchEvent(new CustomEvent('f0ck:view-counted', { detail: { itemId: id } }));
      })
      .catch(() => {
        try { navigator.sendBeacon?.(`/api/v2/view/${id}`, new Blob([body], { type: 'application/json' })); } catch { /* offline */ }
      });
  };

  window.f0ckCountView = (itemId, media) => {
    const id = parseInt(itemId, 10);
    media = media || null;
    // already watching this item on this element (a page asking twice)
    if (stop && watching && watching[0] === id && watching[1] === media) return;
    if (stop) stop();
    stop = null;
    watching = null;
    if (!id || counted.has(id)) return;
    watching = [id, media];
    let seen = 0;
    const done = () => {
      if (stop) stop();
      stop = null;
      watching = null;
      report(id);
    };
    if (media && (media instanceof HTMLVideoElement || media instanceof HTMLAudioElement)) {
      // played time: what the media advanced while playing (a seek is no playing)
      let last = null;
      const onTime = () => {
        const t = media.currentTime;
        if (last !== null && !media.paused && t > last && t - last < 2) seen += (t - last) * 1000;
        last = t;
        if (seen >= NEED_MS) done();
      };
      const onSeek = () => { last = null; };
      media.addEventListener('timeupdate', onTime);
      media.addEventListener('seeking', onSeek);
      stop = () => {
        media.removeEventListener('timeupdate', onTime);
        media.removeEventListener('seeking', onSeek);
      };
    } else {
      // on screen: the time the page is visible
      const timer = setInterval(() => {
        if (document.visibilityState === 'visible') seen += 250;
        if (seen >= NEED_MS) done();
      }, 250);
      stop = () => clearInterval(timer);
    }
  };

  // the item of the page on screen (its beacon); none: stop watching. Frontends that swap pages
  // without loading them call it for every page shown (f0ckv2).
  const fromBeacon = () => {
    const beacon = document.querySelector('img.f0ck-view-beacon[data-item-id]');
    window.f0ckCountView(beacon ? beacon.dataset.itemId : null, document.querySelector('#my-video, #f0ck-album-video'));
  };
  window.f0ckCountViewFromPage = fromBeacon;
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', fromBeacon);
  else fromBeacon();
})();

// Live countdowns (and local dates): every element with data-countdown="<unix seconds>" shows the time left
// ("6d 23h 05m 09s" ... "9s", the server renders the same), ticking every second; `countdown-soon`
// in the last minute, `f0ck:countdown-done` when it runs out. Looked up on every tick, so content swapped in later (ajax pages) ticks too.
(() => {
  const pad = (n) => String(n).padStart(2, '0');
  const text = (left) => {
    const d = Math.floor(left / 86400), h = Math.floor(left % 86400 / 3600), m = Math.floor(left % 3600 / 60), s = left % 60;
    if (d > 0) return `${d}d ${h}h ${pad(m)}m ${pad(s)}s`;
    if (h > 0) return `${h}h ${pad(m)}m ${pad(s)}s`;
    if (m > 0) return `${m}m ${pad(s)}s`;
    return `${s}s`;
  };
  // data-local-time="<unix seconds>": the date in the viewer's own time zone, into the attribute
  // data-local-time-attr names (after data-local-time-prefix); the server can only write its own zone
  const localTimes = () => {
    document.querySelectorAll('[data-local-time]').forEach((el) => {
      const at = parseInt(el.dataset.localTime, 10);
      const attr = el.dataset.localTimeAttr || 'title';
      if (!at || el.dataset.localTimeDone === String(at)) return;
      el.setAttribute(attr, (el.dataset.localTimePrefix || '') + new Date(at * 1000).toLocaleString());
      el.dataset.localTimeDone = String(at);
    });
  };
  const tick = () => {
    localTimes();
    const now = Math.floor(Date.now() / 1000);
    document.querySelectorAll('[data-countdown]').forEach((el) => {
      const at = parseInt(el.dataset.countdown, 10);
      if (!at) return;
      const left = Math.max(0, at - now);
      const t = text(left);
      if (el.textContent !== t) el.textContent = t;
      el.classList.toggle('countdown-soon', left < 60);
      // ran out while on screen: `f0ck:countdown-done` (bubbles) once, so the page can react
      // (not for one that was already at 0 when it appeared, which would just repeat)
      if (left > 0) el.dataset.countdownRunning = '1';
      else if (el.dataset.countdownRunning && el.dataset.countdownDone !== String(at)) {
        el.dataset.countdownDone = String(at);
        el.dispatchEvent(new CustomEvent('f0ck:countdown-done', { bubbles: true, detail: { at } }));
      }
    });
  };
  tick();
  setInterval(tick, 1000);
})();

// Konami code (↑↑↓↓←→←→BA): "Bad Apple!!" played with fake desktop windows over the whole page (npm
// package bad-apple-virus, copied to /s/bad-apple/ by `npm run vendor`; Esc closes it). The player is
// fetched only once ↑↑↓↓ has been typed. From then on the keys that continue the code don't reach the
// site's own shortcuts (← → and a are prev/next on item pages), so typing it never leaves the page.
(() => {
  const SEQ = ['ArrowUp', 'ArrowUp', 'ArrowDown', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'ArrowLeft', 'ArrowRight', 'b', 'a'];
  const ARMED = 4;
  let pos = 0, loading = null;
  const load = () => loading || (loading = new Promise((resolve, reject) => {
    if (window.BadApple) return resolve(window.BadApple);
    const s = document.createElement('script');
    s.src = '/s/bad-apple/bad-apple.js';
    s.onload = () => (window.BadApple ? resolve(window.BadApple) : reject(new Error('BadApple missing')));
    s.onerror = () => reject(new Error('could not load ' + s.src));
    document.head.appendChild(s);
  }).catch((err) => { loading = null; window.f0ckDebug?.('[konami]', err.message); return null; }));

  window.addEventListener('keydown', (e) => {
    const t = e.target;
    if (e.ctrlKey || e.metaKey || e.altKey || window.BadApple?.active) return;
    if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) { pos = 0; return; }
    const k = e.key && e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if (k !== SEQ[pos]) {
      // ↑ is the only key that starts the code again: ↑↑↑↓↓… still counts
      pos = k === 'ArrowUp' ? (pos === 2 ? 2 : 1) : 0;
      return;
    }
    pos++;
    if (pos > ARMED) { e.preventDefault(); e.stopImmediatePropagation(); }
    if (pos === ARMED) load();
    if (pos === SEQ.length) {
      pos = 0;
      // started from this key press, so the browser lets the song play
      load().then((ba) => ba?.takeover({ theme: 'classic' }));
    }
  }, true);
})();
