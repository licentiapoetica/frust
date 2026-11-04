/*
 * f0ck-frontend.js: live events for the alternative frontends (views/frontends/*), which don't load
 * f0ckm.js. Loaded by snippets/frontend-scripts.html after f0ck-core.js, for every visitor (guests
 * too, like f0ckm.js: live comments and new uploads).
 *
 * It starts the shared live socket (F0ckLive) and does what f0ckm.js does with the events that are
 * the same everywhere (DMs, presence, emojis, MOTD, brand image, bans, profile updates). What looks
 * different per frontend goes through its hooks (all optional), set in the frontend's own script:
 *
 *   window.f0ckFrontend = {
 *     itemId,            // id of the item on screen, if any
 *     notify(data),      // a notification arrived: refresh the notification badge/list
 *     comments(data),    // comments of data.item_id changed: reload the comment list if it's on screen
 *     newItem(data),     // a new upload went live ({id, slug, tag_id, username, mime, ...}): add it to the grid
 *     dmBadge(),         // a DM arrived and messages.js isn't loaded: refresh the DM badge
 *     reconnected(),     // the socket came back after a drop: events may have been missed
 *     flash, motd, brand // see f0ck-core.js
 *   }
 *
 * Contract: views/frontends/README.md.
 */
(() => {
  'use strict';
  if (window.__f0ckFrontendLive || !window.F0ckLive || !('WebSocket' in window)) return;
  window.__f0ckFrontendLive = true;

  const fe = () => window.f0ckFrontend || {};
  const hook = (name, ...args) => {
    const fn = fe()[name];
    if (typeof fn !== 'function') return;
    try { fn(...args); } catch (err) { console.error(`[f0ckFrontend.${name}]`, err); }
  };
  // System notifications need an explicit acknowledgement (as in f0ckm.js)
  const SYSTEM_NOTIFS = ['item_deleted', 'deny', 'admin_pending', 'report', 'warning'];

  let wasOpen = false;
  window.F0ckLive.onOpen(() => {
    if (wasOpen) hook('reconnected');
    wasOpen = true;
  });

  window.F0ckLive.onMessage((msg) => {
    const data = msg.data || {};
    switch (msg.type) {
      case 'banned':
        try {
          document.cookie = `f0ck_banned=${encodeURIComponent(JSON.stringify({ banned: true, reason: data.reason, expires: data.expires }))}; Path=/; Max-Age=31536000; SameSite=Lax`;
        } catch (e) { /* ignore */ }
        location.href = data.redirect || '/banned';
        break;
      case 'notify': {
        if (navigator.vibrate && window.f0ckSession?.do_not_disturb !== true) navigator.vibrate([200, 80, 200]);
        // A comment notification for the item on screen is already read
        const itemId = fe().itemId;
        if (itemId && String(data.item_id) === String(itemId) && !SYSTEM_NOTIFS.includes(data.type)) {
          fetch(`/api/notifications/item/${itemId}/read`, { method: 'POST', keepalive: true, headers: { 'X-CSRF-Token': window.getCsrfToken() } })
            .catch(() => {})
            .then(() => hook('notify', data));
        } else {
          hook('notify', data);
        }
        break;
      }
      case 'private_message': {
        if (navigator.vibrate) navigator.vibrate([120, 60, 120]);
        // messages.js appends it to an open conversation and marks it read
        window.dispatchEvent(new CustomEvent('dm:incoming', { detail: data }));
        const dm = window.DMSystem;
        if (dm) {
          const thread = document.getElementById('dm-thread');
          const other = thread ? parseInt(thread.dataset.otherId, 10) : null;
          const viewing = thread && other && Number(data.sender_id) === other && !document.hidden;
          if (!viewing && typeof dm.incrementDmBadge === 'function') dm.incrementDmBadge();
          if (typeof dm.refreshDmBadge === 'function') dm.refreshDmBadge();
        } else {
          hook('dmBadge');
        }
        break;
      }
      case 'global_chat_presence':
        document.dispatchEvent(new CustomEvent('f0ck:global_chat_presence', { detail: data }));
        break;
      case 'comments':
        hook('comments', data);
        break;
      case 'new_item':
        hook('newItem', data);
        break;
      case 'emojis_updated':
        window.dispatchEvent(new CustomEvent('f0ck:emojis_updated'));
        document.dispatchEvent(new Event('f0ck:emojis_updated'));
        break;
      case 'motd':
        if (typeof window.updateMotdUI === 'function') window.updateMotdUI(data.motd);
        break;
      case 'brand_image':
        if (typeof window.applyBrandEvent === 'function') window.applyBrandEvent(data);
        break;
      case 'profile_update':
        if (window.f0ckSession && data.user_id === window.f0ckSession.id) {
          for (const k in data) if (k !== 'user_id') window.f0ckSession[k] = data[k];
        }
        break;
      default:
        break;
    }
  });

  window.F0ckLive.start();
})();
