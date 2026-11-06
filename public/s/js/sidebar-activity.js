(function () {
    F0ckActions.define({
        // Thumbnail failed: hide it and reveal the placeholder that follows it
        'sidebar-thumb-fail': (el) => {
            el.style.display = 'none';
            if (el.nextElementSibling) el.nextElementSibling.classList.remove('hidden');
        },
        'sidebar-img-loaded': (el) => { el.classList.add('loaded'); },
    });

    let customEmojis = {};
    let loading = false;
    let loadingMore = false;
    let currentPage = 1;
    let hasMore = true;
    let ioSentinel = null; // persistent sentinel element for IntersectionObserver
    let lastRenderedIds = ''; // track rendered comment IDs to skip needless re-renders
    // Shared cache for activity across AJAX loads
    if (!window._sidebarActivityCache) window._sidebarActivityCache = [];

    const loadEmojis = async () => {
        // Prefer CommentSystem's already-fetched cache to avoid a redundant request
        const csCache = (typeof CommentSystem !== 'undefined' && CommentSystem.emojiCache)
                     || null;
        if (csCache && Object.keys(csCache).length > 0) {
            Object.assign(customEmojis, csCache);
            return;
        }
        // No shared cache yet — fetch directly
        if (Object.keys(customEmojis).length > 0) return;
        try {
            const res = await fetch('/api/v2/emojis');
            const data = await res.json();
            if (data.success) {
                data.emojis.forEach(e => {
                    customEmojis[e.name] = e.url;
                });
            }
        } catch (e) {
            console.error("Sidebar Activity: Failed to load emojis", e);
        }
    };

    const renderEmoji = (match, name) => {
        if (customEmojis[name]) {
            const url = customEmojis[name];
            if (url.endsWith('.webm')) {
                return `<video class="sidebar-comment-img emoji" src="${url}" title=":${name}:" autoplay loop muted playsinline></video>`;
            }
            return `<img class="sidebar-comment-img emoji" src="${url}" alt="${name}" title=":${name}:" loading="lazy">`;
        }
        return match;
    };

    const escapeHtml = (unsafe) => {
        if (!unsafe) return '';
        const div = document.createElement('div');
        div.textContent = unsafe;
        return div.innerHTML.replace(/"/g, '&quot;').replace(/'/g, '&#39;');
    };

    // Name without capitals, digits or tall letters ("x", "anonymous"): CSS trims it to the
    // x-height so its top lines up with the avatar. Same rule as the `short_letters` template filter.
    const isShortName = (name) => /[a-z]/.test(name || '') && /^[acegmnopqrsuvwxyz _.,-]+$/.test(name);

    const playSidebarEmojiVideos = (container) => {
        if (!container) return;
        container.querySelectorAll('video.emoji').forEach(v => {
            v.play().catch(() => {
                v.addEventListener('canplay', () => v.play().catch(() => {}), { once: true });
            });
        });
    };

    const ytOembedCache = new Map(); // videoId -> meta object
    const ytOembedPending = new Map(); // videoId -> Promise


    const fetchSidebarYoutubeTitles = async (container) => {
        const links = container.querySelectorAll('.sidebar-video-link[data-yt-id]');
        if (links.length === 0) return;

        for (const link of links) {
            const videoId = link.dataset.ytId;
            if (!videoId) continue;

            const titleSpan = link.querySelector('.yt-title');
            if (!titleSpan || titleSpan.dataset.loaded === 'true') continue;

            let meta = ytOembedCache.get(videoId);
            if (!meta) {
                if (ytOembedPending.has(videoId)) {
                    meta = await ytOembedPending.get(videoId);
                } else {
                    const promise = (async () => {
                        const ytUrl = `https://www.youtube.com/watch?v=${encodeURIComponent(videoId)}`;
                        // 1. Try client-side oEmbed first (avoids Tor/proxy consent walls)
                        try {
                            const oembedUrl = `https://www.youtube.com/oembed?url=${encodeURIComponent(ytUrl)}&format=json`;
                            const clientResp = await fetch(oembedUrl);
                            if (clientResp.ok) {
                                const clientData = await clientResp.json();
                                if (clientData.title) {
                                    const metaObj = {
                                        title: clientData.title,
                                        site_name: 'youtube.com',
                                        author: clientData.author_name || 'Unknown'
                                    };
                                    // Cache client-side
                                    ytOembedCache.set(videoId, metaObj);
                                    // Push to server cache so other clients/server benefit
                                    try {
                                        const csrf = window.f0ckSession?.csrf_token;
                                        fetch('/api/v2/meta/cache', {
                                            method: 'POST',
                                            headers: {
                                                'Content-Type': 'application/json',
                                                ...(csrf ? { 'X-CSRF-Token': csrf } : {})
                                            },
                                            body: JSON.stringify({ url: ytUrl, meta: metaObj })
                                        }).catch(() => {});
                                    } catch (_) {}
                                    return metaObj;
                                }
                            }
                        } catch (e) { }

                        // 2. Fall back to backend meta fetch
                        try {
                            const r = await fetch(`/api/v2/meta/fetch?url=${encodeURIComponent(ytUrl)}`);
                            if (r.ok) {
                                const data = await r.json();
                                if (data.success && data.meta) {
                                    ytOembedCache.set(videoId, data.meta);
                                    return data.meta;
                                }
                            }
                        } catch (e) { }
                        return null;
                    })();
                    ytOembedPending.set(videoId, promise);
                    meta = await promise;
                    ytOembedPending.delete(videoId);
                }
            }

            if (meta && meta.title) {
                titleSpan.textContent = meta.title;
            } else {
                // If title fails, just leave it blank or use a generic label
                titleSpan.textContent = 'YouTube Video';
            }
            titleSpan.dataset.loaded = 'true';
            
            // Re-check overflow since the new text might cause wrapping
            const inner = link.closest('.comment-content-inner');
            if (inner && typeof checkOverflow === 'function') {
                checkOverflow(inner);
            }
        }
    };

    // Maximum characters to render in the sidebar per comment
    const SIDEBAR_CONTENT_TRUNCATE = 200;

    const renderCommentContent = (content, commentId = null, itemId = null) => {
        if (!content) return '';

        if (typeof marked === 'undefined') {
            return escapeHtml(content)
                .replace(/:([a-z0-9_]+):/g, (m, n) => renderEmoji(m, n));
        }

        try {
            // Extract and protect code blocks (```...```) before escaping
            const codeBlocks = [];
            let processed = content.replace(/```([\s\S]*?)```/g, (match) => {
                const placeholder = `BLOCKPORTALX${codeBlocks.length}X`;
                codeBlocks.push(marked.parse(match));
                return placeholder;
            });

            let escaped = escapeHtml(processed)
                .replace(/&gt;/g, ">"); // Restore > for markdown markers

            // Handle Image Embeds (Client-side)
            const siteOrigin = window.location.origin;
            const escapedSiteUrl = siteOrigin.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            const allowedHosts = [escapedSiteUrl];
            if (window.f0ckAllowedImages && Array.isArray(window.f0ckAllowedImages)) {
                window.f0ckAllowedImages.forEach(h => {
                    const escapedHost = h.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
                    allowedHosts.push(`(?:[a-z0-9-]+\\.)*${escapedHost}`);
                });
            }
            const hostsRegexPart = allowedHosts.join('|');
            // "Safe non-whitespace" stops at protocol boundaries so concatenated URLs aren't merged.
            const safeS = `(?:(?!https?:\\/\\/)\\S)`;
            const domainOrRelative = `(?:(?:https?:\\/\\/|\\/\\/)?(?:${hostsRegexPart})|(?:(?<!\\S)|(?<=\\]))(?=\\/[a-zA-Z0-9_\\-]))`;
            const imageRegex = new RegExp(`(?<![\\(\\[])(${domainOrRelative}(?:\\/${safeS}+\\.(?:jpg|jpeg|png|gif|webp)(?:\\?${safeS}+)?))(?![\\)\\]])`, 'gi');
            const rawVideoRegex = new RegExp(`(?<![\\(\\[])(${domainOrRelative}(?:\\/[^\\s\\[\\]\\(\\)]*\\.(?:mp4|webm|ogv|mov)(?:\\?[^\\s\\[\\]\\(\\)]+)?(?:#gif)?))`, 'gi');
            const mentionRegex = /(?<!\[)@([a-zA-Z0-9_\-\.]+)(?!\])|\[@([^\]]+)\]\(\/user\/([^)]+)\)|\[@([^\]]+)\]/g;

            const renderer = new marked.Renderer();
            renderer.blockquote = function (quote) {
                let text = (typeof quote === 'string') ? quote : (quote.text || '');
                text = text.replace(/<p>|<\/p>/g, '');
                return text.split('\n').map(line => {
                    if (!line.trim()) return '';
                    return `<span class="greentext">&gt;${line}</span>`;
                }).join('\n');
            };
            renderer.paragraph = function (text) {
                return (typeof text === 'string') ? text : (text.text || '');
            };

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

                if (href.startsWith(siteOrigin) || (href.startsWith('/') && !href.startsWith('//'))) {
                    isSameSite = true;
                } else {
                    try {
                        const urlToParse = href.startsWith('//') ? window.location.protocol + href : href;
                        const urlObj = new URL(urlToParse, siteOrigin);
                        isSameSite = (urlObj.hostname === window.location.hostname);
                    } catch (e) { }
                }

                let displayText = text;
                if (isSameSite && (text === href || text === href.replace(/^https?:\/\//, '') || text === href.replace(siteOrigin, ''))) {
                    try {
                        const urlToParse = href.startsWith('//') ? window.location.protocol + href : href;
                        const url = new URL(urlToParse.startsWith('http') ? urlToParse : siteOrigin + (urlToParse.startsWith('/') ? '' : '/') + urlToParse);
                        displayText = url.pathname + url.search + url.hash;
                    } catch (e) { }
                }

                const isMention = href.startsWith('/user/') && text.startsWith('@');
                if (isExternal && !isSameSite) {
                    return `<a href="${href}"${titleAttr} target="_blank" rel="noopener noreferrer">${displayText}<i class="fa-solid fa-arrow-up-right-from-square external-link-icon"></i></a>${extraSuffix}`;
                }
                return `<a href="${href}"${titleAttr}${isMention ? ' class="mention"' : ''}>${displayText}</a>${extraSuffix}`;
            };
            renderer.image = function (href, title, text) {
                const src = (typeof href === 'object' && href !== null) ? (href.href || '') : (href || '');
                const alt = text || '';
                const ttl = title ? ` title="${title}"` : '';
                return `<img class="sidebar-comment-img" src="${src}" alt="${alt}"${ttl} loading="lazy">`;
            };

            // Line-by-line rendering to avoid paragraph collapsing and recursion
            const renderedLines = escaped.split('\n').map(line => {
                const trimmed = line.trimStart();
                if (trimmed.startsWith('>') && !trimmed.match(/^>>\d+/)) {
                    // Manual greentext handling — apply emoji if the user preference allows it
                    const quoteContent = line.substring(line.indexOf('>') + 1);
                    const quoteEmojis = window.f0ckSession?.quote_emojis === true;
                    const escapedQuote = quoteContent.replace(/>/g, '&gt;');
                    const rendered = quoteEmojis
                        ? escapedQuote.replace(/:([a-z0-9_]+):/g, (m, n) => renderEmoji(m, n))
                        : escapedQuote;
                    return `<span class="greentext">&gt;${rendered}</span>`;
                }

                // Per-line limit to prevent marked.parse recursion on single giant lines
                if (line.length > 10000) return line;

                if (!line.trim()) return '&nbsp;';

                // Perform replacements on the single line
                let processedLine = line;

                // Handle Mentions
                const mentionStore = [];
                processedLine = processedLine.replace(mentionRegex, (match, g1, g2, g3, g4) => {
                    const user = g1 || g2 || g4;
                    const html = `<a href="/user/${encodeURIComponent(user)}" class="mention">@${user}</a>`;
                    const idx = mentionStore.length;
                    mentionStore.push(html);
                    return `\x02MNT${idx}\x03`;
                });

                // Handle Comment Context Links (>>ID)
                processedLine = processedLine.replace(/(?<!\w)>>(\d+)/g, (match, id) => {
                    const targetHref = itemId ? `/${itemId}#c${id}` : `#c${id}`;
                    return `<a href="${targetHref}" class="comment-context-link" data-id="${id}">>>${id}</a>`;
                });

                processedLine = processedLine.replace(imageRegex, (match, url) => {
                    let fullUrl = url;
                    if (!url.startsWith('http') && !url.startsWith('//') && !url.startsWith('/')) {
                        fullUrl = '//' + url;
                    }
                    return `![image](${fullUrl})`;
                });

                processedLine = processedLine.replace(rawVideoRegex, (match, url) => {
                    let fullUrl = url;
                    if (!url.startsWith('http') && !url.startsWith('//') && !url.startsWith('/')) fullUrl = '//' + url;
                    return `[video](${fullUrl})`;
                });

                // Use marked for each line individually.
                // Protect URLs and already-formed Markdown link/image tokens from the
                // italic-prevention pass so that underscores in query params
                // (e.g. ?v=_FcvmypiHg4) are never turned into ?v=\_FcvmypiHg4.
                const mdProtected = [];
                // Match [text](url) / ![alt](url) tokens AND bare http(s) URLs
                let mdSafe = processedLine.replace(
                    /(!?\[[^\]]*\]\([^)]*\))|https?:\/\/\S+/g,
                    (match) => {
                        const idx = mdProtected.length;
                        mdProtected.push(match);
                        return `\x02MDURL${idx}\x03`;
                    }
                );
                // Escape * and _ only in the non-URL portions
                mdSafe = mdSafe
                    .replace(/\\/g, '\\\\')
                    .replace(/\*/g, '\\*')
                    .replace(/_/g, '\\_');
                // Restore protected URLs/tokens
                mdSafe = mdSafe.replace(/\x02MDURL(\d+)\x03/g, (_, i) => mdProtected[+i]);

                let rendered = marked.parseInline ? marked.parseInline(mdSafe, { renderer: renderer }) : marked.parse(mdSafe, { renderer: renderer }).replace(/<p>|<\/p>/g, '');

                // Restore Mentions
                rendered = rendered.replace(/\x02MNT(\d+)\x03/g, (_, i) => mentionStore[+i]);

                // Render emojis ONLY if this is NOT a quote line OR if the user prefers it
                const quoteEmojis = window.f0ckSession?.quote_emojis === true;
                if (!trimmed.startsWith('>') || quoteEmojis) {
                    rendered = rendered.replace(/:([a-z0-9_]+):/g, (m, n) => renderEmoji(m, n));
                }

                return rendered;
            });

            let md = renderedLines.join('\n');

            // YouTube label replacement: show icon + labeled link
            md = md.replace(
                /<a\s[^>]*href="https?:\/\/(?:www\.)?(?:youtube\.com\/watch\?(?:[^"]*&(?:amp;)?)?v=|youtu\.be\/)([a-zA-Z0-9_\-]{11})[^"]*"[^>]*>([\s\S]*?)<\/a>/gi,
                (match, videoId) => {
                    const hrefMatch = match.match(/href="([^"]+)"/i);
                    const ytHref = hrefMatch ? hrefMatch[1] : '#';
                    const targetHref = (itemId && commentId) ? `/${itemId}#c${commentId}` : (commentId ? `#sc${commentId}` : ytHref);
                    const externalAttr = (itemId && commentId) || commentId ? '' : ' target="_blank" rel="noopener noreferrer"';
                    return `<a href="${targetHref}"${externalAttr} class="sidebar-video-link" data-yt-id="${videoId}"><i class="fa-brands fa-youtube"></i> <span class="yt-title"></span></a>`;
                }
            );

            // Vocaroo label replacement
            md = md.replace(
                /<a\s[^>]*href="https?:\/\/(?:www\.)?(?:voca\.ro|vocaroo\.com)\/([a-zA-Z0-9_-]+)[^"]*"[^>]*>([\s\S]*?)<\/a>/gi,
                (match, vocarooId) => {
                    if (['upload', 'contact', 'privacy', 'tos', 'about'].includes(vocarooId.toLowerCase())) return match;
                    const hrefMatch = match.match(/href="([^"]+)"/i);
                    const vocaHref = hrefMatch ? hrefMatch[1] : '#';
                    const targetHref = (itemId && commentId) ? `/${itemId}#c${commentId}` : (commentId ? `#sc${commentId}` : vocaHref);
                    const externalAttr = (itemId && commentId) || commentId ? '' : ' target="_blank" rel="noopener noreferrer"';
                    return `<a href="${targetHref}"${externalAttr} class="sidebar-video-link"><i class="fa-solid fa-microphone"></i> <span>Vocaroo Audio</span></a>`;
                }
            );

            // Abyss label replacement
            md = md.replace(
                /<a\s[^>]*href="(?:https?:\/\/[^\/]+)?\/abyss(?:#|\/)(\d+)"[^>]*>([\s\S]*?)<\/a>/gi,
                (match, abyssId) => {
                    return `<a href="/abyss/${abyssId}" class="sidebar-abyss-link" data-abyss-id="${abyssId}"><i class="fa-solid fa-dice-d6"></i> /abyss/${abyssId}</a>`;
                }
            );

            // Build regex for allowed media hosters (video/audio)
            const escapedSiteHost = window.location.host.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            const mediaHosts = [escapedSiteHost];
            if (window.f0ckAllowedImages && Array.isArray(window.f0ckAllowedImages)) {
                window.f0ckAllowedImages.forEach(h => {
                    const escaped = h.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
                    mediaHosts.push(`(?:[a-z0-9-]+\\.)*${escaped}`);
                });
            }
            const mediaHostsPart = mediaHosts.join('|');
            const mediaDomainOrRelative = `(?:(?:https?:\\/\\/|\\/\\/)?(?:${mediaHostsPart})|(?=\\/[a-zA-Z0-9_\\-]))`;

            // Video label replacement: instead of embedding, show a link
            const videoEmbedRegex = new RegExp(`<a\\s[^>]*href="(${mediaDomainOrRelative}(?:\\/[^\\s\\[\\]\\(\\)]+\\.(?:mp4|webm|ogv|mov)(?:\\?[^\\s\\[\\]\\(\\)]+)?(?:#gif)?))"[^>]*>([\\s\\S]*?)<\\/a>`, 'gi');
            md = md.replace(videoEmbedRegex, (match, url) => {
                const isConvertedGif = url.endsWith('#gif');
                const cleanUrl = url.replace(/#gif$/, '');
                // Converted GIFs → inline autoplay in sidebar too
                if (isConvertedGif) {
                    return `<span class="video-embed-wrap"><video src="${cleanUrl}" class="sidebar-comment-img autoplay-gif" loop muted playsinline preload="auto"></video></span>`;
                }
                let isSameSite = false;
                try {
                    const urlToParse = cleanUrl.startsWith('//') ? window.location.protocol + cleanUrl : cleanUrl;
                    const urlObj = new URL(urlToParse, siteOrigin);
                    isSameSite = (urlObj.hostname === window.location.hostname);
                } catch (e) {
                    isSameSite = cleanUrl.startsWith(siteOrigin) || (cleanUrl.startsWith('/') && !cleanUrl.startsWith('//'));
                }
                const label = isSameSite ? 'Video Link' : 'External Video Link';
                const targetHref = (itemId && commentId) ? `/${itemId}#c${commentId}` : (commentId ? `#sc${commentId}` : cleanUrl);
                const externalAttr = (itemId && commentId) || commentId ? '' : ' target="_blank" rel="noopener noreferrer"';
                return `<a href="${targetHref}"${externalAttr} class="sidebar-video-link"><i class="fa-solid fa-film"></i> ${label} &raquo;</a>`;
            });

            // Handle spoilers [spoiler]text[/spoiler] (supports nesting)
            let prevMd;
            let iterations = 0;
            const spoilerRegex = /\[spoiler\]((?:(?!\[spoiler\])[\s\S])*?)\[\/spoiler\]/gi;
            do {
                prevMd = md;
                md = md.replace(spoilerRegex, (match, content) => {
                    return `<span class="spoiler">${content}</span>`;
                });
                iterations++;
            } while (md !== prevMd && iterations < 10);

            // Handle blur [blur]text[/blur] (supports nesting)
            const blurRegex = /\[blur\]((?:(?!\[blur\])[\s\S])*?)\[\/blur\]/gi;
            iterations = 0;
            do {
                prevMd = md;
                md = md.replace(blurRegex, (match, content) => {
                    return `<span class="blur-text">${content}</span>`;
                });
                iterations++;
            } while (md !== prevMd && iterations < 10);

            // [rb] [reich] [krebs] [glitch] (shared with every frontend: f0ck-core.js)
            if (window.f0ckCommentFx) md = window.f0ckCommentFx(md);

            // Restore protected code blocks
            md = md.replace(/BLOCKPORTALX(\d+)X/g, (match, index) => {
                return codeBlocks[index] || '';
            });

            if (window.Sanitizer && typeof window.Sanitizer.clean === 'function') {
                md = window.Sanitizer.clean(md);
            }

            // Strip #gif from final text if it leaked through
            return md.replace(/#gif/g, '');
        } catch (e) {
            return content;
        }
    };

    const SIDEBAR_MAX_CHARS = 200;
    const SIDEBAR_MAX_EMOJIS = 12;

    const renderCommentAttachments = (files, content = '') => {
        if (!files || files.length === 0) return '';
        const items = files.map(f => {
            const url = `/c/${f.dest}`;
            if (content.includes(url)) return ''; // Skip if already rendered in content
            if (f.mime.startsWith('image/')) {
                return `<a href="${url}" target="_blank" class="cf-attachment cf-image"><img src="${url}" class="sidebar-comment-img" alt="${escapeHtml(f.original_filename || 'image')}" loading="lazy"></a>`;
            } else if (f.mime.startsWith('video/')) {
                return `<div class="cf-attachment cf-video"><video src="${url}" class="sidebar-comment-img" controls preload="metadata"></video></div>`;
            } else if (f.mime.startsWith('audio/')) {
                return `<div class="cf-attachment cf-audio"><audio src="${url}" controls preload="metadata"></audio></div>`;
            }
            return '';
        }).join('');
        return items ? `<div class="comment-attachments">${items}</div>` : '';
    };

    const renderSidebarPoll = (poll, commentId, itemId) => {
        if (!poll) return '';
        const href = (itemId && commentId) ? `/${itemId}#c${commentId}` : (commentId ? `#sc${commentId}` : '#');
        return `<a class="sidebar-poll-preview" href="${href}"><i class="fa-solid fa-chart-bar"></i> ${escapeHtml(poll.question)}</a>`;
    };

    const renderActivityItem = (c) => {
        const itemKey = c.item_slug || c.slug || c.item_id;
        const rawContent = c.content || c.body || '';
        let displayContent = renderCommentContent(rawContent, c.id, itemKey);
        
        displayContent = window.f0cklib?.processMentions ? window.f0cklib.processMentions(displayContent) : displayContent;
        
        // Robust fallback for is_long if server didn't provide it, plus inline media checks
        let isLong = c.is_long;
        if (isLong === undefined) {
            isLong = rawContent.length > 120 
                || rawContent.split('\n').length > 2 
                || (c.files && c.files.length > 0);
        }
        // Always force isLong if there are inline media tags that expand vertically or files are attached
        if (!isLong && ((c.files && c.files.length > 0) || /\[(video|audio|youtube|img)\]|!\[|https?:\/\//i.test(rawContent) || displayContent.includes('<video') || displayContent.includes('<img'))) {
            isLong = true;
        }

        const attachmentsHtml = renderCommentAttachments(c.files, rawContent);
        const pollHtml = renderSidebarPoll(c.poll, c.id, itemKey);

        // Build avatar URL — same priority as the rest of the app
        let avatarSrc = '/a/default.png';
        if (c.avatar_file) {
            avatarSrc = `/a/${c.avatar_file}`;
        } else if (c.avatar) {
            avatarSrc = `/t/${c.avatar}.webp`;
            if (window.applyThumbCacheBust) avatarSrc = window.applyThumbCacheBust(avatarSrc);
        }

        const timeStr = c.created_at
            ? (window.f0ckTimeAgo ? window.f0ckTimeAgo(c.created_at) : (c.timeago || c.created_at))
            : (c.timeago || 'just now');
        const tsAttr = c.created_at ? ` data-ts="${escapeHtml(c.created_at)}" data-iso="${escapeHtml(c.created_at)}"` : '';
        const fullDate = c.created_at
            ? (window.f0ckFormatDateFull ? window.f0ckFormatDateFull(c.created_at) : new Date(c.created_at).toISOString())
            : '';

        let itemPreview = '';
        if (c.item_id) {
            let mediaHtml = '';
            const rClass = c.item_rating_class || 'untagged';
            const blurNsfw = localStorage.getItem('blurNsfw') === 'true';
            const blurNsfl = localStorage.getItem('blurNsfl') === 'true';
            const blurSfw = localStorage.getItem('blurSfw') === 'true';
            const blurUntagged = localStorage.getItem('blurUntagged') === 'true';

            let isBlurred = false;
            if (rClass === 'nsfw' && blurNsfw) isBlurred = true;
            else if (rClass === 'nsfl' && blurNsfl) isBlurred = true;
            else if (rClass === 'sfw' && blurSfw) isBlurred = true;
            else if (rClass === 'untagged' && blurUntagged) isBlurred = true;

            const mime = c.mime || c.item_mime || '';
            const activeDest = c.item_dest || c.dest || '';
            const isAudio = mime.startsWith('audio/') || (!mime && activeDest.match(/\.(mp3|wav|ogg|flac|m4a|aac|opus)$/i));

            let thumbUrl = c.thumb || `/t/${c.item_id}.webp`;
            if (isBlurred) {
                const baseThumb = thumbUrl.replace(/\.webp$/, '');
                thumbUrl = `${baseThumb}_blur.webp`;
            }

            if (window.applyThumbCacheBust) thumbUrl = window.applyThumbCacheBust(thumbUrl);

            if (isAudio && !c.has_coverart) {
                mediaHtml = `
                    <div class="sidebar-media-placeholder audio">
                        <i class="fa-solid fa-music"></i>
                    </div>
                `;
            } else {
                mediaHtml = `
                    <img src="${thumbUrl}" style="width: 32px; height: 32px; object-fit: cover; border-radius: 2px;" loading="lazy" data-on="error:sidebar-thumb-fail" />
                    <div class="sidebar-media-placeholder ${isAudio ? 'audio' : ''} hidden">
                        <i class="${isAudio ? 'fa-solid fa-music' : 'fa-solid fa-image'}"></i>
                    </div>
                `;
            }

            itemPreview = `
                <div class="item-preview">
                    <a href="/${itemKey}" class="sidebar-thumb-link" data-mode="${rClass}">${mediaHtml}</a>
                    <a href="/${itemKey}#c${c.id}" style="font-size: 0.8em; color: var(--accent); text-decoration: none;">${(window.f0ckI18n && window.f0ckI18n.sidebar_view) || 'View'} &raquo;</a>
                </div>`;
        }


        const bannerEnabled = window.f0ckCommentBannerEnabled !== false && window.f0ckSession?.comment_banner_enabled !== false;
        let bannerFile = c.banner_file;
        let bannerPos = c.banner_position;
        let bannerSz = c.banner_size;
        if (!bannerFile && window.f0ckSession && window.f0ckSession.id && (c.user_id === window.f0ckSession.id || c.username === window.f0ckSession.user)) {
            bannerFile = window.f0ckSession.banner_file;
            bannerPos = window.f0ckSession.banner_position;
            bannerSz = window.f0ckSession.banner_size;
        }
        const bannerStyle = (bannerEnabled && bannerFile && bannerFile !== 'null')
            ? `style="--author-banner: url('/a/${bannerFile}'); --author-banner-position: ${bannerPos === 'center' ? 'center top' : (bannerPos || 'center top')}; --author-banner-size: ${(bannerSz && bannerSz !== 'cover') ? bannerSz : '100% auto'}; --author-banner-repeat: no-repeat;"`
            : '';

        const isAnonGuest = window.f0ckSession?.is_anonymized ?? (window.f0ckSession?.guest_anonymize && !window.f0ckSession?.logged_in);
        const effectiveBannerStyle = isAnonGuest ? '' : bannerStyle;
        const authorAvatarHtml = isAnonGuest
            ? `<span class="sidebar-avatar-link"><img src="/a/default.png" class="sidebar-avatar" loading="eager" /></span>`
            : `<a href="/user/${c.username.toLowerCase()}" class="sidebar-avatar-link"><img src="${avatarSrc}" class="sidebar-avatar" loading="eager" data-on="load:sidebar-img-loaded error:sidebar-img-loaded,img-fallback" data-fallback="/a/default.png" /></a>`;
        const authorNameHtml = isAnonGuest
            ? `<span class="comment-author name-short">anonymous</span>`
            : `<a href="/user/${c.username.toLowerCase()}" class="comment-author${isShortName(c.display_name || c.username) ? ' name-short' : ''}" ${c.username_color ? `style="color: ${c.username_color}"` : ''}>${escapeHtml(c.display_name || c.username)}</a>`;

        return `
            <div class="comment" id="sc${c.id}" ${effectiveBannerStyle}>
                <div class="comment-body">
                    <div class="comment-header">
                        <div class="comment-header-left">
                            ${authorAvatarHtml}
                            ${authorNameHtml}
                        </div>
                        <span class="comment-time timeago" tooltip="${fullDate}" style="font-size: 0.75em;"${tsAttr}>${timeStr}</span>
                    </div>
                    <div class="comment-content${isLong ? ' has-overflow' : ''}"><div class="comment-content-inner">${displayContent}${attachmentsHtml}${pollHtml}</div><button class="read-more-btn"${isLong ? ' style="display:block"' : ''}>${window.f0ckI18n?.sidebar_read_more || 'read more'}</button></div>
                    ${itemPreview}
                </div>
            </div>`;
    };


    const checkOverflow = (targetElement) => {
        const targets = targetElement
            ? (targetElement.classList.contains('comment-content-inner') ? [targetElement] : targetElement.querySelectorAll('.comment-content-inner'))
            : document.querySelectorAll('.sidebar-activity .comment-content-inner');

        const results = [];
        // Read phase: batch layout reads first
        targets.forEach(inner => {
            const container = inner.parentElement;
            const btn = container ? container.querySelector('.read-more-btn') : null;
            if (!btn) return;

            const isExpanded = container.classList.contains('expanded');
            const scrollHeight = inner.scrollHeight;
            const clientHeight = inner.clientHeight;
            const hasUnloadedImages = Array.from(inner.querySelectorAll('img, video')).some(
                media => {
                    if (media.tagName === 'IMG') {
                        return (!media.complete || media.naturalHeight === 0) && media.dataset.error !== 'true';
                    }
                    if (media.tagName === 'VIDEO') {
                        return media.readyState < 1 && media.dataset.error !== 'true'; // HAVE_METADATA
                    }
                    return false;
                }
            );

            results.push({
                container,
                btn,
                isExpanded,
                scrollHeight,
                clientHeight,
                hasUnloadedImages
            });
        });

        // Write phase: perform DOM updates after all reads are completed.
        // Visibility of the button is controlled entirely by the CSS rule on
        // .comment-content.has-overflow > .read-more-btn — no btn.style.display here.
        results.forEach(({ container, btn, isExpanded, scrollHeight, clientHeight, hasUnloadedImages }) => {
            // Clear any leftover inline display style (e.g. from the HTML template's
            // style="display:block") so the CSS class-based rule is the single source of truth.
            btn.style.display = '';

            if (isExpanded) {
                btn.textContent = window.f0ckI18n?.sidebar_see_less || 'see less';
                return;
            }

            if (scrollHeight > clientHeight + 2) { // 2px buffer for rounding
                // Content overflows — ensure clamped state (may already be set from HTML default)
                btn.textContent = window.f0ckI18n?.sidebar_read_more || 'read more';
                container.classList.add('has-overflow');
            } else if (hasUnloadedImages) {
                // Images haven't loaded yet — their height is 0 so we can't tell if content
                // will overflow. Keep has-overflow set; attachMediaLoadListeners will
                // re-run checkOverflow once images finish loading.
            } else {
                // Content fits and all images are loaded — safe to remove clamped state
                container.classList.remove('has-overflow');
            }
        });
    };

    const attachMediaLoadListeners = (element) => {
        // Only target images and videos inside comment-content-inner.
        // Avatars and item preview thumbnails have fixed sizes and do not affect text overflow.
        element.querySelectorAll('.comment-content-inner img, .comment-content-inner video').forEach(media => {
            if (media.dataset.loadListenerBound) return;
            media.dataset.loadListenerBound = 'true';

            const inner = media.closest('.comment-content-inner');
            if (inner) {
                const handler = () => checkOverflow(inner);
                const errorHandler = () => {
                    media.dataset.error = 'true';
                    checkOverflow(inner);
                };
                media.addEventListener('load', handler, { once: true });
                media.addEventListener('error', errorHandler, { once: true });
                media.addEventListener('loadedmetadata', handler, { once: true });
                
                // Also use ResizeObserver to catch layout changes (e.g. cached image layout deferred)
                if (window.ResizeObserver) {
                    const ro = new ResizeObserver(() => checkOverflow(inner));
                    ro.observe(media);
                }
            }
        });
    };

    // Event delegation — read-more expands, see-less collapses
    document.addEventListener('click', (e) => {
        // Read more / See less
        const readBtn = e.target.closest('.read-more-btn');
        if (readBtn) {
            const contentDiv = readBtn.closest('.comment-content');
            if (contentDiv) {
                contentDiv.classList.toggle('expanded');
                checkOverflow(contentDiv); // Re-sync button text and visibility for this element
            }
            return;
        }
    });

    const SIDEBAR_SKELETON_COUNT = 15;

    const showSkeletons = () => {
        const container = document.getElementById('sidebar-activity-container');
        if (!container) return;
        container.innerHTML = `
            <div class="sidebar-loading-state">
                <i class="fa-solid fa-circle-notch fa-spin"></i>
                <span>${window.f0ckI18n?.sidebar_loading_activity || 'Loading activity...'}</span>
            </div>
        `;
    };

    const renderFromCache = (force = false) => {
        const container = document.getElementById('sidebar-activity-container');
        if (!container || window._sidebarActivityCache.length === 0) return false;

        const currentIds = window._sidebarActivityCache.map(c => String(c.id)).join(',');
        // If the same comments are already rendered, leave the DOM completely untouched.
        // This keeps video stickers playing and avoids any overflow state churn.
        // force=true bypasses this guard for cases where content changed without ID changes
        // (e.g. emojis just loaded and need to replace raw :codes: in existing comments).
        if (!force && currentIds === lastRenderedIds && container.querySelector('.comment')) {
            return true;
        }
        lastRenderedIds = currentIds;

        let html = '';
        window._sidebarActivityCache.forEach(c => {
            html += renderActivityItem(c);
        });

        container.innerHTML = html;
        // Re-append IO sentinel so the scroll observer keeps working after re-renders
        if (ioSentinel) {
            container.appendChild(ioSentinel);
        }
        attachMediaLoadListeners(container);
        // has-overflow is now set per-comment by the server (via is_long) so the
        // correct clamped/unclamped state is baked into the HTML on first paint.
        // checkOverflow still runs deferred to handle two edge cases:
        //   1. Comments with images whose heights are 0 at inject time — the server
        //      correctly marks them is_long:true, but images that load extra-tall
        //      still need a re-check after load (handled by attachMediaLoadListeners).
        //   2. Any mis-estimates: content that the heuristic got wrong is corrected
        //      after layout so measurements are accurate.
        // Run checkOverflow synchronously immediately after setting innerHTML.
        // This forces a synchronous layout (reflow) but guarantees the browser
        // will not paint the intermediate state, completely eliminating any "flash"
        // of the button popping in or out.
        checkOverflow();

        // checkOverflow still runs deferred to handle image loads (attachMediaLoadListeners)
        // and a fallback 1s timeout for any incredibly slow layout edge cases.
        requestAnimationFrame(() => {
            setTimeout(checkOverflow, 1000);
        });
        fetchSidebarYoutubeTitles(container);
        // Auto-play converted GIF videos and webm emoji stickers
        container.querySelectorAll('video.autoplay-gif').forEach(v => { v.autoplay = true; v.muted = true; v.play().catch(() => { v.addEventListener('canplay', () => v.play().catch(() => { }), { once: true }); }); });
        playSidebarEmojiVideos(container);
        return true;
    };

    const SIDEBAR_PAGE_LIMIT = 15;
    const SIDEBAR_INITIAL_LIMIT = 15;
    const SIDEBAR_MAX_COMMENTS = 20;

    const loadActivity = async (silent = false) => {
        const container = document.getElementById('sidebar-activity-container');
        if (!container || loading) return;

        const hasCache = renderFromCache();
        // If no cache and not silent: show skeletons while we fetch.
        // On the very first page load the server-rendered skeletons are already there;
        // on subsequent loads (e.g. mode change) we inject them programmatically.
        if (!hasCache && !silent) {
            showSkeletons();
        }


        loading = true;
        currentPage = 1;
        hasMore = true;
        try {
            const mode = typeof window.activeMode !== 'undefined' ? window.activeMode : '';
            const res = await fetch(`/activity?json=true&page=1&limit=${SIDEBAR_INITIAL_LIMIT}&mode=${mode}`, {
                headers: { 'X-Requested-With': 'XMLHttpRequest' }
            });
            const data = await res.json();

            if (data.success && data.comments && data.comments.length > 0) {
                window._sidebarActivityCache = data.comments.slice(0, SIDEBAR_MAX_COMMENTS).map(c => ({
                    ...c,
                    body: c.content || c.body
                }));
                hasMore = (data.hasMore === true || data.comments.length === SIDEBAR_INITIAL_LIMIT)
                    && window._sidebarActivityCache.length < SIDEBAR_MAX_COMMENTS;
                renderFromCache(); // no-op if IDs unchanged (renderFromCache self-guards)
            } else if (!hasCache) {
                container.innerHTML = '<div style="text-align:center;padding:20px;color:#888;">' + (window.f0ckI18n?.sidebar_no_activity || 'No recent activity.') + '</div>';
                hasMore = false;
            }
        } catch (e) {
            console.error("Sidebar Activity: Failed to load activity", e);
            if (!hasCache) {
                container.innerHTML = '<div style="text-align:center;padding:20px;color:#888;">' + (window.f0ckI18n?.sidebar_failed_to_load || 'Failed to load.') + '</div>';
            }
            hasMore = false;
        } finally {
            loading = false;
        }
    };


    const loadMoreActivity = async () => {
        const container = document.getElementById('sidebar-activity-container');
        if (!container || loading || loadingMore || !hasMore) return;

        loadingMore = true;
        // Use the current cache length as the exact offset so there's never a gap,
        // regardless of the initial fetch limit.
        const nextOffset = window._sidebarActivityCache.length;

        // Show a subtle loading row at the bottom
        const sentinel = document.createElement('div');
        sentinel.id = 'sidebar-load-more-sentinel';
        sentinel.style.cssText = 'text-align:center;padding:8px 0;font-size:0.78em;color:#666;';
        sentinel.textContent = window.f0ckI18n?.sidebar_loading_more || 'Loading…';
        container.appendChild(sentinel);

        try {
            const mode = typeof window.activeMode !== 'undefined' ? window.activeMode : '';
            const res = await fetch(`/activity?json=true&offset=${nextOffset}&limit=${SIDEBAR_PAGE_LIMIT}&mode=${mode}`, {
                headers: { 'X-Requested-With': 'XMLHttpRequest' }
            });
            const data = await res.json();

            // Remove sentinel before inserting real content
            const s = document.getElementById('sidebar-load-more-sentinel');
            if (s) s.remove();

            if (data.success && data.comments && data.comments.length > 0) {
                currentPage++;

                // Append only comments not already in the cache, up to the global cap
                const remaining = SIDEBAR_MAX_COMMENTS - window._sidebarActivityCache.length;
                const existingIds = new Set(window._sidebarActivityCache.map(c => String(c.id)));
                const newComments = data.comments
                    .filter(c => !existingIds.has(String(c.id)))
                    .slice(0, remaining)
                    .map(c => ({ ...c, body: c.content || c.body }));

                window._sidebarActivityCache.push(...newComments);

                // Stop loading more if the hard cap is reached
                hasMore = data.hasMore === true
                    && window._sidebarActivityCache.length < SIDEBAR_MAX_COMMENTS;

                // Append new items to DOM
                let html = '';
                newComments.forEach(c => { html += renderActivityItem(c); });
                if (html) {
                    const temp = document.createElement('div');
                    temp.innerHTML = html;
                    const addedNodes = [];
                    while (temp.firstElementChild) {
                        const node = temp.firstElementChild;
                        container.appendChild(node);
                        addedNodes.push(node);
                    }
                    // Keep the IO sentinel at the very end so it triggers on the next scroll
                    if (ioSentinel) container.appendChild(ioSentinel);

                    addedNodes.forEach(node => {
                        attachMediaLoadListeners(node);
                        fetchSidebarYoutubeTitles(node);
                        playSidebarEmojiVideos(node);
                    });
                    // Defer overflow checks to after layout so measurements are accurate.
                    addedNodes.forEach(node => checkOverflow(node));
                    requestAnimationFrame(() => {
                        // Deferred re-check for image comments — mirrors renderFromCache behaviour.
                        // Images in newly appended nodes may not be laid out yet at append time.
                        setTimeout(() => addedNodes.forEach(node => checkOverflow(node)), 1000);
                    });
                    // Auto-play converted GIF videos and webm emoji stickers
                    container.querySelectorAll('video.autoplay-gif').forEach(v => { v.autoplay = true; v.muted = true; v.play().catch(() => { v.addEventListener('canplay', () => v.play().catch(() => { }), { once: true }); }); });
                }

                if (!hasMore) {
                    // Show end-of-feed indicator
                    const end = document.createElement('div');
                    end.style.cssText = 'text-align:center;padding:8px 0;font-size:0.75em;color:#444;';
                    end.textContent = window.f0ckI18n?.sidebar_end_of_activity || '─ end of activity ─';
                    container.appendChild(end);
                }
            } else {
                hasMore = false;
                // Show end-of-feed indicator
                const end = document.createElement('div');
                end.style.cssText = 'text-align:center;padding:8px 0;font-size:0.75em;color:#444;';
                end.textContent = window.f0ckI18n?.sidebar_end_of_activity || '─ end of activity ─';
                container.appendChild(end);
            }
        } catch (e) {
            console.error("Sidebar Activity: Failed to load more", e);
            const s = document.getElementById('sidebar-load-more-sentinel');
            if (s) s.remove();
        } finally {
            loadingMore = false;
        }
    };

    const handleNewActivity = (data) => {
        const container = document.getElementById('sidebar-activity-container');

        // 1. Deduplicate: check if this comment ID is already in the cache
        if (window._sidebarActivityCache.some(c => parseInt(c.id) === parseInt(data.id))) {
            window.f0ckDebug("Sidebar Activity: Duplicate comment ignored", data.id);
            return;
        }

        // 2. Update cache (prepend, capped at SIDEBAR_MAX_COMMENTS)
        const newItem = {
            ...data,
            body: data.body || data.content,
            timeago: (window.f0ckI18n && window.f0ckI18n.timeago_just_now) || 'just now'
        };
        window._sidebarActivityCache.unshift(newItem);

        // Trim cache to the hard cap and remove the evicted DOM node (if any)
        if (window._sidebarActivityCache.length > SIDEBAR_MAX_COMMENTS) {
            const evicted = window._sidebarActivityCache.pop();
            if (container && evicted) {
                const evictedEl = document.getElementById('sc' + evicted.id);
                if (evictedEl) evictedEl.remove();
            }
        }

        // Update DOM if visible
        if (container) {
            // is_long may be provided by the server (SSE payload) or absent for legacy events.
            // Default to true so the button is always visible; checkOverflow will hide it
            // if the rendered content is actually short.
            const itemWithLong = { ...newItem, is_long: newItem.is_long !== false };
            const html = renderActivityItem(itemWithLong);
            const temp = document.createElement('div');
            temp.innerHTML = html;
            const node = temp.firstElementChild;
            if (node) {
                node.classList.add('new-item-fade');
                container.prepend(node);
                attachMediaLoadListeners(node);
                checkOverflow(node);
                fetchSidebarYoutubeTitles(container);
                playSidebarEmojiVideos(node);
            }
        }
    };

    const init = async () => {
        // Run emoji loading and activity fetching in parallel — avatars appear
        // immediately without waiting for the emoji API to respond first.
        // After both settle, if emojis finished after the initial render, re-render
        // from cache so custom emoji images show on first page load.
        const emojiPromise = loadEmojis();
        const activityPromise = loadActivity();
        await activityPromise;
        await emojiPromise;
        // If emojis were not yet available when activity first rendered, re-render now.
        if (Object.keys(customEmojis).length > 0 && window._sidebarActivityCache.length > 0) {
            renderFromCache(true); // force: emojis are now loaded, content changed
        }
    };

    // Listen for live activity from f0ckm.js
    document.addEventListener('f0ck:activityReceived', (e) => {
        window.f0ckDebug("Sidebar Activity: Live update received", e.detail);
        handleNewActivity(e.detail);
    });

    const handleLiveEdit = (data) => {
        const container = document.getElementById('sidebar-activity-container');

        // 1. Update cache
        if (window._sidebarActivityCache) {
            const comment = window._sidebarActivityCache.find(c => String(c.id) === String(data.comment_id));
            if (comment) {
                comment.content = data.content;
                comment.body = data.content;
            }
        }

        // 2. Update DOM if visible
        if (container) {
            const el = document.getElementById('sc' + data.comment_id);
            if (el) {
                const inner = el.querySelector('.comment-content-inner');
                if (inner) {
                    const comment = window._sidebarActivityCache.find(c => String(c.id) === String(data.comment_id));
                    inner.innerHTML = renderCommentContent(data.content, data.comment_id, comment ? comment.item_id : null);
                    el.classList.remove('new-item-fade');
                    void el.offsetWidth;
                    el.classList.add('new-item-fade');
                    attachMediaLoadListeners(inner);
                    requestAnimationFrame(() => checkOverflow(inner));
                    fetchSidebarYoutubeTitles(el);
                    // Auto-play converted GIF videos
                    inner.querySelectorAll('video.autoplay-gif').forEach(v => { v.autoplay = true; v.muted = true; v.play().catch(() => { v.addEventListener('canplay', () => v.play().catch(() => { }), { once: true }); }); });
                }
            }
        }
    };

    window.addEventListener('f0ck:comment_edited', (e) => {
        window.f0ckDebug("Sidebar Activity: Live edit received", e.detail);
        handleLiveEdit(e.detail);
    });

    // When emojis are refreshed (e.g. after sticker pack import), sync the sidebar
    // cache and re-render so newly imported stickers appear in existing entries.
    window.addEventListener('f0ck:emojis_ready', () => {
        const csCache = (typeof CommentSystem !== 'undefined' && CommentSystem.emojiCache) || null;
        if (!csCache || Object.keys(csCache).length === 0) return;
        // Merge new emojis into local cache (additive — never removes)
        const hadBefore = Object.keys(customEmojis).length;
        Object.assign(customEmojis, csCache);
        const hasNew = Object.keys(customEmojis).length > hadBefore;
        if (hasNew && window._sidebarActivityCache.length > 0) {
            renderFromCache(true); // force: new emoji images available, re-render
        }
    });

    let lastBoundMode = typeof window.activeMode !== 'undefined' ? window.activeMode : null;

    const getCurrentMimeFilter = () => {
        const allowed = window.f0ckSession?.is_anon && Array.isArray(window.f0ckSession?.anon_permissions?.allowed_mimes) ? window.f0ckSession.anon_permissions.allowed_mimes : null;
        const urlParams = new URLSearchParams(window.location.search);
        let qMime = urlParams.get('mime');
        if (qMime !== null) {
            qMime = qMime.trim();
            if (allowed) {
                const parts = qMime.split(',').filter(m => allowed.includes(m));
                return parts.length > 0 ? parts.join(',') : (allowed.length < 5 ? allowed.join(',') : '');
            }
            return qMime;
        }
        const cookieMime = document.cookie.split('; ').find(row => row.startsWith('mime='));
        if (cookieMime) {
            const val = cookieMime.split('=')[1];
            if (typeof val === 'string') {
                const cMime = decodeURIComponent(val).trim();
                if (allowed) {
                    const parts = cMime.split(',').filter(m => allowed.includes(m));
                    return parts.length > 0 ? parts.join(',') : (allowed.length < 5 ? allowed.join(',') : '');
                }
                return cMime;
            }
        }
        if (allowed && allowed.length < 5) {
            return allowed.join(',');
        }
        return '';
    };

    let lastBoundMime = getCurrentMimeFilter();

    const isStrictMode = () => {
        return !!(window.f0ckSession?.strict_mode || (localStorage.getItem('search_strict') === 'true') || window.location.search.includes('strict=1'));
    };

    let lastBoundStrict = isStrictMode();

    // Handle AJAX item loads
    document.addEventListener('f0ck:contentLoaded', () => {
        const currentMode = typeof window.activeMode !== 'undefined' ? window.activeMode : null;
        const modeChanged = lastBoundMode !== null && lastBoundMode !== currentMode;
        lastBoundMode = currentMode;

        const currentMime = getCurrentMimeFilter();
        const mimeChanged = lastBoundMime !== null && lastBoundMime !== currentMime;
        lastBoundMime = currentMime;

        const currentStrict = isStrictMode();
        const strictChanged = lastBoundStrict !== null && lastBoundStrict !== currentStrict;
        lastBoundStrict = currentStrict;

        window.f0ckDebug("Sidebar Activity: Page transition detected", modeChanged ? "(Mode changed)" : "", mimeChanged ? "(Mime changed)" : "", strictChanged ? "(Strict changed)" : "");

        if (modeChanged || mimeChanged || strictChanged) {
            if (modeChanged) {
                window._sidebarActivityCache = [];
                lastRenderedIds = '';
                currentPage = 1;
                hasMore = true;
                loadActivity(false); // Force reload with loading state
            }

            recommendationsLoaded = false;
            tagFeedLoaded = false;
            tagFeedNeedsReload = true;
            const activeTabEl = document.querySelector('.sidebar-tab.active')?.dataset.tab;
            let savedTab = 'comments';
            try { savedTab = localStorage.getItem('sidebar_active_tab'); } catch (_) {}
            const currentTabName = activeTabEl || savedTab;

            if (currentTabName === 'videos' || currentTabName === 'recommendations') {
                loadRecommendations(false);
            } else if (currentTabName === 'tag' && currentTag) {
                loadTagFeed(true);
            }
        } else {
            // Immediately render from cache to avoid flicker
            renderFromCache();
            // Background sync
            loadActivity(true);
        }
    });

    document.addEventListener('f0ck:mimeChanged', (e) => {
        const newMime = (e.detail && typeof e.detail.mime !== 'undefined') ? e.detail.mime : getCurrentMimeFilter();
        window.f0ckDebug("Sidebar Activity: MIME filter changed", newMime);
        lastBoundMime = newMime;

        recommendationsLoaded = false;
        tagFeedLoaded = false;
        tagFeedNeedsReload = true;

        const activeTabEl = document.querySelector('.sidebar-tab.active')?.dataset.tab;
        let savedTab = 'comments';
        try { savedTab = localStorage.getItem('sidebar_active_tab'); } catch (_) {}
        const currentTabName = activeTabEl || savedTab;

        if (currentTabName === 'videos' || currentTabName === 'recommendations') {
            loadRecommendations(false);
        } else if (currentTabName === 'tag' && currentTag) {
            loadTagFeed(true);
        }
    });

    // Sync sidebar and comments-list layout on initial page load (Legacy View Only)
    if (typeof syncSidebarAndComments === 'function') {
        syncSidebarAndComments();
    }

    // Video / Media Recommendations Logic
    let recommendationsLoading = false;
    let recommendationsLoadingMore = false;
    let recommendationsLoaded = false;
    let currentRecommendations = [];
    const seenRecommendationIds = new Set();
    const RECOMMENDATIONS_LIMIT = SIDEBAR_MAX_COMMENTS;
    let recIoSentinel = null;
    let recObserver = null;

    const renderVideoCard = (video, options = {}) => {
        const videoKey = (window.f0ckSession?.enable_item_slugs !== false && video.slug) ? video.slug : video.id;
        const rClass = video.rating_class || (video.tag_id == 1 ? 'sfw' : (video.tag_id == 2 ? 'nsfw' : ((video.tag_id == 3 || video.tag_id == window.f0ckSession?.nsfl_tag_id) ? 'nsfl' : 'untagged')));
        const blurNsfw = localStorage.getItem('blurNsfw') === 'true';
        const blurNsfl = localStorage.getItem('blurNsfl') === 'true';
        const blurSfw = localStorage.getItem('blurSfw') === 'true';
        const blurUntagged = localStorage.getItem('blurUntagged') === 'true';

        let isBlurred = false;
        if (rClass === 'nsfw' && blurNsfw) isBlurred = true;
        else if (rClass === 'nsfl' && blurNsfl) isBlurred = true;
        else if (rClass === 'sfw' && blurSfw) isBlurred = true;
        else if (rClass === 'untagged' && blurUntagged) isBlurred = true;

        const mime = video.mime || '';
        const isAudio = mime.startsWith('audio/');
        const isVideo = mime.startsWith('video/');
        const isImage = mime.startsWith('image/');

        let thumbUrl = video.thumb || video.matching_sub_thumb || `/t/${video.id}.webp`;
        if (isBlurred) {
            const baseThumb = thumbUrl.replace(/\.webp$/, '');
            thumbUrl = `${baseThumb}_blur.webp`;
        }
        if (window.applyThumbCacheBust) thumbUrl = window.applyThumbCacheBust(thumbUrl);

        let displayTitle = '';
        if (video.title && video.title.trim()) {
            displayTitle = video.title.trim();
        } else if (video.tags) {
            const rawTags = Array.isArray(video.tags)
                ? video.tags
                : (typeof video.tags === 'string' ? video.tags.split(',').map(s => s.trim()).filter(Boolean) : []);
            const formattedTags = rawTags
                .map(t => (typeof t === 'object' && t !== null ? (t.tag || t.name || '') : String(t)).trim())
                .filter(Boolean)
                .map(t => (t.startsWith('#') ? t : '#' + t));
            if (formattedTags.length > 0) {
                displayTitle = formattedTags.join(' ');
            }
        }
        if (!displayTitle) {
            displayTitle = `${videoKey}`;
        }

        const isAnonGuest = window.f0ckSession?.is_anonymized ?? (window.f0ckSession?.guest_anonymize && !window.f0ckSession?.logged_in);
        const authorName = isAnonGuest ? 'anonymous' : (video.display_name || video.username);
        const userColorStyle = (!isAnonGuest && video.username_color) ? `style="color: ${escapeHtml(video.username_color)}"` : '';

        const timeStr = video.stamp 
            ? (window.f0ckTimeAgo ? window.f0ckTimeAgo(new Date(video.stamp * 1000).toISOString()) : '')
            : '';
        const fullDate = video.stamp ? new Date(video.stamp * 1000).toLocaleString() : '';


        const activeMime = video.matching_sub_mime || video.mime || '';
        const activeDest = video.matching_sub_dest || video.dest || '';
        const ext = (activeMime ? activeMime.split('/')[1] : '')
            .replace('jpeg', 'jpg')
            .replace('x-shockwave-flash', 'flash')
            .replace('x-flac', 'flac')
            .replace('mpeg', 'mp3')
            .toUpperCase();
        const formatBadge = ext ? `<span class="sidebar-media-format-badge">${escapeHtml(ext)}</span>` : '';
        const overlayIcon = isAudio ? 'fa-solid fa-music' : (isVideo ? 'fa-solid fa-play' : 'fa-solid fa-image');

        let thumbContentHtml = '';
        if (isAudio && !video.has_coverart) {
            thumbContentHtml = `
                <div class="sidebar-media-placeholder audio">
                    <i class="fa-solid fa-music"></i>
                </div>
            `;
        } else {
            thumbContentHtml = `
                <img src="${thumbUrl}" class="sidebar-video-thumb" alt="${escapeHtml(displayTitle)}" loading="lazy" draggable="false" data-on="error:sidebar-thumb-fail" />
                <div class="sidebar-media-placeholder ${isAudio ? 'audio' : ''} hidden">
                    <i class="${overlayIcon}"></i>
                </div>
            `;
        }

        const forYouBadge = video.personalized 
            ? `<span class="sidebar-personalized-pill" title="Personalized recommendation based on your interests"><i class="fa-solid fa-wand-magic-sparkles"></i> For You</span>` 
            : '';

        const isDev = !!(window.f0ckSession && window.f0ckSession.development);
        const rawScore = (typeof video.score === 'number') 
            ? video.score 
            : (typeof video.rank_score === 'number' ? video.rank_score : 0);
        const formattedScore = (typeof rawScore === 'number')
            ? (Number.isInteger(rawScore) ? rawScore : +(rawScore.toFixed(1)))
            : rawScore;

        const devScoreAttr = isDev ? ` data-score="${escapeHtml(String(formattedScore))}"` : '';
        const devScoreBadge = isDev
            ? `<span class="sidebar-video-score sidebar-dev-score" title="Development Score: ${formattedScore} (Algo: ${video.score ?? video.rank_score ?? 0})"><i class="fa-solid fa-code"></i> score: ${formattedScore}</span>`
            : '';

        const isTagFeed = options.isTagFeed || false;
        const tagContext = options.tag || null;
        const isFavsFeed = options.isFavsFeed || false;
        const favsUser = options.favsUser || null;
        const isStrict = isStrictMode();
        const strictSuffix = (isTagFeed && isStrict) ? '?strict=1' : '';
        const targetSubHash = video.target_subf0ck_slug ? `#${video.target_subf0ck_slug}` : '';
        const targetHref = (isFavsFeed && favsUser)
            ? `/user/${encodeURIComponent(favsUser)}/favs/${videoKey}${targetSubHash}`
            : (isTagFeed && tagContext)
                ? `/tag/${encodeURIComponent(tagContext).replace(/%2C/g, ',').replace(/%20/g, ' ')}/${videoKey}${strictSuffix}${targetSubHash}`
                : `/${videoKey}${targetSubHash}`;
        const activeClass = options.isActive ? ' active-tag-item' : '';


        return `
            <div class="sidebar-video-card${activeClass}" data-id="${video.id}" data-slug="${escapeHtml(video.slug || '')}" data-file="${escapeHtml(activeDest)}" data-mime="${escapeHtml(activeMime)}" data-ext="${escapeHtml(ext ? ext.toLowerCase() : '')}" data-mode="${rClass}" data-target-subf0ck="${escapeHtml(video.target_subf0ck_slug || '')}" data-personalized="${video.personalized ? 'true' : 'false'}"${devScoreAttr}>
                <a href="${targetHref}" class="sidebar-video-link" data-mode="${rClass}" data-inherit-context="false">
                    <div class="sidebar-video-thumb-wrap" data-file="${escapeHtml(activeDest)}" data-mime="${escapeHtml(activeMime)}" data-ext="${escapeHtml(ext ? ext.toLowerCase() : '')}" data-mode="${rClass}">
                        ${thumbContentHtml}
                        <div class="sidebar-video-badges">
                            ${formatBadge}
                            <span class="sidebar-video-badge rating-${rClass}">${rClass.toUpperCase()}</span>
                        </div>
                    </div>
                    <div class="sidebar-video-details">
                        <div class="sidebar-video-info">
                            <div class="sidebar-video-title" title="${escapeHtml(displayTitle)}">${escapeHtml(displayTitle)}</div>
                            <div class="sidebar-video-channel">
                                <span class="sidebar-video-user" ${userColorStyle}>${escapeHtml(authorName)}</span>
                            </div>
                            <div class="sidebar-video-meta">
                                ${timeStr ? `<span class="sidebar-video-time" title="${escapeHtml(fullDate)}">${escapeHtml(timeStr)}</span>` : ''}
                                ${(timeStr && forYouBadge) ? `&bull;` : ''}
                                ${forYouBadge}
                                ${((timeStr || forYouBadge) && devScoreBadge) ? `&bull;` : ''}
                                ${devScoreBadge}
                            </div>
                        </div>
                    </div>
                </a>
            </div>
        `;
    };

    const getSessionAffinityParams = () => {
        try {
            const aff = window.f0ckInterestEngine ? window.f0ckInterestEngine.getTopAffinities() : null;
            if (!aff) return '';
            let q = '';
            if (aff.topTags && aff.topTags.length > 0) {
                q += `&session_tags=${encodeURIComponent(aff.topTags.join(','))}`;
            }
            if (aff.topCreators && aff.topCreators.length > 0) {
                q += `&session_creators=${encodeURIComponent(aff.topCreators.join(','))}`;
            }
            return q;
        } catch (_) {
            return '';
        }
    };

    const enforceSidebarRecommendationDistribution = (items) => {
        if (!Array.isArray(items) || items.length <= 3) return items;
        // Collect indices of personalized recommendations in the first 3 items
        const recIndicesInFirst3 = [];
        for (let i = 0; i < Math.min(3, items.length); i++) {
            if (items[i] && items[i].personalized) recIndicesInFirst3.push(i);
        }

        // If more than 1 recommendation in first 3, swap extras with non-rec items from index >= 3
        if (recIndicesInFirst3.length > 1) {
            let nonRecIdx = 3;
            for (let r = 1; r < recIndicesInFirst3.length; r++) {
                const swapSlot = recIndicesInFirst3[r];
                while (nonRecIdx < items.length && items[nonRecIdx] && items[nonRecIdx].personalized) {
                    nonRecIdx++;
                }
                if (nonRecIdx < items.length) {
                    const tmp = items[swapSlot];
                    items[swapSlot] = items[nonRecIdx];
                    items[nonRecIdx] = tmp;
                    nonRecIdx++;
                }
            }
        }
        return items;
    };

    const loadRecommendations = async (silent = false) => {
        const container = document.getElementById('sidebar-recommendations-container');
        if (!container || recommendationsLoading) return;

        recommendationsLoading = true;
        if (!silent && !recommendationsLoaded) {
            container.innerHTML = `
                <div class="sidebar-loading-state">
                    <i class="fa-solid fa-circle-notch fa-spin"></i>
                    <span>${window.f0ckI18n?.sidebar_loading_recommendations || 'Loading recommendations...'}</span>
                </div>
            `;
        }

        try {
            seenRecommendationIds.clear();
            const mode = typeof window.activeMode !== 'undefined' ? window.activeMode : '';
            const affParams = getSessionAffinityParams();
            const mime = getCurrentMimeFilter();
            const mimeParam = mime ? `&mime=${encodeURIComponent(mime)}` : '';
            const res = await fetch(`/api/v2/recommendations?limit=${RECOMMENDATIONS_LIMIT}&mode=${mode}${mimeParam}${affParams}`, {
                headers: { 'X-Requested-With': 'XMLHttpRequest' }
            });
            const data = await res.json();
            const items = data.items || data.videos || [];

            if (data.success && items.length > 0) {
                enforceSidebarRecommendationDistribution(items);
                currentRecommendations = items;
                recommendationsLoaded = true;

                let html = '';
                items.forEach(v => {
                    seenRecommendationIds.add(v.id);
                    html += renderVideoCard(v);
                });
                container.innerHTML = html;
                if (silent) {
                    container.scrollTop = 0;
                }
                attachRecommendationSentinel();
                bindRecommendationEvents();
            } else {
                container.innerHTML = `
                    <div style="text-align:center;padding:20px;color:#888;">
                        ${window.f0ckI18n?.sidebar_no_recommendations || 'No recommendations found.'}
                    </div>
                `;
            }
        } catch (e) {
            console.error("Sidebar Recommendations: Failed to load", e);
            if (!recommendationsLoaded) {
                container.innerHTML = `<div style="text-align:center;padding:20px;color:#888;">${window.f0ckI18n?.sidebar_failed_to_load || 'Failed to load.'}</div>`;
            }
        } finally {
            recommendationsLoading = false;
        }
    };

    const loadMoreRecommendations = async () => {
        const container = document.getElementById('sidebar-recommendations-container');
        if (!container || recommendationsLoading || recommendationsLoadingMore || !recommendationsLoaded) return;

        recommendationsLoadingMore = true;

        let indicator = document.getElementById('sidebar-recommendations-load-more');
        if (!indicator) {
            indicator = document.createElement('div');
            indicator.id = 'sidebar-recommendations-load-more';
            indicator.style.cssText = 'text-align:center;padding:12px 0;font-size:0.85em;color:#888;';
            indicator.innerHTML = '<i class="fa-solid fa-circle-notch fa-spin"></i>';
        }
        container.appendChild(indicator);

        try {
            const mode = typeof window.activeMode !== 'undefined' ? window.activeMode : '';
            const excludeArr = Array.from(seenRecommendationIds).slice(-100);
            const affParams = getSessionAffinityParams();
            const mime = getCurrentMimeFilter();
            const mimeParam = mime ? `&mime=${encodeURIComponent(mime)}` : '';
            const res = await fetch(`/api/v2/recommendations?limit=15&mode=${mode}${mimeParam}&exclude_ids=${excludeArr.join(',')}&continuation=1${affParams}`, {
                headers: { 'X-Requested-With': 'XMLHttpRequest' }
            });
            const data = await res.json();
            const items = data.items || data.videos || [];

            if (indicator && indicator.parentNode) {
                indicator.remove();
            }

            if (data.success && items.length > 0) {
                const freshItems = items.filter(v => !seenRecommendationIds.has(v.id));
                if (freshItems.length > 0) {
                    const temp = document.createElement('div');
                    let html = '';
                    freshItems.forEach(v => {
                        seenRecommendationIds.add(v.id);
                        html += renderVideoCard(v);
                    });
                    temp.innerHTML = html;
                    while (temp.firstChild) {
                        container.appendChild(temp.firstChild);
                    }
                    attachRecommendationSentinel();
                }
            }
        } catch (e) {
            console.error("Sidebar Recommendations: Failed to load more", e);
            if (indicator && indicator.parentNode) {
                indicator.remove();
            }
        } finally {
            recommendationsLoadingMore = false;
        }
    };

    const attachRecommendationSentinel = () => {
        const container = document.getElementById('sidebar-recommendations-container');
        if (!container) return;

        if (!recIoSentinel) {
            recIoSentinel = document.createElement('div');
            recIoSentinel.id = 'sidebar-recommendations-io-sentinel';
            recIoSentinel.style.height = '1px';
        }

        container.appendChild(recIoSentinel);

        if (typeof IntersectionObserver !== 'undefined') {
            if (!recObserver) {
                recObserver = new IntersectionObserver((entries) => {
                    if (entries[0].isIntersecting && recommendationsLoaded && !recommendationsLoading && !recommendationsLoadingMore) {
                        loadMoreRecommendations();
                    }
                }, { root: container, rootMargin: '0px 0px 250px 0px', threshold: 0 });
            }
            recObserver.disconnect();
            recObserver.observe(recIoSentinel);
        }
    };

    const bindRecommendationScrollListener = () => {
        const container = document.getElementById('sidebar-recommendations-container');
        if (!container) return;

        // Fallback for environments without IntersectionObserver
        if (typeof IntersectionObserver === 'undefined') {
            container.addEventListener('scroll', () => {
                if (recommendationsLoading || recommendationsLoadingMore || !recommendationsLoaded) return;
                const nearBottom = container.scrollTop + container.clientHeight >= container.scrollHeight - 150;
                if (nearBottom) loadMoreRecommendations();
            }, { passive: true });
        }
    };

    const loadVideoRecommendations = loadRecommendations;

    // ── Dedicated Tag Feed (Ordered for Tag Views) ───────────────────────
    let currentTag = null;
    let tagOrder = 'desc'; // 'desc' = newest first (default), 'asc' = oldest first (chronological)
    let tagOffset = 0;
    let tagTotal = 0;
    let tagFeedLoaded = false;
    let tagFeedLoading = false;
    let tagFeedNeedsReload = false;
    let tagHasMore = false;
    let tagSentinel = null;
    let tagObserver = null;

    const getCurrentTag = () => {
        const pathMatch = window.location.pathname.match(/^\/tag\/([^/?#]+)/);
        if (pathMatch) return decodeURIComponent(pathMatch[1]);
        const searchParams = new URLSearchParams(window.location.search);
        if (searchParams.get('tag')) return searchParams.get('tag');
        if (window.currentTag) return window.currentTag;
        return null;
    };

    const getCurrentItemIdentifiers = () => {
        const ids = new Set();

        // 1. From active item on the page (never match background grid thumbnails)
        if (typeof window.getCurrentItemId === 'function') {
            const currentId = window.getCurrentItemId();
            if (currentId) ids.add(String(currentId).trim());
        } else {
            const idElem = document.querySelector('#onara-item-mount [data-item-id], .item-layout-container[data-item-id], #comments-container[data-item-id]');
            if (idElem?.dataset?.itemId) {
                ids.add(String(idElem.dataset.itemId).trim());
            }
        }
        const idLink = document.querySelector('#onara-item-mount .id-link, .item-layout-container .id-link, #main.item-view .id-link');
        if (idLink) {
            if (idLink.dataset.itemId) ids.add(String(idLink.dataset.itemId).trim());
            const txt = idLink.textContent.trim();
            if (txt) ids.add(txt);
        }

        // 2. From URL pathname (/tag/:tag/:slugOrId, /user/:user/favs/:slugOrId, /user/:user/:slugOrId, /:slugOrId, /4/:board/:thread/:post)
        const segments = window.location.pathname.split('/').filter(Boolean);
        if (segments.length >= 4 && segments[0] === '4') {
            ids.add(decodeURIComponent(segments[3]).trim());
        } else if (segments.length >= 2 && segments[0] === 'tag') {
            if (segments[2]) {
                ids.add(decodeURIComponent(segments[2]).trim());
            }
        } else if (segments.length >= 4 && segments[0] === 'user' && segments[2] === 'favs') {
            // /user/:user/favs/:itemid
            if (segments[3] !== 'p') {
                ids.add(decodeURIComponent(segments[3]).trim());
            }
        } else if (segments.length >= 3 && segments[0] === 'user' && segments[2] !== 'favs') {
            // /user/:user/:itemid (non-favs)
            if (segments[2] !== 'p') {
                ids.add(decodeURIComponent(segments[2]).trim());
            }
        } else if (segments.length === 1) {
            const forbidden = ['s', 'b', 't', 'ca', 'a', '4', 'login', 'register', 'settings', 'about', 'terms', 'rules', 'api', 'logout', 'auth', 'admin', 'mod', 'comments', 'notifications', 'feed', 'upload', 'tags', 'halls', 'ranking', 'abyss', 'random', 'scroller', 'p'];
            if (!forbidden.includes(segments[0])) {
                ids.add(decodeURIComponent(segments[0]).trim());
            }
        }


        return Array.from(ids).filter(Boolean);
    };

    const getCurrentItemKey = () => {
        const ids = getCurrentItemIdentifiers();
        return ids.length > 0 ? ids[0] : null;
    };

    let isReCenteringFeed = false;
    let lastAttemptedTagCenterKey = null;

    // ── Eased scrolling of the tag feed ──
    // One rAF glide with a fixed-feel duration. A long way (or a freshly swapped list) first cuts to
    // half a sidebar short of the target, under a quick fade, and glides the rest: no slow crawl
    // through the whole list, and no thumbnail loads for every card passed on the way.
    let tagGlide = null; // { target, raf }
    const reduceMotion = window.matchMedia ? window.matchMedia('(prefers-reduced-motion: reduce)') : { matches: false };
    const glideTagFeed = (container, target, cut = false) => {
        const max = Math.max(0, container.scrollHeight - container.clientHeight);
        target = Math.max(0, Math.min(Math.round(target), max));
        // already heading there (the click-time and the after-load pass ask for the same card)
        if (tagGlide && Math.abs(tagGlide.target - target) < 2) return;
        if (tagGlide) cancelAnimationFrame(tagGlide.raf);
        tagGlide = null;
        let from = container.scrollTop;
        if (Math.abs(target - from) < 2) return;
        if (reduceMotion.matches) {
            container.scrollTop = target;
            return;
        }
        const lead = container.clientHeight * 0.5;
        if (cut || Math.abs(target - from) > container.clientHeight * 1.5) {
            const dir = cut ? -1 : Math.sign(target - from);
            from = target - dir * lead;
            if (from < 0 || from > max) from = target + dir * lead;
            from = Math.max(0, Math.min(from, max));
            container.scrollTop = from;
            const items = container.querySelector('.sidebar-tag-items-container');
            if (items) {
                items.classList.remove('tag-feed-cut');
                void items.offsetWidth; // restart the fade
                items.classList.add('tag-feed-cut');
            }
        }
        const dist = target - from;
        const duration = Math.min(420, 200 + Math.abs(dist) * 0.2);
        // clock starts at the first frame: rAF timestamps are frame start times and can lie before a
        // performance.now() taken here, and a negative t would ease the wrong way
        let t0 = null;
        const step = (now) => {
            if (t0 === null) t0 = now;
            const t = Math.max(0, Math.min(1, (now - t0) / duration));
            container.scrollTop = from + dist * (1 - Math.pow(1 - t, 3)); // ease-out cubic
            if (t < 1) tagGlide.raf = requestAnimationFrame(step);
            else tagGlide = null;
        };
        tagGlide = { target, raf: requestAnimationFrame(step) };
    };

    // cut: the list was just swapped, so glide in from close by instead of from the old position.
    // ids: the item to mark (default: the one on the page) — random passes the next item at click time.
    const highlightActiveTagCard = (scrollIntoView = false, cut = false, ids = null) => {
        const identifiers = ids || getCurrentItemIdentifiers();
        const container = document.getElementById('sidebar-tag-container');
        if (!container) return;

        let matchedCard = null;

        container.querySelectorAll('.sidebar-video-card').forEach(card => {
            const cardId = card.dataset.id ? String(card.dataset.id).trim() : '';
            const cardSlug = card.dataset.slug ? String(card.dataset.slug).trim() : '';

            const isMatch = identifiers.length > 0 && (
                (cardId && identifiers.includes(cardId)) ||
                (cardSlug && identifiers.includes(cardSlug))
            );

            if (isMatch) {
                card.classList.add('active-tag-item');
                if (!matchedCard) matchedCard = card;
            } else {
                card.classList.remove('active-tag-item');
            }
        });

        if (matchedCard) {
            lastAttemptedTagCenterKey = null;
        }

        if (matchedCard && scrollIntoView) {
            const isTagTabActive = document.querySelector('.sidebar-tab.active')?.dataset.tab === 'tag' &&
                                  container.style.display !== 'none';
            if (isTagTabActive) {
                const containerRect = container.getBoundingClientRect();
                const cardRect = matchedCard.getBoundingClientRect();
                const targetScrollTop = container.scrollTop + (cardRect.top - containerRect.top) - (container.clientHeight / 2) + (cardRect.height / 2);
                glideTagFeed(container, targetScrollTop, cut);
            }
        } else if (!matchedCard && identifiers.length > 0 && currentTag && !isReCenteringFeed && !tagFeedLoading) {
            // Active item belongs to current tag context but is not in currently rendered slice
            // (random, or next/prev past its edge): swap in the slice around it, without a loading state
            const centerKey = `${currentTag}:${identifiers[0]}`;
            if (lastAttemptedTagCenterKey === centerKey) {
                return;
            }
            lastAttemptedTagCenterKey = centerKey;
            isReCenteringFeed = true;
            loadTagFeed(true, identifiers[0], { quiet: true, ids }).finally(() => {
                isReCenteringFeed = false;
            });
        }
    };

    let userManuallySelectedNonTagTab = false;

    const updateTagTabVisibility = (forceSwitchToTag = false) => {
        const tag = getCurrentTag();
        const tagBtn = document.getElementById('sidebar-tab-tag');
        if (!tagBtn) return;

        if (tag) {
            tagBtn.style.display = 'inline-flex';
            tagBtn.title = `#${tag}`;
            tagBtn.setAttribute('aria-label', `#${tag}`);

            const titleNameEl = document.querySelector('.sidebar-tag-name');
            if (titleNameEl) titleNameEl.textContent = `#${tag}`;

            if (currentTag !== tag) {
                currentTag = tag;
                lastAttemptedTagCenterKey = null;
                tagOffset = 0;
                tagFeedLoaded = false;
                tagFeedNeedsReload = true;
                userManuallySelectedNonTagTab = false;

                // When entering or switching tag view (e.g. /tag/cat), automatically jump to the tag tab
                switchSidebarTab('tag');
            } else {
                if (forceSwitchToTag || (!userManuallySelectedNonTagTab && document.querySelector('.sidebar-tab.active')?.dataset.tab !== 'tag')) {
                    switchSidebarTab('tag');
                } else {
                    highlightActiveTagCard(true);
                }
            }
        } else {
            tagBtn.style.display = 'none';
            currentTag = null;
            lastAttemptedTagCenterKey = null;
            tagFeedLoaded = false;
            userManuallySelectedNonTagTab = false;

            const activeTab = document.querySelector('.sidebar-tab.active')?.dataset.tab;
            if (activeTab === 'tag') {
                switchSidebarTab('recommendations');
            }
        }
    };

    const tagFeedUrl = (tag, offset, focusId) => {
        const mode = typeof window.activeMode !== 'undefined' ? window.activeMode : '';
        const mime = getCurrentMimeFilter();
        let url = `/api/v2/tag-feed?tag=${encodeURIComponent(tag)}&order=${tagOrder}&offset=${offset}&limit=20&mode=${mode}`;
        if (isStrictMode()) url += `&strict=1`;
        if (mime) url += `&mime=${encodeURIComponent(mime)}`;
        if (focusId) url += `&focus_id=${encodeURIComponent(focusId)}`;
        return url;
    };

    // Short-lived cache of focused tag-feed slices (url → Promise<data>), filled ahead by
    // prefetchTagFeedFocus so jumping to a random item re-centers the feed without waiting
    const TAG_FEED_CACHE_TTL_MS = 10 * 60 * 1000;
    const tagFeedCache = new Map();
    const fetchTagFeedData = (url) => {
        const hit = tagFeedCache.get(url);
        if (hit && Date.now() - hit.ts < TAG_FEED_CACHE_TTL_MS) return hit.promise;
        const promise = fetch(url, { headers: { 'X-Requested-With': 'XMLHttpRequest' } }).then(res => res.json());
        tagFeedCache.set(url, { ts: Date.now(), promise });
        promise.catch(() => tagFeedCache.delete(url));
        if (tagFeedCache.size > 20) tagFeedCache.delete(tagFeedCache.keys().next().value);
        return promise;
    };

    // The slice around `key` (item id or slug) of the current tag's feed, fetched ahead. Stored under
    // the item's id and slug, so it's found whichever of the two the navigation focuses on.
    window.prefetchTagFeedFocus = (key) => {
        const tag = getCurrentTag();
        if (!tag || !key) return;
        const url = tagFeedUrl(tag, 0, key);
        fetchTagFeedData(url).then(data => {
            const items = data?.items || [];
            const idx = items.findIndex(i => String(i.id) === String(key) || String(i.slug || '') === String(key));
            const entry = tagFeedCache.get(url);
            if (idx < 0 || !entry) return;
            const v = items[idx];
            for (const alias of [v.id, v.slug]) {
                if (alias && String(alias) !== String(key)) tagFeedCache.set(tagFeedUrl(tag, 0, alias), entry);
            }
            // thumbnails of the cards that will be on screen around it, so they don't pop in after the jump
            const tpl = document.createElement('template');
            tpl.innerHTML = items.slice(Math.max(0, idx - 4), idx + 5).map(i => renderVideoCard(i, { isTagFeed: true, tag })).join('');
            tpl.content.querySelectorAll('img.sidebar-video-thumb').forEach(img => {
                const src = img.getAttribute('src');
                if (src) new Image().src = src;
            });
        }).catch(() => {});
    };

    // Random knows its next item at click time: mark it and start the glide while the item still loads
    window.sidebarFocusTagItem = (ids) => {
        if (!currentTag || !ids || !ids.length) return;
        if (document.querySelector('.sidebar-tab.active')?.dataset.tab !== 'tag') return;
        highlightActiveTagCard(true, false, ids.map(String));
    };

    // quiet: keep the current cards until the new ones are in (no loading state), then jump to the active one
    const loadTagFeed = async (reset = false, focusId = null, { quiet = false, ids = null } = {}) => {
        const container = document.getElementById('sidebar-tag-container');
        const itemsContainer = container?.querySelector('.sidebar-tag-items-container');
        if (!container || !itemsContainer || tagFeedLoading) return;
        if (!currentTag) return;

        tagFeedLoading = true;
        tagFeedNeedsReload = false;

        if (reset) {
            tagOffset = 0;
        }
        if (reset && !quiet) {
            itemsContainer.innerHTML = `
                <div class="sidebar-loading-state">
                    <i class="fa-solid fa-circle-notch fa-spin"></i>
                    <span>Loading #${escapeHtml(currentTag)} items...</span>
                </div>
            `;
        }

        try {
            const url = tagFeedUrl(currentTag, tagOffset, focusId);
            // focused slices may come from the prefetch cache; plain pages are always fetched fresh
            const data = focusId
                ? await fetchTagFeedData(url)
                : await fetch(url, { headers: { 'X-Requested-With': 'XMLHttpRequest' } }).then(res => res.json());
            const items = data.items || [];
            tagTotal = data.total || 0;
            tagOffset = (data.offset ?? tagOffset) + items.length;
            tagHasMore = typeof data.hasMore !== 'undefined' ? !!data.hasMore : (tagOffset < tagTotal);

            // Update counter in header
            const countEl = container.querySelector('.sidebar-tag-count');
            if (countEl) countEl.textContent = `(${tagTotal})`;

            // Update order button icon and tooltip
            const orderBtn = container.querySelector('.sidebar-tag-order-btn');
            if (orderBtn) {
                if (tagOrder === 'asc') {
                    orderBtn.innerHTML = '<i class="fa-solid fa-arrow-down-1-9"></i>';
                    orderBtn.title = 'Chronological (Oldest first) — Click for Newest first';
                } else {
                    orderBtn.innerHTML = '<i class="fa-solid fa-arrow-down-9-1"></i>';
                    orderBtn.title = 'Newest first (default) — Click for Oldest first';
                }
            }

            if (data.success && items.length > 0) {
                tagFeedLoaded = true;
                const identifiers = ids || getCurrentItemIdentifiers();

                let html = '';
                items.forEach(v => {
                    const vId = String(v.id).trim();
                    const vSlug = String(v.slug || '').trim();
                    const isActive = identifiers.length > 0 && (
                        (vId && identifiers.includes(vId)) ||
                        (vSlug && identifiers.includes(vSlug))
                    );
                    html += renderVideoCard(v, { isTagFeed: true, tag: currentTag, isActive });
                });

                if (reset && quiet) {
                    itemsContainer.innerHTML = html;
                    highlightActiveTagCard(true, true, ids);
                } else if (reset) {
                    itemsContainer.innerHTML = html;
                    setTimeout(() => highlightActiveTagCard(true), 60);
                } else {
                    itemsContainer.insertAdjacentHTML('beforeend', html);
                    highlightActiveTagCard(false);
                }

                initTagSentinel();
            } else if (reset && !quiet) {
                itemsContainer.innerHTML = `
                    <div class="sidebar-empty-state" style="text-align:center;padding:30px 15px;color:var(--text-muted,#888);font-size:0.88em;">
                        <i class="fa-solid fa-tag" style="font-size:1.8em;opacity:0.4;margin-bottom:8px;display:block;"></i>
                        No uploads found for #${escapeHtml(currentTag)}
                    </div>
                `;
            }
        } catch (err) {
            console.error('[TAG-FEED] Failed to load tag items:', err);
            if (reset && !quiet) {
                itemsContainer.innerHTML = `
                    <div class="sidebar-empty-state" style="text-align:center;padding:25px;color:var(--danger,#ff4444);font-size:0.88em;">
                        Failed to load tag items
                    </div>
                `;
            }
        } finally {
            tagFeedLoading = false;
        }
    };

    const loadMoreTagFeed = async () => {
        if (tagFeedLoading || !tagHasMore || !currentTag) return;
        tagOffset += 20;
        await loadTagFeed(false);
    };

    const initTagSentinel = () => {
        const container = document.getElementById('sidebar-tag-container');
        const itemsContainer = container?.querySelector('.sidebar-tag-items-container');
        if (!container || !itemsContainer) return;

        if (tagSentinel && tagSentinel.parentNode) {
            tagSentinel.remove();
        }

        if (!tagHasMore) return;

        tagSentinel = document.createElement('div');
        tagSentinel.className = 'sidebar-tag-sentinel';
        tagSentinel.style.cssText = 'height: 20px; margin: 10px 0;';
        itemsContainer.appendChild(tagSentinel);

        if (tagObserver) tagObserver.disconnect();

        tagObserver = new IntersectionObserver((entries) => {
            if (entries[0].isIntersecting && !tagFeedLoading && tagHasMore) {
                loadMoreTagFeed();
            }
        }, {
            root: container,
            rootMargin: '200px'
        });

        tagObserver.observe(tagSentinel);
    };

    const initTagOrderButton = () => {
        const container = document.getElementById('sidebar-tag-container');
        if (!container || container.dataset.orderBound === 'true') return;
        container.dataset.orderBound = 'true';

        const orderBtn = container.querySelector('.sidebar-tag-order-btn');
        if (orderBtn) {
            orderBtn.addEventListener('click', (e) => {
                e.preventDefault();
                tagOrder = (tagOrder === 'desc') ? 'asc' : 'desc';
                loadTagFeed(true);
            });
        }
    };

    const bindTagContainerEvents = () => {
        const container = document.getElementById('sidebar-tag-container');
        if (!container || container.dataset.eventsBound === 'true') return;
        container.dataset.eventsBound = 'true';

        container.addEventListener('click', (e) => {
            const card = e.target.closest('.sidebar-video-card');
            if (card) {
                container.querySelectorAll('.sidebar-video-card').forEach(c => c.classList.remove('active-tag-item'));
                card.classList.add('active-tag-item');
            }
        });
        container.addEventListener('scroll', () => {
            if (tagFeedLoading || !tagHasMore || !currentTag) return;
            const nearBottom = container.scrollTop + container.clientHeight >= container.scrollHeight - 250;
            if (nearBottom) {
                loadMoreTagFeed();
            }
        }, { passive: true });
    };

    // ── Dedicated Favs Feed (mirrors Tag Feed for /user/x/favs context) ─────
    let currentFavsUser = null;
    let favsOffset = 0;
    let favsTotal = 0;
    let favsFeedLoaded = false;
    let favsFeedLoading = false;
    let favsFeedNeedsReload = false;
    let favsHasMore = false;
    let favsSentinel = null;
    let favsObserver = null;

    const getCurrentFavsUser = () => {
        // Match /user/someuser/favs or /user/someuser/favs/itemid
        const m = window.location.pathname.match(/^\/user\/([^/?#]+)\/favs/);
        if (m) return decodeURIComponent(m[1]);
        return null;
    };

    let isReCenteringFavsFeed = false;
    let lastAttemptedFavsCenterKey = null;

    const highlightActiveFavsCard = (scrollIntoView = false) => {
        const identifiers = getCurrentItemIdentifiers();
        const container = document.getElementById('sidebar-favs-container');
        if (!container) return;

        let matchedCard = null;
        container.querySelectorAll('.sidebar-video-card').forEach(card => {
            const cardId   = card.dataset.id   ? String(card.dataset.id).trim()   : '';
            const cardSlug = card.dataset.slug ? String(card.dataset.slug).trim() : '';
            const isMatch = identifiers.length > 0 && (
                (cardId   && identifiers.includes(cardId))   ||
                (cardSlug && identifiers.includes(cardSlug))
            );
            if (isMatch) {
                card.classList.add('active-tag-item');
                if (!matchedCard) matchedCard = card;
            } else {
                card.classList.remove('active-tag-item');
            }
        });

        if (matchedCard) {
            lastAttemptedFavsCenterKey = null;
        }

        if (matchedCard && scrollIntoView) {
            const isFavsTabActive = document.querySelector('.sidebar-tab.active')?.dataset.tab === 'favs' &&
                                    container.style.display !== 'none';
            if (isFavsTabActive) {
                try {
                    const cr = container.getBoundingClientRect();
                    const mr = matchedCard.getBoundingClientRect();
                    container.scrollTo({ top: Math.max(0, container.scrollTop + (mr.top - cr.top) - (container.clientHeight / 2) + (mr.height / 2)), behavior: 'smooth' });
                } catch (_) {
                    matchedCard.scrollIntoView({ behavior: 'smooth', block: 'center' });
                }
            }
        } else if (scrollIntoView && !matchedCard && identifiers.length > 0 && currentFavsUser && !isReCenteringFavsFeed && !favsFeedLoading) {
            const centerKey = `${currentFavsUser}:${identifiers[0]}`;
            if (lastAttemptedFavsCenterKey === centerKey) {
                return;
            }
            lastAttemptedFavsCenterKey = centerKey;
            isReCenteringFavsFeed = true;
            loadFavsFeed(true, identifiers[0]).finally(() => {
                isReCenteringFavsFeed = false;
            });
        }
    };

    const loadFavsFeed = async (reset = false, focusId = null) => {
        const container = document.getElementById('sidebar-favs-container');
        const itemsContainer = container?.querySelector('.sidebar-favs-items-container');
        if (!container || !itemsContainer || favsFeedLoading) return;
        if (!currentFavsUser) return;

        favsFeedLoading = true;
        favsFeedNeedsReload = false;

        if (reset) {
            favsOffset = 0;
            itemsContainer.innerHTML = `
                <div class="sidebar-loading-state">
                    <i class="fa-solid fa-circle-notch fa-spin"></i>
                    <span>Loading favorites...</span>
                </div>
            `;
        }

        try {
            const mode = typeof window.activeMode !== 'undefined' ? window.activeMode : '';
            let url = `/api/v2/favs-feed?user=${encodeURIComponent(currentFavsUser)}&order=desc&offset=${favsOffset}&limit=20&mode=${mode}`;
            if (focusId) url += `&focus_id=${encodeURIComponent(focusId)}`;

            const res = await fetch(url, { headers: { 'X-Requested-With': 'XMLHttpRequest' } });
            const data = await res.json();
            const items = data.items || [];
            favsTotal = data.total || 0;
            favsOffset = (data.offset ?? favsOffset) + items.length;
            favsHasMore = typeof data.hasMore !== 'undefined' ? !!data.hasMore : (favsOffset < favsTotal);

            // Update header
            const countEl = container.querySelector('.sidebar-favs-count');
            if (countEl) countEl.textContent = `(${favsTotal})`;

            const nameEl = container.querySelector('.sidebar-favs-name');
            if (nameEl) nameEl.textContent = currentFavsUser + "'s favs";

            if (data.success && items.length > 0) {
                favsFeedLoaded = true;
                const identifiers = getCurrentItemIdentifiers();
                let html = '';
                items.forEach(v => {
                    const vId   = String(v.id).trim();
                    const vSlug = String(v.slug || '').trim();
                    const isActive = identifiers.length > 0 && (
                        (vId   && identifiers.includes(vId))   ||
                        (vSlug && identifiers.includes(vSlug))
                    );
                    html += renderVideoCard(v, { isFavsFeed: true, favsUser: currentFavsUser, isActive });
                });

                if (reset) {
                    itemsContainer.innerHTML = html;
                    setTimeout(() => highlightActiveFavsCard(true), 60);
                } else {
                    itemsContainer.insertAdjacentHTML('beforeend', html);
                    highlightActiveFavsCard(false);
                }
                initFavsSentinel();
            } else if (reset) {
                itemsContainer.innerHTML = `
                    <div class="sidebar-empty-state" style="text-align:center;padding:30px 15px;color:var(--text-muted,#888);font-size:0.88em;">
                        <i class="fa-regular fa-heart" style="font-size:1.8em;opacity:0.4;margin-bottom:8px;display:block;"></i>
                        No favorites found
                    </div>
                `;
            }
        } catch (err) {
            console.error('[FAVS-FEED] Failed:', err);
            if (reset) {
                itemsContainer.innerHTML = `<div class="sidebar-empty-state" style="text-align:center;padding:25px;color:var(--danger,#ff4444);font-size:0.88em;">Failed to load favorites</div>`;
            }
        } finally {
            favsFeedLoading = false;
        }
    };

    const initFavsSentinel = () => {
        const container = document.getElementById('sidebar-favs-container');
        const itemsContainer = container?.querySelector('.sidebar-favs-items-container');
        if (!container || !itemsContainer) return;

        if (favsSentinel && favsSentinel.parentNode) favsSentinel.remove();
        if (favsObserver) { favsObserver.disconnect(); favsObserver = null; }
        if (!favsHasMore) return;

        favsSentinel = document.createElement('div');
        favsSentinel.className = 'sidebar-tag-sentinel';
        favsSentinel.style.cssText = 'height: 20px; margin: 10px 0;';
        itemsContainer.appendChild(favsSentinel);

        favsObserver = new IntersectionObserver((entries) => {
            if (entries[0].isIntersecting && !favsFeedLoading && favsHasMore) {
                loadFavsFeed(false);
            }
        }, {
            root: container,
            rootMargin: '200px'
        });
        favsObserver.observe(favsSentinel);
    };


    const updateFavsTabVisibility = (forceSwitchToFavs = false) => {
        const user = getCurrentFavsUser();
        const favsBtn = document.getElementById('sidebar-tab-favs');
        if (!favsBtn) return;

        if (user) {
            favsBtn.style.display = 'inline-flex';
            favsBtn.title = `${user}'s favs`;

            if (currentFavsUser !== user) {
                currentFavsUser = user;
                lastAttemptedFavsCenterKey = null;
                favsOffset = 0;
                favsFeedLoaded = false;
                favsFeedNeedsReload = true;
                switchSidebarTab('favs');
            } else {
                if (forceSwitchToFavs || document.querySelector('.sidebar-tab.active')?.dataset.tab !== 'favs') {
                    switchSidebarTab('favs');
                } else {
                    highlightActiveFavsCard(true);
                }
            }
        } else {
            favsBtn.style.display = 'none';
            currentFavsUser = null;
            lastAttemptedFavsCenterKey = null;
            favsFeedLoaded = false;

            const activeTab = document.querySelector('.sidebar-tab.active')?.dataset.tab;
            if (activeTab === 'favs') {
                // Fall back to tag tab if available, else recommendations
                if (getCurrentTag()) {
                    switchSidebarTab('tag');
                } else {
                    switchSidebarTab('recommendations');
                }
            }
        }
    };

    const bindFavsContainerEvents = () => {
        const container = document.getElementById('sidebar-favs-container');
        if (!container || container.dataset.eventsBound === 'true') return;
        container.dataset.eventsBound = 'true';
        container.addEventListener('click', (e) => {
            const card = e.target.closest('.sidebar-video-card');
            if (card) {
                container.querySelectorAll('.sidebar-video-card').forEach(c => c.classList.remove('active-tag-item'));
                card.classList.add('active-tag-item');
            }
        });
        container.addEventListener('scroll', () => {
            if (favsFeedLoading || !favsHasMore || !currentFavsUser) return;
            const nearBottom = container.scrollTop + container.clientHeight >= container.scrollHeight - 250;
            if (nearBottom) {
                loadFavsFeed(false);
            }
        }, { passive: true });
    };

    // ── Dedicated 4chan Thread Feed (mirrors Tag/Favs Feed for /4/:board/:thread context) ─────
    let currentChanBoard = null;
    let currentChanThread = null;
    let chanFeedLoaded = false;
    let chanFeedLoading = false;

    const getCurrentChanContext = () => {
        const m = window.location.pathname.match(/^\/4\/([a-z0-9_]+)\/(\d+)(?:\/(\d+))?/i);
        if (m) {
            return {
                board: m[1].toLowerCase(),
                thread: m[2],
                post: m[3] || null
            };
        }
        return null;
    };

    const highlightActiveChanCard = (scrollIntoView = false) => {
        const chanCtx = getCurrentChanContext();
        const container = document.getElementById('sidebar-chan-container');
        if (!container || !chanCtx) return;

        const currentPost = chanCtx.post || '';
        let matchedCard = null;

        container.querySelectorAll('.sidebar-video-card').forEach(card => {
            const cardId = card.dataset.id ? String(card.dataset.id).trim() : '';
            const isMatch = (currentPost && cardId === currentPost);
            if (isMatch) {
                card.classList.add('active-tag-item');
                if (!matchedCard) matchedCard = card;
            } else {
                card.classList.remove('active-tag-item');
            }
        });

        if (matchedCard && scrollIntoView) {
            const isChanTabActive = document.querySelector('.sidebar-tab.active')?.dataset.tab === 'chan' &&
                                    container.style.display !== 'none';
            if (isChanTabActive) {
                try {
                    const cr = container.getBoundingClientRect();
                    const mr = matchedCard.getBoundingClientRect();
                    const targetScrollTop = container.scrollTop + (mr.top - cr.top) - (container.clientHeight / 2) + (mr.height / 2);
                    container.scrollTo({ top: Math.max(0, targetScrollTop), behavior: 'smooth' });
                } catch (_) {
                    matchedCard.scrollIntoView({ behavior: 'smooth', block: 'center' });
                }
            }
        }
    };

    const loadChanFeed = async (reset = false) => {
        const chanCtx = getCurrentChanContext();
        const container = document.getElementById('sidebar-chan-container');
        const itemsContainer = container?.querySelector('.sidebar-chan-items-container');
        if (!container || !itemsContainer || chanFeedLoading) return;
        if (!chanCtx) return;

        chanFeedLoading = true;

        if (reset) {
            itemsContainer.innerHTML = `
                <div class="sidebar-loading-state">
                    <i class="fa-solid fa-circle-notch fa-spin"></i>
                    <span>Loading thread media...</span>
                </div>
            `;
        }

        try {
            const res = await fetch(`/api/v2/chan/${encodeURIComponent(chanCtx.board)}/${encodeURIComponent(chanCtx.thread)}/media`, {
                headers: { 'X-Requested-With': 'XMLHttpRequest' }
            });
            const data = await res.json();
            const items = data.items || data.media || [];

            if (data.success && items.length > 0) {
                chanFeedLoaded = true;

                // Update header
                const countEl = container.querySelector('.sidebar-chan-count');
                if (countEl) countEl.textContent = `(${items.length})`;

                const nameEl = container.querySelector('.sidebar-chan-name');
                if (nameEl) nameEl.textContent = `/${chanCtx.board}/${chanCtx.thread}`;

                const overviewBtn = container.querySelector('.sidebar-chan-overview-btn');
                if (overviewBtn) {
                    overviewBtn.href = `/4/${chanCtx.board}/${chanCtx.thread}`;
                }

                let html = '';
                items.forEach(v => {
                    const isActive = chanCtx.post && String(v.no) === String(chanCtx.post);
                    const activeClass = isActive ? ' active-tag-item' : '';
                    const targetHref = `/4/${chanCtx.board}/${chanCtx.thread}/${v.no}`;
                    const formatBadge = v.ext ? `<span class="sidebar-media-format-badge">${escapeHtml(v.ext.replace('.', '').toUpperCase())}</span>` : '';
                    const displayTitle = v.filename ? `${v.filename}` : `#${v.no}`;
                    const overlayIcon = v.is_video ? 'fa-solid fa-play' : 'fa-solid fa-image';
                    const fsizeStr = v.fsize ? `${(v.fsize / 1024 / 1024).toFixed(1)} MB` : '';

                    html += `
                        <div class="sidebar-video-card${activeClass}" data-id="${v.no}">
                            <a href="${targetHref}" class="sidebar-video-link" data-inherit-context="false">
                                <div class="sidebar-video-thumb-wrap">
                                    <img src="${escapeHtml(v.thumb)}" class="sidebar-video-thumb" alt="${escapeHtml(displayTitle)}" loading="lazy" draggable="false" data-on="error:sidebar-thumb-fail" />
                                    <div class="sidebar-media-placeholder hidden">
                                        <i class="${overlayIcon}"></i>
                                    </div>
                                    <div class="sidebar-video-badges">
                                        ${formatBadge}
                                        <span class="sidebar-video-badge" style="background: rgba(74, 222, 128, 0.2); color: #4ade80;">4CHAN</span>
                                    </div>
                                </div>
                                <div class="sidebar-video-details">
                                    <div class="sidebar-video-info">
                                        <div class="sidebar-video-title" title="${escapeHtml(displayTitle)}">${escapeHtml(displayTitle)}</div>
                                        <div class="sidebar-video-channel">
                                            <span class="sidebar-video-user" style="color: #4ade80;">#${v.no}</span>
                                        </div>
                                        <div class="sidebar-video-meta">
                                            ${fsizeStr ? `<span class="sidebar-video-time">${escapeHtml(fsizeStr)}</span>` : ''}
                                            ${v.w && v.h ? `<span>${fsizeStr ? '&bull; ' : ''}${v.w}x${v.h}</span>` : ''}
                                        </div>
                                    </div>
                                </div>
                            </a>
                        </div>
                    `;
                });

                itemsContainer.innerHTML = html;
                setTimeout(() => highlightActiveChanCard(true), 60);
            } else if (reset) {
                itemsContainer.innerHTML = `
                    <div class="sidebar-empty-state" style="text-align:center;padding:30px 15px;color:var(--text-muted,#888);font-size:0.88em;">
                        <i class="fa-solid fa-clover" style="font-size:1.8em;opacity:0.4;margin-bottom:8px;display:block;color:#4ade80;"></i>
                        No media found in thread
                    </div>
                `;
            }
        } catch (err) {
            console.error('[CHAN-FEED] Failed:', err);
            if (reset) {
                itemsContainer.innerHTML = `<div class="sidebar-empty-state" style="text-align:center;padding:25px;color:var(--danger,#ff4444);font-size:0.88em;">Failed to load thread media</div>`;
            }
        } finally {
            chanFeedLoading = false;
        }
    };

    const updateChanTabVisibility = (forceSwitchToChan = false) => {
        const chanCtx = getCurrentChanContext();
        const chanBtn = document.getElementById('sidebar-tab-chan');
        if (!chanBtn) return;

        if (chanCtx) {
            chanBtn.style.display = 'inline-flex';
            chanBtn.title = `/${chanCtx.board}/${chanCtx.thread}`;

            if (currentChanBoard !== chanCtx.board || currentChanThread !== chanCtx.thread) {
                currentChanBoard = chanCtx.board;
                currentChanThread = chanCtx.thread;
                chanFeedLoaded = false;
                userManuallySelectedNonTagTab = false;
                switchSidebarTab('chan');
            } else {
                if (forceSwitchToChan || (!userManuallySelectedNonTagTab && document.querySelector('.sidebar-tab.active')?.dataset.tab !== 'chan')) {
                    switchSidebarTab('chan');
                } else {
                    highlightActiveChanCard(true);
                }
            }
        } else {
            chanBtn.style.display = 'none';
            currentChanBoard = null;
            currentChanThread = null;
            chanFeedLoaded = false;

            const activeTab = document.querySelector('.sidebar-tab.active')?.dataset.tab;
            if (activeTab === 'chan') {
                if (getCurrentTag()) {
                    switchSidebarTab('tag');
                } else {
                    switchSidebarTab('recommendations');
                }
            }
        }
    };

    const bindChanContainerEvents = () => {
        const container = document.getElementById('sidebar-chan-container');
        if (!container || container.dataset.eventsBound === 'true') return;
        container.dataset.eventsBound = 'true';
        container.addEventListener('click', (e) => {
            const link = e.target.closest('a.sidebar-video-link');
            if (link && link.href) {
                e.preventDefault();
                container.querySelectorAll('.sidebar-video-card').forEach(c => c.classList.remove('active-tag-item'));
                const card = link.closest('.sidebar-video-card');
                if (card) card.classList.add('active-tag-item');
                if (typeof window.loadItemAjax === 'function') {
                    window.loadItemAjax(link.href, false);
                } else {
                    window.location.href = link.href;
                }
            }
        });
    };

    const switchSidebarTab = (tabName) => {
        if (window.clearHoverPreview) {
            window.clearHoverPreview();
        }
        const normalizedTab = (tabName === 'videos' || tabName === 'recommendations')
            ? 'recommendations'
            : (tabName === 'tag' ? 'tag' : (tabName === 'favs' ? 'favs' : (tabName === 'chan' ? 'chan' : tabName)));
        const tabs = document.querySelectorAll('.sidebar-tab');
        const contents = document.querySelectorAll('.sidebar-tab-content');

        tabs.forEach(t => {
            const tTab = (t.dataset.tab === 'videos' || t.dataset.tab === 'recommendations')
                ? 'recommendations'
                : (t.dataset.tab === 'tag' ? 'tag' : (t.dataset.tab === 'favs' ? 'favs' : (t.dataset.tab === 'chan' ? 'chan' : t.dataset.tab)));
            if (tTab === normalizedTab) {
                t.classList.add('active');
            } else {
                t.classList.remove('active');
            }
        });

        contents.forEach(c => {
            const cTab = (c.dataset.tabContent === 'videos' || c.dataset.tabContent === 'recommendations')
                ? 'recommendations'
                : (c.dataset.tabContent === 'tag' ? 'tag' : (c.dataset.tabContent === 'favs' ? 'favs' : (c.dataset.tabContent === 'chan' ? 'chan' : c.dataset.tabContent)));
            if (cTab === normalizedTab) {
                c.classList.add('active');
                c.style.display = 'block';
            } else {
                c.classList.remove('active');
                c.style.display = 'none';
            }
        });

        try {
            localStorage.setItem('sidebar_active_tab', normalizedTab);
        } catch (_) {}

        if (normalizedTab === 'recommendations') {
            if (!recommendationsLoaded) {
                loadRecommendations();
            }
        } else if (normalizedTab === 'tag') {
            if (tagFeedNeedsReload || !tagFeedLoaded) {
                loadTagFeed(true);
            } else {
                setTimeout(() => highlightActiveTagCard(true), 60);
            }
        } else if (normalizedTab === 'favs') {
            if (favsFeedNeedsReload || !favsFeedLoaded) {
                loadFavsFeed(true);
            } else {
                setTimeout(() => highlightActiveFavsCard(true), 60);
            }
        } else if (normalizedTab === 'chan') {
            if (!chanFeedLoaded) {
                loadChanFeed(true);
            } else {
                setTimeout(() => highlightActiveChanCard(true), 60);
            }
        }
    };
    window.switchSidebarTab = switchSidebarTab;

    const initSidebarTabs = () => {
        const tabsContainer = document.querySelector('.sidebar-tabs');
        if (tabsContainer) {
            tabsContainer.addEventListener('click', (e) => {
                const btn = e.target.closest('.sidebar-tab');
                if (btn && btn.dataset.tab) {
                    e.preventDefault();
                    if (btn.dataset.tab !== 'tag' && btn.dataset.tab !== 'favs' && btn.dataset.tab !== 'chan') {
                        userManuallySelectedNonTagTab = true;
                    } else {
                        userManuallySelectedNonTagTab = false;
                    }
                    switchSidebarTab(btn.dataset.tab);
                }
            });
        }

        // Restore persisted active tab if any
        let savedTab = 'comments';
        try {
            savedTab = localStorage.getItem('sidebar_active_tab') || 'comments';
        } catch (_) {}

        if (getCurrentChanContext()) {
            switchSidebarTab('chan');
        } else if (getCurrentFavsUser()) {
            switchSidebarTab('favs');
        } else if (getCurrentTag()) {
            switchSidebarTab('tag');
        } else if (savedTab === 'videos' || savedTab === 'recommendations') {
            switchSidebarTab('recommendations');
        } else {
            switchSidebarTab(savedTab);
        }
    };

    const replaceCardWithNewRandom = async (card) => {
        if (!card || card.dataset.swapping === 'true') return;
        if (window.clearHoverPreview) {
            window.clearHoverPreview();
        }
        card.dataset.swapping = 'true';
        card.classList.add('sidebar-card-swapping');

        const clickedId = parseInt(card.dataset.id, 10);

        try {
            const mode = typeof window.activeMode !== 'undefined' ? window.activeMode : '';
            const container = document.getElementById('sidebar-recommendations-container');
            const visibleIds = [];
            if (container) {
                container.querySelectorAll('.sidebar-video-card').forEach(c => {
                    const cid = parseInt(c.dataset.id, 10);
                    if (cid) visibleIds.push(cid);
                });
            }

            const excludeSet = new Set([...seenRecommendationIds, ...visibleIds]);
            if (clickedId) {
                excludeSet.add(clickedId);
                if (window.f0ckInterestEngine) {
                    window.f0ckInterestEngine.recordSuggestionClick(clickedId);
                }
            }
            const excludeArr = Array.from(excludeSet).slice(-120);

            const cards = Array.from(container ? container.querySelectorAll('.sidebar-video-card') : []);
            const cardIndex = cards.indexOf(card);
            let preferParam = '';
            if (cardIndex >= 0 && cardIndex < 3) {
                // Keep exactly 1 recommendation in the first 3 cards
                const isPersonalized = card.dataset.personalized === 'true';
                preferParam = `&prefer_personalized=${isPersonalized ? 'true' : 'false'}`;
            } else if (cardIndex >= 3) {
                // Ensure recommendations remain sporadic (never consecutive)
                const prevCard = cards[cardIndex - 1];
                const nextCard = cards[cardIndex + 1];
                const prevIsRec = prevCard && prevCard.dataset.personalized === 'true';
                const nextIsRec = nextCard && nextCard.dataset.personalized === 'true';
                if (prevIsRec || nextIsRec) {
                    preferParam = '&prefer_personalized=false';
                }
            }

            const affParams = getSessionAffinityParams();
            const mime = getCurrentMimeFilter();
            const mimeParam = mime ? `&mime=${encodeURIComponent(mime)}` : '';
            const res = await fetch(`/api/v2/recommendations?limit=1&mode=${mode}${mimeParam}&exclude_ids=${excludeArr.join(',')}${preferParam}${affParams}`, {
                headers: { 'X-Requested-With': 'XMLHttpRequest' }
            });
            const data = await res.json();
            const items = data.items || data.videos || [];

            if (data.success && items.length > 0) {
                const newItem = items[0];
                seenRecommendationIds.add(newItem.id);

                const idx = currentRecommendations.findIndex(v => v.id === clickedId);
                if (idx !== -1) {
                    currentRecommendations[idx] = newItem;
                } else {
                    currentRecommendations.push(newItem);
                }

                const temp = document.createElement('div');
                temp.innerHTML = renderVideoCard(newItem).trim();
                const newCard = temp.firstElementChild;

                if (newCard && card.parentNode) {
                    card.replaceWith(newCard);
                }
            } else {
                card.classList.remove('sidebar-card-swapping');
                delete card.dataset.swapping;
            }
        } catch (e) {
            console.error("Sidebar Recommendations: Failed to swap clicked card", e);
            if (card) {
                card.classList.remove('sidebar-card-swapping');
                delete card.dataset.swapping;
            }
        }
    };

    const bindRecommendationEvents = () => {
        const container = document.getElementById('sidebar-recommendations-container');
        if (!container || container.dataset.eventsBound === 'true') return;
        container.dataset.eventsBound = 'true';

        const handleCardAction = (e) => {
            // Only respond to primary click (0) or middle-click (1). Ignore right click (2).
            if (e.button !== 0 && e.button !== 1) return;

            const card = e.target.closest('.sidebar-video-card');
            if (card) {
                replaceCardWithNewRandom(card);
            }
        };

        container.addEventListener('click', handleCardAction);
        container.addEventListener('auxclick', handleCardAction);
    };

    // Reload recommendations or sync tag feed when user triggers Random (#random, #nav-random, or 'r' key)
    const handleRandomAction = () => {
        // If pool-based random is active, the context hasn't changed — skip reload
        if (window._randomPool && window._randomPool.items && window._randomPool.items.length > 1) {
            // Context is stable (same tag/hall/user): f0ckm.js focuses the picked item in the tag feed
            // (sidebarFocusTagItem) — highlighting here would still see the old item
            return;
        }
        // Fallback: server-side random may change context, so reload
        recommendationsLoaded = false;
        const tag = getCurrentTag();
        if (tag) {
            userManuallySelectedNonTagTab = false;
            switchSidebarTab('tag');
        } else {
            let savedTab = 'comments';
            try { savedTab = localStorage.getItem('sidebar_active_tab'); } catch (_) {}
            if (savedTab === 'videos' || savedTab === 'recommendations') {
                loadRecommendations(false);
            }
        }
    };

    document.addEventListener('f0ck:randomTriggered', handleRandomAction);
    document.addEventListener('click', (e) => {
        const btn = e.target.closest('#random, #nav-random, a[href="/random"], a[href$="/random"]');
        if (btn) {
            handleRandomAction();
        }
    }, true);

    // Handle explicit mode changes (e.g. from item page where full transition doesn't occur)
    document.addEventListener('f0ck:modeChanged', (e) => {
        window.f0ckDebug("Sidebar Activity: Mode change detected", e.detail.mode);
        lastBoundMode = e.detail.mode;
        window._sidebarActivityCache = [];
        lastRenderedIds = '';
        currentPage = 1;
        hasMore = true;
        loadActivity(false);

        recommendationsLoaded = false;
        tagFeedLoaded = false;
        tagFeedNeedsReload = true;
        const activeTabEl = document.querySelector('.sidebar-tab.active')?.dataset.tab;
        let savedTab = 'comments';
        try { savedTab = localStorage.getItem('sidebar_active_tab'); } catch (_) {}
        const currentTabName = activeTabEl || savedTab;

        if (currentTabName === 'videos' || currentTabName === 'recommendations') {
            loadRecommendations(false);
        } else if (currentTabName === 'tag' && currentTag) {
            loadTagFeed(true);
        }
    });

    // Handle strict mode toggle
    document.addEventListener('f0ck:strictChanged', (e) => {
        const currentStrict = (e.detail && typeof e.detail.strict !== 'undefined') ? !!e.detail.strict : isStrictMode();
        window.f0ckDebug("Sidebar Activity: Strict mode change detected", currentStrict);
        lastBoundStrict = currentStrict;

        tagFeedLoaded = false;
        tagFeedNeedsReload = true;
        const activeTabEl = document.querySelector('.sidebar-tab.active')?.dataset.tab;
        let savedTab = 'comments';
        try { savedTab = localStorage.getItem('sidebar_active_tab'); } catch (_) {}
        const currentTabName = activeTabEl || savedTab;

        if (currentTabName === 'tag' && currentTag) {
            loadTagFeed(true);
        }
    });

    // When the current user posts a comment, silently refresh sidebar to show it
    document.addEventListener('f0ck:commentPosted', () => {
        window.f0ckDebug("Sidebar Activity: Own comment posted, refreshing...");
        loadActivity(true);
    });

    // Infinite scroll: load older comments when scrolling near the bottom
    const bindScrollListener = () => {
        const container = document.getElementById('sidebar-activity-container');
        if (!container) return;

        // Use IntersectionObserver if available (performant), fallback to scroll event
        if (typeof IntersectionObserver !== 'undefined') {
            // Create the sentinel once at module level so re-renders can re-append the same node
            if (!ioSentinel) {
                ioSentinel = document.createElement('div');
                ioSentinel.id = 'sidebar-io-sentinel';
                ioSentinel.style.height = '1px';
                container.appendChild(ioSentinel);
            }

            const observer = new IntersectionObserver((entries) => {
                if (entries[0].isIntersecting && hasMore && !loadingMore && !loading) {
                    loadMoreActivity();
                }
            }, { root: container, rootMargin: '0px 0px 80px 0px', threshold: 0 });

            observer.observe(ioSentinel);
        } else {
            container.addEventListener('scroll', () => {
                if (loading || loadingMore || !hasMore) return;
                const nearBottom = container.scrollTop + container.clientHeight >= container.scrollHeight - 100;
                if (nearBottom) loadMoreActivity();
            }, { passive: true });
        }
    };

    // Initial load
    const _origInit = init;
    const initWithScroll = async () => {
        initSidebarTabs();
        initTagOrderButton();
        bindTagContainerEvents();
        bindFavsContainerEvents();
        bindChanContainerEvents();
        updateTagTabVisibility();
        updateFavsTabVisibility();
        updateChanTabVisibility();
        document.addEventListener('f0ck:contentLoaded', () => {
            updateTagTabVisibility();
            updateFavsTabVisibility();
            updateChanTabVisibility();
            setTimeout(() => {
                highlightActiveTagCard(true);
                highlightActiveFavsCard(true);
                highlightActiveChanCard(true);
            }, 60);
        });
        window.addEventListener('popstate', () => {
            setTimeout(() => {
                updateTagTabVisibility();
                updateFavsTabVisibility();
                updateChanTabVisibility();
                highlightActiveTagCard(true);
                highlightActiveFavsCard(true);
                highlightActiveChanCard(true);
            }, 60);
        });
        bindRecommendationEvents();
        bindRecommendationScrollListener();
        await _origInit();
        bindScrollListener();
    };

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', initWithScroll);
    } else {
        initWithScroll();
    }

    // Live updates are handled via SSE (f0ck:activityReceived event)
})();
