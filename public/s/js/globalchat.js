/**
 * globalchat.js — Site-wide persistent chat widget
 * Panel is always visible. Minimize-only.
 * Features: emoji picker, :emoji autocomplete, @user autocomplete,
 *           greentext, [spoiler], [blur]
 */
(function () {
    'use strict';

    if (!window.f0ckSession?.enable_global_chat) return;
    if (!window.f0ckSession?.logged_in) return;
    if (window.location.pathname.startsWith('/abyss')) return;


    // Embedded chat images hide their wrapper when they fail to load
    F0ckActions.define('gchat-hide-parent', (el) => { if (el.parentNode) el.parentNode.style.display = 'none'; });

    const MAX_VISIBLE_MSGS = 100;
    const RATE_LIMIT_MS    = 800;

    let isMinimized  = localStorage.getItem('f0ck_chat_minimized') !== '0';
    let isClosed     = localStorage.getItem('f0ck_chat_closed')    === '1';
    let lastSent     = 0;
    let customEmojis = null; // name → url
    let unreadCount  = 0;
    let chatFocused  = document.hasFocus();
    const ytOembedCache = new Map(); // videoId → {title, author_name}

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
        if (unreadCount > 0) {
            const label = unreadCount > 99 ? '99+' : String(unreadCount);
            if (badge) { badge.textContent = label; badge.style.display = 'inline-flex'; }
            // Bubble badge — create it lazily if it doesn't exist yet
            if (bubble) {
                let bb = bubble.querySelector('.gchat-bubble-badge');
                if (!bb) {
                    bb = document.createElement('span');
                    bb.className = 'gchat-bubble-badge';
                    bubble.appendChild(bb);
                }
                bb.textContent = label;
                bb.style.display = '';
            }
        } else {
            if (badge) badge.style.display = 'none';
            const bb = document.getElementById('gchat-reopen-bubble')?.querySelector('.gchat-bubble-badge');
            if (bb) bb.style.display = 'none';
        }
    }

    function clearUnread() {
        unreadCount = 0;
        updateBadge();
    }

    window.addEventListener('focus', () => { chatFocused = true;  if (!isMinimized) clearUnread(); });
    window.addEventListener('blur',  () => { chatFocused = false; });


    // ── i18n ─────────────────────────────────────────────────────────────────
    function t(key, fallback) { return window.f0ckI18n?.[key] || fallback; }

    // ── Build widget ─────────────────────────────────────────────────────────
    function buildWidget() {
        const widget = document.createElement('div');
        widget.id = 'gchat-widget';
        widget.innerHTML = `
            <div id="gchat-panel" class="${isMinimized ? 'gchat-minimized' : ''}">
                <div id="gchat-resize-h"></div>
                <div id="gchat-resize-w"></div>
                <div id="gchat-resize-e"></div>
                <div id="gchat-header">
                    <span id="gchat-title"><i class="fa-solid fa-comments"></i> ${t('chat_title', 'Site Chat')}<span id="gchat-badge" class="gchat-badge" style="display:none"></span></span>
                    <div class="gchat-header-btns">
                        <button id="gchat-float-btn" title="Float / Dock" class="gchat-icon-btn">
                            <i class="fa-solid fa-up-right-and-down-left-from-center"></i>
                        </button>
                        <button id="gchat-minimize-btn" title="${isMinimized ? t('chat_expand','Expand') : t('chat_minimize','Minimize')}" class="gchat-icon-btn">
                            <i class="fa-solid ${isMinimized ? 'fa-chevron-up' : 'fa-chevron-down'}"></i>
                        </button>
                        <button id="gchat-close-btn" title="${t('chat_close','Close chat')}" class="gchat-icon-btn">
                            <i class="fa-solid fa-xmark"></i>
                        </button>
                    </div>
                </div>
                <div id="gchat-topic" style="display:none"></div>
                <div id="gchat-online"></div>
                <div id="gchat-messages"></div>
                <div id="gchat-input-area">
                    <div id="gchat-toolbar">
                        <button class="gchat-tool-btn" id="gchat-emoji-btn" title="Emoji">☺</button>
                        <button class="gchat-tool-btn" id="gchat-spoiler-btn" title="[spoiler]"><span class="spoiler" style="pointer-events:none;font-size:0.7em;padding:0 2px">S</span></button>
                        <button class="gchat-tool-btn" id="gchat-blur-btn" title="[blur]"><span class="blur-text" style="pointer-events:none;font-size:0.7em;padding:0 2px">B</span></button>
                    </div>
                    <div id="gchat-input-row">
                        <textarea id="gchat-input" placeholder="${t('chat_placeholder','Chat\u2026')}" maxlength="500" rows="1"></textarea>
                        <button id="gchat-send-btn" title="${t('common_send','Send')}">
                            <i class="fa-solid fa-paper-plane"></i>
                        </button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(widget);
    }

    // ── Helpers ───────────────────────────────────────────────────────────────
    function esc(str) {
        const d = document.createElement('div');
        d.textContent = str || '';
        return d.innerHTML;
    }

    function avatarSrc(msg) {
        if (msg.avatar_file) return `/a/${msg.avatar_file}`;
        if (msg.avatar)      return `/t/${msg.avatar}.webp`;
        return '/a/default.png';
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
                `<div class="gchat-yt-thumb-wrap">` +
                    `<img src="${thumb}" class="gchat-yt-thumb" alt="YouTube thumbnail" loading="lazy">` +
                    `<div class="gchat-yt-play"><i class="fa-brands fa-youtube"></i></div>` +
                `</div>` +
                `<div class="gchat-yt-info">` +
                    `<span class="gchat-yt-title">…</span>` +
                    `<span class="gchat-yt-author"></span>` +
                `</div>` +
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
                `<span class="gchat-post-card__thumb-wrap"><span class="gchat-post-card__thumb-placeholder"><i class="fa-solid fa-spinner fa-spin"></i></span></span>` +
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

            return `${pre}<a href="${url}" target="_blank" rel="noopener noreferrer">${url}<i class="fa-solid fa-arrow-up-right-from-square" style="font-size:0.7em;margin-left:3px;opacity:0.6"></i></a>`;
        });

        return html;
    }

    function renderMsg(msg) {
        const isSelf      = (msg.username === window.f0ckSession.user);
        const isAdmin     = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
        const displayName = msg.display_name || msg.username;
        const color       = msg.username_color ? `style="color:${esc(msg.username_color)}"` : '';
        const ts          = msg.created_at
            ? new Date(msg.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
            : '';
        const content = renderMsgContent(msg.message || '');
        const msgId   = msg.id ? `data-msg-id="${msg.id}"` : '';
        const replyUser = esc(msg.username || '');

        return `<div class="gchat-msg ${isSelf ? 'gchat-msg-self' : ''}" ${msgId}>
            ${!isSelf ? `<a href="/user/${esc(msg.username)}" class="gchat-avatar-link">
                <img src="${avatarSrc(msg)}" class="gchat-avatar" alt="${esc(displayName)}" loading="lazy">
            </a>` : ''}
            <div class="gchat-bubble-wrap">
                ${!isSelf ? `<span class="gchat-username" ${color}>${esc(displayName)}</span>` : ''}
                <div class="gchat-bubble">${content}</div>
                <div class="gchat-msg-actions">
                    ${ts ? `<span class="gchat-time">${ts}</span>` : ''}
                    <button class="gchat-reply-btn" data-username="${replyUser}" title="Reply"><i class="fa-solid fa-reply"></i></button>
                    ${isAdmin && msg.id ? `<button class="gchat-del-btn" data-id="${msg.id}" title="Delete"><i class="fa-solid fa-trash"></i></button>` : ''}
                </div>
            </div>
        </div>`;
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

    /**
     * Watch the message container and keep snapping to bottom for durationMs.
     * Only stops if the user actively scrolls up via wheel / touch / keyboard.
     * Same logic as the DM snapToBottomSticky.
     */
    function startStickyScroll(durationMs = 8000) {
        const el = document.getElementById('gchat-messages');
        if (!el) return;

        let userScrolledUp = false;

        const onWheel      = (e) => { if (e.deltaY < 0) userScrolledUp = true; };
        const onKey        = (e) => { if (['ArrowUp', 'PageUp', 'Home'].includes(e.key)) userScrolledUp = true; };
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

        if (typeof ResizeObserver !== 'undefined') {
            // Debounce: batch rapid layout changes (e.g. progressive image renders)
            // into a single smooth scroll instead of many jarring instant jumps.
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
            setTimeout(() => scrollToBottom(true), 300);
            setTimeout(() => scrollToBottom(true), 800);
            setTimeout(() => scrollToBottom(true), 2000);
        }

        setTimeout(cleanup, durationMs);

        // First snap is instant (no animation — the panel just opened)
        scrollToBottom(true);
        setTimeout(() => { if (!userScrolledUp) scrollToBottom(true); }, 150);
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
        if (mime.startsWith('video/')) typeBadge = '<i class="fa-solid fa-film"></i>';
        else if (mime.startsWith('audio/')) typeBadge = '<i class="fa-solid fa-music"></i>';
        else if (mime.startsWith('image/')) typeBadge = '<i class="fa-solid fa-image"></i>';

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
                `<span class="gchat-post-card__uploader"><i class="fa-solid fa-user"></i> ${uploader}</span>` +
                `<span class="gchat-post-card__comments"><i class="fa-solid fa-comment"></i> ${commentCount}</span>` +
            `</span>`;

        wrapEl.replaceWith(card);
    }

    function appendMsg(msg, scrollForce = false) {
        const container = document.getElementById('gchat-messages');
        if (!container) return;
        const div = document.createElement('div');
        div.innerHTML = renderMsg(msg);
        const node = div.firstElementChild;
        while (container.children.length >= MAX_VISIBLE_MSGS) container.removeChild(container.firstChild);
        container.appendChild(node);
        setTimeout(() => node.classList.add('gchat-msg-in'), 10);

        // Wire up spoiler clicks on newly appended msg
        node.querySelectorAll('.spoiler,.blur-text').forEach(s => {
            s.addEventListener('click', () => s.classList.toggle('revealed'));
        });

        // Embedded images: register with lazy observer; scroll on load only for new messages (not history)
        node.querySelectorAll('.gchat-embed-img img[data-lazy-src]').forEach(img => {
            // Only snap to bottom on image load for NEW incoming messages, not history.
            // History already scrolls once at the end of loadHistory; an extra scroll
            // here is what causes the double jump.
            if (scrollForce) img.addEventListener('load', () => scrollToBottom(true));
            img.addEventListener('click', () => openImgModal(img.src));
            img.style.cursor = 'zoom-in';
            lazyImgObserver.observe(img);
        });
        // Already-src'd images (avatars etc.) — same rule
        node.querySelectorAll('.gchat-embed-img img:not([data-lazy-src])').forEach(img => {
            if (scrollForce) img.addEventListener('load', () => scrollToBottom(true));
            img.addEventListener('click', () => openImgModal(img.src));
            img.style.cursor = 'zoom-in';
        });

        // Wire up YouTube oEmbed cards
        node.querySelectorAll('.gchat-yt-card[data-yt-id]').forEach(fetchYtOembed);
        // Wire up same-site item embeds
        node.querySelectorAll('.gchat-item-embed[data-item-id]').forEach(fetchItemPreview);

        scrollToBottom(scrollForce);

        // Unread badge: count if minimized or window not focused
        if (isMinimized || !chatFocused) {
            unreadCount++;
            updateBadge();
        }
    }

    async function loadHistory() {
        try {
            const res  = await fetch('/api/chat', { headers: { 'X-Requested-With': 'XMLHttpRequest' } });
            const data = await res.json();
            if (!data.success) return;
            const container = document.getElementById('gchat-messages');
            if (!container) return;
            container.innerHTML = '';
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

    async function sendMessage() {
        const input = document.getElementById('gchat-input');
        if (!input) return;
        const raw = input.value.trim();
        if (!raw) return;

        // Admin slash commands
        if (raw === '/clear') {
            const isAdmin = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
            if (!isAdmin) { input.value = ''; return; }
            const csrf = window.f0ckSession?.csrf_token;
            try {
                await fetch('/api/chat', {
                    method: 'DELETE',
                    headers: { 'X-Requested-With': 'XMLHttpRequest', ...(csrf ? { 'X-CSRF-Token': csrf } : {}) }
                });
                const container = document.getElementById('gchat-messages');
                if (container) container.innerHTML = '';
            } catch (e) { console.error('[Chat] Clear error', e); }
            input.value = '';
            return;
        }

        // /setbackground <url> [css opts]  — e.g. /setbackground https://… center / contain no-repeat
        if (raw.startsWith('/setbackground ') || raw === '/clearbg') {
            const isAdmin = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
            if (!isAdmin) { input.value = ''; return; }
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
            input.value = '';
            return;
        }

        // /settopic <text>  or  /cleartopic
        if (raw.startsWith('/settopic') || raw === '/cleartopic') {
            const isAdmin = window.f0ckSession.is_admin || window.f0ckSession.is_moderator;
            if (!isAdmin) { input.value = ''; return; }
            const csrf = window.f0ckSession?.csrf_token;
            const headers = { 'Content-Type': 'application/x-www-form-urlencoded', 'X-Requested-With': 'XMLHttpRequest', ...(csrf ? { 'X-CSRF-Token': csrf } : {}) };
            const topic = raw === '/cleartopic' ? '' : raw.slice('/settopic'.length).trim();
            try { await fetch('/api/chat/topic', { method: 'POST', headers, body: new URLSearchParams({ topic }) }); }
            catch (e) { console.error('[Chat] settopic error', e); }
            input.value = '';
            return;
        }

        const now = Date.now();
        if (now - lastSent < RATE_LIMIT_MS) { return; }
        lastSent = now;

        input.value = '';
        input.style.height = 'auto';

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
        const icon  = btn?.querySelector('i');
        if (panel) panel.classList.toggle('gchat-minimized', isMinimized);
        if (btn)   btn.title = isMinimized ? t('chat_expand','Expand') : t('chat_minimize','Minimize');
        if (icon)  icon.className = `fa-solid ${isMinimized ? 'fa-chevron-up' : 'fa-chevron-down'}`;
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
                    img.onerror = () => { img.style.display = 'none'; };
                    img.addEventListener('click', (e) => {
                        e.stopPropagation();
                        const pos = textarea.selectionStart ?? textarea.value.length;
                        const v   = textarea.value;
                        textarea.value = v.slice(0, pos) + `:${name}:` + v.slice(pos);
                        const newPos = pos + name.length + 2;
                        textarea.setSelectionRange(newPos, newPos);
                        textarea.focus();
                        picker.style.display = 'none';
                        if (closeHandler) { document.removeEventListener('click', closeHandler); closeHandler = null; }
                    });
                    picker.appendChild(img);
                });
            } else {
                picker.innerHTML = '<div style="padding:8px;opacity:0.6;font-size:0.85em">No custom emojis</div>';
            }

            // Position above input area
            const panel = document.getElementById('gchat-panel');
            panel.appendChild(picker);
        };

        btn.addEventListener('click', (e) => {
            e.preventDefault();
            if (!picker) buildPicker();

            const isVisible = picker.style.display !== 'none' && picker.style.display !== '';
            if (isVisible) {
                picker.style.display = 'none';
                if (closeHandler) { document.removeEventListener('click', closeHandler); closeHandler = null; }
            } else {
                picker.style.display = 'grid';
                closeHandler = (ev) => {
                    if (!picker.contains(ev.target) && ev.target !== btn) {
                        picker.style.display = 'none';
                        document.removeEventListener('click', closeHandler);
                        closeHandler = null;
                    }
                };
                setTimeout(() => document.addEventListener('click', closeHandler), 0);
            }
        });

        // Rebuild picker if emojis load after init
        window.addEventListener('f0ck:emojis_ready', (e) => {
            customEmojis = e.detail;
            if (picker) { picker.remove(); picker = null; } // force rebuild on next open
        });
    }

    // ── :emoji inline autocomplete ────────────────────────────────────────────
    function setupEmojiAutocomplete(textarea) {
        const ac = document.createElement('div');
        ac.id = 'gchat-emoji-ac';
        ac.style.display = 'none';
        document.body.appendChild(ac);

        let acMatches = [], acIdx = -1;

        const getColon = () => {
            const pos  = textarea.selectionStart;
            const text = textarea.value.slice(0, pos);
            const m    = text.match(/:([a-z0-9_]{0,})$/i);
            if (!m) return null;
            return { query: m[1].toLowerCase(), colonPos: pos - m[0].length };
        };

        const positionAC = () => {
            const rect = textarea.getBoundingClientRect();
            ac.style.left   = rect.left + 'px';
            ac.style.width  = rect.width + 'px';
            ac.style.bottom = (window.innerHeight - rect.top + 4) + 'px';
            ac.style.top    = 'auto';
        };

        const hideAC = () => {
            ac.style.display = 'none';
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
            hideAC();
        };

        const setActiveAC = (idx) => {
            ac.querySelectorAll('.gchat-emoji-ac-item').forEach(el => el.classList.remove('active'));
            const items = ac.querySelectorAll('.gchat-emoji-ac-item');
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
            acMatches.forEach((name, i) => {
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
            positionAC();
            ac.style.display = 'flex';
            // Auto-select first match so Enter works immediately
            setActiveAC(0);
        };

        textarea.addEventListener('input',  () => renderAC());
        textarea.addEventListener('click',  () => { if (getColon()) renderAC(); });
        textarea.addEventListener('blur',   () => setTimeout(hideAC, 150));

        textarea.addEventListener('keydown', e => {
            if (ac.style.display === 'none') return;
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

    // ── @user inline autocomplete (always opens upward) ──────────────────────
    function setupMentionAutocomplete(textarea) {
        const mac = document.createElement('div');
        mac.id = 'gchat-mention-ac';
        mac.style.display = 'none';
        document.body.appendChild(mac);

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

        const positionMAC = () => {
            const rect = textarea.getBoundingClientRect();
            mac.style.left   = rect.left + 'px';
            mac.style.width  = rect.width + 'px';
            mac.style.bottom = (window.innerHeight - rect.top + 4) + 'px';
            mac.style.top    = 'auto';
        };

        const hideMAC = () => {
            mac.style.display = 'none';
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
            hideMAC();
        };

        const setActiveMAC = (idx) => {
            mac.querySelectorAll('.gchat-mention-item').forEach(el => el.classList.remove('active'));
            const items = mac.querySelectorAll('.gchat-mention-item');
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
                img.onerror = () => { img.src = '/a/default.png'; };
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
            positionMAC();
            mac.style.display = 'flex';
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
            if (mac.style.display === 'none') return;
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

    // ── Toolbar button helpers ────────────────────────────────────────────────
    function insertWrap(textarea, open, close) {
        const s = textarea.selectionStart, e = textarea.selectionEnd;
        const v = textarea.value;
        const selected = v.substring(s, e);
        if (selected) {
            textarea.value = v.slice(0, s) + open + selected + close + v.slice(e);
            textarea.setSelectionRange(s + open.length + selected.length + close.length,
                                       s + open.length + selected.length + close.length);
        } else {
            textarea.value = v.slice(0, s) + open + close + v.slice(s);
            textarea.setSelectionRange(s + open.length, s + open.length);
        }
        textarea.focus();
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
        document.body.style.overflow = 'hidden';
    }
    function closeImgModal() {
        _imgModal?.classList.remove('gchat-img-modal-open');
        document.body.style.overflow = '';
    }

    // ── Init ─────────────────────────────────────────────────────────────────
    function init() {
        buildWidget();

        // Gate chat on SSE connection — block input until stream is live
        const _panel     = document.getElementById('gchat-panel');
        const _inputArea = document.getElementById('gchat-input-area');
        const _messages  = document.getElementById('gchat-messages');
        let _sseReady = false;

        function setConnecting(connecting) {
                if (_inputArea) _inputArea.style.opacity       = connecting ? '0.35' : '1';
                if (_inputArea) _inputArea.style.pointerEvents = connecting ? 'none' : '';
                if (_messages)  _messages.style.opacity        = connecting ? '0.35' : '1';
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
        // Failsafe: if SSE already fired before this ran (e.g. fast reconnect), check flag
        if (document.documentElement.dataset.sseReady === '1') onSseReady();

        // Absolute fallback: if SSE hasn't connected within 10s, unlock anyway and poll
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

        const textarea = document.getElementById('gchat-input');



        // Returns min top position for float panel (below navbar + MOTD)
        function getTopBound() {
            const nav  = document.querySelector('nav.navbar');
            const motd = document.getElementById('motd-container');
            let bound = nav ? nav.getBoundingClientRect().bottom : 0;
            if (motd && motd.offsetParent !== null && motd.getBoundingClientRect().bottom > bound) {
                bound = motd.getBoundingClientRect().bottom;
            }
            return Math.max(0, bound);
        }

        // Restore saved height
        const panel = document.getElementById('gchat-panel');
        const savedH = localStorage.getItem('f0ck_chat_height');
        if (savedH) panel.style.height = savedH + 'px';

        // Restore saved width
        const savedW = localStorage.getItem('f0ck_chat_width');
        if (savedW) panel.style.width = savedW + 'px';

        // Drag-to-resize height via top invisible strip
        const resizeH = document.getElementById('gchat-resize-h');
        function startResizeH(startY, startH, startTop) {
            const onMove = (clientY) => {
                const delta = startY - clientY;
                const maxH  = window.innerHeight * 0.95;
                const newH  = Math.min(Math.max(startH + delta, 180), maxH);
                panel.style.height = newH + 'px';
                if (isFloating) {
                    const newTop = startTop - (newH - startH);
                    panel.style.top = Math.max(getTopBound(), newTop) + 'px';
                }
            };
            const onEnd = () => {
                const h = parseInt(panel.style.height);
                if (h) localStorage.setItem('f0ck_chat_height', h);
                if (isFloating) localStorage.setItem('f0ck_chat_float_y', parseInt(panel.style.top));
                document.removeEventListener('mousemove', mouseMove);
                document.removeEventListener('mouseup',   mouseEnd);
                document.removeEventListener('touchmove', touchMove);
                document.removeEventListener('touchend',  touchEnd);
            };
            const mouseMove = mv => onMove(mv.clientY);
            const mouseEnd  = onEnd;
            const touchMove = mv => { mv.preventDefault(); onMove(mv.touches[0].clientY); };
            const touchEnd  = onEnd;
            document.addEventListener('mousemove', mouseMove);
            document.addEventListener('mouseup',   mouseEnd);
            document.addEventListener('touchmove', touchMove, { passive: false });
            document.addEventListener('touchend',  touchEnd);
        }
        resizeH.addEventListener('mousedown', (e) => {
            if (e.button !== 0) return;
            if (isMinimized) return;
            e.preventDefault();
            startResizeH(e.clientY, panel.getBoundingClientRect().height, panel.getBoundingClientRect().top);
        });
        resizeH.addEventListener('touchstart', (e) => {
            if (isMinimized) return;
            e.preventDefault();
            startResizeH(e.touches[0].clientY, panel.getBoundingClientRect().height, panel.getBoundingClientRect().top);
        }, { passive: false });


        // Drag-to-resize width from the left handle
        const resizeW = document.getElementById('gchat-resize-w');
        function startResizeW(startX, startW, startLeft) {
            const onMove = (clientX) => {
                const delta  = startX - clientX;
                const newW   = Math.min(Math.max(startW + delta, 200), window.innerWidth * 0.6);
                panel.style.width = newW + 'px';
                if (isFloating) {
                    const newLeft = startLeft - (newW - startW);
                    panel.style.left = Math.max(0, newLeft) + 'px';
                }
            };
            const onEnd = () => {
                const w = parseInt(panel.style.width);
                if (w) localStorage.setItem('f0ck_chat_width', w);
                if (isFloating) localStorage.setItem('f0ck_chat_float_x', parseInt(panel.style.left));
                document.removeEventListener('mousemove', mouseMove);
                document.removeEventListener('mouseup',   mouseEnd);
                document.removeEventListener('touchmove', touchMove);
                document.removeEventListener('touchend',  touchEnd);
            };
            const mouseMove = mv => onMove(mv.clientX);
            const mouseEnd  = onEnd;
            const touchMove = mv => { mv.preventDefault(); onMove(mv.touches[0].clientX); };
            const touchEnd  = onEnd;
            document.addEventListener('mousemove', mouseMove);
            document.addEventListener('mouseup',   mouseEnd);
            document.addEventListener('touchmove', touchMove, { passive: false });
            document.addEventListener('touchend',  touchEnd);
        }
        resizeW.addEventListener('mousedown', (e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            const r = panel.getBoundingClientRect();
            startResizeW(e.clientX, r.width, r.left);
        });
        resizeW.addEventListener('touchstart', (e) => {
            e.preventDefault();
            const r = panel.getBoundingClientRect();
            startResizeW(e.touches[0].clientX, r.width, r.left);
        }, { passive: false });

        // Drag-to-resize width from the RIGHT handle (left edge stays fixed)
        const resizeE = document.getElementById('gchat-resize-e');
        function startResizeE(startX, startW) {
            const onMove = (clientX) => {
                const delta = clientX - startX;
                const newW  = Math.min(Math.max(startW + delta, 200), window.innerWidth * 0.6);
                panel.style.width = newW + 'px';
            };
            const onEnd = () => {
                const w = parseInt(panel.style.width);
                if (w) localStorage.setItem('f0ck_chat_width', w);
                document.removeEventListener('mousemove', mouseMove);
                document.removeEventListener('mouseup',   mouseEnd);
                document.removeEventListener('touchmove', touchMove);
                document.removeEventListener('touchend',  touchEnd);
            };
            const mouseMove = mv => onMove(mv.clientX);
            const mouseEnd  = onEnd;
            const touchMove = mv => { mv.preventDefault(); onMove(mv.touches[0].clientX); };
            const touchEnd  = onEnd;
            document.addEventListener('mousemove', mouseMove);
            document.addEventListener('mouseup',   mouseEnd);
            document.addEventListener('touchmove', touchMove, { passive: false });
            document.addEventListener('touchend',  touchEnd);
        }
        resizeE.addEventListener('mousedown', (e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            const r = panel.getBoundingClientRect();
            startResizeE(e.clientX, r.width);
        });
        resizeE.addEventListener('touchstart', (e) => {
            e.preventDefault();
            const r = panel.getBoundingClientRect();
            startResizeE(e.touches[0].clientX, r.width);
        }, { passive: false });

        // ── Float / Dock toggle ───────────────────────────────────────────────
        let isFloating = localStorage.getItem('f0ck_chat_floating') === '1';

        function applyFloatMode() {
            const widget = document.getElementById('gchat-widget');
            if (isFloating) {
                panel.classList.add('gchat-floating');
                const fx = localStorage.getItem('f0ck_chat_float_x');
                const fy = localStorage.getItem('f0ck_chat_float_y');
                if (fx) panel.style.left = fx + 'px';
                if (fy) panel.style.top  = Math.max(getTopBound(), parseInt(fy)) + 'px';
                panel.style.right  = 'auto';
                panel.style.bottom = 'auto';
                document.getElementById('gchat-float-btn').innerHTML = '<i class="fa-solid fa-down-left-and-up-right-to-center"></i>';
            } else {
                panel.classList.remove('gchat-floating');
                panel.style.left   = '';
                panel.style.top    = '';
                panel.style.right  = '';
                panel.style.bottom = '';
                document.getElementById('gchat-float-btn').innerHTML = '<i class="fa-solid fa-up-right-and-down-left-from-center"></i>';
            }
        }
        applyFloatMode();

        document.getElementById('gchat-float-btn').addEventListener('click', () => {
            isFloating = !isFloating;
            localStorage.setItem('f0ck_chat_floating', isFloating ? '1' : '0');
            if (isFloating) {
                // Snap to current visual position so it doesn't jump
                const r = panel.getBoundingClientRect();
                panel.style.left = r.left + 'px';
                panel.style.top  = r.top  + 'px';
                localStorage.setItem('f0ck_chat_float_x', Math.round(r.left));
                localStorage.setItem('f0ck_chat_float_y', Math.round(r.top));
            }
            applyFloatMode();
        });

        // Drag-to-move in float mode — drag the header
        const header = document.getElementById('gchat-header');
        function startDragMove(startX, startY, offX, offY) {
            const r = panel.getBoundingClientRect();
            const onMove = (clientX, clientY) => {
                const newX = Math.max(0,            Math.min(clientX - offX, window.innerWidth  - r.width));
                const newY = Math.max(getTopBound(), Math.min(clientY - offY, window.innerHeight - r.height));
                panel.style.left = newX + 'px';
                panel.style.top  = newY + 'px';
            };
            const onEnd = () => {
                const curY = parseInt(panel.style.top);
                localStorage.setItem('f0ck_chat_float_x', parseInt(panel.style.left));
                localStorage.setItem('f0ck_chat_float_y', curY);

                if (isMinimized) {
                    const topBound = getTopBound();
                    const bottomBound = window.innerHeight;
                    const distToTop = Math.abs(curY - topBound);
                    const distToBottom = Math.abs(bottomBound - (curY + 42));
                    localStorage.setItem('f0ck_chat_anchor_top', distToTop < distToBottom ? '1' : '0');
                }

                document.removeEventListener('mousemove', mouseMove);
                document.removeEventListener('mouseup',   mouseEnd);
                document.removeEventListener('touchmove', touchMove);
                document.removeEventListener('touchend',  touchEnd);
            };
            const mouseMove = mv => onMove(mv.clientX, mv.clientY);
            const mouseEnd  = onEnd;
            const touchMove = mv => { mv.preventDefault(); onMove(mv.touches[0].clientX, mv.touches[0].clientY); };
            const touchEnd  = onEnd;
            document.addEventListener('mousemove', mouseMove);
            document.addEventListener('mouseup',   mouseEnd);
            document.addEventListener('touchmove', touchMove, { passive: false });
            document.addEventListener('touchend',  touchEnd);
        }
        header.addEventListener('mousedown', (e) => {
            if (e.button !== 0) return;
            if (!isFloating) return;
            if (e.target.closest('button')) return;
            e.preventDefault();
            const r = panel.getBoundingClientRect();
            startDragMove(e.clientX, e.clientY, e.clientX - r.left, e.clientY - r.top);
        });
        header.addEventListener('touchstart', (e) => {
            if (!isFloating) return;
            if (e.target.closest('button')) return;
            e.preventDefault();
            const r = panel.getBoundingClientRect();
            const t = e.touches[0];
            startDragMove(t.clientX, t.clientY, t.clientX - r.left, t.clientY - r.top);
        }, { passive: false });

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
                        let newTop;
                        if (anchorTop) {
                            // Anchor to top: keep current top
                            newTop = Math.max(topBound, curTop);
                        } else {
                            // Anchor to bottom: keep current bottom (existing behavior)
                            newTop = Math.min(bottomBound - 42, Math.max(topBound, curBottom - 42));
                        }
                        panel.style.top = newTop + 'px';
                        localStorage.setItem('f0ck_chat_float_y', newTop);
                    });
                } else {
                    // Expanding: use saved anchor or default to bottom if not set
                    const wasAnchorTop = localStorage.getItem('f0ck_chat_anchor_top') === '1';
                    setMinimized(false);

                    requestAnimationFrame(() => {
                        const fullH  = panel.getBoundingClientRect().height;
                        const curTop = parseInt(panel.style.top) || 0;
                        let newTop;
                        if (wasAnchorTop) {
                            // Expanding from top anchor: top stays same
                            newTop = Math.max(topBound, curTop);
                        } else {
                            // Expanding from bottom anchor: shift top UP (existing behavior)
                            newTop = Math.max(topBound, curTop - (fullH - 42));
                        }
                        panel.style.top = newTop + 'px';
                        localStorage.setItem('f0ck_chat_float_y', newTop);
                    });
                }
            } else {
                setMinimized(!isMinimized);
                if (!isMinimized) {
                    // Normal docked expand
                    loadHistory();
                }
            }
        }

        document.getElementById('gchat-minimize-btn').addEventListener('click', e => {
            e.stopPropagation(); toggleMinimized();
        });
        document.getElementById('gchat-header').addEventListener('dblclick', e => {
            if (!e.target.closest('.gchat-icon-btn')) toggleMinimized();
        });

        // Close button — hide widget and show reopen bubble
        function closeChat() {
            isClosed = true;
            localStorage.setItem('f0ck_chat_closed', '1');
            const widget = document.getElementById('gchat-widget');
            if (widget) widget.style.display = 'none';
            buildReopenBubble();
        }
        function reopenChat() {
            isClosed = false;
            localStorage.removeItem('f0ck_chat_closed');
            const widget = document.getElementById('gchat-widget');
            if (widget) widget.style.display = '';
            const bubble = document.getElementById('gchat-reopen-bubble');
            if (bubble) bubble.remove();
            if (isMinimized) { setMinimized(false); } else { loadHistory(); }
        }
        function buildReopenBubble() {
            if (document.getElementById('gchat-reopen-bubble')) return;
            const el = document.createElement('button');
            el.id = 'gchat-reopen-bubble';
            el.title = t('chat_open', 'Open chat');
            el.innerHTML = '<i class="fa-solid fa-comments"></i>';
            el.addEventListener('click', reopenChat);
            document.body.appendChild(el);
        }
        document.getElementById('gchat-close-btn').addEventListener('click', e => {
            e.stopPropagation(); closeChat();
        });
        // Expose globally for settings page
        window.reopenChat  = reopenChat;
        window.closeChatWidget = closeChat;


        // Send
        document.getElementById('gchat-send-btn').addEventListener('click', sendMessage);
        textarea.addEventListener('keydown', e => {
            if (e.key === 'Enter' && !e.shiftKey) {
                // Don't send while emoji or mention autocomplete is active
                const emojiAC   = document.getElementById('gchat-emoji-ac');
                const mentionAC = document.getElementById('gchat-mention-ac');
                if ((emojiAC && emojiAC.style.display !== 'none') ||
                    (mentionAC && mentionAC.style.display !== 'none')) return;
                e.preventDefault();
                sendMessage();
            }
        });

        // Auto-grow
        textarea.addEventListener('input', function () {
            this.style.height = 'auto';
            this.style.height = Math.min(this.scrollHeight, 80) + 'px';
        });

        // Toolbar
        document.getElementById('gchat-spoiler-btn').addEventListener('click', e => {
            e.preventDefault(); insertWrap(textarea, '[spoiler]', '[/spoiler]');
        });
        document.getElementById('gchat-blur-btn').addEventListener('click', e => {
            e.preventDefault(); insertWrap(textarea, '[blur]', '[/blur]');
        });

        // @mention autocomplete (always opens upward)
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

        // SSE live messages
        document.addEventListener('f0ck:global_chat', e => appendMsg(e.detail, !isMinimized));

        // SSE clear — wipe messages instantly for all clients
        document.addEventListener('f0ck:global_chat_clear', () => {
            const container = document.getElementById('gchat-messages');
            if (container) container.innerHTML = '';
        });

        // SSE delete — remove a single message for all clients
        document.addEventListener('f0ck:global_chat_delete', (e) => {
            const id = e.detail?.id;
            if (!id) return;
            const msgEl = document.querySelector(`#gchat-messages [data-msg-id="${id}"]`);
            if (msgEl) {
                msgEl.style.opacity = '0';
                msgEl.style.transform = 'scaleY(0)';
                setTimeout(() => msgEl.remove(), 200);
            }
        });

        // ── Chat background helpers ───────────────────────────────────────────
        function applyBackground(css) {
            const msgs = document.getElementById('gchat-messages');
            if (!msgs) return;
            msgs.style.background = css || '';
        }

        // Restore background on load
        fetch('/api/chat/background').then(r => r.json()).then(d => {
            if (d.success && d.background) applyBackground(d.background);
        }).catch(() => {});

        // SSE background change — apply instantly to all clients
        document.addEventListener('f0ck:global_chat_background', (e) => {
            applyBackground(e.detail?.background || null);
        });

        // ── Chat topic helpers ────────────────────────────────────────────────
        function applyTopic(text) {
            const el = document.getElementById('gchat-topic');
            if (!el) return;
            if (text) {
                el.textContent = text;
                el.style.display = '';
            } else {
                el.style.display = 'none';
                el.textContent = '';
            }
        }

        // Restore topic on load
        fetch('/api/chat/topic').then(r => r.json()).then(d => {
            if (d.success && d.topic) applyTopic(d.topic);
        }).catch(() => {});

        // SSE topic change — apply instantly to all clients
        document.addEventListener('f0ck:global_chat_topic', (e) => {
            applyTopic(e.detail?.topic || null);
        });

        // ── Online users bar ─────────────────────────────────────────────────
        function renderOnline(users, guestCount = 0) {
            const el = document.getElementById('gchat-online');
            if (!el) return;
            if ((!users || users.length === 0) && guestCount === 0) {
                el.innerHTML = '';
                el.style.display = 'none';
                return;
            }
            el.style.display = 'block';
            // Show up to 8 avatars, then "+N more" pill
            const MAX_SHOWN = 8;
            const shown  = users.slice(0, MAX_SHOWN);
            const extra  = users.length - shown.length;
            const avatarHTML = shown.map(u => {
                const name = esc(u.display_name || u.username || '?');
                const src  = avatarSrc(u);
                const colorStyle = u.username_color ? `style="border-color:${esc(u.username_color)}"` : '';
                return `<img src="${src}" class="gchat-online-avatar" title="${name}" alt="${name}" loading="lazy" ${colorStyle}>`;
            }).join('');
            const extraPill = extra > 0 ? `<span class="gchat-online-extra">+${extra}</span>` : '';
            
            let countText = `${users.length} online`;
            if (guestCount > 0 && (window.f0ckSession.is_admin || window.f0ckSession.is_moderator)) {
                countText += ` (${guestCount} guests)`;
            }
            const countLabel = `<span class="gchat-online-count">${countText}</span>`;
            el.innerHTML = `<div class="gchat-online-inner">${countLabel}<div class="gchat-online-avatars">${avatarHTML}${extraPill}</div></div>`;
        }
        document.addEventListener('f0ck:global_chat_presence', (e) => {
            renderOnline(e.detail?.users || [], e.detail?.guestCount || 0);
        });

        // Event delegation: reply + admin delete buttons inside #gchat-messages
        const msgArea = document.getElementById('gchat-messages');
        const csrf = () => window.f0ckSession?.csrf_token;

        // Clear unread when user interacts with the chat
        msgArea.addEventListener('mouseenter', clearUnread);
        textarea.addEventListener('focus', clearUnread);

        msgArea.addEventListener('click', async (e) => {
            // Reply button
            const replyBtn = e.target.closest('.gchat-reply-btn');
            if (replyBtn) {
                const username = replyBtn.dataset.username;
                if (username) {
                    const mention = `@${username} `;
                    textarea.value = mention + textarea.value;
                    textarea.setSelectionRange(mention.length, mention.length);
                    textarea.focus();
                }
                return;
            }

            // Admin delete button
            const delBtn = e.target.closest('.gchat-del-btn');
            if (delBtn) {
                const msgId = delBtn.dataset.id;
                if (!msgId) return;
                try {
                    const res  = await fetch(`/api/chat/${msgId}`, {
                        method: 'DELETE',
                        headers: { 'X-Requested-With': 'XMLHttpRequest', ...(csrf() ? { 'X-CSRF-Token': csrf() } : {}) }
                    });
                    const data = await res.json();
                    if (data.success) {
                        const msgEl = msgArea.querySelector(`[data-msg-id="${msgId}"]`);
                        if (msgEl) {
                            msgEl.style.opacity = '0';
                            msgEl.style.transform = 'scaleY(0)';
                            setTimeout(() => msgEl.remove(), 200);
                        }
                    }
                } catch (err) { console.error('[Chat] Delete error', err); }
                return;
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
            if (!isMinimized) loadHistory();
        })();

        // If user had previously closed the chat, hide on load and show bubble
        if (isClosed) {
            const widget = document.getElementById('gchat-widget');
            if (widget) widget.style.display = 'none';
            buildReopenBubble();
        }
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();
