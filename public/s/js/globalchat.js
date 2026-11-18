/**
 * globalchat.js — Site-wide persistent chat widget, in every frontend.
 * Panel is always visible. Minimize-only.
 * Features: emoji picker, :emoji autocomplete, @user autocomplete,
 *           greentext, [spoiler], [blur]
 *
 * Loaded by snippets/globalchat.html (f0ckm's footer and snippets/frontend-scripts.html). The look is
 * a skin: the structure is in /s/css/globalchat.css, each frontend styles it in its own CSS under
 * html.gchat-skin-<frontend> (f0ckm: f0ckm.css). The page data blocks:
 *   #gchat-skin        { skin, title, placeholder, open, online, guests, guests_one, users, bar }
 *                      (snippets/globalchat.html); <count>_one: the text for 1 (online_one, guests_one);
 *                      bar: a bar the closed launcher sits beside, its height as --gc-bar-h (f0ckm:
 *                      the right sidebar's footer)
 *   #gchat-skin-texts  … avoid: an element at the page's end to stay above (f0ckgle: its footer):
 *                      how much of it shows at the window's bottom, as --gc-avoid-h (on scroll)
 *   #gchat-skin-texts  the frontend's own texts, optional (views/frontends/<fe>/snippets/chat.html):
 *                      title, placeholder, open, online ("{n}"), guests ("{n}"), badge (chip next to
 *                      the title), greeting (top of the message list, "{name}"), intro (text under
 *                      it), note (small print under the input), icon (title icon: chat | spark)
 * No inline styles: states are classes / the hidden attribute; sizes and positions the visitor drags
 * are custom properties (--gc-w, --gc-h, --gc-x, --gc-y) the CSS uses.
 */
(function () {
    'use strict';

    if (window.__f0ckGlobalChat) return;
    if (!window.f0ckSession?.enable_global_chat) return;
    if (!window.f0ckSession?.logged_in) return;
    if (window.location.pathname.startsWith('/abyss')) return;
    window.__f0ckGlobalChat = true;

    // Embedded chat images hide their wrapper when they fail to load
    window.F0ckActions?.define('gchat-hide-parent', (el) => { if (el.parentNode) el.parentNode.hidden = true; });

    const MAX_VISIBLE_MSGS = 100;
    const RATE_LIMIT_MS    = 800;
    const HEADER_H         = 42;

    let isMinimized  = localStorage.getItem('f0ck_chat_minimized') !== '0';
    let isClosed     = localStorage.getItem('f0ck_chat_closed')    === '1';
    let lastSent     = 0;
    let customEmojis = null; // name → url
    let unreadCount  = 0;
    let chatFocused  = document.hasFocus();
    // Phones (<= 600px): a chat button in the corner opens the chat full screen (html.gchat-m-open);
    // no detach / minimize / resize there, the desktop size and position are kept for desktop
    const phoneMQ    = window.matchMedia('(max-width: 600px)');
    const noHoverMQ  = window.matchMedia('(hover: none)');
    let mobileOpen   = false;
    const isPhone    = () => phoneMQ.matches;
    // whether new messages are seen: the open phone chat, or the expanded desktop panel
    const chatShown  = () => (isPhone() ? mobileOpen : !isMinimized && !isClosed);
    // how much of the bottom edge the docked chat (or the closed launcher) takes: --gc-dock-h
    // (f0ckm: the sidebar's drag handle stays above it); set up in init
    let updateDock   = () => {};
    const ytOembedCache = new Map(); // videoId → {title, author_name}

    // ── i18n + skin ──────────────────────────────────────────────────────────
    function t(key, fallback) { return window.f0ckI18n?.[key] || fallback; }

    function readJson(id) {
        try { return JSON.parse(document.getElementById(id)?.textContent || 'null') || {}; }
        catch (_) { return {}; }
    }
    const skin = Object.assign({
        skin:        'f0ckm',
        title:       t('chat_title', 'Site Chat'),
        placeholder: t('chat_placeholder', 'Chat…'),
        open:        t('chat_open', 'Open chat'),
        online:      t('chat_online', '{n} online'),
        guests:      t('chat_guests', '{n} guests'),
        guests_one:  t('chat_guest', '{n} guest'),
        users:       t('chat_users', "Who's online"),
        icon:        'chat',
        badge: '', greeting: '', intro: '', note: ''
    }, readJson('gchat-skin'));
    for (const [k, v] of Object.entries(readJson('gchat-skin-texts'))) {
        if (typeof v === 'string') skin[k] = v;
    }
    const skinName = String(skin.skin || 'f0ckm').replace(/[^a-z0-9_-]/gi, '');
    document.documentElement.classList.add('gchat-skin-' + skinName);

    // A count text: skin.<key>_one when n is 1 (if the skin has it), else skin.<key>
    function count(key, n) {
        return fill((n === 1 && skin[key + '_one']) || skin[key], { n });
    }

    function fill(str, vars) {
        return String(str || '').replace(/\{(\w+)\}/g, (m, k) => (k in vars ? vars[k] : m));
    }

    // ── Icons (inline SVG, coloured with currentColor) ───────────────────────
    const ICONS = {
        chat:     'M3 4h18v13H8l-5 4z',
        float:    'M14 3h7v7h-2V6.41l-6.29 6.3-1.42-1.42L17.59 5H14zM3 5h8v2H5v12h12v-6h2v8H3z',
        dock:     'M3 3h8v2H5v14h14v-6h2v8H3zM21 4.41 15.41 10H19v2h-7V5h2v3.59L19.59 3z',
        up:       'M7.41 15.41 12 10.83l4.59 4.58L18 14l-6-6-6 6z',
        down:     'M7.41 8.59 12 13.17l4.59-4.58L18 10l-6 6-6-6z',
        close:    'M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z',
        send:     'M2.01 21 23 12 2.01 3 2 10l15 2-15 2z',
        reply:    'M10 9V5l-7 7 7 7v-4.1c5 0 8.5 1.6 11 5.1-1-5-4-10-11-11z',
        trash:    'M6 7h12v14H6zM15.5 4l-1-1h-5l-1 1H5v2h14V4z',
        external: 'M19 19H5V5h7V3H3v18h18v-9h-2zM14 3v2h3.59l-9.83 9.83 1.41 1.41L19 6.41V10h2V3z',
        video:    'M2 4h20v16H2zm8 4v8l6-4z',
        audio:    'M12 3v10.55A4 4 0 1 0 14 17V7h4V3z',
        image:    'M3 3h18v18H3zm2 14h14l-4.5-6-3.5 4.5-2.5-3z',
        user:     'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zm0 2c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z',
        comment:  'M2 2h20v16H6l-4 4z',
        play:     'M8 5v14l11-7z',
        emoji:    'M15.5 11a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zm-7 0a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zm3.5 6.5c2.33 0 4.31-1.46 5.11-3.5H6.89c.8 2.04 2.78 3.5 5.11 3.5zM11.99 2C6.47 2 2 6.48 2 12s4.47 10 9.99 10C17.52 22 22 17.52 22 12S17.52 2 11.99 2zM12 20c-4.42 0-8-3.58-8-8s3.58-8 8-8 8 3.58 8 8-3.58 8-8 8z',
        users:    'M16 11a3 3 0 1 0 0-6 3 3 0 0 0 0 6zm-8 0a3 3 0 1 0 0-6 3 3 0 0 0 0 6zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5C15 14.17 10.33 13 8 13zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 1.97 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z',
        spark:    'M12 2c.6 5.4 4.6 9.4 10 10-5.4.6-9.4 4.6-10 10-.6-5.4-4.6-9.4-10-10 5.4-.6 9.4-4.6 10-10z'
    };
    let sparkSeq = 0;
    function ico(name, cls = '') {
        if (name === 'spark') {
            // The f0ckgle sparkle: its gradient stops are coloured by the skin (.gchat-spark-s1…s3).
            // Its own gradient id each time: one inside a hidden panel wouldn't paint the others.
            const gid = `gchat-spark-g${++sparkSeq}`;
            return `<svg class="gchat-ico gchat-ico-spark ${cls}" viewBox="0 0 24 24" aria-hidden="true" focusable="false">` +
                `<defs><linearGradient id="${gid}" x1="0" y1="1" x2="1" y2="0">` +
                `<stop offset="0" class="gchat-spark-s1"/><stop offset=".5" class="gchat-spark-s2"/><stop offset="1" class="gchat-spark-s3"/>` +
                `</linearGradient></defs><path d="${ICONS.spark}" fill="url(#${gid})"/></svg>`;
        }
        return `<svg class="gchat-ico gchat-ico-${name} ${cls}" viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="${ICONS[name] || ICONS.chat}" fill="currentColor"/></svg>`;
    }

    // Shared IntersectionObserver for lazy-loading embedded images.
    // Images are rendered with data-lazy-src; this observer sets the real src
    // when the image is within 200px of the visible scroll area.
    const lazyImgObserver = new IntersectionObserver((entries) => {
        for (const entry of entries) {
            if (!entry.isIntersecting) continue;
            const img = entry.target;
            const src = img.dataset.lazySrc;
            if (src) { img.src = src; delete img.dataset.lazySrc; }
            lazyImgObserver.unobserve(img);
        }
    }, {
        rootMargin: '200px',   // start loading 200px before entering viewport
        threshold: 0
    });

    function updateBadge() {
        const badge  = document.getElementById('gchat-badge');
        const bubble = document.getElementById('gchat-reopen-bubble');
        const label  = unreadCount > 99 ? '99+' : String(unreadCount);
        if (badge) { badge.textContent = label; badge.hidden = unreadCount <= 0; }
        if (bubble) {
            let bb = bubble.querySelector('.gchat-bubble-badge');
            if (!bb && unreadCount > 0) {
                bb = document.createElement('span');
                bb.className = 'gchat-bubble-badge';
                bubble.appendChild(bb);
            }
            if (bb) { bb.textContent = label; bb.hidden = unreadCount <= 0; }
        }
    }

    function clearUnread() {
        unreadCount = 0;
        updateBadge();
    }

    window.addEventListener('focus', () => { chatFocused = true;  if (!isMinimized) clearUnread(); });
    window.addEventListener('blur',  () => { chatFocused = false; });

    // ── Helpers ───────────────────────────────────────────────────────────────
    // Also quotes: the result goes into attribute values (src="…", alt="…")
    function esc(str) {
        const d = document.createElement('div');
        d.textContent = str || '';
        return d.innerHTML.replace(/"/g, '&quot;').replace(/'/g, '&#39;');
    }

    function avatarSrc(msg) {
        if (msg.avatar_file) return `/a/${msg.avatar_file}`;
        if (msg.avatar)      return `/t/${msg.avatar}.webp`;
        return '/a/default.png';
    }

    // A user colour (from the database) as the one inline value the CSS uses: --user-color
    function userColorAttr(color) {
        return color ? ` style="--user-color:${esc(String(color).replace(/[;{}<>]/g, ''))}"` : '';
    }

    // ── Build widget ─────────────────────────────────────────────────────────
    function introHTML() {
        if (!skin.greeting && !skin.intro) return '';
        const name = window.f0ckSession.display_name || window.f0ckSession.user || '';
        return `<div class="gchat-intro">` +
            (skin.greeting ? `<div class="gchat-greeting">${esc(fill(skin.greeting, { name }))}</div>` : '') +
            (skin.intro ? `<div class="gchat-intro-text">${esc(fill(skin.intro, { name }))}</div>` : '') +
        `</div>`;
    }

    // resize handles: edges and corners (docked, only the free ones: top, left, top-left)
    const RESIZE_DIRS = ['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw'];

    function buildWidget() {
        const widget = document.createElement('div');
        widget.id = 'gchat-widget';
        widget.innerHTML = `
            <div id="gchat-panel" class="${isMinimized ? 'gchat-minimized' : ''}">
                ${RESIZE_DIRS.map(d => `<div class="gchat-rz gchat-rz-${d}" data-rz="${d}"></div>`).join('')}
                <div id="gchat-header">
                    <span id="gchat-title">${ico(skin.icon === 'spark' ? 'spark' : 'chat', 'gchat-title-ico')}<span class="gchat-title-text">${esc(skin.title)}</span>${skin.badge ? `<span class="gchat-chip">${esc(skin.badge)}</span>` : ''}<span id="gchat-badge" class="gchat-badge" hidden></span></span>
                    <div class="gchat-header-btns">
                        <button type="button" id="gchat-float-btn" title="${esc(t('chat_float', 'Float / Dock'))}" class="gchat-icon-btn">${ico('float')}</button>
                        <button type="button" id="gchat-minimize-btn" title="${esc(isMinimized ? t('chat_expand','Expand') : t('chat_minimize','Minimize'))}" class="gchat-icon-btn">${ico(isMinimized ? 'up' : 'down')}</button>
                        <button type="button" id="gchat-close-btn" title="${esc(t('chat_close','Close chat'))}" class="gchat-icon-btn">${ico('close')}</button>
                    </div>
                </div>
                <div id="gchat-topic" hidden></div>
                <div id="gchat-online" role="button" tabindex="0" title="${esc(skin.users)}" hidden></div>
                <div id="gchat-messages">${introHTML()}</div>
                <div id="gchat-input-area">
                    <div id="gchat-emoji-ac" class="gchat-ac" hidden></div>
                    <div id="gchat-mention-ac" class="gchat-ac" hidden></div>
                    <div id="gchat-input-row">
                        <button type="button" class="gchat-tool-btn" id="gchat-emoji-btn" title="Emoji">${ico('emoji')}</button>
                        <textarea id="gchat-input" placeholder="${esc(skin.placeholder)}" maxlength="500" rows="1"></textarea>
                        <button type="button" id="gchat-send-btn" title="${esc(t('common_send','Send'))}">${ico('send')}</button>
                    </div>
                    <div id="gchat-toolbar"><span id="gchat-count" class="gchat-count">0/500</span></div>
                    ${skin.note ? `<div class="gchat-note">${esc(skin.note)}</div>` : ''}
                </div>
            </div>
        `;
        document.body.appendChild(widget);
    }

    // Clears the message list but keeps the skin's greeting on top
    function resetMessages() {
        const container = document.getElementById('gchat-messages');
        if (container) container.innerHTML = introHTML();
        return container;
    }

    // ── Build allowed-hosts regex (mirrors comments.js logic) ─────────────────
    function buildAllowedHostsRegex() {
        const escapedSiteHost = window.location.host.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
        const hosts = [escapedSiteHost];
        if (window.f0ckAllowedImages && Array.isArray(window.f0ckAllowedImages)) {
            window.f0ckAllowedImages.forEach(h => {
                hosts.push(`(?:[a-z0-9-]+\\.)*${h.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`);
            });
        }
        return hosts.join('|');
    }

    // ── Message rendering with formatting ────────────────────────────────────
    function renderMsgContent(raw) {
        // 1. Process line by line for greentext, then HTML-escape each line
        const lines = raw.split('\n').map(line => {
            const trimmed = line.trimStart();
            if (trimmed.startsWith('>')) {
                const content = esc(trimmed.slice(1));
                return `<span class="gchat-greentext">&gt;${content}</span>`;
            }
            return esc(line);
        });
        // Join lines — skip <br> between consecutive greentext spans
        let html = '';
        for (let i = 0; i < lines.length; i++) {
            html += lines[i];
            if (i < lines.length - 1) {
                const curGreen  = lines[i].startsWith('<span class="gchat-greentext">');
                const nextGreen = lines[i + 1].startsWith('<span class="gchat-greentext">');
                if (!(curGreen && nextGreen)) html += '<br>';
            }
        }

        // 2. [spoiler]...[/spoiler]
        let prev, iter = 0;
        do {
            prev = html;
            html = html.replace(/\[spoiler\]([\s\S]*?)\[\/spoiler\]/gi,
                (_, c) => `<span class="spoiler">${c}</span>`);
        } while (html !== prev && ++iter < 10);

        // 3. [blur]...[/blur]
        iter = 0;
        do {
            prev = html;
            html = html.replace(/\[blur\]([\s\S]*?)\[\/blur\]/gi,
                (_, c) => `<span class="blur-text">${c}</span>`);
        } while (html !== prev && ++iter < 10);

        // 4. @mention links
        html = html.replace(/(?<!\[)@([a-zA-Z0-9_\-.]+)/g,
            (_, u) => `<a href="/user/${encodeURIComponent(u)}" class="mention">@${u}</a>`);

        // 5. Custom emoji :name:
        if (customEmojis) {
            html = html.replace(/:([a-z0-9_]+):/g, (m, name) => {
                if (customEmojis[name]) return `<img src="${customEmojis[name]}" class="emoji" alt="${m}" title="${m}">`;
                return m;
            });
        }

        // 6. URL linkification + media embeds
        const hostsRegex = buildAllowedHostsRegex();

        // 6a. Raw image URLs from allowed hosts → <img>
        const imageRegex = new RegExp(
            `(https?:\\/\\/(?:${hostsRegex})\\/(?:(?!https?:\\/\\/).)+?\\.(?:jpg|jpeg|png|gif|webp)(?:\\?(?:(?!https?:\\/\\/)\\S)+)?)`,
            'gi'
        );
        html = html.replace(imageRegex, url =>
            `<span class="gchat-embed-img"><img data-lazy-src="${url}" src="" loading="lazy" alt="" data-on="error:gchat-hide-parent"></span>`
        );

        // 6b. Raw video URLs from allowed hosts → <video>
        const videoRegex = new RegExp(
            `(https?:\\/\\/(?:${hostsRegex})\\/(?:(?!https?:\\/\\/)(?![\\[\\]()\\s]))+?\\.(?:mp4|webm|ogv|mov)(?:\\?(?:(?!https?:\\/\\/)\\S)+)?)`,
            'gi'
        );
        html = html.replace(videoRegex, url =>
            `<span class="gchat-embed-video"><video src="${url}" controls loop muted playsinline preload="metadata"></video></span>`
        );

        // 6c. Raw audio URLs from allowed hosts → <audio>
        const audioRegex = new RegExp(
            `(https?:\\/\\/(?:${hostsRegex})\\/(?:(?!https?:\\/\\/)(?![\\[\\]()\\s]))+?\\.(?:mp3|ogg|wav|flac|aac|opus|m4a)(?:\\?(?:(?!https?:\\/\\/)\\S)+)?)`,
            'gi'
        );
        html = html.replace(audioRegex, url =>
            `<span class="gchat-embed-audio"><audio src="${url}" controls preload="metadata"></audio></span>`
        );

        // 6d. YouTube URLs → oEmbed preview card (thumbnail + title fetched async)
        const ytRegex = /https?:\/\/(?:www\.)?(?:youtube\.com\/watch\?(?:[^"&\s]*&)?v=|youtu\.be\/)([a-zA-Z0-9_\-]{11})[^\s]*/gi;
        html = html.replace(ytRegex, (match, videoId) => {
            const watchUrl = `https://www.youtube.com/watch?v=${videoId}`;
            const thumb    = `https://img.youtube.com/vi/${videoId}/hqdefault.jpg`;
            return `<a class="gchat-yt-card" data-yt-id="${videoId}" href="${watchUrl}" target="_blank" rel="noopener noreferrer">` +
                `<span class="gchat-yt-thumb-wrap">` +
                    `<img src="${thumb}" class="gchat-yt-thumb" alt="YouTube thumbnail" loading="lazy">` +
                    `<span class="gchat-yt-play">${ico('play')}</span>` +
                `</span>` +
                `<span class="gchat-yt-info">` +
                    `<span class="gchat-yt-title">…</span>` +
                    `<span class="gchat-yt-author"></span>` +
                `</span>` +
            `</a>`;
        });

        // 6d.1 Vocaroo URLs → iframe embed
        const vocarooRegex = /https?:\/\/(?:www\.)?(?:voca\.ro|vocaroo\.com)\/([a-zA-Z0-9_-]+)[^\s]*/gi;
        html = html.replace(vocarooRegex, (match, vocarooId) => {
            if (['upload', 'contact', 'privacy', 'tos', 'about'].includes(vocarooId.toLowerCase())) return match;
            return `<span class="vocaroo-embed-wrap gchat-embed-audio"><iframe src="https://vocaroo.com/embed/${vocarooId}?autoplay=0" width="300" height="60" frameborder="0" allow="autoplay"></iframe></span>`;
        });

        // 6d.5  Same-site item page links → post preview card (resolved async)
        //       Only catches /digits paths — direct media file URLs are handled by 6a-6c & 6e.
        const siteHostEsc = window.location.host.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
        const siteItemRx  = new RegExp(
            `https?:\/\/${siteHostEsc}\/(\\d+)(?=[\\s<"']|$)`,
            'gi'
        );
        html = html.replace(siteItemRx, (match, itemId) =>
            `<span class="gchat-item-embed gchat-post-card gchat-post-card--loading" data-item-id="${itemId}">` +
                `<span class="gchat-post-card__thumb-wrap"><span class="gchat-post-card__thumb-placeholder"><span class="gchat-spin"></span></span></span>` +
                `<span class="gchat-post-card__info"><span class="gchat-post-card__id">#${itemId}</span></span>` +
            `</span>`
        );

        // 6e. Remaining https URLs → embed media if from allowed host, else plain link
        html = html.replace(/(^|[\s>])(https?:\/\/[^\s<"]+)/g, (match, pre, url) => {
            // Skip URLs already embedded by earlier steps
            if (match.includes('<img') || match.includes('<video') || match.includes('<audio') ||
                match.includes('<iframe') || match.includes('gchat-yt-card') || match.includes('gchat-item-embed'))
                return match;

            // Use URL API for reliable host + extension detection
            try {
                const urlObj = new URL(url);
                const host   = urlObj.host;
                const path   = urlObj.pathname;
                // Derive CDN host from window.f0ckMediaBase (may be on a different subdomain in prod)
                let mediaHost = '';
                try { mediaHost = new URL(window.f0ckMediaBase || '').host; } catch (_) {}
                const isSameSite      = host === window.location.host;
                const isMediaHost     = !!mediaHost && host === mediaHost;
                const isAllowedHoster = !isSameSite && !isMediaHost && (window.f0ckAllowedImages || []).some(h =>
                    host === h || host.endsWith('.' + h)
                );
                if (isSameSite || isMediaHost || isAllowedHoster) {
                    if (/\.(mp4|webm|ogv|mov)(\?.*)?$/i.test(path))
                        return `${pre}<span class="gchat-embed-video"><video src="${url}" controls loop muted playsinline preload="metadata"></video></span>`;
                    if (/\.(jpg|jpeg|png|gif|webp)(\?.*)?$/i.test(path))
                        return `${pre}<span class="gchat-embed-img"><img data-lazy-src="${url}" src="" loading="lazy" alt="" data-on="error:gchat-hide-parent"></span>`;
                    if (/\.(mp3|ogg|wav|flac|aac|opus|m4a)(\?.*)?$/i.test(path))
                        return `${pre}<span class="gchat-embed-audio"><audio src="${url}" controls preload="metadata"></audio></span>`;
                }
            } catch (_) {}

            return `${pre}<a href="${url}" class="gchat-link" target="_blank" rel="noopener noreferrer">${url}${ico('external', 'gchat-ico-ext')}</a>`;
        });

        return html;
    }

    // A time: short on screen, the full date and time on hover (title) and for machines (datetime)
    function timeTag(cls, when, short) {
        const full = when.toLocaleString([], {
            weekday: 'long', year: 'numeric', month: 'long', day: 'numeric',
            hour: '2-digit', minute: '2-digit', second: '2-digit'
        });
        return `<time class="${cls}" datetime="${esc(when.toISOString())}" title="${esc(full)}">${esc(short)}</time>`;
    }

    // Every message carries avatar, name and time; the skins decide what shows (f0ckm hides them on
    // your own messages, f0ckv2 shows an IRC line, f0ckyou a live-chat row).
    function renderMsg(msg) {
        const isSelf      = (msg.username === window.f0ckSession.user);
        const isAdmin     = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
        const displayName = msg.display_name || msg.username;
        const when        = msg.created_at ? new Date(msg.created_at) : null;
        const ts          = when && !isNaN(when)
            ? when.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
            : '';
        const content  = renderMsgContent(msg.message || '');
        const msgId    = msg.id ? `data-msg-id="${msg.id}"` : '';
        const user     = esc(msg.username || '');
        const userHref = `/user/${encodeURIComponent(msg.username || '')}`;

        // No whitespace between the parts: skins that put them on one line (f0ckv2, f0ckyou) set the gaps
        return `<div class="gchat-msg${isSelf ? ' gchat-msg-self' : ''}" ${msgId}>` +
            `<a href="${userHref}" class="gchat-avatar-link" tabindex="-1"><img src="${avatarSrc(msg)}" class="gchat-avatar" alt="" loading="lazy"></a>` +
            `<div class="gchat-bubble-wrap">` +
                `<div class="gchat-meta">` +
                    `<a href="${userHref}" class="gchat-username${msg.username_color ? ' gchat-user-colored' : ''}"${userColorAttr(msg.username_color)}>${esc(displayName)}</a>` +
                    (ts ? timeTag('gchat-time', when, ts) : '') +
                `</div>` +
                `<div class="gchat-bubble">${content}${ts ? timeTag('gchat-stamp', when, ts) : ''}</div>` +
            `</div>` +
            // reply / delete: shown on hover, in space the skin keeps free for them
            `<div class="gchat-msg-actions">` +
                `<button type="button" class="gchat-reply-btn" data-username="${user}" title="${esc(t('chat_reply', 'Reply'))}">${ico('reply')}</button>` +
                (isAdmin && msg.id ? `<button type="button" class="gchat-del-btn" data-id="${msg.id}" title="${esc(t('chat_delete', 'Delete'))}">${ico('trash')}</button>` : '') +
            `</div>` +
        `</div>`;
    }

    function scrollToBottom(force = false, smooth = false) {
        const el = document.getElementById('gchat-messages');
        if (!el) return;
        const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
        if (!force && !nearBottom) return;
        if (smooth) {
            el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
        } else {
            // Double rAF ensures layout is committed before reading scrollHeight
            requestAnimationFrame(() => requestAnimationFrame(() => { el.scrollTop = el.scrollHeight; }));
        }
    }

    async function fetchYtOembed(cardEl) {
        const id = cardEl.dataset.ytId;
        if (!id) return;
        let meta = ytOembedCache.get(id);
        if (!meta) {
            try {
                const r = await fetch(
                    `https://www.youtube.com/oembed?url=https://www.youtube.com/watch?v=${encodeURIComponent(id)}&format=json`
                );
                meta = await r.json();
                ytOembedCache.set(id, meta);
            } catch (_) {
                meta = { title: 'YouTube', author_name: '' };
            }
        }
        const titleEl  = cardEl.querySelector('.gchat-yt-title');
        const authorEl = cardEl.querySelector('.gchat-yt-author');
        if (titleEl)  titleEl.textContent  = meta.title       || 'YouTube';
        if (authorEl) authorEl.textContent = meta.author_name || '';
    }

    // Resolve same-site item links → post preview card
    const itemPreviewCache = new Map(); // id → { item, meta } | null
    async function fetchItemPreview(wrapEl) {
        const id = wrapEl.dataset.itemId;
        if (!id) return;

        let cached = itemPreviewCache.get(id);
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
            itemPreviewCache.set(id, cached);
        }

        if (!cached) {
            // Fallback: plain link
            const link = document.createElement('a');
            link.href = `/${id}`;
            link.target = '_blank';
            link.rel = 'noopener';
            link.textContent = `#${id}`;
            wrapEl.replaceWith(link);
            return;
        }

        const { item, meta } = cached;
        const commentCount = meta ? (meta.comment_count || 0) : 0;
        const uploader = esc(item.username || 'unknown');
        const mime = item.mime || '';
        const thumbSrc = `/t/${id}.webp`;

        // Media type badge
        let typeBadge = '';
        if (mime.startsWith('video/')) typeBadge = ico('video');
        else if (mime.startsWith('audio/')) typeBadge = ico('audio');
        else if (mime.startsWith('image/')) typeBadge = ico('image');

        const card = document.createElement('a');
        card.className = 'gchat-post-card';
        card.href = `/${id}`;
        card.innerHTML =
            `<span class="gchat-post-card__thumb-wrap">` +
                `<img class="gchat-post-card__thumb" src="${esc(thumbSrc)}" alt="#${id}" loading="lazy" data-on="error:hide">` +
                (typeBadge ? `<span class="gchat-post-card__type-badge">${typeBadge}</span>` : '') +
            `</span>` +
            `<span class="gchat-post-card__info">` +
                `<span class="gchat-post-card__id">#${id}</span>` +
                `<span class="gchat-post-card__uploader">${ico('user')} ${uploader}</span>` +
                `<span class="gchat-post-card__comments">${ico('comment')} ${commentCount}</span>` +
            `</span>`;

        wrapEl.replaceWith(card);
    }

    function appendMsg(msg, scrollForce = false) {
        const container = document.getElementById('gchat-messages');
        if (!container) return;
        const div = document.createElement('div');
        div.innerHTML = renderMsg(msg);
        const node = div.firstElementChild;
        const shown = container.getElementsByClassName('gchat-msg');
        while (shown.length >= MAX_VISIBLE_MSGS) shown[0].remove();
        container.appendChild(node);
        setTimeout(() => node.classList.add('gchat-msg-in'), 10);

        // Wire up spoiler clicks on newly appended msg
        node.querySelectorAll('.gchat-bubble .spoiler, .gchat-bubble .blur-text').forEach(s => {
            s.addEventListener('click', () => s.classList.toggle('revealed'));
        });

        // Embedded images: register with lazy observer; scroll on load only for new messages (not history)
        node.querySelectorAll('.gchat-embed-img img').forEach(img => {
            // Only snap to bottom on image load for NEW incoming messages, not history.
            // History already scrolls once at the end of loadHistory; an extra scroll
            // here is what causes the double jump.
            if (scrollForce) img.addEventListener('load', () => scrollToBottom(true));
            img.addEventListener('click', () => openImgModal(img.src));
            if (img.dataset.lazySrc) lazyImgObserver.observe(img);
        });

        // Wire up YouTube oEmbed cards
        node.querySelectorAll('.gchat-yt-card[data-yt-id]').forEach(fetchYtOembed);
        // Wire up same-site item embeds
        node.querySelectorAll('.gchat-item-embed[data-item-id]').forEach(fetchItemPreview);

        scrollToBottom(scrollForce);

        // Unread badge: count if minimized or window not focused
        if (!chatShown() || !chatFocused) {
            unreadCount++;
            updateBadge();
        }
    }

    // Fades a message out (deleted by a moderator), then removes it
    function removeMsg(msgEl) {
        if (!msgEl) return;
        msgEl.classList.add('gchat-msg-out');
        setTimeout(() => msgEl.remove(), 200);
    }

    async function loadHistory() {
        try {
            const res  = await fetch('/api/chat', { headers: { 'X-Requested-With': 'XMLHttpRequest' } });
            const data = await res.json();
            if (!data.success) return;
            const container = resetMessages();
            if (!container) return;
            (data.messages || []).forEach(m => appendMsg(m, false));
            // Double rAF: wait for the browser to commit the layout (panel just became
            // visible from display:none) before reading scrollHeight.
            requestAnimationFrame(() => requestAnimationFrame(() => {
                container.scrollTop = container.scrollHeight;
            }));
        } catch (e) {
            console.error('[Chat] Failed to load history:', e);
        }
    }

    function updateCount(input) {
        const el = document.getElementById('gchat-count');
        if (el) el.textContent = `${input.value.length}/${input.maxLength > 0 ? input.maxLength : 500}`;
    }

    // Grows the field with its text, up to 4 lines (rows, not a height style). One line: a skin may give
    // the input row a fixed height (f0ckm: the sidebar bar's); more: .gchat-input-multi lets it grow.
    function autoGrow(input) {
        const area = document.getElementById('gchat-input-area');
        area?.classList.remove('gchat-input-multi');
        input.rows = 1;
        // a second line overflows by a whole line; a font's taller glyphs (or the placeholder's) only by
        // a pixel or two, which must not count
        const line = parseFloat(getComputedStyle(input).lineHeight) || 16;
        if (input.value && input.scrollHeight > input.clientHeight + line / 2) {
            area?.classList.add('gchat-input-multi');
            while (input.rows < 4 && input.scrollHeight > input.clientHeight + 1) input.rows++;
        }
    }

    function clearInput(input) {
        input.value = '';
        autoGrow(input);
        updateCount(input);
    }

    // A note only this visitor sees (not sent): /help
    function appendNote(title, rows) {
        const container = document.getElementById('gchat-messages');
        if (!container) return;
        const note = document.createElement('div');
        note.className = 'gchat-note-msg';
        note.innerHTML = `<div class="gchat-note-title">${esc(title)}</div>` + (rows.length
            ? `<dl class="gchat-note-list">${rows.map(([cmd, what]) => `<dt><code>${esc(cmd)}</code></dt><dd>${esc(what)}</dd>`).join('')}</dl>`
            : '');
        container.appendChild(note);
        scrollToBottom(true);
    }

    const STAFF_COMMANDS = [
        ['/help',                          t('chat_cmd_help', 'this list (only you see it)')],
        ['/settopic <text>',               t('chat_cmd_settopic', 'pin a topic above the chat, for everyone')],
        ['/cleartopic',                    t('chat_cmd_cleartopic', 'remove the topic')],
        ['/setbackground <url> [css]',     t('chat_cmd_setbackground', "the messages' background; css after the url, default: center / cover no-repeat")],
        ['/clearbg',                       t('chat_cmd_clearbg', 'remove the background')],
        ['/clear',                         t('chat_cmd_clear', 'delete every message, for everyone')],
    ];

    async function sendMessage() {
        const input = document.getElementById('gchat-input');
        if (!input) return;
        const raw = input.value.trim();
        if (!raw) return;

        // /help: the commands, shown only here
        if (raw === '/help') {
            const isAdmin = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
            if (isAdmin) appendNote(t('chat_cmd_title', 'Chat commands (staff)'), STAFF_COMMANDS);
            else appendNote(t('chat_cmd_none', 'There are no chat commands.'), []);
            clearInput(input);
            return;
        }

        // Admin slash commands
        if (raw === '/clear') {
            const isAdmin = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
            if (!isAdmin) { clearInput(input); return; }
            const csrf = window.f0ckSession?.csrf_token;
            try {
                await fetch('/api/chat', {
                    method: 'DELETE',
                    headers: { 'X-Requested-With': 'XMLHttpRequest', ...(csrf ? { 'X-CSRF-Token': csrf } : {}) }
                });
                resetMessages();
            } catch (e) { console.error('[Chat] Clear error', e); }
            clearInput(input);
            return;
        }

        // /setbackground <url> [css opts]  — e.g. /setbackground https://… center / contain no-repeat
        if (raw.startsWith('/setbackground ') || raw === '/clearbg') {
            const isAdmin = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
            if (!isAdmin) { clearInput(input); return; }
            const csrf = window.f0ckSession?.csrf_token;
            const headers = { 'Content-Type': 'application/x-www-form-urlencoded', 'X-Requested-With': 'XMLHttpRequest', ...(csrf ? { 'X-CSRF-Token': csrf } : {}) };

            if (raw === '/clearbg') {
                // Clear background
                await fetch('/api/chat/background', { method: 'POST', headers, body: new URLSearchParams({}) });
            } else {
                // Parse "/setbackground <url> [opts]"
                const rest = raw.slice('/setbackground '.length).trim();
                const spaceAfterUrl = rest.search(/\s/);
                const url  = spaceAfterUrl > -1 ? rest.slice(0, spaceAfterUrl) : rest;
                const opts = spaceAfterUrl > -1 ? rest.slice(spaceAfterUrl + 1).trim() : 'center / cover no-repeat';
                try {
                    const res  = await fetch('/api/chat/background', { method: 'POST', headers, body: new URLSearchParams({ url, opts }) });
                    const data = await res.json();
                    if (!data.success) console.warn('[Chat] setbackground error:', data.msg);
                } catch (e) { console.error('[Chat] setbackground error', e); }
            }
            clearInput(input);
            return;
        }

        // /settopic <text>  or  /cleartopic
        if (raw.startsWith('/settopic') || raw === '/cleartopic') {
            const isAdmin = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
            if (!isAdmin) { clearInput(input); return; }
            const csrf = window.f0ckSession?.csrf_token;
            const headers = { 'Content-Type': 'application/x-www-form-urlencoded', 'X-Requested-With': 'XMLHttpRequest', ...(csrf ? { 'X-CSRF-Token': csrf } : {}) };
            const topic = raw === '/cleartopic' ? '' : raw.slice('/settopic'.length).trim();
            try { await fetch('/api/chat/topic', { method: 'POST', headers, body: new URLSearchParams({ topic }) }); }
            catch (e) { console.error('[Chat] settopic error', e); }
            clearInput(input);
            return;
        }

        const now = Date.now();
        if (now - lastSent < RATE_LIMIT_MS) { return; }
        lastSent = now;

        clearInput(input);

        const csrf = window.f0ckSession?.csrf_token;
        try {
            const res  = await fetch('/api/chat', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/x-www-form-urlencoded',
                    'X-Requested-With': 'XMLHttpRequest',
                    ...(csrf ? { 'X-CSRF-Token': csrf } : {})
                },
                body: new URLSearchParams({ message: raw })
            });
            const data = await res.json();
            if (!data.success) console.warn('[Chat] Send error:', data.msg);
        } catch (e) {
            console.error('[Chat] Network error', e);
        }
    }

    function setMinimized(val) {
        isMinimized = val;
        localStorage.setItem('f0ck_chat_minimized', isMinimized ? '1' : '0');
        const panel = document.getElementById('gchat-panel');
        const btn   = document.getElementById('gchat-minimize-btn');
        if (panel) panel.classList.toggle('gchat-minimized', isMinimized);
        requestAnimationFrame(() => updateDock());
        if (btn) {
            btn.title = isMinimized ? t('chat_expand','Expand') : t('chat_minimize','Minimize');
            btn.innerHTML = ico(isMinimized ? 'up' : 'down');
        }
        if (!isMinimized) {
            clearUnread();
            // Wait one rAF so the panel transitions from display:none to its full
            // height before loadHistory measures scrollHeight.
            requestAnimationFrame(() => loadHistory());
            if (!window.matchMedia('(pointer: coarse)').matches)
                setTimeout(() => document.getElementById('gchat-input')?.focus(), 150);
        }
    }

    // ── Emoji picker ──────────────────────────────────────────────────────────
    function setupEmojiPicker(textarea) {
        const btn = document.getElementById('gchat-emoji-btn');
        if (!btn) return;

        let picker = null;
        let closeHandler = null;

        const hidePicker = () => {
            if (picker) picker.classList.remove('gchat-open');
            if (closeHandler) { document.removeEventListener('click', closeHandler); closeHandler = null; }
        };

        const buildPicker = () => {
            if (picker) { picker.remove(); picker = null; }
            picker = document.createElement('div');
            picker.id = 'gchat-emoji-picker';

            if (customEmojis && Object.keys(customEmojis).length > 0) {
                Object.keys(customEmojis).forEach(name => {
                    const img = document.createElement('img');
                    img.src = customEmojis[name];
                    img.title = `:${name}:`;
                    img.loading = 'lazy';
                    img.addEventListener('error', () => { img.hidden = true; });
                    img.addEventListener('click', (e) => {
                        e.stopPropagation();
                        const pos = textarea.selectionStart ?? textarea.value.length;
                        const v   = textarea.value;
                        textarea.value = v.slice(0, pos) + `:${name}:` + v.slice(pos);
                        const newPos = pos + name.length + 2;
                        textarea.setSelectionRange(newPos, newPos);
                        textarea.focus();
                        updateCount(textarea);
                        hidePicker();
                    });
                    picker.appendChild(img);
                });
            } else {
                const empty = document.createElement('div');
                empty.className = 'gchat-emoji-empty';
                empty.textContent = 'No custom emojis';
                picker.appendChild(empty);
            }

            // Above the input area
            const area = document.getElementById('gchat-input-area');
            area.parentNode.insertBefore(picker, area);
        };

        btn.addEventListener('click', (e) => {
            e.preventDefault();
            if (!picker) buildPicker();

            if (picker.classList.contains('gchat-open')) {
                hidePicker();
            } else {
                picker.classList.add('gchat-open');
                closeHandler = (ev) => {
                    if (!picker.contains(ev.target) && !btn.contains(ev.target)) hidePicker();
                };
                setTimeout(() => document.addEventListener('click', closeHandler), 0);
            }
        });

        // Rebuild picker if emojis load after init
        window.addEventListener('f0ck:emojis_ready', (e) => {
            customEmojis = e.detail;
            if (picker) { hidePicker(); picker.remove(); picker = null; } // force rebuild on next open
        });
    }

    // ── :emoji inline autocomplete (a list above the input, inside the panel) ─
    function setupEmojiAutocomplete(textarea) {
        const ac = document.getElementById('gchat-emoji-ac');
        let acMatches = [], acIdx = -1;

        const getColon = () => {
            const pos  = textarea.selectionStart;
            const text = textarea.value.slice(0, pos);
            const m    = text.match(/:([a-z0-9_]{0,})$/i);
            if (!m) return null;
            return { query: m[1].toLowerCase(), colonPos: pos - m[0].length };
        };

        const hideAC = () => {
            ac.hidden = true;
            ac.innerHTML = '';
            acMatches = []; acIdx = -1;
        };

        const insertEmoji = (name) => {
            const hit = getColon();
            if (!hit) return;
            const before = textarea.value.slice(0, hit.colonPos);
            const after  = textarea.value.slice(textarea.selectionStart);
            const insert = `:${name}:`;
            textarea.value = before + insert + after;
            const newPos = hit.colonPos + insert.length;
            textarea.setSelectionRange(newPos, newPos);
            textarea.focus();
            updateCount(textarea);
            hideAC();
        };

        const setActiveAC = (idx) => {
            const items = ac.querySelectorAll('.gchat-emoji-ac-item');
            items.forEach(el => el.classList.remove('active'));
            if (idx >= 0 && idx < items.length) {
                items[idx].classList.add('active');
                items[idx].scrollIntoView({ block: 'nearest' });
            }
            acIdx = idx;
        };

        const renderAC = () => {
            const hit = getColon();
            if (!hit || !customEmojis) return hideAC();
            acMatches = Object.keys(customEmojis).filter(n => n.includes(hit.query)).slice(0, 30);
            if (!acMatches.length) return hideAC();

            ac.innerHTML = '';
            acIdx = -1;
            acMatches.forEach((name) => {
                const item = document.createElement('div');
                item.className = 'gchat-emoji-ac-item';
                const img = document.createElement('img');
                img.src = customEmojis[name]; img.alt = name; img.loading = 'lazy';
                const label = document.createElement('span');
                label.textContent = `:${name}:`;
                item.append(img, label);
                item.addEventListener('mousedown', ev => { ev.preventDefault(); insertEmoji(name); });
                ac.appendChild(item);
            });
            ac.hidden = false;
            // Auto-select first match so Enter works immediately
            setActiveAC(0);
        };

        textarea.addEventListener('input',  () => renderAC());
        textarea.addEventListener('click',  () => { if (getColon()) renderAC(); });
        textarea.addEventListener('blur',   () => setTimeout(hideAC, 150));

        textarea.addEventListener('keydown', e => {
            if (ac.hidden) return;
            if (e.key === 'ArrowDown') {
                e.preventDefault();
                setActiveAC((acIdx + 1) % acMatches.length);
            } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                setActiveAC((acIdx <= 0 ? acMatches.length : acIdx) - 1);
            } else if (e.key === 'Enter' || e.key === 'Tab') {
                // Insert the active match (always at least 0 after renderAC)
                const idx = acIdx >= 0 ? acIdx : 0;
                if (acMatches[idx]) {
                    e.preventDefault(); e.stopImmediatePropagation();
                    insertEmoji(acMatches[idx]);
                }
            } else if (e.key === 'Escape') {
                hideAC();
            }
        });
    }

    // ── @user inline autocomplete (a list above the input, inside the panel) ─
    function setupMentionAutocomplete(textarea) {
        const mac = document.getElementById('gchat-mention-ac');
        let macMatches = [], macIdx = -1;
        const DEBOUNCE = 200;
        let debounceTimer = null;

        const getMention = () => {
            const pos  = textarea.selectionStart;
            const text = textarea.value.slice(0, pos);
            const m    = text.match(/@([a-zA-Z0-9_\-.]{0,})$/);
            if (!m) return null;
            return { query: m[1], atPos: pos - m[0].length };
        };

        const hideMAC = () => {
            mac.hidden = true;
            mac.innerHTML = '';
            macMatches = []; macIdx = -1;
        };

        const insertMention = (username) => {
            const hit = getMention();
            if (!hit) return;
            const before = textarea.value.slice(0, hit.atPos);
            const after  = textarea.value.slice(textarea.selectionStart);
            const insert = username.includes(' ') ? `[@${username}]` : `@${username}`;
            textarea.value = before + insert + after;
            textarea.setSelectionRange(hit.atPos + insert.length, hit.atPos + insert.length);
            textarea.focus();
            updateCount(textarea);
            hideMAC();
        };

        const setActiveMAC = (idx) => {
            const items = mac.querySelectorAll('.gchat-mention-item');
            items.forEach(el => el.classList.remove('active'));
            if (idx >= 0 && idx < items.length) {
                items[idx].classList.add('active');
                items[idx].scrollIntoView({ block: 'nearest' });
            }
            macIdx = idx;
        };

        const renderMAC = (users) => {
            mac.innerHTML = '';
            macMatches = users;
            macIdx = -1;
            if (!users.length) return hideMAC();
            users.forEach(user => {
                const item = document.createElement('div');
                item.className = 'gchat-mention-item';
                const src = user.avatar_file ? `/a/${user.avatar_file}` : (user.avatar ? `/t/${user.avatar}.webp` : '/a/default.png');
                const img = document.createElement('img');
                img.src = src; img.loading = 'lazy';
                img.addEventListener('error', () => { img.src = '/a/default.png'; }, { once: true });
                const name = document.createElement('span');
                name.className = 'gchat-mention-name';
                name.textContent = user.user;
                item.append(img, name);
                if (user.display_name) {
                    const disp = document.createElement('span');
                    disp.className = 'gchat-mention-display';
                    disp.textContent = user.display_name;
                    item.appendChild(disp);
                }
                item.addEventListener('mousedown', ev => { ev.preventDefault(); insertMention(user.user); });
                mac.appendChild(item);
            });
            mac.hidden = false;
            setActiveMAC(0);
        };

        textarea.addEventListener('input', () => {
            const hit = getMention();
            if (!hit) return hideMAC();
            clearTimeout(debounceTimer);
            debounceTimer = setTimeout(async () => {
                try {
                    const res  = await fetch(`/api/v2/users/suggest?q=${encodeURIComponent(hit.query)}`);
                    const data = await res.json();
                    renderMAC(data.suggestions || []);
                } catch { hideMAC(); }
            }, DEBOUNCE);
        });

        textarea.addEventListener('blur',  () => setTimeout(hideMAC, 150));

        textarea.addEventListener('keydown', e => {
            if (mac.hidden) return;
            if (e.key === 'ArrowDown') {
                e.preventDefault();
                setActiveMAC((macIdx + 1) % macMatches.length);
            } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                setActiveMAC((macIdx <= 0 ? macMatches.length : macIdx) - 1);
            } else if (e.key === 'Enter' || e.key === 'Tab') {
                const idx = macIdx >= 0 ? macIdx : 0;
                if (macMatches[idx]) {
                    e.preventDefault(); e.stopImmediatePropagation();
                    insertMention(macMatches[idx].user);
                }
            } else if (e.key === 'Escape') {
                hideMAC();
            }
        });
    }

    // ── Image modal ───────────────────────────────────────────────────────────
    let _imgModal = null;
    function openImgModal(src) {
        if (!_imgModal) {
            _imgModal = document.createElement('div');
            _imgModal.id = 'gchat-img-modal';
            _imgModal.innerHTML = '<div id="gchat-img-modal-inner"><img id="gchat-img-modal-img" alt=""></div>';
            document.body.appendChild(_imgModal);
            _imgModal.addEventListener('click', () => closeImgModal());
            document.addEventListener('keydown', e => { if (e.key === 'Escape') closeImgModal(); });
        }
        document.getElementById('gchat-img-modal-img').src = src;
        _imgModal.classList.add('gchat-img-modal-open');
        document.documentElement.classList.add('gchat-img-open');
    }
    function closeImgModal() {
        _imgModal?.classList.remove('gchat-img-modal-open');
        document.documentElement.classList.remove('gchat-img-open');
    }

    // ── Init ─────────────────────────────────────────────────────────────────
    function init() {
        buildWidget();

        const panel    = document.getElementById('gchat-panel');
        const widget   = document.getElementById('gchat-widget');
        const textarea = document.getElementById('gchat-input');

        // Gate chat on the live socket — block input until stream is live
        let _sseReady = false;

        function setConnecting(connecting) {
            panel.classList.toggle('gchat-connecting', connecting);
            const sendBtn = document.getElementById('gchat-send-btn');
            if (sendBtn) sendBtn.disabled = connecting;
        }
        setConnecting(true);

        const onSseReady = () => {
            if (_sseReady) return;
            _sseReady = true;
            clearTimeout(_sseUnlockTimer);
            clearInterval(_pollFallback);
            setConnecting(false);
        };
        document.addEventListener('f0ck:sse_ready', onSseReady);

        // Absolute fallback: if the socket hasn't connected within 10s, unlock anyway and poll
        let _pollFallback = null;
        const _sseUnlockTimer = setTimeout(() => {
            if (_sseReady) return;
            console.warn('[Chat] SSE not ready after 10s — unlocking in polling mode');
            setConnecting(false);
            // Poll /api/chat every 15s as fallback so messages still appear
            _pollFallback = setInterval(async () => {
                if (_sseReady) { clearInterval(_pollFallback); return; }
                try {
                    const res  = await fetch('/api/chat', { headers: { 'X-Requested-With': 'XMLHttpRequest' } });
                    const data = await res.json();
                    if (!data.success) return;
                    const container = document.getElementById('gchat-messages');
                    if (!container) return;
                    const lastId = parseInt(container.dataset.lastPollId || '0', 10);
                    const newMsgs = (data.messages || []).filter(m => m.id > lastId);
                    if (newMsgs.length) {
                        newMsgs.forEach(m => appendMsg(m, false));
                        container.dataset.lastPollId = String(Math.max(...newMsgs.map(m => m.id)));
                    }
                } catch (_) {}
            }, 15000);
        }, 10000);
        // Failsafe: if the socket already opened before this ran (e.g. fast reconnect), check flag
        if (document.documentElement.dataset.sseReady === '1') onSseReady();

        // ── Size and position: custom properties the CSS reads ───────────────
        const geo = {};
        function setGeo(name, px) {
            if (px == null || Number.isNaN(px)) {
                delete geo[name];
                panel.style.removeProperty(`--gc-${name}`);
            } else {
                geo[name] = Math.round(px);
                panel.style.setProperty(`--gc-${name}`, geo[name] + 'px');
            }
        }

        // Returns min top position for float panel (below the site's top bar + MOTD)
        function getTopBound() {
            const nav  = document.querySelector('nav.navbar');
            const motd = document.getElementById('motd-container');
            let bound = nav ? nav.getBoundingClientRect().bottom : 0;
            if (motd && motd.offsetParent !== null && motd.getBoundingClientRect().bottom > bound) {
                bound = motd.getBoundingClientRect().bottom;
            }
            return Math.max(0, bound);
        }

        // Restore saved size
        const savedH = parseInt(localStorage.getItem('f0ck_chat_height'), 10);
        if (savedH) setGeo('h', savedH);
        const savedW = parseInt(localStorage.getItem('f0ck_chat_width'), 10);
        if (savedW) setGeo('w', savedW);

        // Mouse + touch drag helper: onMove(clientX, clientY), onEnd()
        function startDrag(onMove, onEnd) {
            const mouseMove = mv => onMove(mv.clientX, mv.clientY);
            const touchMove = mv => { mv.preventDefault(); onMove(mv.touches[0].clientX, mv.touches[0].clientY); };
            const end = () => {
                onEnd();
                document.removeEventListener('mousemove', mouseMove);
                document.removeEventListener('mouseup',   end);
                document.removeEventListener('touchmove', touchMove);
                document.removeEventListener('touchend',  end);
                panel.classList.remove('gchat-dragging');
            };
            panel.classList.add('gchat-dragging');
            document.addEventListener('mousemove', mouseMove);
            document.addEventListener('mouseup',   end);
            document.addEventListener('touchmove', touchMove, { passive: false });
            document.addEventListener('touchend',  end);
        }
        function onPointerDown(el, start, opts = {}) {
            el.addEventListener('mousedown', (e) => {
                if (e.button !== 0) return;
                if (opts.skip && opts.skip(e)) return;
                e.preventDefault();
                start(e.clientX, e.clientY);
            });
            el.addEventListener('touchstart', (e) => {
                if (opts.skip && opts.skip(e)) return;
                e.preventDefault();
                start(e.touches[0].clientX, e.touches[0].clientY);
            }, { passive: false });
        }

        // ── Float / Dock toggle ───────────────────────────────────────────────
        let isFloating = localStorage.getItem('f0ck_chat_floating') === '1';

        // Drag-to-resize from an edge or corner (data-rz: n, s, e, w, ne, nw, se, sw). Docked, the
        // panel hangs from the bottom right, so only the top and left edges move; detached, every
        // edge and corner does and the opposite ones stay put.
        const MIN_W = 220, MIN_H = 180;
        panel.querySelectorAll('.gchat-rz').forEach(handle => {
            const dir = handle.dataset.rz;
            onPointerDown(handle, (startX, startY) => {
                const r = panel.getBoundingClientRect();
                const topBound = isFloating ? getTopBound() : 0;
                startDrag((x, y) => {
                    let { left, top, width, height } = r;
                    if (dir.includes('e')) width = Math.min(Math.max(r.width + (x - startX), MIN_W), window.innerWidth - r.left);
                    if (dir.includes('w')) {
                        const maxW = isFloating ? r.right : window.innerWidth * 0.95;
                        width = Math.min(Math.max(r.width + (startX - x), MIN_W), maxW);
                        left = r.right - width;
                    }
                    if (dir.includes('s')) height = Math.min(Math.max(r.height + (y - startY), MIN_H), window.innerHeight - r.top);
                    if (dir.includes('n')) {
                        const maxH = isFloating ? r.bottom - topBound : window.innerHeight * 0.95;
                        height = Math.min(Math.max(r.height + (startY - y), MIN_H), maxH);
                        top = r.bottom - height;
                    }
                    setGeo('w', width);
                    setGeo('h', height);
                    if (isFloating) { setGeo('x', left); setGeo('y', top); }
                }, () => {
                    if (geo.w) localStorage.setItem('f0ck_chat_width', geo.w);
                    if (geo.h) localStorage.setItem('f0ck_chat_height', geo.h);
                    if (isFloating) {
                        localStorage.setItem('f0ck_chat_float_x', geo.x);
                        localStorage.setItem('f0ck_chat_float_y', geo.y);
                    }
                });
            }, { skip: () => isPhone() || isMinimized || (!isFloating && !['n', 'w', 'nw'].includes(dir)) });
        });

        // Saved sizes / a detached position from a bigger window: pulled back onto the screen (on
        // load, on resize), not saved, so they come back when the window grows again
        function clampGeo() {
            if (isPhone()) return;
            const topBound = getTopBound();
            const savedW = parseInt(localStorage.getItem('f0ck_chat_width'), 10);
            const savedH = parseInt(localStorage.getItem('f0ck_chat_height'), 10);
            if (savedW) setGeo('w', Math.max(Math.min(savedW, window.innerWidth - 16), Math.min(MIN_W, window.innerWidth - 16)));
            if (savedH) setGeo('h', Math.max(Math.min(savedH, window.innerHeight - topBound), Math.min(MIN_H, window.innerHeight - topBound)));
            if (!isFloating) return;
            const r = panel.getBoundingClientRect();
            const fx = parseInt(localStorage.getItem('f0ck_chat_float_x'), 10);
            const fy = parseInt(localStorage.getItem('f0ck_chat_float_y'), 10);
            const x = Number.isNaN(fx) ? r.left : fx;
            const y = Number.isNaN(fy) ? r.top : fy;
            setGeo('x', Math.min(Math.max(0, x), Math.max(0, window.innerWidth - r.width)));
            setGeo('y', Math.min(Math.max(topBound, y), Math.max(topBound, window.innerHeight - r.height)));
        }
        let clampFrame = 0;
        window.addEventListener('resize', () => {
            cancelAnimationFrame(clampFrame);
            clampFrame = requestAnimationFrame(clampGeo);
        });

        function applyFloatMode() {
            const btn = document.getElementById('gchat-float-btn');
            panel.classList.toggle('gchat-floating', isFloating);
            if (isFloating) {
                const fx = parseInt(localStorage.getItem('f0ck_chat_float_x'), 10);
                const fy = parseInt(localStorage.getItem('f0ck_chat_float_y'), 10);
                if (!Number.isNaN(fx)) setGeo('x', fx);
                if (!Number.isNaN(fy)) setGeo('y', Math.max(getTopBound(), fy));
                btn.innerHTML = ico('dock');
            } else {
                setGeo('x', null);
                setGeo('y', null);
                btn.innerHTML = ico('float');
            }
            clampGeo();
        }
        applyFloatMode();

        document.getElementById('gchat-float-btn').addEventListener('click', () => {
            isFloating = !isFloating;
            localStorage.setItem('f0ck_chat_floating', isFloating ? '1' : '0');
            if (isFloating) {
                // Snap to current visual position so it doesn't jump
                const r = panel.getBoundingClientRect();
                localStorage.setItem('f0ck_chat_float_x', Math.round(r.left));
                localStorage.setItem('f0ck_chat_float_y', Math.round(r.top));
            }
            applyFloatMode();
            updateDock();
        });

        // Drag-to-move in float mode — drag the header
        const header = document.getElementById('gchat-header');
        onPointerDown(header, (startX, startY) => {
            const r = panel.getBoundingClientRect();
            const offX = startX - r.left, offY = startY - r.top;
            startDrag((clientX, clientY) => {
                setGeo('x', Math.max(0,             Math.min(clientX - offX, window.innerWidth  - r.width)));
                setGeo('y', Math.max(getTopBound(), Math.min(clientY - offY, window.innerHeight - r.height)));
            }, () => {
                localStorage.setItem('f0ck_chat_float_x', geo.x);
                localStorage.setItem('f0ck_chat_float_y', geo.y);
                if (isMinimized) {
                    const distToTop    = Math.abs(geo.y - getTopBound());
                    const distToBottom = Math.abs(window.innerHeight - (geo.y + HEADER_H));
                    localStorage.setItem('f0ck_chat_anchor_top', distToTop < distToBottom ? '1' : '0');
                }
            });
        }, { skip: (e) => isPhone() || !isFloating || !!e.target.closest('button') });

        // Minimize toggle — in float mode anchor to bottom edge in both directions
        function toggleMinimized() {
            const willExpand = isMinimized; // about to expand
            if (isFloating) {
                const r = panel.getBoundingClientRect();
                const topBound = getTopBound();
                const bottomBound = window.innerHeight;

                if (!willExpand) {
                    // About to minimize: decide anchor based on proximity
                    const distToTop = Math.abs(r.top - topBound);
                    const distToBottom = Math.abs(bottomBound - (r.top + r.height));
                    const anchorTop = distToTop < distToBottom;
                    localStorage.setItem('f0ck_chat_anchor_top', anchorTop ? '1' : '0');

                    const curTop = r.top;
                    const curBottom = r.top + r.height;
                    setMinimized(true);

                    requestAnimationFrame(() => {
                        const newTop = anchorTop
                            ? Math.max(topBound, curTop)                                           // keep the top
                            : Math.min(bottomBound - HEADER_H, Math.max(topBound, curBottom - HEADER_H)); // keep the bottom
                        setGeo('y', newTop);
                        localStorage.setItem('f0ck_chat_float_y', geo.y);
                    });
                } else {
                    // Expanding: use saved anchor or default to bottom if not set
                    const wasAnchorTop = localStorage.getItem('f0ck_chat_anchor_top') === '1';
                    setMinimized(false);

                    requestAnimationFrame(() => {
                        const fullH  = panel.getBoundingClientRect().height;
                        const curTop = geo.y || 0;
                        const newTop = wasAnchorTop
                            ? Math.max(topBound, curTop)                            // top stays
                            : Math.max(topBound, curTop - (fullH - HEADER_H));      // grow upwards
                        setGeo('y', newTop);
                        localStorage.setItem('f0ck_chat_float_y', geo.y);
                    });
                }
            } else {
                setMinimized(!isMinimized);
            }
        }

        document.getElementById('gchat-minimize-btn').addEventListener('click', e => {
            e.stopPropagation(); toggleMinimized();
        });
        // Docked and minimized, a click on the bar opens it (the bar is the launcher); a double click
        // toggles, but not right after that click opened it
        let openedByClick = 0;
        header.addEventListener('click', e => {
            if (!isPhone() && isMinimized && !isFloating && !e.target.closest('.gchat-icon-btn')) {
                openedByClick = Date.now();
                toggleMinimized();
            }
        });
        header.addEventListener('dblclick', e => {
            if (isPhone() || e.target.closest('.gchat-icon-btn') || Date.now() - openedByClick < 600) return;
            toggleMinimized();
        });

        // Close button — desktop: hide the widget, the launcher button reopens it (html.gchat-closed)
        function closeChat() {
            isClosed = true;
            localStorage.setItem('f0ck_chat_closed', '1');
            document.documentElement.classList.add('gchat-closed');
            updateDock();
        }
        function reopenChat() {
            isClosed = false;
            localStorage.removeItem('f0ck_chat_closed');
            document.documentElement.classList.remove('gchat-closed');
            clampGeo();
            updateDock();
            if (isMinimized) { setMinimized(false); } else { loadHistory(); }
        }

        // ── Phones: full screen ───────────────────────────────────────────────
        // It follows the visible viewport (--gc-vv-h / --gc-vv-top), so the field stays above the
        // on-screen keyboard; back (button or gesture) closes it: opening adds a history entry.
        function fitViewport() {
            const vv = window.visualViewport;
            if (!mobileOpen || !vv) return;
            panel.style.setProperty('--gc-vv-h', Math.round(vv.height) + 'px');
            panel.style.setProperty('--gc-vv-top', Math.round(vv.offsetTop) + 'px');
            scrollToBottom(false);
        }
        window.visualViewport?.addEventListener('resize', fitViewport);
        window.visualViewport?.addEventListener('scroll', fitViewport);

        function openMobile() {
            if (mobileOpen) return;
            mobileOpen = true;
            panel.classList.remove('gchat-minimized');
            document.documentElement.classList.add('gchat-m-open');
            try { history.pushState({ ...(history.state || {}), gchat: 1 }, ''); } catch (_) {}
            fitViewport();
            clearUnread();
            loadHistory();
        }
        // fromHistory: back was pressed (the entry is gone already); else take our entry off again
        function closeMobile(fromHistory = false) {
            if (!mobileOpen) return;
            mobileOpen = false;
            document.documentElement.classList.remove('gchat-m-open');
            panel.classList.toggle('gchat-minimized', isMinimized);
            panel.style.removeProperty('--gc-vv-h');
            panel.style.removeProperty('--gc-vv-top');
            document.querySelectorAll('.gchat-msg-tapped').forEach(m => m.classList.remove('gchat-msg-tapped'));
            if (!fromHistory && history.state?.gchat) history.back();
        }
        // Captured, so it runs before the page's own handlers (f0ckm reloads the page on any back):
        // closing the chat is all this back does
        window.addEventListener('popstate', (e) => {
            if (mobileOpen && !history.state?.gchat) {
                e.stopImmediatePropagation();
                closeMobile(true);
            }
        }, true);
        phoneMQ.addEventListener('change', () => {
            if (!isPhone()) closeMobile();
            clampGeo();
            updateDock();
        });

        // The bar the launcher lines up with (skin.bar): its height as --gc-bar-h on <html>, kept up
        // to date (it grows with its content; f0ckm swaps the sidebar on page loads)
        if (skin.bar) {
            let barEl = null;
            const barRO = new ResizeObserver(() => applyBar());
            const applyBar = () => {
                const h = barEl?.isConnected ? barEl.offsetHeight : 0;
                if (h) document.documentElement.style.setProperty('--gc-bar-h', h + 'px');
                else document.documentElement.style.removeProperty('--gc-bar-h');
            };
            const pickBar = () => {
                let el = null;
                // the one at the bottom of the window (a page may have more than one)
                try {
                    el = [...document.querySelectorAll(skin.bar)].filter(e => e.offsetHeight)
                        .sort((a, b) => Math.abs(window.innerHeight - a.getBoundingClientRect().bottom)
                                      - Math.abs(window.innerHeight - b.getBoundingClientRect().bottom))[0] || null;
                } catch (_) {}
                if (el !== barEl) {
                    if (barEl) barRO.unobserve(barEl);
                    barEl = el;
                    if (el) barRO.observe(el);
                }
                applyBar();
            };
            pickBar();
            document.addEventListener('f0ck:contentLoaded', pickBar);
            window.addEventListener('resize', pickBar);
        }

        // The element to stay above (skin.avoid): how much of it shows at the window's bottom, as
        // --gc-avoid-h on <html>, on scroll / resize (once per frame)
        if (skin.avoid) {
            let avoidFrame = 0;
            const measureAvoid = () => {
                avoidFrame = 0;
                let el = null;
                try { el = document.querySelector(skin.avoid); } catch (_) {}
                const h = el ? Math.max(0, Math.round(window.innerHeight - el.getBoundingClientRect().top)) : 0;
                if (h) document.documentElement.style.setProperty('--gc-avoid-h', h + 'px');
                else document.documentElement.style.removeProperty('--gc-avoid-h');
            };
            const queueAvoid = () => { if (!avoidFrame) avoidFrame = requestAnimationFrame(measureAvoid); };
            window.addEventListener('scroll', queueAvoid, { passive: true });
            window.addEventListener('resize', queueAvoid);
            document.addEventListener('f0ck:contentLoaded', queueAvoid);
            new ResizeObserver(queueAvoid).observe(document.body);
            measureAvoid();
        }

        // the launcher: phones always (collapsed chat), desktop when the chat is closed
        const launcher = document.createElement('button');
        launcher.type = 'button';
        launcher.id = 'gchat-reopen-bubble';
        launcher.title = skin.open;
        launcher.innerHTML = ico(skin.icon === 'spark' ? 'spark' : 'chat') + `<span class="gchat-reopen-label">${esc(skin.open)}</span>`;
        launcher.addEventListener('click', () => (isPhone() ? openMobile() : reopenChat()));
        document.body.appendChild(launcher);
        updateBadge();

        updateDock = () => {
            const el = isPhone() ? null : (isClosed ? launcher : (isFloating ? null : panel));
            const top = el ? el.getBoundingClientRect().top : window.innerHeight;
            const h = Math.max(0, Math.round(window.innerHeight - top));
            if (h) document.documentElement.style.setProperty('--gc-dock-h', h + 'px');
            else document.documentElement.style.removeProperty('--gc-dock-h');
        };
        const dockRO = new ResizeObserver(() => updateDock());
        dockRO.observe(panel);
        dockRO.observe(launcher);
        window.addEventListener('resize', () => updateDock());
        updateDock();

        document.getElementById('gchat-close-btn').addEventListener('click', e => {
            e.stopPropagation();
            if (isPhone()) closeMobile(); else closeChat();
        });
        // Expose globally for settings page
        window.reopenChat  = reopenChat;
        window.closeChatWidget = closeChat;

        // Send
        document.getElementById('gchat-send-btn').addEventListener('click', sendMessage);
        textarea.addEventListener('keydown', e => {
            if (e.key === 'Enter' && !e.shiftKey) {
                // Don't send while emoji or mention autocomplete is active
                if (!document.getElementById('gchat-emoji-ac').hidden ||
                    !document.getElementById('gchat-mention-ac').hidden) return;
                e.preventDefault();
                sendMessage();
            }
        });

        // Auto-grow + character count
        textarea.addEventListener('input', () => { autoGrow(textarea); updateCount(textarea); });

        // @mention autocomplete
        setupMentionAutocomplete(textarea);

        // Emoji features — must complete before loadHistory() so :codes: render correctly
        // Use existing cache if CommentSystem already loaded emojis (synchronous)
        if (window.CommentSystem?.emojiCache) {
            customEmojis = window.CommentSystem.emojiCache;
        }

        // Also subscribe to the f0ck:emojis_ready event for late-loading
        window.addEventListener('f0ck:emojis_ready', e => { customEmojis = e.detail; });

        setupEmojiPicker(textarea);
        setupEmojiAutocomplete(textarea);

        // Live messages
        document.addEventListener('f0ck:global_chat', e => appendMsg(e.detail, chatShown()));

        // Clear — wipe messages instantly for all clients
        document.addEventListener('f0ck:global_chat_clear', () => resetMessages());

        // Delete — remove a single message for all clients
        document.addEventListener('f0ck:global_chat_delete', (e) => {
            const id = e.detail?.id;
            if (!id) return;
            removeMsg(document.querySelector(`#gchat-messages [data-msg-id="${CSS.escape(String(id))}"]`));
        });

        // ── Chat background (set by admins with /setbackground: a value from the database) ──
        function applyBackground(css) {
            const msgs = document.getElementById('gchat-messages');
            if (!msgs) return;
            if (css) msgs.style.setProperty('--gc-room-bg', String(css).replace(/[;{}<>]/g, ''));
            else msgs.style.removeProperty('--gc-room-bg');
            panel.classList.toggle('gchat-has-bg', !!css);
        }

        // Restore background on load
        fetch('/api/chat/background').then(r => r.json()).then(d => {
            if (d.success && d.background) applyBackground(d.background);
        }).catch(() => {});

        document.addEventListener('f0ck:global_chat_background', (e) => {
            applyBackground(e.detail?.background || null);
        });

        // ── Chat topic ────────────────────────────────────────────────────────
        function applyTopic(text) {
            const el = document.getElementById('gchat-topic');
            if (!el) return;
            el.textContent = text || '';
            el.hidden = !text;
        }

        // Restore topic on load
        fetch('/api/chat/topic').then(r => r.json()).then(d => {
            if (d.success && d.topic) applyTopic(d.topic);
        }).catch(() => {});

        document.addEventListener('f0ck:global_chat_topic', (e) => {
            applyTopic(e.detail?.topic || null);
        });

        // ── Online users bar ─────────────────────────────────────────────────
        function renderOnline(users, guestCount = 0) {
            const el = document.getElementById('gchat-online');
            if (!el) return;
            if ((!users || users.length === 0) && guestCount === 0) {
                el.innerHTML = '';
                el.hidden = true;
                return;
            }
            el.hidden = false;
            // Show up to 8 avatars, then "+N more" pill
            const MAX_SHOWN = 8;
            const shown  = users.slice(0, MAX_SHOWN);
            const extra  = users.length - shown.length;
            const avatarHTML = shown.map(u => {
                const name = esc(u.display_name || u.username || '?');
                return `<img src="${avatarSrc(u)}" class="gchat-online-avatar${u.username_color ? ' gchat-user-colored' : ''}" title="${name}" alt="${name}" loading="lazy"${userColorAttr(u.username_color)}>`;
            }).join('');
            const extraPill = extra > 0 ? `<span class="gchat-online-extra">+${extra}</span>` : '';

            let countText = count('online', users.length);
            if (guestCount > 0 && (window.f0ckSession.is_admin || window.f0ckSession.is_moderator)) {
                countText += ` (${count('guests', guestCount)})`;
            }
            el.innerHTML = `<div class="gchat-online-inner"><span class="gchat-online-count">${esc(countText)}</span><div class="gchat-online-avatars">${avatarHTML}${extraPill}</div></div>`;
        }
        // ── Who's online: a dialog with everyone, opened from the online bar ──
        let presence = { users: [], guestCount: 0 };
        let usersModal = null;

        function renderUsersList() {
            if (!usersModal) return;
            const { users, guestCount } = presence;
            const rows = users.map(u => {
                const name = u.display_name || u.username || '?';
                const href = `/user/${encodeURIComponent(u.username || '')}`;
                return `<li class="gchat-users-item"><a href="${href}" class="gchat-users-link">` +
                    `<img src="${avatarSrc(u)}" class="gchat-users-avatar${u.username_color ? ' gchat-user-colored' : ''}" alt="" loading="lazy"${userColorAttr(u.username_color)}>` +
                    `<span class="gchat-users-names"><span class="gchat-users-name${u.username_color ? ' gchat-user-colored' : ''}"${userColorAttr(u.username_color)}>${esc(name)}</span>` +
                    (u.display_name && u.display_name !== u.username ? `<span class="gchat-users-login">@${esc(u.username)}</span>` : '') +
                    `</span></a></li>`;
            }).join('');
            const guests = guestCount > 0
                ? `<li class="gchat-users-item gchat-users-guests"><span class="gchat-users-link"><span class="gchat-users-avatar gchat-users-guest-ico">${ico('user')}</span>` +
                  `<span class="gchat-users-names"><span class="gchat-users-name">${esc(count('guests', guestCount))}</span></span></span></li>`
                : '';
            usersModal.querySelector('.gchat-users-count').textContent =
                count('online', users.length) + (guestCount > 0 ? ` · ${count('guests', guestCount)}` : '');
            usersModal.querySelector('.gchat-users-list').innerHTML = rows + guests;
        }

        function closeUsers() {
            if (!usersModal || usersModal.hidden) return;
            usersModal.hidden = true;
            document.getElementById('gchat-online')?.focus();
        }

        function openUsers() {
            if (!usersModal) {
                usersModal = document.createElement('div');
                usersModal.id = 'gchat-users-modal';
                usersModal.hidden = true;
                usersModal.innerHTML =
                    `<div class="gchat-users-box" role="dialog" aria-modal="true" aria-labelledby="gchat-users-title">` +
                        `<div class="gchat-users-head">` +
                            `<span id="gchat-users-title" class="gchat-users-title">${ico('users')}<span>${esc(skin.users)}</span></span>` +
                            `<button type="button" class="gchat-icon-btn gchat-users-close" title="${esc(t('chat_close_list', 'Close'))}">${ico('close')}</button>` +
                        `</div>` +
                        `<div class="gchat-users-count"></div>` +
                        `<ul class="gchat-users-list"></ul>` +
                    `</div>`;
                document.body.appendChild(usersModal);
                usersModal.addEventListener('click', e => {
                    if (e.target === usersModal || e.target.closest('.gchat-users-close')) closeUsers();
                });
                document.addEventListener('keydown', e => { if (e.key === 'Escape') closeUsers(); });
            }
            renderUsersList();
            usersModal.hidden = false;
            usersModal.querySelector('.gchat-users-close').focus();
        }

        const onlineBar = document.getElementById('gchat-online');
        onlineBar.addEventListener('click', openUsers);
        onlineBar.addEventListener('keydown', e => {
            if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openUsers(); }
        });

        document.addEventListener('f0ck:global_chat_presence', (e) => {
            presence = { users: e.detail?.users || [], guestCount: e.detail?.guestCount || 0 };
            renderOnline(presence.users, presence.guestCount);
            renderUsersList();
        });

        // Reply + admin delete buttons (event delegation)
        const csrf = () => window.f0ckSession?.csrf_token;

        // Clear unread when user interacts with the chat
        document.getElementById('gchat-messages').addEventListener('mouseenter', clearUnread);
        textarea.addEventListener('focus', clearUnread);

        document.getElementById('gchat-messages').addEventListener('click', async (e) => {
            // Touch screens have no hover: a tap on a message shows its reply / delete
            if (noHoverMQ.matches && !e.target.closest('a, button, .spoiler, .blur-text, img, video, audio, iframe')) {
                const msg = e.target.closest('.gchat-msg');
                if (msg) {
                    const was = msg.classList.contains('gchat-msg-tapped');
                    document.querySelectorAll('.gchat-msg-tapped').forEach(m => m.classList.remove('gchat-msg-tapped'));
                    msg.classList.toggle('gchat-msg-tapped', !was);
                }
            }
            // A link that leaves the page (profile, item) closes the phone chat
            const link = e.target.closest('a[href]');
            if (link && mobileOpen && link.target !== '_blank') closeMobile();

            // Reply button
            const replyBtn = e.target.closest('.gchat-reply-btn');
            if (replyBtn) {
                const username = replyBtn.dataset.username;
                if (username) {
                    const mention = `@${username} `;
                    textarea.value = mention + textarea.value;
                    textarea.setSelectionRange(mention.length, mention.length);
                    textarea.focus();
                    updateCount(textarea);
                }
                replyBtn.closest('.gchat-msg')?.classList.remove('gchat-msg-tapped');
                return;
            }

            // Admin delete button
            const delBtn = e.target.closest('.gchat-del-btn');
            if (delBtn) {
                const msgId = delBtn.dataset.id;
                if (!msgId) return;
                try {
                    const res  = await fetch(`/api/chat/${encodeURIComponent(msgId)}`, {
                        method: 'DELETE',
                        headers: { 'X-Requested-With': 'XMLHttpRequest', ...(csrf() ? { 'X-CSRF-Token': csrf() } : {}) }
                    });
                    const data = await res.json();
                    if (data.success) removeMsg(delBtn.closest('.gchat-msg'));
                } catch (err) { console.error('[Chat] Delete error', err); }
            }
        });

        // Initial history — load emojis first so :codes: render as images from the start
        (async () => {
            if (!customEmojis) {
                try {
                    const r = await fetch('/api/v2/emojis');
                    const data = await r.json();
                    if (data.success) {
                        customEmojis = {};
                        data.emojis.forEach(e => { customEmojis[e.name] = e.url; });
                        if (window.CommentSystem) window.CommentSystem.emojiCache = customEmojis;
                    }
                } catch (_) {}
            }
            if (!isMinimized && !isClosed && !isPhone()) loadHistory();
        })();

        // If user had previously closed the chat, hide on load and show the launcher
        if (isClosed) document.documentElement.classList.add('gchat-closed');
        updateDock();
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();
