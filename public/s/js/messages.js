/**
 * messages.js — End-to-End Encrypted Private Messaging
 *
 * Encryption: ECDH P-256 key exchange → AES-GCM message encryption
 * Key storage: localStorage (private key as JWK)
 * Key backup: PBKDF2-wrapped export file (.f0ckkey) for multi-device support
 * Everything runs in the browser; the server only sees ciphertext.
 *
 * INIT GUARD: this IIFE checks window.__dmLoaded to prevent double-init
 * when the script is accidentally loaded twice (e.g. during development).
 */

if (window.__dmLoaded) {
    console.warn('[DM] messages.js already loaded — skipping duplicate init');
} else {
    window.__dmLoaded = true;

(function () {
    'use strict';

    const DM_KEY_NAME      = 'f0ck_dm_privkey';
    const DM_PUBKEY_NAME  = 'f0ck_dm_pubkey';
    const DM_KEY_VERSION  = 'f0ck_dm_keyver';
    const CURRENT_VERSION = 3; // v3 = force-wipe all pre-vault localStorage keys

    // ─────────────────────────────────────────────────────────────────────────
    // Crypto helpers
    // ─────────────────────────────────────────────────────────────────────────

    const subtle = crypto.subtle;

    async function generateKeyPair() {
        return subtle.generateKey({ name: 'ECDH', namedCurve: 'P-256' }, true, ['deriveKey', 'deriveBits']);
    }

    async function exportKey(key) {
        return JSON.stringify(await subtle.exportKey('jwk', key));
    }

    async function importPublicKey(jwkString) {
        return subtle.importKey('jwk', typeof jwkString === 'string' ? JSON.parse(jwkString) : jwkString, { name: 'ECDH', namedCurve: 'P-256' }, true, []);
    }

    async function importPrivateKey(jwkString) {
        return subtle.importKey('jwk', typeof jwkString === 'string' ? JSON.parse(jwkString) : jwkString, { name: 'ECDH', namedCurve: 'P-256' }, true, ['deriveKey', 'deriveBits']);
    }

    async function deriveSharedKey(privateKey, theirPublicKey) {
        return subtle.deriveKey({ name: 'ECDH', public: theirPublicKey }, privateKey, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
    }

    async function encryptMessage(sharedKey, plaintext) {
        const iv = crypto.getRandomValues(new Uint8Array(12));
        const buf = await subtle.encrypt({ name: 'AES-GCM', iv }, sharedKey, new TextEncoder().encode(plaintext));
        return { ciphertext: toB64u(buf), iv: toB64u(iv) };
    }

    async function decryptMessage(sharedKey, ciphertext, iv) {
        const buf = await subtle.decrypt({ name: 'AES-GCM', iv: fromB64u(iv) }, sharedKey, fromB64u(ciphertext));
        return new TextDecoder().decode(buf);
    }

    // ── Seed-phrase vault crypto ───────────────────────────────────────────────

    // The wordlist (~50 KB) is only needed to create a recovery phrase, so it's loaded on first use
    // instead of on every page. Same cache-busting version as this script.
    const WORDLIST_SRC = (() => {
        const v = (document.currentScript?.src || '').match(/[?&]v=([^&]+)/);
        return '/s/js/wordlist.js' + (v ? '?v=' + v[1] : '');
    })();
    let wordlistPromise = null;
    function loadWordlist() {
        if (window.DM_WORDLIST) return Promise.resolve();
        if (!wordlistPromise) {
            wordlistPromise = new Promise((resolve, reject) => {
                const s = document.createElement('script');
                s.src = WORDLIST_SRC;
                s.onload = () => resolve();
                s.onerror = () => { wordlistPromise = null; s.remove(); reject(new Error('Wordlist failed to load')); };
                document.head.appendChild(s);
            });
        }
        return wordlistPromise;
    }

    function generateMnemonic() {
        const list = window.DM_WORDLIST;
        if (!list || list.length < 2) throw new Error('Wordlist not loaded');
        const indices = new Uint32Array(12);
        crypto.getRandomValues(indices);
        return Array.from(indices).map(i => list[i % list.length]);
    }

    async function deriveWrapKey(mnemonic, saltBytes, usage) {
        const enc = new TextEncoder();
        const base = await subtle.importKey('raw', enc.encode(mnemonic.join(' ')), 'PBKDF2', false, ['deriveKey']);
        return subtle.deriveKey(
            { name: 'PBKDF2', salt: saltBytes, iterations: 600000, hash: 'SHA-256' },
            base,
            { name: 'AES-GCM', length: 256 },
            false,
            [usage]
        );
    }

    async function encryptPrivkeyForVault(privkeyJwk, mnemonic) {
        const salt = crypto.getRandomValues(new Uint8Array(16));
        const iv   = crypto.getRandomValues(new Uint8Array(12));
        const wrapKey = await deriveWrapKey(mnemonic, salt, 'encrypt');
        const cipher  = await subtle.encrypt({ name: 'AES-GCM', iv }, wrapKey, new TextEncoder().encode(privkeyJwk));
        return { salt: toB64u(salt), iv: toB64u(iv), ciphertext: toB64u(cipher) };
    }

    async function decryptPrivkeyFromVault(vault, mnemonic) {
        const saltBytes = new Uint8Array(fromB64u(vault.salt));
        const wrapKey   = await deriveWrapKey(mnemonic, saltBytes, 'decrypt');
        const plain     = await subtle.decrypt({ name: 'AES-GCM', iv: fromB64u(vault.iv) }, wrapKey, fromB64u(vault.ciphertext));
        return new TextDecoder().decode(plain);
    }

    // ── base64url ─────────────────────────────────────────────────────────────

    function toB64u(buf) {
        const u8 = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
        let s = ''; for (const b of u8) s += String.fromCharCode(b);
        return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
    }
    function fromB64u(s) {
        const b64 = s.replace(/-/g, '+').replace(/_/g, '/');
        const raw = atob(b64); const buf = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) buf[i] = raw.charCodeAt(i);
        return buf.buffer;
    }

    async function sha256Hex(str) {
        const buf = await subtle.digest('SHA-256', new TextEncoder().encode(str));
        return Array.from(new Uint8Array(buf)).map(b => b.toString(16).padStart(2, '0')).join('');
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Key management
    // ─────────────────────────────────────────────────────────────────────────

    let _privateKey   = null;
    let _publicKeyJwk = null;

    /**
     * Load key from localStorage (v2 only).
     * Returns: 'local' | 'vault' | 'new'
     *   'local' — key is ready in memory
     *   'vault' — server has a vault blob, caller must show recovery modal
     *   'new'   — no key anywhere, fresh pair generated, caller must show setup modal
     */
    async function loadOrCreateKeyPair() {
        // ── Version gate: wipe any pre-v2 key material ──────────────────────
        const ver = parseInt(localStorage.getItem(DM_KEY_VERSION) || '0', 10);
        if (ver < CURRENT_VERSION) {
            localStorage.removeItem(DM_KEY_NAME);
            localStorage.removeItem(DM_PUBKEY_NAME);
            localStorage.removeItem(DM_KEY_VERSION);
        }

        // ── Try localStorage first (fast path) ──────────────────────────────
        const storedPriv = localStorage.getItem(DM_KEY_NAME);
        const storedPub  = localStorage.getItem(DM_PUBKEY_NAME);
        if (storedPriv && storedPub) {
            try {
                _privateKey   = await importPrivateKey(storedPriv);
                _publicKeyJwk = storedPub;
                return 'local';
            } catch {
                localStorage.removeItem(DM_KEY_NAME);
                localStorage.removeItem(DM_PUBKEY_NAME);
            }
        }

        // ── Check server vault ───────────────────────────────────────────────
        try {
            const vdata = await dmFetch('GET', '/api/dm/keyvault');
            if (vdata.success && vdata.vault) return 'vault';
        } catch { /* offline or no vault */ }

        // ── Generate fresh key pair ──────────────────────────────────────────
        const pair    = await generateKeyPair();
        const privJwk = await exportKey(pair.privateKey);
        const pubJwk  = await exportKey(pair.publicKey);
        localStorage.setItem(DM_KEY_NAME, privJwk);
        localStorage.setItem(DM_PUBKEY_NAME, pubJwk);
        localStorage.setItem(DM_KEY_VERSION, String(CURRENT_VERSION));
        _privateKey   = pair.privateKey;
        _publicKeyJwk = pubJwk;
        return 'new';
    }

    async function uploadPublicKey() {
        if (!_publicKeyJwk) return;
        const fingerprint = await sha256Hex(_publicKeyJwk);
        await dmFetch('POST', '/api/dm/pubkey', { pubkey: _publicKeyJwk, fingerprint });
    }

    /** Store key in localStorage from a raw JWK string (after vault recovery). */
    async function storeKeyFromJwk(privJwk) {
        const privObj = JSON.parse(privJwk);
        const pubJwk  = JSON.stringify({ kty: privObj.kty, crv: privObj.crv, x: privObj.x, y: privObj.y, key_ops: [], ext: true });
        _privateKey   = await importPrivateKey(privJwk);
        _publicKeyJwk = pubJwk;
        localStorage.setItem(DM_KEY_NAME, privJwk);
        localStorage.setItem(DM_PUBKEY_NAME, pubJwk);
        localStorage.setItem(DM_KEY_VERSION, String(CURRENT_VERSION));
        pubkeyCache.clear();
    }

    function hasKey() { return _privateKey !== null; }

    // ─────────────────────────────────────────────────────────────────────────
    // API helpers
    // ─────────────────────────────────────────────────────────────────────────

    const csrfToken = () => window.f0ckSession?.csrf_token || '';

    async function dmFetch(method, url, body = null) {
        const opts = { method, headers: { 'X-CSRF-Token': csrfToken() } };
        if (body && method !== 'GET') {
            opts.headers['Content-Type'] = 'application/x-www-form-urlencoded';
            opts.body = new URLSearchParams(body).toString();
        }
        const res = await fetch(url, opts);
        return res.json();
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Public key cache
    // ─────────────────────────────────────────────────────────────────────────

    const pubkeyCache = new Map(); // userId → CryptoKey (public)

    async function getRemotePublicKey(userId) {
        if (pubkeyCache.has(userId)) return pubkeyCache.get(userId);
        const data = await dmFetch('GET', `/api/dm/pubkey/${userId}`);
        if (!data.success) return null; // null = no key yet
        const key = await importPublicKey(data.pubkey);
        pubkeyCache.set(userId, key);
        return key;
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Inbox Page
    // ─────────────────────────────────────────────────────────────────────────

    async function initInbox() {
        const container = document.getElementById('dm-inbox-list');
        if (!container) return;
        await renderInbox(container);
    }

    async function renderInbox(container) {
        container.innerHTML = '<div class="dm-loading">Loading conversations…</div>';
        try {
            const data = await dmFetch('GET', '/api/dm/conversations');
            if (!data.success || !data.conversations.length) {
                container.innerHTML = '<div class="dm-empty">No conversations yet.<br><span style="font-size:0.85em;color:#888;">Visit a user profile and click "✉ Message" to start one.</span></div>';
                return;
            }
            container.innerHTML = '';
            for (const c of data.conversations) container.appendChild(buildConvoCard(c));
        } catch (e) {
            container.innerHTML = `<div class="dm-error">Failed to load: ${escHtml(e.message)}</div>`;
        }
    }

    function buildConvoCard(c) {
        const el  = document.createElement('a');
        el.href   = `/messages/${encodeURIComponent(c.username.toLowerCase())}`;
        const unread = Number(c.unread_count) || 0;
        el.className = 'dm-convo-card' + (unread > 0 ? ' dm-convo-unread' : '');
        el.dataset.userId = c.user_id;
        const avatarSrc = c.avatar_file ? `/a/${c.avatar_file}` : (c.avatar ? `/t/${c.avatar}.webp` : '/a/default.png');
        const color = c.username_color ? `color:${escHtml(c.username_color)}` : '';
        el.innerHTML = `
            <img class="dm-convo-avatar" src="${escHtml(avatarSrc)}" alt="" data-on="error:img-fallback" data-fallback="/a/default.png">
            <div class="dm-convo-info">
                <span class="dm-convo-name" style="${color}">${escHtml(c.display_name || c.username)}</span>
                <span class="dm-convo-time" data-ts="${escHtml(c.last_message_at)}">${escHtml(timeAgo(c.last_message_at))}</span>
            </div>
            ${unread > 0 ? `<span class="dm-convo-badge">${unread}</span>` : ''}
            <button class="dm-convo-delete" title="Delete conversation" data-id="${c.user_id}">×</button>
        `;

        el.querySelector('.dm-convo-delete').onclick = async (e) => {
            e.preventDefault(); e.stopPropagation();
            if (!confirm(`Close conversation with ${c.username}? History will be preserved.`)) return;
            const res = await dmFetch('POST', `/api/dm/conversation/${c.user_id}/delete`);
            if (res.success) el.remove();
        };

        // When the user explicitly clicks an unread conversation card,
        // mark it as read immediately and sync the navbar badge.
        // This is the intentional "I'm reading this" action — we do NOT
        // auto-mark on thread load so the badge persists until they choose to open it.
        if (unread > 0) {
            el.addEventListener('click', (e) => {
                if (e.target.closest('.dm-convo-delete')) return; // let delete handler handle it
                dmFetch('POST', `/api/dm/read/${c.user_id}`)
                    .then(() => refreshDmBadge())
                    .catch(() => {});
                // Optimistically remove the unread indicator on this card
                el.classList.remove('dm-convo-unread');
                const badge = el.querySelector('.dm-convo-badge');
                if (badge) badge.remove();
            });
        }

        return el;
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Conversation / Thread Page
    // ─────────────────────────────────────────────────────────────────────────

    let currentOtherId   = null;
    let currentOtherPubKey = null; // CryptoKey or null (if recipient has no key yet)
    let myId             = null;
    let latestMsgId      = 0;
    let oldestMsgId      = null;
    let threadHasMore    = false;
    const renderedIds    = new Set();
    let threadMessages   = []; // Cache for re-rendering (e.g. emojis)
    const dmPostPreviewCache = new Map(); // itemId → { item, meta } | null
    let activeReply      = null; // { senderName, preview } | null

    // Title management — global across all pages
    let _dmTitleCount  = 0;

    function applyDmTitleBadge() {
        const stripped = document.title.replace(/^\[\d+\]\s*/, '');
        document.title = _dmTitleCount > 0 ? `[${_dmTitleCount}] ${stripped}` : stripped;
    }

    // Re-apply prefix after SPA navigation changes the title (poll is safe, observer causes loops)
    setInterval(() => {
        if (_dmTitleCount > 0 && !document.title.match(/^\[\d+\]/)) applyDmTitleBadge();
    }, 1000);



    async function initConversation() {
        const thread = document.getElementById('dm-thread');
        if (!thread) return;

        currentOtherId  = parseInt(thread.dataset.otherId, 10);
        myId            = parseInt(thread.dataset.myId,    10);

        // Reset state for a fresh load (essential if key just changed or multiple inits ran)
        renderedIds.clear();
        threadMessages = [];
        latestMsgId    = 0;
        oldestMsgId    = null;
        threadHasMore  = false;

        // Update page title to reflect the conversation
        const otherName = thread.dataset.otherName || '';
        document.title  = otherName ? `DM with ${otherName}` : 'Messages';
        applyDmTitleBadge(); // Re-apply any pending [N] prefix

        // Try to get recipient's key — but don't block if they don't have one yet
        currentOtherPubKey = await getRemotePublicKey(currentOtherId);

        if (!window.f0ckSession?.dm_unencrypted && !currentOtherPubKey) {
            // Recipient has no key — clearly block sending
            const notice = document.getElementById('dm-key-notice');
            if (notice) {
                notice.style.display = 'flex';
                notice.innerHTML = '⚠️ This user hasn\'t set up their encryption key yet. ';
            }
            // Disable the send form so nothing can be submitted
            const input = document.getElementById('dm-input');
            const btn   = document.getElementById('dm-send-btn');
            if (input) { input.disabled = true; input.placeholder = 'Waiting for recipient to set up encryption…'; }
            if (btn)   { btn.disabled = true; }
        }

        // Pre-fetch emojis so :name: syntax renders correctly even on first /messages load.
        // Must be awaited before loadThread() so the cache is populated when renderDmEmojis runs.
        await (async () => {
            const cachedInstance = window.commentSystem?.customEmojis;
            const cachedStatic   = (typeof CommentSystem !== 'undefined') ? CommentSystem.emojiCache : null;
            const activeCache    = cachedStatic || cachedInstance;

            // Only skip if the cache is non-empty (an empty {} means "no patterns scanned",
            // not "no emojis exist" — we must still fetch for the DM thread).
            if (activeCache && Object.keys(activeCache).length > 0) return;

            // If a fetch is already in flight, wait for it to resolve instead of firing a second one.
            if (typeof CommentSystem !== 'undefined' && CommentSystem.loadingEmojis) {
                await new Promise(resolve => {
                    const h = () => { window.removeEventListener('f0ck:emojis_ready', h); resolve(); };
                    window.addEventListener('f0ck:emojis_ready', h);
                });
                return;
            }

            try {
                const res  = await fetch('/api/v2/emojis');
                const data = await res.json();
                if (data.success && data.emojis?.length) {
                    const map = {};
                    data.emojis.forEach(e => { map[e.name] = e.url; });
                    if (typeof CommentSystem !== 'undefined') CommentSystem.emojiCache = map;
                    if (window.commentSystem) window.commentSystem.customEmojis = map;
                }
            } catch { /* non-critical — emojis can fall back to :name: text */ }
        })();

        await loadThread(thread, false);
        setupDmEmojiPicker();
        setupSendForm();

        // Live timestamp ticker — clear any previous one and start fresh
        if (window._dmTimestampTicker) clearInterval(window._dmTimestampTicker);
        const tickTimestamps = () =>
            document.querySelectorAll('[data-ts]').forEach(el =>
                (el.textContent = timeAgo(el.dataset.ts)));
        tickTimestamps(); // Run immediately so values are fresh
        window._dmTimestampTicker = setInterval(tickTimestamps, 10_000);

        // ── Online presence ───────────────────────────────────────────────────
        if (window._dmPresenceTicker) clearInterval(window._dmPresenceTicker);
        if (window._dmPresenceHandler) {
            document.removeEventListener('f0ck:global_chat_presence', window._dmPresenceHandler);
        }

        const presenceEl = document.getElementById('dm-presence');

        const renderPresence = (online, lastSeenUnix) => {
            if (!presenceEl) return;
            const now  = ~~(Date.now() / 1000);
            const diff = now - (lastSeenUnix || 0);

            if (online) {
                presenceEl.className = 'dm-presence dm-presence--online';
                presenceEl.innerHTML = '<span class="dm-presence-dot"></span>Online';
            } else if (diff < 3600) {
                const mins = Math.max(1, Math.floor(diff / 60));
                presenceEl.className = 'dm-presence dm-presence--recent';
                presenceEl.innerHTML = `<span class="dm-presence-dot"></span>Active ${mins}m ago`;
            } else {
                presenceEl.className = 'dm-presence dm-presence--offline';
                presenceEl.innerHTML = lastSeenUnix
                    ? `Last seen ${timeAgo(new Date(lastSeenUnix * 1000).toISOString())}`
                    : 'Last seen a long time ago';
            }
        };

        const pollPresence = async () => {
            if (!presenceEl) return;
            try {
                const data = await (await fetch(`/api/dm/presence/${currentOtherId}`)).json();
                if (data.success) renderPresence(data.online, data.last_seen);
            } catch { /* non-critical */ }
        };

        // Live update: fires immediately when the other user connects/disconnects from SSE
        window._dmPresenceHandler = (e) => {
            const users  = e.detail?.users || [];
            const isLive = users.some(u => u.id === currentOtherId);
            if (isLive) {
                // They just connected — mark online immediately, no HTTP round-trip needed
                renderPresence(true, ~~(Date.now() / 1000));
            } else {
                // They disconnected — fall back to a fresh poll to get accurate last_seen
                pollPresence();
            }
        };
        document.addEventListener('f0ck:global_chat_presence', window._dmPresenceHandler);

        // Also poll on page-visibility restore (user returns to tab)
        const onVisible = () => { if (!document.hidden) pollPresence(); };
        document.removeEventListener('visibilitychange', window._dmVisibilityHandler || (() => {}));
        window._dmVisibilityHandler = onVisible;
        document.addEventListener('visibilitychange', onVisible);

        await pollPresence();
        window._dmPresenceTicker = setInterval(pollPresence, 30_000);
    }

    async function loadThread(thread, prepend = false) {
        const url = prepend
            ? `/api/dm/thread/${currentOtherId}?before=${oldestMsgId}`
            : `/api/dm/thread/${currentOtherId}`;

        const data = await dmFetch('GET', url);
        if (!data.success) { thread.innerHTML = `<div class="dm-error">${escHtml(data.msg || 'Failed to load messages')}</div>`; return; }

        threadHasMore = data.hasMore;

        if (!data.messages.length) {
            if (!prepend) thread.innerHTML = '<div class="dm-empty" style="margin:auto;">No messages yet.</div>';
            return;
        }

        const decrypted = await decryptBatch(data.messages);
        if (!prepend) {
            thread.innerHTML = '';
        }

        const prevHeight = thread.scrollHeight;

        for (const m of decrypted) {
            if (renderedIds.has(m.id)) continue;
            renderedIds.add(m.id);
            if (prepend) {
                threadMessages.unshift(m);
            } else {
                threadMessages.push(m);
            }

            if (m.id > latestMsgId) latestMsgId = m.id;
            if (oldestMsgId === null || m.id < oldestMsgId) oldestMsgId = m.id;

            if (prepend) {
                const bubble = buildMessageBubble(m);
                thread.insertBefore(bubble, thread.firstChild);
                playDmEmojiVideos(bubble);
            } else {
                const bubble = buildMessageBubble(m);
                thread.appendChild(bubble);
                playDmEmojiVideos(bubble);
            }
        }

        if (prepend) {
            // Maintain scroll position when loading older history
            thread.scrollTop = thread.scrollHeight - prevHeight;
        } else {
            // Initial load: jump to most recent message
            snapToBottom(thread, true);
            snapToBottomSticky(thread, 1200);
        }
    }

    /**
     * Fetch messages newer than latestMsgId and append them.
     * Called when we receive an SSE 'dm:incoming' event.
     */
    async function appendNewMessages(thread) {
        if (!latestMsgId) { await loadThread(thread, false); return; }

        const data = await dmFetch('GET', `/api/dm/thread/${currentOtherId}?after=${latestMsgId}`);
        if (!data.success || !data.messages.length) return;

        const decrypted = await decryptBatch(data.messages);
        let appended = false;

        for (const m of decrypted) {
            if (renderedIds.has(m.id)) continue;
            renderedIds.add(m.id);
            threadMessages.push(m);
            if (m.id > latestMsgId) latestMsgId = m.id;
            const bubble = buildMessageBubble(m);
            thread.appendChild(bubble);
            playDmEmojiVideos(bubble);
            appended = true;
        }

        if (appended) snapToBottom(thread); // Soft scroll (only if near bottom)
    }

    async function decryptBatch(messages) {
        if (!messages.length) return [];

        let sharedKey = null;
        if (_privateKey && currentOtherPubKey) {
            try {
                sharedKey = await deriveSharedKey(_privateKey, currentOtherPubKey);
            } catch (e) {
                // Key derivation failed — will treat encrypted messages as null plaintext
            }
        }

        const result = [];
        for (const m of messages) {
            if (m.iv === 'unencrypted' || !m.iv) {
                // Unencrypted message
                result.push({ ...m, plaintext: m.ciphertext });
            } else {
                // Encrypted message
                if (sharedKey) {
                    try {
                        const plaintext = await decryptMessage(sharedKey, m.ciphertext, m.iv);
                        result.push({ ...m, plaintext });
                    } catch (e) {
                        result.push({ ...m, plaintext: null });
                    }
                } else {
                    result.push({ ...m, plaintext: null });
                }
            }
        }
        return result;
    }

    function renderDmEmojis(text) {
        // Prefer instance cache (comment system active on this page),
        // then fall back to the static class cache (populated by the DM picker fetch).
        const emojis = window.commentSystem?.customEmojis
                    || (typeof CommentSystem !== 'undefined' && CommentSystem.emojiCache)
                    || null;
        if (!emojis || !Object.keys(emojis).length) return text;
        return text.replace(/:([a-z0-9_]+):/g, (match, name) => {
            const url = emojis[name];
            if (!url) return match;
            if (url.endsWith('.webm')) {
                return `<video src="${url}" class="emoji" title="${match}" autoplay loop muted playsinline></video>`;
            }
            return `<img src="${url}" class="emoji" alt="${match}" title="${match}" loading="lazy">`;
        });
    }

    function playDmEmojiVideos(el) {
        if (!el) return;
        el.querySelectorAll('video.emoji').forEach(v => {
            v.play().catch(() => {
                v.addEventListener('canplay', () => v.play().catch(() => {}), { once: true });
            });
        });
    }

    function renderDmContent(plaintext) {
        if (!plaintext) return '';

        // Strip attachment sentinels before rendering — resolveAttachments() replaces them with cards
        const strippedPlaintext = plaintext.replace(/\[attachment:\d+:[A-Za-z0-9+/=]+:[^:]+:\d+\]/g, '').trim();
        if (!strippedPlaintext) return '';
        plaintext = strippedPlaintext;

        // Anti-recursion / Performance safeguard for extremely long messages
        if (plaintext.length > 50000) {
            console.warn('[DM] Message too long, skipping markdown');
            return `<pre style="white-space: pre-wrap; font-family: inherit; margin: 0; padding: 0; background: none; border: none; font-size: inherit; color: inherit;">${escHtml(plaintext)}</pre>`;
        }

        const cs = window.commentSystem;
        if (cs && typeof cs.renderCommentContent === 'function') {
            return cs.renderCommentContent(plaintext);
        }

        if (typeof marked === 'undefined') {
            return renderDmEmojis(escHtml(plaintext));
        }

        try {
            // 1. Pre-normalize: If the line is JUST a YouTube URL, wrap it in a markdown link
            let normalized = plaintext.split('\n').map(line => {
                const trimmed = line.trim();
                const ytMatch = trimmed.match(/^https?:\/\/(?:www\.)?(?:youtube\.com\/watch\?(?:[^\s<"]*&)?v=|youtu\.be\/)([a-zA-Z0-9_\-]{11})[^\s<"]*$/);
                if (ytMatch) return `[${trimmed}](${trimmed})`;
                return line;
            }).join('\n');

            // 2. Initial escaping using native method. Restore > for markdown markers.
            let escaped = escHtml(normalized).replace(/&gt;/g, ">");

            // 3. Mentions
            const mentionRegex = /(?<!\[)@([a-zA-Z0-9_\-\.]+)(?!\])|\[@([^\]]+)\]\(\/user\/([^)]+)\)|\[@([^\]]+)\]/g;

            const siteOrigin = window.location.origin;
            const renderer = new marked.Renderer();

            // 4. Link renderer
            renderer.link = function (href, title, text) {
                if (typeof href === 'object' && href !== null) {
                    title = href.title; text = href.text || text; href = href.href;
                }
                if (!href) return text || '';
                const titleAttr = title ? ` title="${title}"` : '';
                const isExternal = href.startsWith('http://') || href.startsWith('https://') || href.startsWith('//');
                let isSameSite = false;
                
                // Marked greedy autolink fix for spoiler brackets appended to URLs
                let extraSuffix = '';
                const lowerHref = href.toLowerCase();
                if (lowerHref.endsWith('%5b/spoiler%5d')) {
                    href = href.substring(0, href.length - 14);
                    text = text.replace(/\[\/spoiler\]/ig, '');
                    extraSuffix = '[/spoiler]';
                } else if (lowerHref.endsWith('[/spoiler]')) {
                    href = href.substring(0, href.length - 10);
                    text = text.replace(/\[\/spoiler\]/ig, '');
                    extraSuffix = '[/spoiler]';
                }

                // Links to this site become /path (f0ck-core.js)
                const site = window.f0ckSiteLink(href, text);
                href = site.href;
                isSameSite = site.internal;
                const displayText = site.text;

                const isMention = href.startsWith('/user/') && text.startsWith('@');
                if (isExternal && !isSameSite) {
                    return `<a href="${href}"${titleAttr} target="_blank" rel="noopener noreferrer">${displayText}<i class="fa-solid fa-arrow-up-right-from-square external-link-icon"></i></a>${extraSuffix}`;
                }
                return `<a href="${href}"${titleAttr}${isMention ? ' class="mention"' : ''}>${displayText}</a>${extraSuffix}`;
            };

            // 5. Blockquote
            renderer.blockquote = function (quote) {
                let text = (typeof quote === 'string') ? quote : (quote.text || '');
                text = text.replace(/<p>|<\/p>/g, '');
                return text.split('\n').map(line => {
                    if (!line.trim()) return '';
                    return `<span class="greentext">&gt;${line}</span>`;
                }).join('\n');
            };

            // 6. Line-by-line rendering to avoid paragraph collapsing and recursion
            const renderedLines = escaped.split('\n').map(line => {
                const trimmed = line.trimStart();
                if (trimmed.startsWith('>') && !trimmed.match(/^>>\d+/)) {
                    const quoteContent = line.substring(line.indexOf('>') + 1);
                    return `<span class="greentext">&gt;${quoteContent}</span>`;
                }
                
                // Per-line limit
                if (line.length > 10000) return line;
                if (!line.trim()) return '&nbsp;';

                const mentionStore = [];
                let processedLine = line.replace(mentionRegex, (match, g1, g2, g3, g4) => {
                    const user = g1 || g2 || g4;
                    const html = `<a href="/user/${encodeURIComponent(user)}" class="mention">@${user}</a>`;
                    const idx = mentionStore.length;
                    mentionStore.push(html);
                    return `\x02MNT${idx}\x03`;
                });
                
                const escapedSiteHost = window.location.host.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
                const rawVideoRegex = new RegExp(`(?<![\\(\\[])((?:(?:https?:\\/\\/|\\/\\/)?${escapedSiteHost}|(?=\\/[a-zA-Z0-9_\\-]))(?:\\/[^\\s\\[\\]\\(\\)]+\\.(?:mp4|webm|ogv|mov)(?:\\?[^\\s\\[\\]\\(\\)]+)?))`, 'gi');
                processedLine = processedLine.replace(rawVideoRegex, (match, url) => {
                    let fullUrl = url;
                    if (!url.startsWith('http') && !url.startsWith('//') && !url.startsWith('/')) fullUrl = '//' + url;
                    return `[video](${fullUrl})`;
                });
                
                const escapedAsterisks = processedLine.replace(/\*/g, '\\*');
                let rendered = marked.parseInline ? marked.parseInline(escapedAsterisks, { renderer: renderer }) : marked.parse(escapedAsterisks, { renderer: renderer }).replace(/<p>|<\/p>/g, '');
                
                // Restore Mentions
                rendered = rendered.replace(/\x02MNT(\d+)\x03/g, (_, i) => mentionStore[+i]);
                
                return rendered;
            });

            let html = renderedLines.join('\n');

            // Handle spoilers [spoiler]text[/spoiler] (supports nesting)
            let lastHtml;
            let iterations = 0;
            const spoilerRegex = /\[spoiler\]((?:(?!\[spoiler\])[\s\S])*?)\[\/spoiler\]/gi;
            do {
                lastHtml = html;
                html = html.replace(spoilerRegex, (match, content) => {
                    return `<span class="spoiler">${content}</span>`;
                });
                iterations++;
            } while (html !== lastHtml && iterations < 10);

            // Handle blur [blur]text[/blur] (supports nesting)
            const blurRegex = /\[blur\]((?:(?!\[blur\])[\s\S])*?)\[\/blur\]/gi;
            iterations = 0;
            do {
                lastHtml = html;
                html = html.replace(blurRegex, (match, content) => {
                    return `<span class="blur-text">${content}</span>`;
                });
                iterations++;
            } while (html !== lastHtml && iterations < 10);

            // 7. YouTube embed logic
            const ytEmbedRegex = /(?:<p>)?\s*<a\s+[^>]*href="https?:\/\/(?:www\.)?(?:youtube\.com\/watch\?(?:[^"]*&(?:amp;)?)?v=|youtu\.be\/)([a-zA-Z0-9_\-]{11})[^"]*"[^>]*>([\s\S]*?)<\/a>\s*(?:<\/p>)?/gi;
            html = html.replace(ytEmbedRegex, (match, videoId) => {
                return `<span class="yt-embed-wrap"><iframe src="https://www.youtube.com/embed/${videoId}" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture" allowfullscreen referrerpolicy="strict-origin-when-cross-origin"></iframe></span>`;
            });
            
            // 7.5 Vocaroo embed logic
            const vocarooEmbedRegex = /(?:<p>)?\s*<a\s+[^>]*href="https?:\/\/(?:www\.)?(?:voca\.ro|vocaroo\.com)\/([a-zA-Z0-9_-]+)[^"]*"[^>]*>([\s\S]*?)<\/a>\s*(?:<\/p>)?/gi;
            html = html.replace(vocarooEmbedRegex, (match, vocarooId) => {
                if (['upload', 'contact', 'privacy', 'tos', 'about'].includes(vocarooId.toLowerCase())) return match;
                return `<span class="vocaroo-embed-wrap"><iframe src="https://vocaroo.com/embed/${vocarooId}?autoplay=0" width="300" height="60" frameborder="0" allow="autoplay"></iframe></span>`;
            });

            // 8. Same-site video embed logic
            const escapedSiteHost = window.location.host.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            const mediaDomainOrRelative = `(?:(?:https?:\\/\\/|\\/\\/)?${escapedSiteHost}|(?=\\/[a-zA-Z0-9_\\-]))`;
            const videoEmbedRegex = new RegExp(`(?:<p>)?\\s*<a\\s+[^>]*href="(${mediaDomainOrRelative}(?:\\/[^\\s\\[\\]\\(\\)]+\\.(?:mp4|webm|ogv|mov)))"[^>]*>([\\s\\S]*?)<\\/a>\\s*(?:<\\/p>)?`, 'gi');
            html = html.replace(videoEmbedRegex, (match, url) => {
                return `<span class="video-embed-wrap"><video src="${url}" controls loop muted playsinline preload="metadata"></video></span>`;
            });

            // 9. Site Sanitizer
            if (window.Sanitizer && typeof Sanitizer.clean === 'function') {
                html = Sanitizer.clean(html);
            }

            return renderDmEmojis(html);
        } catch (e) {
            console.error('[DM Render] ERROR:', e);
            return renderDmEmojis(escHtml(plaintext));
        }
    }

    function buildMessageBubble(m) {
        const div    = document.createElement('div');
        const isMine = m.sender_id === myId;

        const content = m.plaintext !== null
            ? renderDmContent(m.plaintext).trimEnd()
            : '<span class="dm-unreadable">[Unable to decrypt — key mismatch]</span>';

        const hasEmbed = content.includes('yt-embed-wrap');
        div.className = `dm-msg ${isMine ? 'dm-msg-mine' : 'dm-msg-theirs'}${hasEmbed ? ' dm-has-embed' : ''}`;
        div.dataset.msgId = m.id;
        if (m.plaintext) div.dataset.plaintext = m.plaintext;

        const time      = timeAgo(m.created_at);
        const editedBit = m.edited_at ? ' <span class="dm-edited">(edited)</span>' : '';
        div.innerHTML =
            `<div class="dm-bubble comment-content">${content}</div>` +
            `<span class="dm-msg-time" data-ts="${escHtml(m.created_at)}">${escHtml(time)}${editedBit}</span>`;

        // Actions row — always shown (reply for all, edit/delete for own)
        const actions = document.createElement('div');
        actions.className = 'dm-msg-actions';

        const replyBtn = `<button class="dm-msg-action-btn" data-action="reply" title="Reply"><i class="fa-solid fa-reply"></i></button>`;
        const editBtn  = `<button class="dm-msg-action-btn" data-action="edit"  title="Edit"><i class="fa-solid fa-pen-to-square"></i></button>`;
        const delBtn   = `<button class="dm-msg-action-btn" data-action="delete" title="Delete"><i class="fa-solid fa-trash"></i></button>`;

        actions.innerHTML = replyBtn + (isMine ? editBtn + delBtn : '');
        actions.addEventListener('click', async (e) => {
            const btn    = e.target.closest('[data-action]');
            if (!btn) return;
            const action = btn.dataset.action;
            if (action === 'reply')  replyToDmMessage(m, div);
            if (action === 'edit')   editDmMessage(m, div);
            if (action === 'delete') deleteDmMessage(m, div);
        });
        div.appendChild(actions);

        // Async: extract post IDs from raw plaintext and inject preview cards into the bubble
        if (m.plaintext) resolvePostPreviews(div, m.plaintext);

        // Async: resolve encrypted attachment sentinels
        if (m.plaintext && m.plaintext.includes('[attachment:')) resolveAttachments(div, m.plaintext);

        return div;
    }

    function replyToDmMessage(m, div) {
        const thread     = document.getElementById('dm-thread');
        const otherName  = thread?.dataset.otherName || 'them';
        const myName     = window.f0ckSession?.user || 'me';
        const senderName = (m.sender_id === myId) ? myName : otherName;

        const raw = m.plaintext || '';

        // Strip attachment sentinels from the text part, collect their filenames
        const attachments = parseAttachmentSentinels(raw);
        const textOnly    = raw.replace(/\[attachment:\d+:[A-Za-z0-9+/=]+:[^:]+:\d+\]/g, '').trim();
        const textPreview = textOnly.replace(/\n/g, ' ').slice(0, 80) + (textOnly.length > 80 ? '…' : '');

        // One preview string for the banner; quoteLines drives the actual sent quote
        const preview     = [textPreview, ...attachments.map(a => a.filename)].filter(Boolean).join(' · ');

        // Build the multi-line blockquote: first line has the sender + text, then one line per attachment
        const quoteLines  = [];
        quoteLines.push(`> @${senderName}: ${textPreview}`);
        for (const att of attachments) quoteLines.push(`> ${att.filename}`);

        activeReply = { senderName, preview, quoteLines };
        showReplyBanner();

        const input = document.getElementById('dm-input');
        if (input) { input.focus(); }
    }

    function showReplyBanner() {
        const form = document.getElementById('dm-send-form');
        if (!form) return;

        let banner = form.querySelector('.dm-reply-banner');
        if (!banner) {
            banner = document.createElement('div');
            banner.className = 'dm-reply-banner';
            form.insertBefore(banner, form.firstChild);
        }

        const name    = escHtml(activeReply?.senderName || '');
        const preview = escHtml(activeReply?.preview    || '');
        banner.innerHTML =
            `<span class="dm-reply-banner__icon"><i class="fa-solid fa-reply"></i></span>` +
            `<span class="dm-reply-banner__body"><strong>${name}</strong> ${preview}</span>` +
            `<button class="dm-reply-banner__close" title="Cancel reply"><i class="fa-solid fa-xmark"></i></button>`;

        banner.querySelector('.dm-reply-banner__close').addEventListener('click', clearReply);
    }

    function clearReply() {
        activeReply = null;
        const banner = document.querySelector('.dm-reply-banner');
        if (banner) banner.remove();
    }


    async function editDmMessage(m, div) {
        if (div.dataset.editing === 'true') return;
        if (!m.plaintext) { showFlashMsg('Cannot edit — message could not be decrypted', 'error'); return; }
        div.dataset.editing = 'true';

        const bubble = div.querySelector('.dm-bubble');
        const origHtml = bubble.innerHTML;

        const wrap = document.createElement('div');
        wrap.className = 'dm-edit-wrap';

        const ta = document.createElement('textarea');
        ta.className = 'dm-edit-textarea';
        ta.value = m.plaintext;

        const actions = document.createElement('div');
        actions.className = 'dm-edit-actions';

        const saveBtn   = document.createElement('button');
        saveBtn.className = 'dm-edit-save';
        saveBtn.textContent = 'Save';

        const cancelBtn = document.createElement('button');
        cancelBtn.className = 'dm-edit-cancel';
        cancelBtn.textContent = 'Cancel';

        actions.append(saveBtn, cancelBtn);
        wrap.append(ta, actions);
        bubble.innerHTML = '';
        bubble.appendChild(wrap);
        ta.focus();

        cancelBtn.addEventListener('click', () => {
            bubble.innerHTML = origHtml;
            div.dataset.editing = '';
        });

        saveBtn.addEventListener('click', async () => {
            const newText = ta.value.trim();
            if (!newText || newText === m.plaintext) {
                bubble.innerHTML = origHtml;
                div.dataset.editing = '';
                return;
            }

            saveBtn.disabled = true;
            saveBtn.textContent = '…';
            try {
                let newIv, newCt;
                if (window.f0ckSession?.dm_unencrypted) {
                    newIv = 'unencrypted';
                    newCt = newText;
                } else {
                    if (!currentOtherPubKey) { showFlashMsg('Cannot encrypt: no key', 'error'); saveBtn.disabled = false; saveBtn.textContent = 'Save'; return; }
                    const sharedKey   = await deriveSharedKey(_privateKey, currentOtherPubKey);
                    const enc = await encryptMessage(sharedKey, newText);
                    newIv = enc.iv;
                    newCt = enc.ciphertext;
                }

                const res = await fetch(`/api/dm/message/${m.id}`, {
                    method: 'PATCH',
                    headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'X-CSRF-Token': csrfToken() },
                    body: new URLSearchParams({ ciphertext: newCt, iv: newIv })
                });
                const data = await res.json();
                if (!data.success) { showFlashMsg('Edit failed: ' + (data.msg || res.status), 'error'); return; }

                // Update in-memory plaintext and re-render bubble
                m.plaintext  = newText;
                m.edited_at  = data.edited_at || new Date().toISOString();
                div.dataset.plaintext = newText;

                const newContent = renderDmContent(newText).trimEnd();
                const editedBit  = '<span class="dm-edited">(edited)</span>';
                bubble.innerHTML = newContent;

                // Update the time span
                const timeEl = div.querySelector('.dm-msg-time');
                if (timeEl) timeEl.innerHTML = escHtml(timeAgo(m.created_at)) + ' ' + editedBit;

                if (m.plaintext) resolvePostPreviews(div, m.plaintext);
                if (m.plaintext && m.plaintext.includes('[attachment:')) resolveAttachments(div, m.plaintext);
            } catch (e) {
                showFlashMsg('Edit error: ' + e.message, 'error');
            } finally {
                div.dataset.editing = '';
                saveBtn.disabled = false;
            }
        });
    }

    async function deleteDmMessage(m, div) {
        if (div.dataset.deleting === 'true') return;
        div.dataset.deleting = 'true';

        // Extract attachment IDs from plaintext sentinels
        const attachmentIds = [];
        if (m.plaintext) {
            for (const att of parseAttachmentSentinels(m.plaintext)) {
                const id = parseInt(att.id, 10);
                if (id) attachmentIds.push(id);
            }
        }

        try {
            const body = new URLSearchParams();
            attachmentIds.forEach(id => body.append('attachment_ids[]', id));

            const res = await fetch(`/api/dm/message/${m.id}`, {
                method: 'DELETE',
                headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'X-CSRF-Token': csrfToken() },
                body
            });
            const data = await res.json();
            if (!data.success) {
                showFlashMsg('Delete failed: ' + (data.msg || res.status), 'error');
                div.dataset.deleting = '';
                return;
            }

            div.classList.add('dm-msg-deleting');
            div.addEventListener('animationend', () => div.remove(), { once: true });
        } catch (e) {
            showFlashMsg('Delete error: ' + e.message, 'error');
            div.dataset.deleting = '';
        }
    }

    // ── Post link preview cards ───────────────────────────────────────────────
    // Extracts item IDs from the raw plaintext (immune to rendering pipeline
    // variations: marked / commentSystem / plain-text fallback), then appends
    // a preview card below the bubble content for each unique ID found.
    async function resolvePostPreviews(msgDiv, plaintext) {
        const bubble = msgDiv.querySelector('.dm-bubble');
        if (!bubble) return;

        // Match bare /12345 and full same-site URLs like https://site.com/12345.
        // Negative lookbehinds prevent /10 inside "10/10" or "//10" from matching.
        const siteOriginEsc = window.location.origin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
        const itemRx = new RegExp(
            `(?<!\\d)(?<!/)(?:${siteOriginEsc})?\\/(\\d+)(?=[\\s,!?\"'\\)\\]<]|$)`,
            'g'
        );

        const seen = new Set();
        let match;
        while ((match = itemRx.exec(plaintext)) !== null) {
            const id = match[1];
            if (!seen.has(id)) seen.add(id);
        }
        if (!seen.size) return;

        for (const id of seen) {
            // Insert loading placeholder card below the bubble text
            const placeholder = document.createElement('span');
            placeholder.className = 'dm-post-card dm-post-card--loading';
            placeholder.innerHTML =
                `<span class="dm-post-card__thumb-wrap"><span class="dm-post-card__thumb-placeholder"><i class="fa-solid fa-spinner fa-spin"></i></span></span>`+
                `<span class="dm-post-card__info"><span class="dm-post-card__id">#${id}</span></span>`;
            bubble.appendChild(placeholder);

            // Fetch item info and meta (with cache)
            let cached = dmPostPreviewCache.get(id);
            if (cached === undefined) {
                try {
                    const [itemRes, metaRes] = await Promise.all([
                        fetch(`/api/v2/item/${id}`),
                        fetch(`/api/v2/scroller/meta?ids=${id}`)
                    ]);
                    const itemData = await itemRes.json();
                    const metaData = await metaRes.json();
                    const item = (itemData.success && itemData.rows) ? itemData.rows : null;
                    const meta = metaData[id] || null;
                    cached = item ? { item, meta } : null;
                } catch (_) {
                    cached = null;
                }
                dmPostPreviewCache.set(id, cached);
            }

            if (!cached) {
                placeholder.remove(); // no item found — silently drop
                continue;
            }

            const { item, meta } = cached;
            const commentCount = meta ? (meta.comment_count || 0) : 0;
            const uploader = escHtml(item.username || 'unknown');
            const mime = item.mime || '';
            const thumbSrc = `/t/${id}.webp`;

            // Media type badge
            let typeBadge = '';
            if (mime.startsWith('video/')) typeBadge = '<i class="fa-solid fa-film"></i>';
            else if (mime.startsWith('audio/')) typeBadge = '<i class="fa-solid fa-music"></i>';
            else if (mime.startsWith('image/')) typeBadge = '<i class="fa-solid fa-image"></i>';

            const card = document.createElement('a');
            card.className = 'dm-post-card';
            card.href = `/${id}`;
            card.innerHTML =
                `<span class="dm-post-card__thumb-wrap">`+
                    `<img class="dm-post-card__thumb" src="${escHtml(thumbSrc)}" alt="#${id}" loading="lazy" data-on="error:hide">`+
                    (typeBadge ? `<span class="dm-post-card__type-badge">${typeBadge}</span>` : '') +
                `</span>`+
                `<span class="dm-post-card__info">`+
                    `<span class="dm-post-card__id">#${id}</span>`+
                    `<span class="dm-post-card__uploader"><i class="fa-solid fa-user"></i> ${uploader}</span>`+
                    `<span class="dm-post-card__comments"><i class="fa-solid fa-comment"></i> ${commentCount}</span>`+
                `</span>`;

            placeholder.replaceWith(card);
        }
    }

    // ── Encrypted Attachment System ───────────────────────────────────────────
    // Sentinel format embedded in message plaintext:
    //   [attachment:ID:FILENAME_B64:MIME:SIZE]
    // where FILENAME_B64 is btoa(filename) to avoid colon conflicts.


    const DM_ATT_MAX_BYTES = 50 * 1024 * 1024; // 50 MB

    async function encryptAttachment(sharedKey, fileBuffer) {
        const iv  = crypto.getRandomValues(new Uint8Array(12));
        const enc = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, sharedKey, fileBuffer);
        return { iv: toB64u(iv), encryptedBuffer: new Uint8Array(enc) };
    }

    async function decryptAttachment(sharedKey, ivB64u, encryptedBuffer) {
        const plain = await crypto.subtle.decrypt(
            { name: 'AES-GCM', iv: fromB64u(ivB64u) },
            sharedKey,
            encryptedBuffer
        );
        return plain; // ArrayBuffer
    }

    /**
     * Encrypt and upload a file as a DM attachment.
     * Returns the sentinel string to embed in the message, or null on failure.
     */
    async function uploadDmAttachment(file, sharedKey, recipientId, onProgress) {
        if (file.size > DM_ATT_MAX_BYTES) {
            showFlashMsg(`File too large (max 50 MB): ${file.name}`, 'error');
            return null;
        }

        onProgress('encrypt');
        let iv, encryptedBuffer;
        try {
            const raw = await file.arrayBuffer();
            if (window.f0ckSession?.dm_unencrypted) {
                iv = 'unencrypted';
                encryptedBuffer = raw;
            } else {
                ({ iv, encryptedBuffer } = await encryptAttachment(sharedKey, raw));
            }
        } catch (e) {
            showFlashMsg('Encryption failed: ' + e.message, 'error');
            return null;
        }

        onProgress('upload');
        try {
            const fd = new FormData();
            fd.append('iv', iv);
            fd.append('original_name', file.name);
            fd.append('mime_hint', file.type || 'application/octet-stream');
            fd.append('size_bytes', String(file.size));
            fd.append('file', new Blob([encryptedBuffer], { type: 'application/octet-stream' }), 'enc');

            const res = await fetch(`/api/dm/attachment/upload/${recipientId}`, {
                method: 'POST',
                headers: { 'X-CSRF-Token': csrfToken() },
                body: fd
            });
            const data = await res.json();
            if (!data.success) {
                showFlashMsg('Upload failed: ' + (data.msg || 'Unknown error'), 'error');
                return null;
            }

            // Build sentinel: [attachment:ID:base64(filename):mime:size]
            const b64name = btoa(unescape(encodeURIComponent(file.name)));
            return `[attachment:${data.id}:${b64name}:${file.type || 'application/octet-stream'}:${file.size}]`;
        } catch (e) {
            showFlashMsg('Upload failed: ' + e.message, 'error');
            return null;
        }
    }

    // Parse [attachment:ID:b64name:mime:size] sentinels from plaintext
    function parseAttachmentSentinels(text) {
        const rx = /\[attachment:(\d+):([A-Za-z0-9+/=]+):([^:]+):(\d+)\]/g;
        const results = [];
        let m;
        while ((m = rx.exec(text)) !== null) {
            let filename = m[2];
            try { filename = decodeURIComponent(escape(atob(m[2]))); } catch { /* use raw */ }
            results.push({ id: m[1], filename, mime: m[3], size: parseInt(m[4], 10), raw: m[0] });
        }
        return results;
    }

    // Inject attachment previews for any sentinels found in plaintext.
    // Media (image/video/audio) decrypts automatically and renders inline.
    // Other file types show the clickable download card.
    async function resolveAttachments(msgDiv, plaintext) {
        const bubble = msgDiv.querySelector('.dm-bubble');
        if (!bubble) return;
        const sentinels = parseAttachmentSentinels(plaintext);
        if (!sentinels.length) return;

        for (const att of sentinels) {
            const isImage = att.mime.startsWith('image/');
            const isVideo = att.mime.startsWith('video/');
            const isAudio = att.mime.startsWith('audio/');
            const isMedia = isImage || isVideo || isAudio;

            // Build a placeholder element to swap into the bubble first
            const placeholder = document.createElement('div');
            const rawHtmlEncoded = escHtml(att.raw);

            if (isMedia) {
                // Loading state
                placeholder.className = 'dm-attachment-preview dm-attachment-preview--loading';
                placeholder.innerHTML = '<span class="dm-attachment-preview__spinner"><i class="fa-solid fa-spinner fa-spin"></i></span>';
            } else {
                // Non-media: just build the download card immediately, no async needed
                const card = buildAttachmentCard(att);
                if (bubble.innerHTML.includes(rawHtmlEncoded)) {
                    // Replace sentinel text with card HTML, then re-query the live node to attach events
                    bubble.innerHTML = bubble.innerHTML.replace(rawHtmlEncoded, `<span data-att-slot="${att.id}"></span>`);
                    const slot = bubble.querySelector(`[data-att-slot="${att.id}"]`);
                    if (slot) slot.replaceWith(card);
                } else {
                    bubble.appendChild(card);
                }
                continue;
            }

            // Insert placeholder where the sentinel text is
            if (bubble.innerHTML.includes(rawHtmlEncoded)) {
                bubble.innerHTML = bubble.innerHTML.replace(rawHtmlEncoded, `<span data-att-slot="${att.id}"></span>`);
                const slot = bubble.querySelector(`[data-att-slot="${att.id}"]`);
                if (slot) slot.replaceWith(placeholder);
            } else {
                bubble.appendChild(placeholder);
            }

            // Defer decrypt until the placeholder scrolls near the viewport.
            // This means attachments above the initial view never cause layout shifts.
            const decryptWhenVisible = () => {
                (async () => {
                const blob = await fetchAndDecryptAttachment(att.id, att.mime);
                if (!blob || !placeholder.parentNode) return;

                const url = URL.createObjectURL(blob);
                const wrap = document.createElement('div');
                wrap.className = 'dm-attachment-preview';

                let el;
                if (isImage) {
                    el = document.createElement('img');
                    el.src = url;
                    el.className = 'dm-attachment-preview__img';
                    el.alt = att.filename;
                } else if (isVideo) {
                    el = document.createElement('video');
                    el.src = url;
                    el.controls = true;
                    el.className = 'dm-attachment-preview__video';
                    el.preload = 'metadata';
                } else {
                    el = document.createElement('audio');
                    el.src = url;
                    el.controls = true;
                    el.className = 'dm-attachment-preview__audio';
                    el.preload = 'metadata';
                }

                // Download button overlaid on the preview
                const dlBtn = document.createElement('a');
                dlBtn.className = 'dm-attachment-preview__dl';
                dlBtn.href = url;
                dlBtn.download = att.filename;
                dlBtn.title = `Download ${att.filename}`;
                dlBtn.innerHTML = '<i class="fa-solid fa-download"></i>';
                dlBtn.addEventListener('click', e => e.stopPropagation());

                wrap.appendChild(el);
                wrap.appendChild(dlBtn);
                placeholder.replaceWith(wrap);

                // Snap to bottom once the media has decoded and the browser knows its dimensions.
                // This is the correct moment — the ResizeObserver fires too early (before layout is final).
                const thread = msgDiv.closest('#dm-thread');
                if (thread) {
                    const snapIfAtBottom = () => {
                        const dist = thread.scrollHeight - thread.scrollTop - thread.clientHeight;
                        // Always snap if we're within 400px of the bottom (covers initial load offset too)
                        if (dist < 400) thread.scrollTop = thread.scrollHeight;
                    };
                    if (isImage) {
                        el.addEventListener('load',  snapIfAtBottom, { once: true });
                        el.addEventListener('error', snapIfAtBottom, { once: true });
                    } else {
                        // video / audio: loadedmetadata fires when dimensions / duration are known
                        el.addEventListener('loadedmetadata', snapIfAtBottom, { once: true });
                        el.addEventListener('error',          snapIfAtBottom, { once: true });
                    }
                    // Also snap immediately after DOM insertion for any already-cached blob
                    requestAnimationFrame(() => requestAnimationFrame(snapIfAtBottom));
                }
            })();
            }; // end decryptWhenVisible

            // Use IntersectionObserver to defer decryption until near viewport.
            // Bottom messages (already visible) fire immediately; older ones wait.
            const io = new IntersectionObserver((entries, obs) => {
                if (!entries[0].isIntersecting) return;
                obs.disconnect();
                decryptWhenVisible();
            }, { rootMargin: '200px', threshold: 0 });
            io.observe(placeholder);
        }
    }

    function fmtBytes(bytes) {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
        return `${(bytes / 1048576).toFixed(1)} MB`;
    }

    function getAttachmentIcon(mime) {
        if (mime.startsWith('image/')) return 'fa-image';
        if (mime.startsWith('video/')) return 'fa-film';
        if (mime.startsWith('audio/')) return 'fa-music';
        return 'fa-file';
    }

    function buildAttachmentCard(att) {
        const isImage = att.mime.startsWith('image/');
        const isVideo = att.mime.startsWith('video/');
        const isAudio = att.mime.startsWith('audio/');
        const isMedia = isImage || isVideo || isAudio;

        const card = document.createElement('div');
        card.className = 'dm-attachment-card';
        card.dataset.attId   = att.id;
        card.dataset.attMime = att.mime;
        card.dataset.attName = att.filename;

        const icon = getAttachmentIcon(att.mime);
        // Action label: media shows "tap to preview", files show download arrow
        const actionIcon = isMedia ? 'fa-eye' : 'fa-download';
        const actionTitle = isMedia ? 'Click to preview' : 'Download';

        card.innerHTML =
            `<span class="dm-attachment-card__icon"><i class="fa-solid ${icon}"></i></span>`+
            `<span class="dm-attachment-card__info">`+
                `<span class="dm-attachment-card__name">${escHtml(att.filename)}</span>`+
                `<span class="dm-attachment-card__size">${fmtBytes(att.size)}</span>`+
            `</span>`+
            `<span class="dm-attachment-card__dl" title="${actionTitle}"><i class="fa-solid ${actionIcon}"></i></span>`;

        card.addEventListener('click', async (e) => {
            e.preventDefault();
            if (card.dataset.busy === 'true') return;

            // If media is already previewed inline, clicking the card again toggles it
            const existing = card.nextElementSibling;
            if (existing && existing.classList.contains('dm-attachment-preview')) {
                existing.remove();
                if (card._blobUrl) { URL.revokeObjectURL(card._blobUrl); card._blobUrl = null; }
                card.dataset.previewed = '';
                return;
            }

            card.dataset.busy = 'true';
            const dlIcon = card.querySelector('.dm-attachment-card__dl i');
            if (dlIcon) dlIcon.className = 'fa-solid fa-spinner fa-spin';

            try {
                const blob = await fetchAndDecryptAttachment(att.id, att.mime);
                if (!blob) return;

                const url = URL.createObjectURL(blob);
                card._blobUrl = url;

                if (isMedia) {
                    // Render inline preview
                    const wrap = document.createElement('div');
                    wrap.className = 'dm-attachment-preview';

                    let el;
                    if (isImage) {
                        el = document.createElement('img');
                        el.src = url;
                        el.className = 'dm-attachment-preview__img';
                        el.alt = att.filename;
                        // Click image to open full-size in new tab
                        } else if (isVideo) {
                        el = document.createElement('video');
                        el.src = url;
                        el.controls = true;
                        el.className = 'dm-attachment-preview__video';
                        el.preload = 'metadata';
                    } else { // audio
                        el = document.createElement('audio');
                        el.src = url;
                        el.controls = true;
                        el.className = 'dm-attachment-preview__audio';
                        el.preload = 'metadata';
                    }

                    // Download button inside preview
                    const dlBtn = document.createElement('a');
                    dlBtn.className = 'dm-attachment-preview__dl';
                    dlBtn.href = url;
                    dlBtn.download = att.filename;
                    dlBtn.title = 'Download';
                    dlBtn.innerHTML = '<i class="fa-solid fa-download"></i>';
                    dlBtn.addEventListener('click', e => e.stopPropagation());

                    wrap.appendChild(el);
                    wrap.appendChild(dlBtn);
                    card.insertAdjacentElement('afterend', wrap);
                    card.dataset.previewed = 'true';
                } else {
                    // Non-media: just trigger download
                    const a = document.createElement('a');
                    a.href = url;
                    a.download = att.filename;
                    document.body.appendChild(a);
                    a.click();
                    setTimeout(() => { URL.revokeObjectURL(url); a.remove(); card._blobUrl = null; }, 5000);
                }
            } catch (e) {
                showFlashMsg('Preview failed: ' + e.message, 'error');
            } finally {
                card.dataset.busy = '';
                if (dlIcon) {
                    const newIcon = isMedia ? 'fa-eye' : 'fa-download';
                    dlIcon.className = `fa-solid ${card.dataset.previewed ? 'fa-eye-slash' : newIcon}`;
                }
            }
        });

        return card;
    }

    // Shared fetch+decrypt helper — returns a Blob or null
    async function fetchAndDecryptAttachment(id, mime) {
        try {
            const res = await fetch(`/api/dm/attachment/${id}`, {
                headers: { 'X-CSRF-Token': csrfToken() }
            });
            if (!res.ok) { showFlashMsg('Download failed (' + res.status + ')', 'error'); return null; }

            const ivHeader = res.headers.get('X-DM-IV');
            if (!ivHeader) { showFlashMsg('Server did not return IV — please refresh', 'error'); return null; }

            const encBuf   = await res.arrayBuffer();
            if (ivHeader === 'unencrypted') {
                return new Blob([encBuf], { type: mime || 'application/octet-stream' });
            }

            if (!currentOtherPubKey) {
                showFlashMsg('Cannot decrypt: no encryption key', 'error');
                return null;
            }
            const sharedKey = await deriveSharedKey(_privateKey, currentOtherPubKey);
            const plainBuf = await decryptAttachment(sharedKey, ivHeader, new Uint8Array(encBuf));
            return new Blob([plainBuf], { type: mime || 'application/octet-stream' });
        } catch (e) {
            showFlashMsg('Decryption failed: ' + e.message, 'error');
            return null;
        }
    }

    let sendInFlight = false; // debounce guard against double-submit

    function setupDmEmojiPicker() {
        const form = document.getElementById('dm-send-form');
        if (!form) return;

        const textarea = form.querySelector('#dm-input');
        const actions  = form.querySelector('.input-actions');
        if (!textarea || !actions || actions.querySelector('.emoji-trigger')) return;

        // Attach mentions
        if (window.MentionAutocomplete) window.MentionAutocomplete.attach(textarea);

        // Spoiler button
        const spoilerBtn = document.createElement('button');
        spoilerBtn.type = 'button';
        spoilerBtn.innerText = '[spoiler]';
        spoilerBtn.className = 'spoiler-trigger';
        spoilerBtn.title = 'Insert spoiler tag';
        spoilerBtn.addEventListener('click', (e) => {
            e.preventDefault();
            const start = textarea.selectionStart;
            const end = textarea.selectionEnd;
            const val = textarea.value;
            const selected = val.substring(start, end);
            if (selected) {
                const tagStart = '[spoiler]';
                const tagEnd = '[/spoiler]';
                textarea.value = val.substring(0, start) + tagStart + selected + tagEnd + val.substring(end);
                const newPos = start + tagStart.length + selected.length + tagEnd.length;
                textarea.setSelectionRange(newPos, newPos);
            } else {
                const tagStart = '[spoiler]';
                textarea.value = val.substring(0, start) + '[spoiler][/spoiler]' + val.substring(start);
                const newPos = start + tagStart.length;
                textarea.setSelectionRange(newPos, newPos);
            }
            textarea.focus();
        });

        // Trigger button
        const trigger = document.createElement('button');
        trigger.type      = 'button';
        trigger.innerText = '☺';
        trigger.className = 'emoji-trigger';
        
        // Put them in: spoiler then emoji
        actions.prepend(trigger);
        actions.prepend(spoilerBtn);

        // ── Attachment button ─────────────────────────────────────────────────
        if (window.f0ckSession?.dm_attachments === true) {
        const attachInput = document.createElement('input');
        attachInput.type     = 'file';
        attachInput.id       = 'dm-attach-input';
        attachInput.accept   = 'audio/*,video/*,image/*';
        attachInput.style.display = 'none';
        form.appendChild(attachInput);

        const attachBtn = document.createElement('button');
        attachBtn.type      = 'button';
        attachBtn.title     = window.f0ckSession?.dm_unencrypted ? 'Send attachment' : 'Send encrypted attachment';
        attachBtn.className = 'dm-attach-btn';
        attachBtn.innerHTML = '<i class="fa-solid fa-paperclip"></i>';
        actions.prepend(attachBtn);

        attachBtn.addEventListener('click', (e) => {
            e.preventDefault();
            attachInput.value = '';
            attachInput.click();
        });

        attachInput.addEventListener('change', async () => {
            const file = attachInput.files[0];
            if (!file) return;

            if (!window.f0ckSession?.dm_unencrypted) {
                if (!currentOtherPubKey) {
                    showFlashMsg('Cannot attach: recipient has no encryption key', 'error');
                    return;
                }
                if (!hasKey()) {
                    showFlashMsg('Cannot attach: your encryption key is not loaded', 'error');
                    return;
                }
            }

            // Show progress state on button
            const origHtml = attachBtn.innerHTML;
            attachBtn.disabled = true;
            attachBtn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i>';

            const onProgress = (stage) => {
                if (stage === 'encrypt') attachBtn.title = 'Encrypting…';
                if (stage === 'upload')  attachBtn.title = 'Uploading…';
            };

            try {
                let sharedKey = null;
                if (!window.f0ckSession?.dm_unencrypted) {
                    sharedKey = await deriveSharedKey(_privateKey, currentOtherPubKey);
                }
                const sentinel  = await uploadDmAttachment(file, sharedKey, currentOtherId, onProgress);
                if (sentinel) {
                    // Append sentinel to the textarea (with a newline separator)
                    const ta = form.querySelector('#dm-input');
                    if (ta) {
                        ta.value = ta.value ? ta.value + '\n' + sentinel : sentinel;
                        ta.dispatchEvent(new Event('input'));
                        ta.focus();
                    }
                }
            } catch (e) {
                showFlashMsg('Attachment error: ' + e.message, 'error');
            } finally {
                attachBtn.disabled = false;
                attachBtn.innerHTML = origHtml;
                attachBtn.title = window.f0ckSession?.dm_unencrypted ? 'Send attachment' : 'Send encrypted attachment';
                attachInput.value = '';
            }
        });

        } // end if (dm_attachments)

        // Picker lives inside the form — CSS positions it above via position:absolute + bottom:100%
        const picker = document.createElement('div');
        picker.className    = 'emoji-picker dm-emoji-picker';
        picker.style.display = 'none';
        form.appendChild(picker);

        // ── Inline emoji autocomplete ─────────────────────────────────────────
        const autocomplete = document.createElement('div');
        autocomplete.className = 'emoji-autocomplete';
        autocomplete.style.display = 'none';
        autocomplete.style.overscrollBehavior = 'contain';
        document.body.appendChild(autocomplete);

        let acActiveIdx = -1;
        let acMatches = [];
        let acDisplayedCount = 0;
        const BATCH_SIZE = 20;

        const getEmojiMap = () => {
            return (typeof CommentSystem !== 'undefined' && CommentSystem.emojiCache)
                || window.commentSystem?.customEmojis
                || null;
        };

        const positionAC = () => {
            const rect = textarea.getBoundingClientRect();
            autocomplete.style.position = 'fixed';
            autocomplete.style.left = rect.left + 'px';
            autocomplete.style.width = rect.width + 'px';
            autocomplete.style.bottom = (window.innerHeight - rect.top) + 'px';
            autocomplete.style.top = 'auto';
        };

        const hideAC = () => {
            autocomplete.style.display = 'none';
            autocomplete.innerHTML = '';
            acActiveIdx = -1;
            acMatches = [];
            acDisplayedCount = 0;
        };

        const getColon = () => {
            const pos = textarea.selectionStart;
            const text = textarea.value.slice(0, pos);
            const match = text.match(/:([a-z0-9_]{0,})$/i);
            if (!match) return null;
            return { query: match[1].toLowerCase(), colonPos: pos - match[0].length };
        };

        const insertEmojiCode = (name) => {
            const hit = getColon();
            if (!hit) return;
            const before = textarea.value.slice(0, hit.colonPos);
            const after = textarea.value.slice(textarea.selectionStart);
            const insert = `:${name}: `;
            textarea.value = before + insert + after;
            const newPos = hit.colonPos + insert.length;
            textarea.setSelectionRange(newPos, newPos);
            textarea.focus();
            hideAC();
            // Trigger input event for auto-resize
            textarea.dispatchEvent(new Event('input'));
        };

        const appendMoreItems = () => {
            const emojiMap = getEmojiMap();
            if (!emojiMap || acDisplayedCount >= acMatches.length) return;
            const nextBatch = acMatches.slice(acDisplayedCount, acDisplayedCount + BATCH_SIZE);
            nextBatch.forEach((name, i) => {
                const idx = acDisplayedCount + i;
                const item = document.createElement('div');
                item.className = 'emoji-ac-item';
                item.dataset.idx = idx;
                const img = document.createElement('img');
                img.src = emojiMap[name];
                img.alt = name;
                img.loading = 'lazy';
                const label = document.createElement('span');
                label.textContent = `:${name}:`;
                item.appendChild(img);
                item.appendChild(label);
                item.addEventListener('mousedown', ev => {
                    ev.preventDefault();
                    insertEmojiCode(name);
                });
                autocomplete.appendChild(item);
            });
            acDisplayedCount += nextBatch.length;
        };

        const renderAC = () => {
            const hit = getColon();
            const emojiMap = getEmojiMap();
            if (!hit || !emojiMap) return hideAC();
            const { query } = hit;
            acMatches = Object.keys(emojiMap).filter(n => n.includes(query));
            if (!acMatches.length) return hideAC();

            autocomplete.innerHTML = '';
            acActiveIdx = -1;
            acDisplayedCount = 0;
            appendMoreItems(); // Initial batch
            
            positionAC();
            autocomplete.style.display = 'flex';
        };

        const setActive = (idx) => {
            const items = autocomplete.querySelectorAll('.emoji-ac-item');
            items.forEach(el => el.classList.remove('active'));
            if (idx >= 0 && idx < items.length) {
                items[idx].classList.add('active');
                items[idx].scrollIntoView({ block: 'nearest' });
            }
            acActiveIdx = idx;
        };

        textarea.addEventListener('input', () => renderAC());
        textarea.addEventListener('keydown', (e) => {
            if (autocomplete.style.display === 'none') return;
            if (e.key === 'ArrowDown') {
                e.preventDefault();
                if (acActiveIdx === acDisplayedCount - 1 && acDisplayedCount < acMatches.length) {
                    appendMoreItems();
                }
                const nextIdx = (acActiveIdx + 1) % acMatches.length;
                setActive(nextIdx);
            } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                const nextIdx = (acActiveIdx <= 0) ? acMatches.length - 1 : acActiveIdx - 1;
                if (nextIdx > acDisplayedCount - 1) {
                    while (acDisplayedCount < acMatches.length) appendMoreItems();
                }
                setActive(nextIdx);
            } else if ((e.key === 'Enter' || e.key === 'Tab') && acActiveIdx >= 0) {
                e.preventDefault();
                e.stopImmediatePropagation();
                insertEmojiCode(acMatches[acActiveIdx]);
            } else if (e.key === 'Escape') {
                hideAC();
            }
        });

        // Lazy load on scroll
        autocomplete.addEventListener('scroll', () => {
            const threshold = 50; 
            if (autocomplete.scrollHeight - autocomplete.scrollTop - autocomplete.clientHeight < threshold) {
                appendMoreItems();
            }
        });
        textarea.addEventListener('blur', () => setTimeout(hideAC, 150));
        textarea.addEventListener('click', () => { if (getColon()) renderAC(); });

        const ro = new ResizeObserver(() => {
            if (autocomplete.style.display !== 'none') positionAC();
        });
        ro.observe(textarea);
        window.addEventListener('resize', () => { if (autocomplete.style.display !== 'none') positionAC(); });
        // ─────────────────────────────────────────────────────────────────────

        // ── Helper: img or video element for a given emoji URL ────────────────
        function makeDmEmojiEl(url, name) {
            const isVideo = url && url.endsWith('.webm');
            let el;
            if (isVideo) {
                el = document.createElement('video');
                el.src = url;
                el.autoplay = true;
                el.loop = true;
                el.muted = true;
                el.playsInline = true;
            } else {
                el = document.createElement('img');
                el.src = url;
                el.loading = 'lazy';
            }
            el.title = `:${name}:`;
            el.onerror = () => { el.style.display = 'none'; };
            el._stickerSrc = url;
            el._stickerIsVideo = isVideo;
            // Hold-to-preview (uses shared window.stickerPreview from comments.js)
            let holdTimer = null;
            let holdFired = false;
            let isTouchHold = false;
            const startHold = (fromTouch) => {
                holdFired = false;
                isTouchHold = !!fromTouch;
                holdTimer = setTimeout(() => {
                    holdFired = true;
                    window.stickerPreview?.show(url, isVideo, name, url);
                }, 800);
            };
            const cancelHold = () => { clearTimeout(holdTimer); isTouchHold = false; };
            const endHold = () => {
                clearTimeout(holdTimer);
                if (holdFired) {
                    holdFired = false;
                    el.addEventListener('click', (e) => {
                        e.stopImmediatePropagation();
                        e.preventDefault();
                    }, { once: true, capture: true });
                    if (!isTouchHold) window.stickerPreview?.hide();
                    // Mobile: preview stays open until user taps overlay
                } else {
                    window.stickerPreview?.hide();
                }
                isTouchHold = false;
            };
            el.addEventListener('mousedown', () => startHold(false));
            el.addEventListener('mouseup', endHold);
            el.addEventListener('mouseleave', cancelHold);
            // Instant switch when already previewing
            el.addEventListener('mouseenter', () => {
                if (window.stickerPreview?.isShowing) window.stickerPreview.show(url, isVideo, name, url);
            });
            el.addEventListener('touchstart', () => startHold(true), { passive: true });
            el.addEventListener('touchmove', cancelHold, { passive: true });
            el.addEventListener('touchend', endHold);
            el.addEventListener('touchcancel', cancelHold);
            // Desktop-only right-click → fav context menu
            el.addEventListener('contextmenu', (e) => {
                e.preventDefault();
                e.stopPropagation();
                if (navigator.maxTouchPoints > 0) return;
                if (holdFired) return;
                window._favContextMenu?.show(e.clientX, e.clientY, name, url, () => {
                    if (activeTabId === '__favs__') showDmTab('__favs__');
                });
            });
            return el;
        }

        // Ensure fav utils are available even when no comment section exists on this page
        if (!window._emojiPickerFavUtils) {
            window._emojiPickerFavUtils = {
                KEY: 'f0ck_emoji_favs',
                get()  { try { return JSON.parse(localStorage.getItem(this.KEY) || '[]'); } catch(e) { return []; } },
                has(name) { return this.get().some(f => f.name === name); },
                toggle(name, url) {
                    const favs = this.get();
                    const idx = favs.findIndex(f => f.name === name);
                    const action = idx >= 0 ? 'remove' : 'add';
                    if (idx >= 0) favs.splice(idx, 1); else favs.push({ name, url });
                    localStorage.setItem(this.KEY, JSON.stringify(favs));
                    const csrf = window.f0ckSession?.csrf_token;
                    if (csrf) {
                        fetch('/api/v2/user/emoji-favorites/toggle', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrf, 'X-Requested-With': 'XMLHttpRequest' },
                            body: JSON.stringify({ name, url, action })
                        }).catch(() => {});
                    }
                },
                syncFromDB() {
                    fetch('/api/v2/user/emoji-favorites', { headers: { 'X-Requested-With': 'XMLHttpRequest' } })
                        .then(r => r.ok ? r.json() : null)
                        .then(data => {
                            if (data?.success && Array.isArray(data.favorites)) {
                                localStorage.setItem(this.KEY, JSON.stringify(data.favorites));
                                window._emojiPickerRefresh?.();
                            }
                        }).catch(() => {});
                }
            };
            if (window.f0ckSession?.logged_in) window._emojiPickerFavUtils.syncFromDB();
        }

        let activeTabId = null;
        let dmTabBar = null;
        let dmGridArea = null;

        function showDmTab(packId, packs) {
            activeTabId = packId;
            if (dmTabBar) dmTabBar.querySelectorAll('.ep-tab').forEach(t =>
                t.classList.toggle('active', t.dataset.packId === String(packId ?? 'null'))
            );
            if (!dmGridArea) return;
            dmGridArea.innerHTML = '';

            if (packId === '__favs__') {
                const favs = window._emojiPickerFavUtils?.get() || [];
                if (!favs.length) {
                    dmGridArea.innerHTML = '<div style="padding:10px;color:rgba(255,255,255,0.4);font-size:0.8em;text-align:center;">No favorites yet.<br>Hold a sticker to preview, then tap ★</div>';
                    return;
                }
                favs.forEach(({ name, url }) => {
                    const el = makeDmEmojiEl(url, name);
                    el.addEventListener('click', (ev) => {
                        ev.stopPropagation();
                        const pos = textarea.selectionStart ?? textarea.value.length;
                        const val = textarea.value;
                        textarea.value = val.slice(0, pos) + `:${name}:` + val.slice(pos);
                        textarea.focus();
                        const newPos = pos + name.length + 2;
                        textarea.setSelectionRange(newPos, newPos);
                    });
                    dmGridArea.appendChild(el);
                });
                return;
            }

            const pack = packs.find(p => String(p.id ?? null) === String(packId ?? null)) || packs[0];
            if (!pack) return;
            pack.emojis.forEach(({ name, url }) => {
                const el = makeDmEmojiEl(url, name);
                el.addEventListener('click', (ev) => {
                    ev.stopPropagation();
                    const pos = textarea.selectionStart ?? textarea.value.length;
                    const val = textarea.value;
                    textarea.value = val.slice(0, pos) + `:${name}:` + val.slice(pos);
                    textarea.focus();
                    const newPos = pos + name.length + 2;
                    textarea.setSelectionRange(newPos, newPos);
                });
                dmGridArea.appendChild(el);
            });
            // Autoplay webm stickers
            dmGridArea.querySelectorAll('video').forEach(v =>
                v.play().catch(() => v.addEventListener('canplay', () => v.play().catch(() => {}), { once: true }))
            );
            // Touch slide: switch preview as finger moves
            dmGridArea.addEventListener('touchmove', (e) => {
                if (!window.stickerPreview?.isShowing) return;
                const touch = e.touches[0];
                const target = document.elementFromPoint(touch.clientX, touch.clientY);
                if (target && target !== dmGridArea && (target.tagName === 'IMG' || target.tagName === 'VIDEO') && target._stickerSrc) {
                    window.stickerPreview.show(target._stickerSrc, target._stickerIsVideo);
                }
            }, { passive: true });
        }

        function buildDmPickerContent(emojis, packs) {
            picker.innerHTML = '';
            picker.classList.remove('has-tabs');

            const hasPacks = packs && packs.length > 0;

            if (!hasPacks) {
                // Flat list — old behaviour
                Object.entries(emojis).forEach(([name, url]) => {
                    const el = makeDmEmojiEl(url, name);
                    el.addEventListener('click', (ev) => {
                        ev.stopPropagation();
                        const pos = textarea.selectionStart ?? textarea.value.length;
                        const val = textarea.value;
                        textarea.value = val.slice(0, pos) + ` :${name}: ` + val.slice(pos);
                        textarea.focus();
                        textarea.selectionStart = textarea.selectionEnd = pos + name.length + 4;
                    });
                    picker.appendChild(el);
                });
                return;
            }

            // ── Tabbed picker ──
            picker.classList.add('has-tabs');

            dmTabBar = document.createElement('div');
            dmTabBar.className = 'emoji-picker-tabs';

            dmGridArea = document.createElement('div');
            dmGridArea.className = 'emoji-picker-grid';

            // Helper: reliable touch handling for tab buttons on mobile (supports hold-to-action)
            let lastDmHoldTime = 0;
            const addDmTabTouch = (btn, cb, onHold) => {
                let ts = null;
                let holdTimer = null;
                let holdFired = false;
                btn.addEventListener('touchstart', (e) => {
                    holdFired = false;
                    ts = { x: e.touches[0].clientX, y: e.touches[0].clientY };
                    if (onHold) {
                        holdTimer = setTimeout(() => {
                            holdFired = true;
                            lastDmHoldTime = Date.now();
                            try { navigator.vibrate?.(40); } catch(err) {}
                            onHold();
                        }, 500);
                    }
                    e.stopPropagation();
                }, { passive: true });

                btn.addEventListener('touchmove', (e) => {
                    if (!ts) return;
                    const dx = Math.abs(e.touches[0].clientX - ts.x);
                    const dy = Math.abs(e.touches[0].clientY - ts.y);
                    if (dx > 10 || dy > 10) {
                        clearTimeout(holdTimer);
                        ts = null;
                    }
                }, { passive: true });

                btn.addEventListener('touchend', (e) => {
                    clearTimeout(holdTimer);
                    if (holdFired) {
                        e.preventDefault();
                        e.stopPropagation();
                        holdFired = false;
                        ts = null;
                        return;
                    }
                    if (!ts) return;
                    const dx = Math.abs(e.changedTouches[0].clientX - ts.x);
                    const dy = Math.abs(e.changedTouches[0].clientY - ts.y);
                    ts = null;
                    if (dx < 10 && dy < 10) { e.preventDefault(); cb(); }
                });

                btn.addEventListener('touchcancel', () => {
                    clearTimeout(holdTimer);
                    holdFired = false;
                    ts = null;
                });
            };

            const toggleDmFavDefault = (tabEl) => {
                const KEY = 'f0ck_emoji_default_tab';
                const isDefault = localStorage.getItem(KEY) === '__favs__';
                if (isDefault) {
                    localStorage.removeItem(KEY);
                    tabEl.classList.remove('ep-tab-default');
                    window._showEmojiToast?.('Favorites unset as default tab');
                } else {
                    localStorage.setItem(KEY, '__favs__');
                    tabEl.classList.add('ep-tab-default');
                    window._showEmojiToast?.('Favorites set as default tab');
                }
            };

            // ── Favorites tab ──
            const favsTab = document.createElement('button');
            favsTab.className = 'ep-tab ep-tab-favs';
            favsTab.dataset.packId = '__favs__';
            favsTab.title = 'Favorites';
            favsTab.type = 'button';
            favsTab.innerHTML = '<i class="fa-solid fa-star" style="font-size:14px;color:var(--emoji-fav-color,#f5c518);"></i>';
            favsTab.addEventListener('mousedown', e => e.preventDefault());
            favsTab.addEventListener('click', () => showDmTab('__favs__', packs));
            addDmTabTouch(favsTab, () => showDmTab('__favs__', packs), () => toggleDmFavDefault(favsTab));
            // Right-click on fav tab → set/unset as default
            favsTab.addEventListener('contextmenu', (e) => {
                e.preventDefault();
                e.stopPropagation();
                if (Date.now() - lastDmHoldTime < 1000) return;
                toggleDmFavDefault(favsTab);
            });
            // Mark as default if already set
            if (localStorage.getItem('f0ck_emoji_default_tab') === '__favs__') {
                favsTab.classList.add('ep-tab-default');
            }
            dmTabBar.appendChild(favsTab);

            // ── Pack tabs ──
            packs.forEach((pack) => {
                const tab = document.createElement('button');
                tab.className = 'ep-tab';
                tab.dataset.packId = String(pack.id ?? null);
                tab.title = pack.name || 'Emojis';
                tab.type = 'button';
                if (pack.emojis && pack.emojis.length > 0) {
                    const iconUrl = pack.thumb_url
                        || (pack.emojis.find(e => !e.url.endsWith('.webm')) || pack.emojis[0]).url;
                    const icon = document.createElement('img');
                    icon.src = iconUrl;
                    icon.alt = pack.name;
                    icon.loading = 'lazy';
                    tab.appendChild(icon);
                } else {
                    tab.textContent = '☺';
                }
                tab.addEventListener('mousedown', e => e.preventDefault());
                tab.addEventListener('click', () => showDmTab(pack.id ?? null, packs));
                addDmTabTouch(tab, () => showDmTab(pack.id ?? null, packs));
                dmTabBar.appendChild(tab);
            });

            picker.appendChild(dmTabBar);
            picker.appendChild(dmGridArea);
            // Show default tab (favorites if set, otherwise first pack)
            const defaultTab = localStorage.getItem('f0ck_emoji_default_tab');
            showDmTab(defaultTab === '__favs__' ? '__favs__' : (packs[0]?.id ?? null), packs);

            // Expose refresh hook so syncFromDB can update the open favs tab
            window._emojiPickerRefresh = () => {
                if (activeTabId === '__favs__') showDmTab('__favs__', packs);
            };
        }

        function populatePicker() {
            // Only use cache if BOTH emojis and packs are present — otherwise fetch fresh
            // so the tab bar is always built from the full /api/v2/emojis response.
            const cachedPacks  = (typeof CommentSystem !== 'undefined' && CommentSystem.emojiPacks) || null;
            const cachedEmojis = (typeof CommentSystem !== 'undefined' && CommentSystem.emojiCache) || null;

            if (cachedEmojis && Object.keys(cachedEmojis).length && cachedPacks && cachedPacks.length > 0) {
                buildDmPickerContent(cachedEmojis, cachedPacks);
                return;
            }

            // Fetch fresh — ensures packs are always included
            picker.innerHTML = '<div style="padding:8px;color:#aaa;font-size:0.82em;">Loading…</div>';
            fetch('/api/v2/emojis')
                .then(r => r.json())
                .then(data => {
                    if (data.success && data.emojis?.length) {
                        const map = {};
                        data.emojis.forEach(e => { map[e.name] = e.url; });
                        if (typeof CommentSystem !== 'undefined') {
                            CommentSystem.emojiCache = map;
                            CommentSystem.emojiPacks = data.packs || [];
                        }
                        buildDmPickerContent(map, data.packs || []);
                    } else {
                        picker.innerHTML = '<div style="padding:8px;color:#aaa;font-size:0.82em;">No emojis yet</div>';
                    }
                })
                .catch(() => {
                    picker.innerHTML = '<div style="padding:8px;color:#aaa;font-size:0.82em;">Failed to load emojis</div>';
                });
        }

        let closeHandler = null;
        trigger.addEventListener('click', (e) => {
            e.preventDefault();
            const isHidden = picker.style.display === 'none';
            if (isHidden) {
                populatePicker();
                picker.style.display = '';
                requestAnimationFrame(() => requestAnimationFrame(() =>
                    window.scrollTo({ top: document.body.scrollHeight, behavior: 'smooth' })
                ));
                closeHandler = (ev) => {
                    if (!picker.contains(ev.target) && ev.target !== trigger) {
                        picker.style.display = 'none';
                        document.removeEventListener('click', closeHandler);
                        closeHandler = null;
                    }
                };
                setTimeout(() => document.addEventListener('click', closeHandler), 0);
            } else {
                picker.style.display = 'none';
                if (closeHandler) document.removeEventListener('click', closeHandler);
            }
        });

        // Refresh picker when emojis are ready (e.g. first page load)
        window.addEventListener('f0ck:emojis_ready', () => {
            if (picker.style.display !== 'none') populatePicker();
        });
    }

    function setupSendForm() {
        const form  = document.getElementById('dm-send-form');
        if (!form) return;

        const input = form.querySelector('#dm-input');
        const btn   = form.querySelector('#dm-send-btn');

        // Auto-resize textarea
        input.addEventListener('input', () => {
            input.style.height = 'auto';
            input.style.height = Math.min(input.scrollHeight, 140) + 'px';
        });

        // Pre-fill from ?share=URL — set by the scroller share panel
        const shareUrl = new URLSearchParams(window.location.search).get('share');
        if (shareUrl && !input.value) {
            input.value = shareUrl;
            // Trigger resize
            input.style.height = 'auto';
            input.style.height = Math.min(input.scrollHeight, 140) + 'px';
            // Enable send button
            if (btn) btn.disabled = false;
            // Move cursor to end and focus
            input.focus();
            input.setSelectionRange(input.value.length, input.value.length);
        }

        // Enter sends; Shift+Enter inserts newline; Ctrl+Enter also sends
        input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                form.requestSubmit();
            }
        });

        form.addEventListener('submit', async (e) => {
            e.preventDefault();
            if (sendInFlight) return; // prevent double-submit
            const rawText = input.value.trim();
            if (!rawText) return;

            // Prepend reply quote if active
            const text = activeReply
                ? `${activeReply.quoteLines.join('\n')}\n\n${rawText}`
                : rawText;

            if (!window.f0ckSession?.dm_unencrypted) {
                // If recipient has no key, block sending — we cannot encrypt for them
                if (!currentOtherPubKey) {
                    // Re-check in case they registered since page load
                    currentOtherPubKey = await getRemotePublicKey(currentOtherId);
                }
                if (!currentOtherPubKey) {
                    showFlashMsg('This user hasn\'t set up their encryption key yet. Sending is not possible until they do.', 'error');
                    return;
                }
            }

            sendInFlight = true;
            btn.disabled = true;
            try {
                let ciphertext, iv;
                if (window.f0ckSession?.dm_unencrypted) {
                    ciphertext = text;
                    iv = 'unencrypted';
                } else {
                    const sharedKey = await deriveSharedKey(_privateKey, currentOtherPubKey);
                    const enc = await encryptMessage(sharedKey, text);
                    ciphertext = enc.ciphertext;
                    iv = enc.iv;
                }
                const res = await dmFetch('POST', `/api/dm/send/${currentOtherId}`, { ciphertext, iv });
                if (res.success) {
                    input.value = '';
                    input.style.height = '';
                    clearReply();

                    // Optimistic render with the real server ID so dedup works
                    const thread = document.getElementById('dm-thread');
                    if (thread && !renderedIds.has(res.id)) {
                        renderedIds.add(res.id);
                        if (res.id > latestMsgId) latestMsgId = res.id;
                        const msg = { id: res.id, sender_id: myId, plaintext: text, created_at: res.created_at || new Date().toISOString() };
                        const sentBubble = buildMessageBubble(msg);
                        thread.appendChild(sentBubble);
                        playDmEmojiVideos(sentBubble);
                        snapToBottom(thread, true); // Force scroll for your own sent message
                    }
                } else {
                    showFlashMsg(res.msg || 'Failed to send', 'error');
                }
            } catch (err) {
                showFlashMsg('Encryption error: ' + err.message, 'error');
            } finally {
                sendInFlight = false;
                btn.disabled = false;
                input.focus();
            }
        });
    }


    // ─────────────────────────────────────────────────────────────────────────
    // Global key init — runs on every page for logged-in users
    // so keys are always registered before someone tries to DM them.
    // ─────────────────────────────────────────────────────────────────────────

    async function ensureKeyReady() {
        if (window.f0ckSession?.dm_unencrypted) {
            const btns = document.querySelectorAll('.dm-manage-keys-btn');
            btns.forEach(btn => btn.style.display = 'none');
            return;
        }
        const status = await loadOrCreateKeyPair();
        // Always re-upload public key silently (on every page, so recipients can find us)
        uploadPublicKey().catch(e => console.warn('[DM] pubkey upload failed:', e));

        // Modals only on /messages — don't interrupt the user elsewhere
        const onMessagesPage = window.location.pathname.startsWith('/messages');
        if (!onMessagesPage) return;

        if (status === 'vault') {
            // Blocking: must recover key from seed phrase before anything works
            await showRecoveryModal();
        } else if (status === 'new') {
            // Blocking: must back up new key before sending messages
            await showSeedSetupModal();
        } else if (status === 'local') {
            // Key is in LS — but verify the server vault also exists.
            // If not, the user hasn't backed up yet (e.g. modal failed on first load).
            try {
                const vdata = await dmFetch('GET', '/api/dm/keyvault');
                if (!vdata.success) await showSeedSetupModal();
            } catch { /* offline — skip */ }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Seed phrase setup modal (shown once on key generation — cannot be dismissed)
    // ─────────────────────────────────────────────────────────────────────────

    function showSeedSetupModal() {
        return new Promise(resolve => {
            const tryBuild = () => {
                let words;
                try { words = generateMnemonic(); }
                catch (e) {
                    console.error('[DM] generateMnemonic failed:', e);
                    // Don't silently resolve — show a fallback error notice
                    const notice = document.createElement('div');
                    notice.style.cssText = 'position:fixed;bottom:16px;left:50%;transform:translateX(-50%);background:#d94f4f;color:#fff;padding:12px 20px;border-radius:8px;z-index:20001;font-size:0.9em';
                    notice.textContent = 'Recovery phrase wordlist failed to load. Please refresh the page.';
                    document.body.appendChild(notice);
                    return; // leave Promise pending so page is blocked
                }
                buildSeedModal(words, resolve);
            };
            // A failed load leaves the list missing: generateMnemonic then shows the error notice
            loadWordlist().catch((e) => console.error('[DM]', e)).then(tryBuild);
        });
    }

    function buildSeedModal(words, resolve) {

            const modal = document.createElement('div');
            modal.id = 'dm-seed-modal';
            modal.className = 'dm-modal-overlay dm-modal-blocking';
            modal.innerHTML = `
                <div class="dm-modal">
                    <h2>Secure your messages</h2>
                    <p class="dm-modal-sub">Your messages are end-to-end encrypted. Write down the following 12 words — they are your only way to regain access on a new device.</p>
                    <div class="dm-seed-grid" id="dm-seed-words"></div>
                    <button class="dm-key-btn dm-copy-seed" id="dm-copy-seed-btn">Copy words</button>
                    <div class="dm-seed-confirm">
                        <label>
                            <input type="checkbox" id="dm-seed-ack">
                            <span>I have written down all 12 words and stored them safely.</span>
                        </label>
                    </div>
                    <div class="dm-key-msg" id="dm-seed-msg"></div>
                    <button class="dm-key-btn dm-key-btn-primary" id="dm-seed-continue" disabled>Continue →</button>
                    <hr style="border-color:rgba(255,255,255,0.1);margin:16px 0">
                    <p style="font-size:0.82em;color:#888">Already have a recovery phrase from a previous session?</p>
                    <button class="dm-key-btn" id="dm-seed-switch-recover" style="background:rgba(255,255,255,0.07);border:1px solid #444;color:var(--fg,#ddd);width:100%">Enter existing recovery phrase</button>
                </div>
            `;

            // Prevent any dismiss (ESC, overlay click) — this is forced
            modal.addEventListener('click', e => e.stopPropagation());
            document.addEventListener('keydown', trapEsc, true);
            function trapEsc(e) { if (e.key === 'Escape') e.stopImmediatePropagation(); }

            const grid = modal.querySelector('#dm-seed-words');
            words.forEach((w, i) => {
                const el = document.createElement('div');
                el.className = 'dm-seed-word';
                el.innerHTML = `<span class="dm-seed-num">${i + 1}</span><span class="dm-seed-val">${escHtml(w)}</span>`;
                grid.appendChild(el);
            });

            modal.querySelector('#dm-copy-seed-btn').onclick = () => {
                navigator.clipboard.writeText(words.join(' ')).then(() => {
                    setMsg(modal.querySelector('#dm-seed-msg'), 'Words copied to clipboard.', 'ok');
                });
            };

            modal.querySelector('#dm-seed-ack').onchange = (e) => {
                modal.querySelector('#dm-seed-continue').disabled = !e.target.checked;
            };

            modal.querySelector('#dm-seed-continue').onclick = async () => {
                const btn = modal.querySelector('#dm-seed-continue');
                btn.disabled = true;
                btn.textContent = 'Saving…';
                try {
                    const privJwk = localStorage.getItem(DM_KEY_NAME);
                    const vault   = await encryptPrivkeyForVault(privJwk, words);
                    const res     = await dmFetch('POST', '/api/dm/keyvault', vault);
                    if (!res.success) throw new Error(res.msg || 'Save failed');
                    document.removeEventListener('keydown', trapEsc, true);
                    modal.remove();
                    resolve();
                } catch (err) {
                    setMsg(modal.querySelector('#dm-seed-msg'), err.message, 'err');
                    btn.disabled = false;
                    btn.textContent = 'Continue →';
                }
            };

            // Switch to recovery flow instead of setting up a new key
            modal.querySelector('#dm-seed-switch-recover').onclick = async () => {
                document.removeEventListener('keydown', trapEsc, true);
                modal.remove();
                // Clean up the freshly generated key — recovery will replace it
                localStorage.removeItem(DM_KEY_NAME);
                localStorage.removeItem(DM_PUBKEY_NAME);
                localStorage.removeItem(DM_KEY_VERSION);
                _privateKey = null; _publicKeyJwk = null;
                await showRecoveryModal();
                // REFRESH UI after recovery
                if (typeof initMessagesPage === 'function') await initMessagesPage();
                resolve();
            };

            document.body.appendChild(modal);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Recovery modal (shown when vault exists but no local key)
    // ─────────────────────────────────────────────────────────────────────────

    function showRecoveryModal({ dismissible = false } = {}) {
        return new Promise(resolve => {
            const modal = document.createElement('div');
            modal.id = 'dm-recovery-modal';
            modal.className = 'dm-modal-overlay' + (dismissible ? '' : ' dm-modal-blocking');
            modal.innerHTML = `
                <div class="dm-modal">
                    ${dismissible ? '<button class="dm-modal-close" id="dm-recovery-close">\u00d7</button>' : ''}
                    <h2>Restore your key</h2>
                    <p class="dm-modal-sub">Enter your 12 recovery words to regain access to your messages.</p>
                    <div class="dm-recovery-grid" id="dm-recovery-inputs"></div>
                    <div class="dm-key-msg" id="dm-recovery-msg"></div>
                    <button class="dm-key-btn dm-key-btn-primary" id="dm-recovery-btn">Restore key</button>
                    <hr style="border-color:rgba(255,255,255,0.1);margin:16px 0">
                    <p style="font-size:0.82em;color:#888">Forgot your words? You can create a new key — <strong>all old messages will become unreadable.</strong></p>
                    <button class="dm-key-btn dm-key-btn-danger" id="dm-recovery-reset">Start fresh (lose old messages)</button>
                </div>
            `;

            const dismiss = () => {
                document.removeEventListener('keydown', trapEsc2, true);
                modal.remove();
                resolve();
            };

            if (dismissible) {
                modal.querySelector('#dm-recovery-close').onclick = dismiss;
                modal.addEventListener('click', e => { if (e.target === modal) dismiss(); });
            } else {
                modal.addEventListener('click', e => e.stopPropagation());
            }

            document.addEventListener('keydown', trapEsc2, true);
            function trapEsc2(e) {
                if (e.key === 'Escape') {
                    if (dismissible) { e.stopImmediatePropagation(); dismiss(); }
                    else e.stopImmediatePropagation();
                }
            }

            const grid = modal.querySelector('#dm-recovery-inputs');
            for (let i = 0; i < 12; i++) {
                const wrap = document.createElement('div');
                wrap.className = 'dm-seed-word dm-seed-input-wrap';
                wrap.innerHTML = `<span class="dm-seed-num">${i + 1}</span><input type="text" class="dm-recovery-word" autocomplete="off" autocorrect="off" spellcheck="false" placeholder="Word ${i + 1}">`;
                grid.appendChild(wrap);
            }

            const attemptRecovery = async () => {
                const inputs = [...modal.querySelectorAll('.dm-recovery-word')];
                const words  = inputs.map(i => i.value.trim().toLowerCase()).filter(Boolean);
                const msg    = modal.querySelector('#dm-recovery-msg');
                const btn    = modal.querySelector('#dm-recovery-btn');
                if (words.length < 12) { setMsg(msg, 'Please enter all 12 words.', 'err'); return; }

                btn.disabled = true; btn.textContent = 'Decrypting…';
                try {
                    const vdata = await dmFetch('GET', '/api/dm/keyvault');
                    if (!vdata.success) throw new Error('No vault found');
                    const privJwk = await decryptPrivkeyFromVault(vdata.vault, words);
                    await storeKeyFromJwk(privJwk);
                    await uploadPublicKey();
                    document.removeEventListener('keydown', trapEsc2, true);
                    modal.remove();
                    resolve();
                } catch {
                    setMsg(msg, 'Wrong words or corrupted vault. Please try again.', 'err');
                    btn.disabled = false; btn.textContent = 'Restore key';
                }
            };

            modal.querySelector('#dm-recovery-btn').onclick = attemptRecovery;

            const allInputs = [...modal.querySelectorAll('.dm-recovery-word')];

            allInputs.forEach((inp, idx) => {
                // Enter key → attempt recovery
                inp.addEventListener('keydown', e => { if (e.key === 'Enter') attemptRecovery(); });

                // Helper: distribute word list across inputs starting at idx
                const distributeWords = (parts, startIdx) => {
                    parts.forEach((word, i) => {
                        if (allInputs[startIdx + i]) allInputs[startIdx + i].value = word.toLowerCase();
                    });
                    const nextEmpty = allInputs.find(f => !f.value.trim());
                    if (nextEmpty) nextEmpty.focus();
                    else modal.querySelector('#dm-recovery-btn').focus();
                };

                // Paste event: works on desktop and some mobile browsers
                inp.addEventListener('paste', e => {
                    const clip = e.clipboardData || window.clipboardData;
                    const text = clip?.getData('text/plain') || clip?.getData('text') || '';
                    const parts = text.trim().split(/\s+/).filter(Boolean);
                    if (parts.length > 0) {
                        e.preventDefault(); // Block native paste — we handle it
                        if (parts.length > 1) {
                            distributeWords(parts, idx);
                        } else {
                            inp.value = parts[0].toLowerCase();
                            if (allInputs[idx + 1]) allInputs[idx + 1].focus();
                        }
                    }
                    // If clip was empty, fall through — `input` event below catches it
                });

                // Input event: catches keyboard-paste on mobile which skips the paste event.
                // Users only type one word per field, so multiple words → must be a paste.
                inp.addEventListener('input', () => {
                    const parts = inp.value.trim().split(/\s+/).filter(Boolean);
                    if (parts.length > 1) {
                        distributeWords(parts, idx);
                    }
                });

            });

            modal.querySelector('#dm-recovery-reset').onclick = async () => {
                if (!confirm('Really start fresh? All old messages will become unreadable.')) return;
                await dmFetch('DELETE', '/api/dm/keyvault');
                await dmFetch('DELETE', '/api/dm/all'); // wipe own messages
                const pair    = await generateKeyPair();
                const privJwk = await exportKey(pair.privateKey);
                const pubJwk  = await exportKey(pair.publicKey);
                localStorage.setItem(DM_KEY_NAME, privJwk);
                localStorage.setItem(DM_PUBKEY_NAME, pubJwk);
                localStorage.setItem(DM_KEY_VERSION, String(CURRENT_VERSION));
                _privateKey   = pair.privateKey;
                _publicKeyJwk = pubJwk;
                document.removeEventListener('keydown', trapEsc2, true);
                modal.remove();
                // Show seed setup for the new key
                await showSeedSetupModal();
                resolve();
            };

            document.body.appendChild(modal);
        });
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Key Manager Modal (simplified — shows vault status, allows regenerate)
    // ─────────────────────────────────────────────────────────────────────────

    function openKeyManager() {
        let modal = document.getElementById('dm-key-modal');
        if (!modal) { modal = buildKeyManagerModal(); document.body.appendChild(modal); }
        modal.style.display = 'flex';
    }

    function buildKeyManagerModal() {
        const modal = document.createElement('div');
        modal.id = 'dm-key-modal';
        modal.className = 'dm-modal-overlay';
        modal.innerHTML = `
            <div class="dm-modal">
                <button class="dm-modal-close" id="dm-key-modal-close">×</button>
                <h2>Encryption Key</h2>
                <p class="dm-modal-sub">Your private key is secured with your recovery phrase and stored encrypted on the server.</p>

                <div class="dm-key-status" id="dm-key-status">${hasKey() ? 'Key loaded and backed up.' : 'No key found.'}</div>

                <div class="dm-key-section">
                    <h3>Recover from phrase</h3>
                    <p>Already have a recovery phrase? Enter it here to restore access to previous messages on this device.</p>
                    <button class="dm-key-btn" id="dm-recover-phrase-btn" style="background:rgba(255,255,255,0.07);border:1px solid #444;color:var(--fg,#ddd);width:100%">Enter recovery phrase</button>
                    <div class="dm-key-msg" id="dm-recover-phrase-msg"></div>
                </div>


            </div>
        `;

        modal.querySelector('#dm-key-modal-close').onclick = () => modal.style.display = 'none';
        modal.onclick = (e) => { if (e.target === modal) modal.style.display = 'none'; };
        document.addEventListener('keydown', e => { if (e.key === 'Escape' && modal.style.display !== 'none') modal.style.display = 'none'; });

        // Recover from existing phrase
        modal.querySelector('#dm-recover-phrase-btn').onclick = async () => {
            modal.style.display = 'none';
            await showRecoveryModal({ dismissible: true });

            // REFRESH UI after recovery
            if (typeof initMessagesPage === 'function') await initMessagesPage();

            // Refresh status label on re-open
            const statusEl = modal.querySelector('#dm-key-status');
            if (statusEl) statusEl.textContent = hasKey() ? 'Key loaded and backed up.' : 'No key found.';
        };



        return modal;
    }

    function setMsg(el, text, type) {
        el.textContent  = text;
        el.className    = `dm-key-msg dm-msg-${type}`;
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Navbar DM badge
    // ─────────────────────────────────────────────────────────────────────────

    function updateDmBadge(count) {
        const badge = document.getElementById('dm-badge');
        if (badge) {
            badge.textContent   = count > 99 ? '99+' : String(count);
            badge.style.display = count > 0 ? 'inline-flex' : 'none';
        }
        // Sync page title badge on every page
        _dmTitleCount = Math.max(0, count);
        applyDmTitleBadge();
    }

    function incrementDmBadge() {
        const badge = document.getElementById('dm-badge');
        if (!badge) return;
        const current = parseInt(badge.textContent, 10) || 0;
        updateDmBadge(current + 1);
    }

    async function refreshDmBadge() {
        try {
            const data = await dmFetch('GET', '/api/dm/unread');
            if (data.success) updateDmBadge(data.count);
        } catch (_) {}
    }

    // ─────────────────────────────────────────────────────────────────────────
    // SSE integration (bridge from NotificationSystem in f0ckm.js)
    // ─────────────────────────────────────────────────────────────────────────

    function hookSSE() {
        // incrementDmBadge() is called directly from f0ckm.js SSE handler for instant update.
        // We only listen here to update chat thread live when we're in a conversation.
        // (badge refresh after conversation read is handled by initConversation)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // User profile "Send DM" button
    // ─────────────────────────────────────────────────────────────────────────

    function setupProfileDmBtn() {
        const btn = document.getElementById('send-dm-btn');
        if (!btn) return;
        btn.onclick = () => {
            const username = btn.dataset.username;
            if (username) window.location.href = `/messages/${encodeURIComponent(username.toLowerCase())}`;
        };
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Utilities
    // ─────────────────────────────────────────────────────────────────────────

    function escHtml(str) {
        if (!str) return '';
        const div = document.createElement('div');
        div.textContent = str;
        return div.innerHTML;
    }


    function timeAgo(dateStr) {
        if (window.f0ckTimeAgo) return window.f0ckTimeAgo(dateStr);
        const sec = Math.floor((Date.now() - new Date(dateStr).getTime()) / 1000);
        if (sec < 60)    return 'just now';
        if (sec < 3600)  return `${Math.floor(sec / 60)}m ago`;
        if (sec < 86400) return `${Math.floor(sec / 3600)}h ago`;
        return `${Math.floor(sec / 86400)}d ago`;
    }

    function showFlashMsg(msg, type) {
        if (window.showFlash) window.showFlash(msg, type === 'error' ? 'error' : 'success');
        else console.warn('[DM]', msg);
    }


    // ─────────────────────────────────────────────────────────────────────────
    // Tab Activation (Catch up on missed messages/badges)
    // ─────────────────────────────────────────────────────────────────────────

    const handleActivation = () => {
        if (document.hidden) return;

        const thread = document.getElementById('dm-thread');
        if (thread && currentOtherId && parseInt(thread.dataset.otherId) === currentOtherId) {
            window.f0ckDebug('[DM] Tab active: catching up on conversation...');
            appendNewMessages(thread).then(() => {
                dmFetch('POST', `/api/dm/read/${currentOtherId}`)
                    .then(() => refreshDmBadge()) // refreshDmBadge clears title via updateDmBadge(0)
                    .catch(() => {});
            });
        }
    };

    document.addEventListener('visibilitychange', handleActivation);
    window.addEventListener('focus', handleActivation);

    // ─────────────────────────────────────────────────────────────────────────
    // Init
    // ─────────────────────────────────────────────────────────────────────────


    async function init() {
        hookSSE();
        setupProfileDmBtn();

        // All .dm-manage-keys-btn hooks
        document.addEventListener('click', (e) => {
            if (e.target.closest('.dm-manage-keys-btn')) openKeyManager();
        });

        // Global DM listener for live updates.
        // We only mark as read if the user is actively viewing this specific thread.
        window.addEventListener('dm:incoming', async (e) => {
            const thread = document.getElementById('dm-thread');
            // Use Number() to avoid type mismatch between SSE string and local integer
            if (thread && currentOtherId && Number(e.detail.sender_id) === currentOtherId) {
                await appendNewMessages(thread);

                if (!document.hidden) {
                    // Tab is visible — mark as read immediately
                    dmFetch('POST', `/api/dm/read/${currentOtherId}`)
                        .then(() => refreshDmBadge())
                        .catch(() => {});
                }
                // Badge (and title) increment is handled by incrementDmBadge() in f0ckm.js
            }
        });

        // Re-render DMs when emojis are ready
        window.addEventListener('f0ck:emojis_ready', () => {
            const thread = document.getElementById('dm-thread');
            if (!thread || !threadMessages.length) return;

            window.f0ckDebug('[DM] Emojis ready, re-rendering thread...');
            const isAtBottom = (thread.scrollHeight - thread.scrollTop - thread.clientHeight) < 50;
            
            // Clear and re-render from cache
            renderedIds.clear();
            thread.innerHTML = '';
            
            // Re-render everyone
            for (const m of threadMessages) {
                renderedIds.add(m.id);
                const bubble = buildMessageBubble(m);
                thread.appendChild(bubble);
                playDmEmojiVideos(bubble);
            }

            if (isAtBottom) snapToBottom(thread, true);
        });

        // Generate + upload key silently for every logged-in user on every page.
        // This ensures that by the time someone tries to DM them, a key already exists.
        await ensureKeyReady();

        // Refresh badge
        if (document.getElementById('dm-badge')) {
            await refreshDmBadge();
        }

        // Page-specific init
        if (document.getElementById('dm-inbox-list')) await initInbox();
        if (document.getElementById('dm-thread'))     await initConversation();
    }

    // ─────────────────────────────────────────────────────────────────────────
    // AJAX re-init hook  (called by f0ckm.js after every AJAX navigation)
    // Resets page-level state so the inbox and conversation can re-initialize
    // cleanly without a full page reload.
    // ─────────────────────────────────────────────────────────────────────────

    async function initMessagesPage() {
        // Reset conversation-level state
        currentOtherId     = null;
        currentOtherPubKey = null;
        myId               = null;
        latestMsgId        = 0;
        oldestMsgId        = null;
        threadHasMore      = false;
        renderedIds.clear();
        threadMessages     = [];
        sendInFlight       = false;

        // Clear timestamp ticker on page reset
        if (window._dmTimestampTicker) {
            clearInterval(window._dmTimestampTicker);
            window._dmTimestampTicker = null;
        }

        // Restore base title on every messages page navigation then re-apply badge
        const stripped = document.title.replace(/^\[\d+\]\s*/, '');
        document.title = stripped || 'Messages';
        applyDmTitleBadge();

        // Show seed phrase setup / recovery modal if needed (AJAX navigation lands here)
        await ensureKeyReady();

        // Re-run page-specific init (initConversation sets its own title)
        if (document.getElementById('dm-inbox-list')) await initInbox();
        if (document.getElementById('dm-thread'))     await initConversation();

        // Refresh badge on any messages page
        await refreshDmBadge();
    }

    /**
     * Snap a container to the bottom.
     * Uses double requestAnimationFrame to ensure the browser has finished layout
     * (especially for newly appended images/emojis) before calculating scroll height.
     */
    function snapToBottom(el, force = false) {
        if (!el) return;

        // "Near bottom" threshold: 120px
        const isNearBottom = (el.scrollHeight - el.scrollTop - el.clientHeight) < 120;

        if (force || isNearBottom) {
            // Double-kick ensures the DOM engine has calculated the new total height
            requestAnimationFrame(() => {
                requestAnimationFrame(() => {
                    el.scrollTop = el.scrollHeight;
                });
            });
        }
    }

    /**
     * Watch a thread container and keep snapping to bottom for `durationMs`.
     * Uses pointer/wheel/touch events to detect intentional user scrolling
     * instead of a distance heuristic, so async content (decrypting attachments,
     * images loading) does not fool it into stopping early.
     */
    function snapToBottomSticky(el, durationMs = 8000) {
        if (!el) return;

        let userScrolledUp = false;

        // Detect intentional upward scroll via input devices only
        const onWheel = (e) => { if (e.deltaY < 0) userScrolledUp = true; };
        const onKey   = (e) => { if (['ArrowUp', 'PageUp', 'Home'].includes(e.key)) userScrolledUp = true; };
        let touchStartY = 0;
        const onTouchStart = (e) => { touchStartY = e.touches[0]?.clientY ?? 0; };
        const onTouchMove  = (e) => { if ((e.touches[0]?.clientY ?? 0) > touchStartY + 10) userScrolledUp = true; };

        el.addEventListener('wheel',      onWheel,       { passive: true });
        el.addEventListener('keydown',    onKey,         { passive: true });
        el.addEventListener('touchstart', onTouchStart,  { passive: true });
        el.addEventListener('touchmove',  onTouchMove,   { passive: true });

        let ro;
        const cleanup = () => {
            el.removeEventListener('wheel',      onWheel);
            el.removeEventListener('keydown',    onKey);
            el.removeEventListener('touchstart', onTouchStart);
            el.removeEventListener('touchmove',  onTouchMove);
            if (ro) ro.disconnect();
        };

        // ResizeObserver: re-snap whenever content grows (images, attachments decrypting)
        if (typeof ResizeObserver !== 'undefined') {
            let debounceTimer = null;
            ro = new ResizeObserver(() => {
                if (userScrolledUp) { cleanup(); return; }
                clearTimeout(debounceTimer);
                debounceTimer = setTimeout(() => {
                    if (!userScrolledUp) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
                }, 80);
            });
            ro.observe(el);
        } else {
            // Fallback for older browsers
            setTimeout(() => snapToBottom(el, true), 300);
            setTimeout(() => snapToBottom(el, true), 800);
            setTimeout(() => snapToBottom(el, true), 2000);
        }

        // Disconnect after the deadline regardless
        setTimeout(cleanup, durationMs);

        // First snaps are instant (panel just opened — no animation needed)
        snapToBottom(el, true);
        setTimeout(() => { if (!userScrolledUp) snapToBottom(el, true); }, 200);
        setTimeout(() => { if (!userScrolledUp) snapToBottom(el, true); }, 600);
    }

    // Expose for external use (template inline scripts, f0ckm.js SSE handler, AJAX nav)
    window.DMSystem = { openKeyManager, updateDmBadge, incrementDmBadge, refreshDmBadge, setupProfileDmBtn };
    window.initMessagesPage = initMessagesPage;

    // ── Global DM Shortcuts ───────────────────────────────────────────────────
    document.addEventListener('keydown', (e) => {
        if (!e.ctrlKey || e.altKey || e.metaKey) return;
        if (e.key !== '.') return;
        
        const input = document.getElementById('dm-input');
        if (input && document.activeElement !== input) {
            e.preventDefault();
            input.focus();
        }
    });

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }

})();

} // end __dmLoaded guard
