// Safe wrapper — window.f0ckDebug may not be defined yet when this file first executes
// (comments.js loads before the footer script block that sets window.f0ckDebug).
// Calling through this helper defers the lookup to invocation time, not parse time.
const _f0ckDebug = (...args) => (typeof window.f0ckDebug === 'function' ? window.f0ckDebug(...args) : void 0);

// Guest comment box (renderGuestInput): clicking it opens the login modal
F0ckActions.define('comments-guest-login', () => {
    const b = document.getElementById('nav-login-btn');
    if (b) b.click();
});

class CommentSystem {
    get isMainSubmitting() {
        return this._globalState ? this._globalState.isMainSubmitting : false;
    }
    set isMainSubmitting(val) {
        if (this._globalState) this._globalState.isMainSubmitting = val;
    }
    get pendingSubmissions() {
        return this._globalState ? this._globalState.pendingSubmissions : new Set();
    }

    constructor() {
        this.container = document.getElementById('comments-container');
        this.itemId = this.container ? this.container.dataset.itemId : null;
        this.user = this.container ? this.container.dataset.user : null; // logged in user?
        this.isAdmin = this.container ? this.container.dataset.isAdmin === 'true' : false;
        this.isLocked = this.container ? this.container.dataset.isLocked === 'true' : false;
        this.displayMode = window.f0ckSession?.comment_display_mode || 0; // 0=Tree, 1=Linear
        this.sort = (document.body.classList.contains('layout-legacy') || document.body.classList.contains('legacy-view')) ? 'old' : 'new';

        // Linear mode usually implies chronological order (4chan style)
        if (this.displayMode === 1) this.sort = 'old';

        this.customEmojis = CommentSystem.emojiCache || {};
        this._selfPostedCommentIds = new Set();
        this._pendingSelfCommentTexts = new Set();

        this.icons = {
            reply: `<i class="fa-solid fa-reply"></i>`,
            pin: `<i class="fa-solid fa-thumbtack"></i>`,
            unpin: `<i class="fa-solid fa-thumbtack"></i>`,
            edit: `<i class="fa-solid fa-pen"></i>`,
            delete: `<i class="fa-solid fa-trash"></i>`,
            link: `<i class="fa-solid fa-link"></i>`,
            pinned: `<i class="fa-solid fa-thumbtack"></i>`,
            lock: `<i class="fa-solid fa-lock"></i>`,
            unlock: `<i class="fa-solid fa-lock-open"></i>`
        };

        // Restore visibility state
        // The body class 'sidebar-left-hidden' is already set by the blocking
        // inline script in header.html (prevents flash). Here we sync the
        // container-level class and display state to match.
        if (this.container) {
            // A slide cut short by navigating away must not leave its border behind
            this._legacySlide = null;
            document.body.classList.remove('comments-sliding');
            const isHidden = localStorage.getItem('comments_hidden') === 'true';
            // Force show if hash is present
            if (window.location.hash && window.location.hash.startsWith('#c')) {
                this.container.classList.remove('faded-out', 'is-hidden');
                this.container.style.removeProperty('display');
                this.container.style.display = '';
                localStorage.setItem('comments_hidden', 'false');
                document.body.classList.remove('sidebar-left-hidden', 'comments-hidden');
                const layout = this.container.closest('.item-layout-container');
                if (layout) layout.classList.remove('sidebar-hidden');
                const sidebar = this.container.closest('.item-sidebar-left');
                if (sidebar) {
                    sidebar.classList.remove('is-hidden');
                    sidebar.style.removeProperty('display');
                    sidebar.style.display = '';
                }
            } else if (isHidden) {
                this.container.classList.add('faded-out', 'is-hidden');
                this.container.style.setProperty('display', 'none', 'important');
                document.body.classList.add('sidebar-left-hidden', 'comments-hidden');
                const layout = this.container.closest('.item-layout-container');
                if (layout) layout.classList.add('sidebar-hidden');
                const sidebar = this.container.closest('.item-sidebar-left');
                if (sidebar) {
                    sidebar.classList.add('is-hidden');
                    sidebar.style.setProperty('display', 'none', 'important');
                }
            }
        }

        this.initialLoadDone = false;
        
        // Retrieve or initialize global submission state for this item to survive f0ck:contentLoaded re-inits
        if (this.itemId) {
            window._f0ckActiveSubmissions = window._f0ckActiveSubmissions || {};
            if (!window._f0ckActiveSubmissions[this.itemId]) {
                window._f0ckActiveSubmissions[this.itemId] = {
                    isMainSubmitting: false,
                    pendingSubmissions: new Set()
                };
            }
            this._globalState = window._f0ckActiveSubmissions[this.itemId];
        } else {
            this._globalState = {
                isMainSubmitting: false,
                pendingSubmissions: new Set()
            };
        }

        this.scrollListenerAdded = false;
        this.commentCache = new Map();
        this._anchorScrollDone = false; // true after the first hash-anchor scroll on initial load

        // Defer emoji loading — only fetch if the page actually has :emoji: patterns
        // (avoids a full API round-trip on every item page that has no custom emojis)
        this._emojiLoadScheduled = false;
        this.setupHoverPreviews();

        if (this.itemId) {
            this.init();
        }
    }

    async init() {
        if (!this.container) {
            console.warn('[CommentSystem] Container not found during init');
            return;
        }
        if (this.container.dataset.commentSystemInit) {
            _f0ckDebug('[CommentSystem] Already initialized for this container');
            return;
        }
        this.container.dataset.commentSystemInit = 'true';
        _f0ckDebug('[CommentSystem] Initializing for item:', this.itemId);

        this.loadComments();
        this.setupGlobalListeners();
        this.setupDelegatedEvents();
        this.startLiveTimestamps();
        this.setupScrollListener();
    }

    setupScrollListener() {
        if (this.scrollListenerAdded) return;
        if (!document.body.classList.contains('layout-legacy') && !document.body.classList.contains('legacy-view')) return;

        const updateBtn = () => {
            const btn = document.querySelector('.scroll-to-bottom');
            if (!btn) return;

            const container = document.getElementById('comments-container');
            if (!container) return;

            const currentlyAtBottom = btn.classList.contains('is-at-bottom');
            let isAtBottom = currentlyAtBottom;

            // Base the bottom state primarily on whether the input box / end-of-thread marker is visible
            const bottomElement = container.querySelector('.main-input') || container.querySelector('.lock-notice');
            
            if (bottomElement) {
                const bottomRect = bottomElement.getBoundingClientRect();
                if (currentlyAtBottom) {
                    // We are pointing UP. Switch to DOWN if we scroll significantly UP away from bottomElement
                    if (bottomRect.top > window.innerHeight + 250) {
                        isAtBottom = false;
                    }
                } else {
                    // We are pointing DOWN. Switch to UP if bottomElement enters viewport or we scroll past it
                    if (bottomRect.top <= window.innerHeight + 50) {
                        isAtBottom = true;
                    }
                }
            } else {
                // Fallback to container rect if no bottom element
                const rect = container.getBoundingClientRect();
                if (currentlyAtBottom) {
                    if (rect.bottom > window.innerHeight + 250) {
                        isAtBottom = false;
                    }
                } else {
                    if (rect.bottom <= window.innerHeight + 50) {
                        isAtBottom = true;
                    }
                }
            }

            if (isAtBottom && !currentlyAtBottom) {
                btn.classList.add('is-at-bottom');
                btn.setAttribute('title', 'Scroll to top');
            } else if (!isAtBottom && currentlyAtBottom) {
                btn.classList.remove('is-at-bottom');
                btn.setAttribute('title', 'Scroll to bottom');
            }
        };

        this.scrollHandler = updateBtn;
        window.addEventListener('scroll', this.scrollHandler, { passive: true });
        this.scrollListenerAdded = true;

        // Dynamic detection after potential renders
        this.scrollTimer = setInterval(this.scrollHandler, 1000);
    }

    /**
     * Boot or reload the Danmaku engine with fresh comment data.
     * Called after render() so the overlay is always in sync with visible comments.
     */
    _loadDanmaku(comments) {
        if (typeof Danmaku === 'undefined') return;

        const playerEl = document.querySelector('.v0ck') || document.getElementById('ruffle-container');
        if (!playerEl) return;

        // Ensure the container is positioned so the absolute overlay works
        if (getComputedStyle(playerEl).position === 'static') {
            playerEl.style.position = 'relative';
        }

        // Prefer real <video>/<audio>; fall back to SyntheticClock for Flash/Ruffle
        let mediaEl = document.querySelector('.v0ck video, .v0ck audio');
        let usingSynth = false;
        if (!mediaEl) {
            // Only create a synthetic clock if the player container exists (Flash/Ruffle etc.)
            if (typeof SyntheticClock !== 'undefined') {
                mediaEl = new SyntheticClock();
                usingSynth = true;
            } else {
                return; // danmaku.js not loaded yet
            }
        }

        // Stale-check: only compare player element (not mediaEl — SyntheticClock is always new)
        if (window.danmakuInstance) {
            const stale = window.danmakuInstance.player !== playerEl
                || !document.contains(window.danmakuInstance.player);
            if (stale) {
                window.danmakuInstance.destroy();
                window.danmakuInstance = null;
            } else if (usingSynth) {
                // Reuse existing instance for Flash — just reload comments, don't recreate clock
                mediaEl.destroy && mediaEl.destroy(); // discard the newly created clock
                mediaEl = window.danmakuInstance.media;
            }
        }

        if (!window.danmakuInstance) {
            window.danmakuInstance = new Danmaku(playerEl, mediaEl);
        }

        // Cancel any previous pending loadedmetadata listener before registering a new one
        if (this._danmakuLoadHandler) {
            mediaEl.removeEventListener('loadedmetadata', this._danmakuLoadHandler);
            this._danmakuLoadHandler = null;
        }

        const load = () => {
            window.danmakuInstance.load(comments);
            this._danmakuLoadHandler = null;
        };

        // SyntheticClock has duration=Infinity (always "ready"); real video may need to wait
        if (usingSynth || (isFinite(mediaEl.duration) && mediaEl.duration > 0)) {
            load();
        } else {
            this._danmakuLoadHandler = load;
            mediaEl.addEventListener('loadedmetadata', load, { once: true });
        }
    }

    destroy() {
        if (this.scrollHandler) {
            window.removeEventListener('scroll', this.scrollHandler);
        }
        if (this.scrollTimer) {
            clearInterval(this.scrollTimer);
        }
        if (this.stabilizationTimer) {
            clearTimeout(this.stabilizationTimer);
        }
        if (this.scrollDebounce) {
            clearTimeout(this.scrollDebounce);
        }
        if (this.stabilizationObserver) {
            this.stabilizationObserver.disconnect();
        }
        this.stopStabilization();
        _f0ckDebug('[CommentSystem] Instance destroyed');
    }

    /**
     * Load custom emoji map from the API.
     * Lazy by default — only fetches if the current page actually contains :emoji: patterns.
     * Pass force=true to bypass the page-scan check (e.g. after SSE emojis_updated).
     */
    async loadEmojis(force = false) {
        if (CommentSystem.emojiCache) {
            this.customEmojis = CommentSystem.emojiCache;
            // Immediate dispatch if already ready
            window.dispatchEvent(new CustomEvent('f0ck:emojis_ready', { detail: this.customEmojis }));
            return;
        }
        if (CommentSystem.loadingEmojis) return;

        // Skip the fetch entirely if there are no :emoji: shortcodes visible on this page.
        // This avoids an unnecessary API round-trip on every item page where custom emojis aren't used.
        if (!force) {
            const textContent = this.container ? this.container.textContent : document.body.textContent;
            if (!/:([a-z0-9_]+):/.test(textContent)) {
                _f0ckDebug('[CommentSystem] No emoji patterns found on page, skipping emoji fetch.');
                // Still emit ready so callers (e.g. emoji picker) don't hang
                window.dispatchEvent(new CustomEvent('f0ck:emojis_ready', { detail: {} }));
                return;
            }
        }

        CommentSystem.loadingEmojis = true;

        try {
            const res = await fetch('/api/v2/emojis');
            const data = await res.json();
            if (data.success) {
                this.customEmojis = {};
                data.emojis.forEach(e => {
                    this.customEmojis[e.name] = e.url;
                });
                CommentSystem.emojiCache = this.customEmojis;
                // Store pack structure for tabbed picker
                CommentSystem.emojiPacks = data.packs || [];
                _f0ckDebug('Loaded Emojis:', this.customEmojis, 'Packs:', CommentSystem.emojiPacks);

                if (this.container && this.lastData) {
                    const state = this.saveState();
                    // Invalidate data-raw on any comment content that contains emoji shortcodes,
                    // so that reconcile() sees them as "changed" and re-renders with the real emoji imgs.
                    this.container.querySelectorAll('.comment-content[data-raw]').forEach(el => {
                        if (/:([a-z0-9_]+):/.test(el.dataset.raw)) {
                            el.dataset.raw = '';
                        }
                    });
                    // Use reconciliation instead of full render to protect playing media
                    // even when emojis load late.
                    this.reconcile(this.lastData, this.lastUserId, this.lastIsSubscribed);
                    this.restoreState(state);
                    // Only scroll to anchor on the very first load — never on async emoji reloads
                    if (!this._anchorScrollDone && !this.preservingScroll && window.location.hash && window.location.hash.startsWith('#c')) {
                        const hashId = window.location.hash.substring(2);
                        this._anchorScrollDone = true;
                        this.scrollToComment(hashId);
                    }
                }

                // Global signal for other components (e.g. DMs)
                window.dispatchEvent(new CustomEvent('f0ck:emojis_ready', { detail: this.customEmojis }));
            } else {
                this.customEmojis = {};
            }
        } catch (e) {
            console.error("Failed to load emojis", e);
            this.customEmojis = {};
        } finally {
            CommentSystem.loadingEmojis = false;
        }
    }

    // ...

    renderEmoji(match, name) {
        // _f0ckDebug('Rendering Emoji:', name, this.customEmojis ? this.customEmojis[name] : 'No list');
        if (this.customEmojis && this.customEmojis[name]) {
            const url = this.customEmojis[name];
            if (url.endsWith('.webm')) {
                return `<video src="${url}" style="height:60px;vertical-align:middle;" autoplay loop muted playsinline title=":${name}:"></video>`;
            }
            return `<img src="${url}" style="height:60px;vertical-align:middle;" alt=":${name}:">`;
        }
        return match;
    }

    saveState() {
        const state = {
            mainText: '',
            isMainSubmitting: this.isMainSubmitting,
            openReplies: [],
            focused: null
        };

        if (!this.container) return state;

        // 1. Save main input
        const mainInput = this.container.querySelector('.main-input textarea');
        if (mainInput) {
            state.mainText = mainInput.value;
            if (document.activeElement === mainInput) {
                state.focused = { type: 'main', start: mainInput.selectionStart, end: mainInput.selectionEnd };
            }
        }

        // 2. Save open replies
        this.container.querySelectorAll('.reply-input').forEach(form => {
            const parentId = form.dataset.parent;
            if (!parentId) return;

            const textarea = form.querySelector('textarea');
            const text = textarea ? textarea.value : '';
            const replyState = {
                parentId,
                text,
                isPending: this.pendingSubmissions.has(parentId)
            };
            if (document.activeElement === textarea) {
                state.focused = { type: 'reply', parentId, start: textarea.selectionStart, end: textarea.selectionEnd };
            }
            state.openReplies.push(replyState);
        });

        return state;
    }

    restoreState(state) {
        if (!this.container) return;

        // 1. Restore open replies
        state.openReplies.forEach(({ parentId, text, isPending }) => {
            const commentBody = this.container.querySelector(`#c${parentId} > .comment-body`);
            if (commentBody && !commentBody.querySelector('.reply-input')) {
                const div = document.createElement('div');
                div.innerHTML = this.renderInput(parentId);
                commentBody.appendChild(div.firstElementChild);
                const newForm = commentBody.querySelector('.reply-input');
                if (newForm) {
                    const textarea = newForm.querySelector('textarea');
                    if (textarea) {
                        textarea.value = text;
                        if (isPending) textarea.disabled = true;
                    }
                    if (isPending) {
                        const submitBtn = newForm.querySelector('.submit-comment');
                        if (submitBtn) {
                            submitBtn.classList.add('loading');
                            submitBtn.disabled = true;
                        }
                    }
                    this.setupEmojiPicker(newForm);
                }
            }
        });

        // 2. Restore main input text & submitting state
        const mainInput = this.container.querySelector('.main-input textarea');
        if (mainInput) {
            mainInput.value = state.mainText;
            if (state.isMainSubmitting) {
                mainInput.disabled = true;
                const wrap = mainInput.closest('.comment-input');
                const submitBtn = wrap ? wrap.querySelector('.submit-comment') : null;
                if (submitBtn) {
                    submitBtn.classList.add('loading');
                    submitBtn.disabled = true;
                }
            }
        }

        // 3. Restore focus
        if (state.focused) {
            let targetEl = null;
            if (state.focused.type === 'main') {
                targetEl = this.container.querySelector('.main-input textarea');
            } else if (state.focused.type === 'reply') {
                targetEl = this.container.querySelector(`#c${state.focused.parentId} .reply-input textarea`);
            }

            if (targetEl && !targetEl.disabled) {
                targetEl.focus();
                // Ensure selection is restored after focus
                if (typeof targetEl.setSelectionRange === 'function') {
                    try {
                        targetEl.setSelectionRange(state.focused.start, state.focused.end);
                    } catch (e) {
                        // Fallback to end if range is invalid
                        const len = targetEl.value.length;
                        targetEl.setSelectionRange(len, len);
                    }
                }
            }
        }
    }

    _isSelfComment(data) {
        if (!data) return false;
        if (data.id && this._selfPostedCommentIds?.has(data.id)) return true;
        if (data.body && this._pendingSelfCommentTexts?.has(data.body)) return true;

        const session = window.f0ckSession || {};
        const currentUserId = session.id || session.user_id;
        if (currentUserId && data.user_id && parseInt(data.user_id, 10) === parseInt(currentUserId, 10)) {
            return true;
        }

        const currentUsername = session.user || this.user;
        if (currentUsername && data.username && currentUsername.toLowerCase() === data.username.toLowerCase()) {
            return true;
        }

        if (session.is_anon && data.is_anon) {
            if (data.anon_fingerprint && session.fingerprint && data.anon_fingerprint === session.fingerprint) {
                return true;
            }
            if (data.anon_short_fingerprint && session.fingerprint && session.fingerprint.includes(data.anon_short_fingerprint)) {
                return true;
            }
        }

        return false;
    }

    handleLiveComment(data) {
        if (!this.container || !this.itemId) return;
        // 1. Check if comment belongs to this item
        if (parseInt(data.item_id) !== parseInt(this.itemId)) return;

        // 2. Check for duplicates (if we just posted it ourselves via optimistic insert).
        //    Even on early return, ensure the button is present if body was truncated.
        if (document.getElementById('c' + data.id)) {
            const el = document.getElementById('c' + data.id);
            if (el && data.banner_file && data.banner_file !== 'null') {
                el.style.setProperty('--author-banner', `url('/a/${data.banner_file}')`);
                el.style.setProperty('--author-banner-position', data.banner_position === 'center' ? 'center top' : (data.banner_position || 'center top'));
                el.style.setProperty('--author-banner-size', (data.banner_size && data.banner_size !== 'cover') ? data.banner_size : '100% auto');
                el.style.setProperty('--author-banner-repeat', 'no-repeat');
            }
            if (data.body && data.body.endsWith('\u2026')) {
                console.log('[handleLiveComment] duplicate+truncated, ensuring button for', data.id);
                const contentEl = el?.querySelector('.comment-content');
                if (contentEl && !contentEl.querySelector('.load-full-comment-btn')) {
                    const btnLabel = (window.f0ckI18n?.sidebar_show_full_comment) || 'show full comment';
                    contentEl.insertAdjacentHTML('beforeend',
                        `<span class="item-comment-truncated-notice"><button class="load-full-comment-btn" type="button">${btnLabel}</button></span>`
                    );
                }
                this._patchLiveCommentContent(data.id);
            }
            return;
        }

        // 3. Robustness check: If we don't have lastData, initialize it.
        if (!this.lastData) {
            this.lastData = [];
        }

        // However, if we ARE currently loading comments from server, skip re-render.
        if (this.initialLoadDone === false && this.lastData.length === 0) {
            _f0ckDebug('[CommentSystem] Live comment skipped - initial comments load in progress.');
            return;
        }

        // 4. Construct comment object compatible with render
        const comment = {
            id: data.id,
            item_id: data.item_id,
            parent_id: data.parent_id || null,
            content: data.body,
            created_at: data.created_at,
            username: data.username,
            user_id: data.user_id,
            display_name: data.display_name || null,
            avatar: data.avatar,
            avatar_file: data.avatar_file,
            username_color: data.username_color,
            banner_file: data.banner_file || null,
            banner_position: data.banner_position || null,
            banner_size: data.banner_size || null,
            banner_repeat: data.banner_repeat || null,
            video_time: data.video_time ?? null,
            is_new: true,
            is_deleted: false,
            is_pinned: false
        };

        // Danmaku: fire one-shot for other users' comments.
        // Own comment is handled by the optimistic submit path (fire + addItem).
        const isSelf = this._isSelfComment(data);
        if (window.danmakuInstance && !isSelf) {
            window.danmakuInstance.fire(
                data.body,
                data.display_name || data.username || '?',
                data.username_color || null
            );
            window.danmakuInstance.addItem({
                id: data.id,
                content: data.body,
                video_time: data.video_time ?? null,
                display_name: data.display_name || data.username || '?',
                username_color: data.username_color || null
            });
        }

        // Update backlinks for live comment
        if (data.body) {
            const matches = data.body.matchAll(/(?<!\w)>>(\d+)/g);
            for (const match of matches) {
                this.updateCommentBacklinks(match[1], data.id || data.comment_id);
            }
        }
        if (data.parent_id) {
            this.updateCommentBacklinks(data.parent_id, data.id || data.comment_id);
        }

        if (!this.lastData) this.lastData = [];

        // 4. Save state before re-render
        const state = this.saveState();

        if (this.sort === 'new') {
            this.lastData.unshift(comment); // Add to top
        } else {
            this.lastData.push(comment); // Add to bottom
        }

        // Reconciliation for live comments preserves media state
        this.reconcile(this.lastData, this.lastUserId, this.lastIsSubscribed);
        this.restoreState(state);

        // 6. Highlight and animate
        setTimeout(() => {
            const el = document.getElementById('c' + comment.id);
            CommentSystem._animateNewComment(el, false);
        }, 100);

        // 7. The NOTIFY body is capped at 500 chars to stay under PostgreSQL's 8KB limit.
        //    If the body was truncated, immediately show the "show full comment" button
        //    and then fetch + patch the real full content asynchronously.
        if (data.body && data.body.endsWith('\u2026')) {
            console.log('[handleLiveComment] body truncated, patching comment', data.id);
            const el = document.getElementById('c' + data.id);
            if (el) {
                const contentEl = el.querySelector('.comment-content');
                if (contentEl && !contentEl.querySelector('.load-full-comment-btn')) {
                    const btnLabel = (window.f0ckI18n?.sidebar_show_full_comment) || 'show full comment';
                    contentEl.insertAdjacentHTML('beforeend',
                        `<span class="item-comment-truncated-notice"><button class="load-full-comment-btn" type="button">${btnLabel}</button></span>`
                    );
                }
            }
            this._patchLiveCommentContent(data.id);
        }
    }

    async _patchLiveCommentContent(commentId) {
        try {
            const res = await fetch(`/api/comment/${commentId}`);
            if (!res.ok) return;
            const json = await res.json();
            if (!json.success || !json.comment) return;

            const fullContent = json.comment.content;
            const el = document.getElementById('c' + commentId);
            if (!el) return;

            const contentEl = el.querySelector('.comment-content');
            if (!contentEl) return;

            // Update in-memory data so future reconciles use the full content
            if (this.lastData) {
                const cached = this.lastData.find(c => String(c.id) === String(commentId));
                if (cached) {
                    cached.content = fullContent;
                    if (json.comment.banner_file) cached.banner_file = json.comment.banner_file;
                    if (json.comment.banner_position) cached.banner_position = json.comment.banner_position;
                    if (json.comment.banner_size) cached.banner_size = json.comment.banner_size;
                    if (json.comment.banner_repeat) cached.banner_repeat = json.comment.banner_repeat;
                }
            }

            if (json.comment && json.comment.banner_file) {
                el.style.setProperty('--author-banner', `url('/a/${json.comment.banner_file}')`);
                el.style.setProperty('--author-banner-position', json.comment.banner_position === 'center' ? 'center top' : (json.comment.banner_position || 'center top'));
                el.style.setProperty('--author-banner-size', (json.comment.banner_size && json.comment.banner_size !== 'cover') ? json.comment.banner_size : '100% auto');
                el.style.setProperty('--author-banner-repeat', 'no-repeat');
            }

            contentEl.dataset.raw = fullContent;
            contentEl.innerHTML = this.renderCommentContent(fullContent, commentId);
            CommentSystem.autoplayConvertedGifs(contentEl);
            CommentSystem.playEmojiVideos(contentEl);
        } catch (e) {
            _f0ckDebug('[CommentSystem] _patchLiveCommentContent failed:', e);
        }
    }

    handleLiveEdit(data) {
        if (data.item_id != this.itemId) return;

        // Update local cache
        if (this.lastData) {
            const comment = this.lastData.find(c => String(c.id) === String(data.comment_id));
            if (comment) {
                comment.content = data.content;
                comment.updated_at = new Date().toISOString();
            }
        }

        const el = document.getElementById('c' + data.comment_id);
        if (!el) return;

        const contentEl = el.querySelector('.comment-content');
        if (contentEl) {
            contentEl.dataset.raw = data.content;
            contentEl.innerHTML = this.renderCommentContent(data.content, data.comment_id);
            CommentSystem.autoplayConvertedGifs(contentEl);
            CommentSystem.playEmojiVideos(contentEl);

            // Flash effect to draw attention
            CommentSystem._animateNewComment(el, false);

            // Update admin edit button data attribute
            const editBtn = el.querySelector('.admin-edit-btn');
            if (editBtn) {
                editBtn.dataset.content = this.escapeHtml(data.content);
            }
        }
    }

    handleLiveDeletion(data) {
        if (!this.container || !this.itemId) return;
        if (parseInt(data.item_id) !== parseInt(this.itemId)) return;

        // Update local cache
        if (this.lastData) {
            const comment = this.lastData.find(c => String(c.id) === String(data.comment_id));
            if (comment) {
                comment.is_deleted = true;
                comment.content = '[deleted]';
            }
        }

        // Surgical update of the DOM to preserve other comments' media
        const el = this.container.querySelector('#c' + data.comment_id);
        if (el) {
            el.classList.add('deleted');
            const contentEl = el.querySelector('.comment-content');
            if (contentEl) {
                contentEl.innerHTML = '<span class="deleted-msg">[deleted]</span>';
                contentEl.dataset.raw = '[deleted]';
            }
            // Also remove admin buttons and reply button
            el.querySelectorAll('.admin-pin-btn, .admin-edit-btn, .admin-delete-btn, .reply-btn').forEach(b => b.remove());
        }
    }

    async loadComments(scrollToId = null, preserveScroll = false) {
        if (!this.container) return;

        // Preserve state before re-render
        const state = this.saveState();

        /* Guest handling is managed by the template and server-side routes.
           If we are here and there is no user, it means hide_comments_from_public is false.
           We should still allow loading comments for viewing. */
        if (!this.user) {
            // Check if we have preloaded data first
            const dataEl = document.getElementById('initial-comments');
            if (!dataEl) {
                // If no preloaded data and no user, we might be in a state where we shouldn't be
                // but let's let it proceed to fetch if container exists.
                // Actually, let's just make sure we don't return early if it's a guest.
            }
        }

        // Check for server-side preloaded comments
        const dataEl = document.getElementById('initial-comments');
        const subEl = document.getElementById('initial-subscription');
        const initialIsSubscribed = subEl ? (subEl.textContent.trim() === 'true') : false;

        if (dataEl) {
            try {
                // Decode Base64 for safe template transfer
                const raw = dataEl.textContent.trim();
                const json = decodeURIComponent(escape(atob(raw)));
                const comments = JSON.parse(json);

                // Consume
                dataEl.remove();
                if (subEl) subEl.remove();

                this.render(comments, this.user, initialIsSubscribed);
                this.initialLoadDone = true;
                this.restoreState(state);
                this._loadDanmaku(comments);

                if (scrollToId) {
                    this._anchorScrollDone = true;
                    this.scrollToComment(scrollToId);
                } else if (!this._anchorScrollDone && !preserveScroll && window.location.hash && window.location.hash.startsWith('#c')) {
                    const hashId = window.location.hash.substring(2);
                    this._anchorScrollDone = true;
                    this.scrollToComment(hashId);
                }
                return;
            } catch (e) {
                console.error("SSR comments parse error", e);
            }
        }

        // Render skeleton only if the fetch is slow. Rendering it immediately made every item switch flash an
        // empty list that the real comments then replaced a moment later.
        // Skip when preserveScroll=true (tab re-focus refresh): the user already sees comments,
        // so wiping the DOM causes the browser to lose the #c anchor element and auto-scroll to top.
        let skeletonTimer = null;
        if (!scrollToId && !preserveScroll) {
            if (this.user || !this.container.querySelector('.comment-input, .lock-notice')) {
                skeletonTimer = setTimeout(() => {
                    skeletonTimer = null;
                    this.render([], this.user, initialIsSubscribed);
                    this.restoreState(state);
                }, 250);
            }
        }
        const cancelSkeleton = () => { if (skeletonTimer) { clearTimeout(skeletonTimer); skeletonTimer = null; } };

        try {
            const res = await fetch(`/api/comments/${this.itemId}?sort=${this.sort}`);
            cancelSkeleton();

            // If server is restarting (502/503), res.ok will be false.
            if (!res.ok) throw new Error(`Server returned ${res.status}`);

            const data = await res.json();

            if (data.success) {
                if (data.require_login) {
                    this.container.innerHTML = '';
                    this.container.style.display = 'none';
                    return;
                }

                this.isAdmin = data.is_admin || false;
                this.isLocked = data.is_locked || false;

                // Snapshot scroll position before DOM replacement (used when preserving position)
                const savedScrollY = preserveScroll ? window.scrollY : null;

                // Signal to render() and any async media/emoji handlers to NOT scroll to anchor.
                if (preserveScroll) {
                    this.preservingScroll = true;
                    this.stopStabilization();
                }

                const savedHash = (preserveScroll && window.location.hash.startsWith('#c'))
                    ? window.location.hash
                    : null;

                // 1. Early Bail-out: If data is bit-for-bit identical, do nothing.
                // This is the primary defense against tab-switch reloads when nothing changed.
                if (preserveScroll && this._isDeeplyIdentical(data.comments, data.user_id, data.is_subscribed)) {
                    _f0ckDebug('[CommentSystem] Sync: Data identical, bailing early to protect media.');
                    this.restoreState(state);
                    this.preservingScroll = false;
                    return;
                }

                // 2. Reconciliation: If data changed but we want to preserve media.
                if (preserveScroll && this.lastData && this.lastData.length > 0) {
                    _f0ckDebug('[CommentSystem] Sync: Data changed, reconciling DOM.');
                    this.reconcile(data.comments, data.user_id, data.is_subscribed);
                    this.initialLoadDone = true;
                    this.restoreState(state);

                    if (scrollToId) {
                        this.preservingScroll = false;
                        this.scrollToComment(scrollToId);
                    } else if (preserveScroll && savedScrollY !== null) {
                        requestAnimationFrame(() => {
                            window.scrollTo({ top: savedScrollY, behavior: 'instant' });
                            if (savedHash) {
                                history.replaceState(null, '', window.location.pathname + window.location.search + savedHash);
                                const hashId = savedHash.substring(2);
                                const el = document.getElementById(`c${hashId}`);
                                if (el) el.classList.add('comment-highlighted');
                            }
                            setTimeout(() => { this.preservingScroll = false; }, 500);
                        });
                    } else {
                        this.preservingScroll = false;
                    }
                    return;
                }

                if (savedHash) {
                    history.replaceState(null, '', window.location.pathname + window.location.search + savedHash);
                }

                // Render real data
                this.render(data.comments, data.user_id, data.is_subscribed);
                this.initialLoadDone = true;
                this.restoreState(state);
                this._loadDanmaku(data.comments);

                if (scrollToId) {
                    this.preservingScroll = false;
                    if (savedHash) history.replaceState(null, '', window.location.pathname + window.location.search + savedHash);
                    this.scrollToComment(scrollToId);
                } else if (preserveScroll && savedScrollY !== null) {
                    // Restore scroll position without animation to avoid visible jump.
                    // Use rAF so the browser has painted the new DOM before we override its scroll.
                    requestAnimationFrame(() => {
                        window.scrollTo({ top: savedScrollY, behavior: 'instant' });
                        // Silently restore the hash — history.replaceState does NOT trigger a scroll
                        if (savedHash) {
                            history.replaceState(null, '', window.location.pathname + window.location.search + savedHash);
                            // Re-apply the highlight class without scrolling
                            const hashId = savedHash.substring(2);
                            const el = document.getElementById(`c${hashId}`);
                            if (el) el.classList.add('comment-highlighted');
                        }
                        // Keep the flag active briefly to suppress any deferred media-load scrolls
                        setTimeout(() => { this.preservingScroll = false; }, 500);
                    });
                } else {
                    this.preservingScroll = false;
                    if (savedHash) history.replaceState(null, '', window.location.pathname + window.location.search + savedHash);
                    // Only jump to anchor on first load — never on tab re-focus (preserveScroll).
                    if (!this._anchorScrollDone && !preserveScroll && window.location.hash && window.location.hash.startsWith('#c')) {
                        const hashId = window.location.hash.substring(2);
                        this._anchorScrollDone = true;
                        this.scrollToComment(hashId);
                    }
                }
            } else {
                console.warn('[CommentSystem] Failed to load comments:', data.message);
                if (!this.initialLoadDone) {
                    this.container.innerHTML = `<div class="error">Failed to load comments: ${data.message}</div>`;
                }
            }
        } catch (e) {
            cancelSkeleton();
            console.error('[CommentSystem] Error loading comments:', e);
            // Catch-all for network errors, 502s, JSON parse errors (e.g. server restart)
            // If initial load already finished (SSR or first fetch), just keep existing comments on screen.
            if (!this.initialLoadDone) {
                this.container.innerHTML = `<div class="error">Error loading comments. Please try again later.</div>`;
            }
        }
    }

    // ...

    /**
     * Attach animation classes to a comment element and automatically remove them
     * once the animation finishes, so the element's natural border (including
     * border-right) is fully restored after the effect completes.
     *
     * @param {Element} el       - The .comment element to animate.
     * @param {boolean} entering - Unused (kept for compatibility).
     */
    static _animateNewComment(el, entering = false) {
        if (!el) return;
        el.classList.add('new-item-fade');
        setTimeout(() => {
            el.classList.remove('new-item-fade');
        }, 2500);
    }



    scrollToComment(id, retries = 5, isStabilizing = false) {
        if (this.isUserInteracting && isStabilizing) return;
        // Never scroll during a tab-refocus restore — preservingScroll is set
        // synchronously in the visibilitychange handler before the async fetch.
        if (this.preservingScroll) return;

        const attempt = () => {
            const el = document.getElementById(`c${id}`);
            if (el) {
                // Remove highlight from any previously highlighted or newly-posted comment
                document.querySelectorAll('.comment-highlighted').forEach(c => c.classList.remove('comment-highlighted'));
                document.querySelectorAll('.new-item-fade').forEach(c => c.classList.remove('new-item-fade'));

                // Always smooth-scroll so there's no jarring jump
                el.scrollIntoView({ behavior: 'smooth', block: 'center' });
                el.classList.add('comment-highlighted');

                // If not already stabilizing, start a short-lived stabilization phase
                if (!isStabilizing) {
                    this.startStabilization(id);
                }
            } else if (retries > 0) {
                _f0ckDebug(`[CommentSystem] Scroll target #c${id} not found, retrying... (${retries} left)`);
                setTimeout(() => this.scrollToComment(id, retries - 1), 200);
            }
        };

        // Delay to allow for layout stabilization (e.g. images, other comments)
        // Shorter delay for stabilization calls
        setTimeout(attempt, isStabilizing ? 50 : 300);
    }

    startStabilization(id) {
        this.stopStabilization(); // Clear any existing
        this.isUserInteracting = false;

        const el = document.getElementById(`c${id}`);
        if (!el) return;

        // Interaction listeners to abort stabilization
        this.boundStopStabilization = (e) => {
            // Ignore small jitter, but stop on significant interaction
            if (e.type === 'keydown') {
                const scrollKeys = ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', ' ', 'Home', 'End'];
                if (!scrollKeys.includes(e.key)) return;
            }
            _f0ckDebug(`[CommentSystem] Stabilization aborted due to ${e.type}`);
            this.isUserInteracting = true;
            this.stopStabilization();
        };

        ['wheel', 'touchmove', 'mousedown', 'keydown'].forEach(evt => {
            window.addEventListener(evt, this.boundStopStabilization, { passive: true });
        });

        let lastTop = el.getBoundingClientRect().top;
        let checks = 0;
        const maxChecks = 20; // ~4 seconds at 200ms

        const checkShift = () => {
            if (this.isUserInteracting || this.preservingScroll) return;
            const currentEl = document.getElementById(`c${id}`);
            if (!currentEl) return;

            const currentTop = currentEl.getBoundingClientRect().top;
            const diff = Math.abs(currentTop - lastTop);

            // If it shifted more than 10px (e.g. media loaded), re-scroll
            if (diff > 10 && checks < maxChecks) {
                _f0ckDebug(`[CommentSystem] Layout shift detected (${Math.round(diff)}px), re-stabilizing scroll...`);
                this.scrollToComment(id, 0, true);
                lastTop = currentEl.getBoundingClientRect().top;
            }

            checks++;
            if (checks < maxChecks) {
                this.stabilizationTimer = setTimeout(checkShift, 200);
            }
        };

        this.stabilizationTimer = setTimeout(checkShift, 200);

        // Also use ResizeObserver for immediate reaction to content loading
        if (window.ResizeObserver) {
            this.stabilizationObserver = new ResizeObserver(() => {
                if (this.preservingScroll) return; // Don't fight scroll-position restoration
                if (this.scrollDebounce) clearTimeout(this.scrollDebounce);
                this.scrollDebounce = setTimeout(() => {
                    if (this.preservingScroll) return;
                    const rect = el.getBoundingClientRect();
                    // If it's no longer near the center (roughly), re-center
                    const center = window.innerHeight / 2;
                    if (Math.abs(rect.top + rect.height / 2 - center) > 100) {
                        this.scrollToComment(id, 0, true);
                    }
                }, 100);
            });
            this.stabilizationObserver.observe(this.container);

            // Disconnect after 5 seconds to save resources
            setTimeout(() => this.stopStabilization(), 5000);
        }
    }

    stopStabilization() {
        if (this.stabilizationTimer) clearTimeout(this.stabilizationTimer);
        if (this.stabilizationObserver) this.stabilizationObserver.disconnect();
        this.stabilizationTimer = null;
        this.stabilizationObserver = null;

        if (this.boundStopStabilization) {
            ['wheel', 'touchmove', 'mousedown', 'keydown'].forEach(evt => {
                window.removeEventListener(evt, this.boundStopStabilization);
            });
            this.boundStopStabilization = null;
        }
    }

    async showCommentPreview(link, event) {
        if (this.previewCloseTimer) {
            clearTimeout(this.previewCloseTimer);
            this.previewCloseTimer = null;
        }

        const targetId = link.dataset.id;
        let targetEl = document.getElementById('c' + targetId);

        const parentPopup = link.closest('.comment-preview-popup');
        const level = parentPopup ? parseInt(parentPopup.dataset.level || 0) + 1 : 0;

        if (!targetEl) {
            const cached = this.commentCache.get(targetId);
            if (cached) {
                targetEl = this.createTemporaryCommentNode(cached);
            } else {
                // Fetch from API
                this.fetchCommentForPreview(targetId, link, level);
                return;
            }
        }

        // Close any previews at this level or higher (replacing the current branch)
        this.closePreviewsAboveLevel(level - 1);

        // If this exact ID is already the immediate child of this parent, don't re-open
        const existing = document.querySelector(`.comment-preview-popup[data-id="${targetId}"][data-level="${level}"]`);
        if (existing) return;

        // Clone the target comment
        const preview = targetEl.cloneNode(true);
        preview.id = 'comment-preview-' + Date.now();
        preview.classList.add('comment-preview-popup');
        preview.dataset.level = level;
        preview.dataset.id = targetId;

        // Remove temporary animation/highlight classes
        preview.classList.remove('new-item-fade', 'comment-highlighted');

        // Remove input forms or action buttons
        const actions = preview.querySelector('.comment-actions');
        if (actions) actions.remove();
        const footer = preview.querySelector('.comment-footer');
        if (footer) footer.remove();

        // Position the preview
        const rect = link.getBoundingClientRect();
        preview.style.position = 'fixed';
        preview.style.zIndex = (100000 + level).toString();

        document.body.appendChild(preview);
        CommentSystem.playEmojiVideos(preview);

        // If the emoji cache isn't ready yet but this preview contains :emoji: shortcodes,
        // trigger a load and re-render just the preview's content once the cache arrives.
        const previewContentEl = preview.querySelector('.comment-content');
        if (previewContentEl && !CommentSystem.emojiCache && /:([a-z0-9_]+):/.test(previewContentEl.textContent)) {
            const rerender = () => {
                if (!document.body.contains(preview)) return;
                const raw = previewContentEl.dataset.raw;
                if (raw) {
                    previewContentEl.innerHTML = this.renderCommentContent(raw);
                    CommentSystem.playEmojiVideos(previewContentEl);
                }
            };
            if (!CommentSystem.loadingEmojis) {
                this.loadEmojis(true).then(rerender);
            } else {
                window.addEventListener('f0ck:emojis_ready', rerender, { once: true });
            }
        }

        const previewRect = preview.getBoundingClientRect();

        // Default: position preview to the right of the link
        let left = rect.right + 10;
        let top = rect.top;

        // If there's NOT enough space on the right, open to the left
        if (left + previewRect.width > window.innerWidth - 10) {
            left = rect.left - previewRect.width - 10;
        }

        // Clamp to screen boundaries if left/right are both tight
        if (left < 10) {
            left = 10;
        }
        if (left + previewRect.width > window.innerWidth - 10) {
            left = Math.max(10, window.innerWidth - previewRect.width - 10);
        }

        // Clamp top & bottom boundary
        if (top + previewRect.height > window.innerHeight - 10) {
            top = window.innerHeight - previewRect.height - 10;
        }
        if (top < 10) top = 10;

        preview.style.left = left + 'px';
        preview.style.top = top + 'px';
    }

    async fetchCommentForPreview(id, link, level) {
        if (this.commentCache.has(id)) return; // Already fetching or fetched

        // Prevent double fetches
        this.commentCache.set(id, { loading: true });

        try {
            const res = await fetch(`/api/comment/${id}`);
            const json = await res.json();
            if (json.success && json.comment) {
                this.commentCache.set(id, json.comment);
                // If the link is still being hovered (or we are in the middle of a dwell), show it
                if (this.currentHoverLink === link) {
                    this.showCommentPreview(link);
                }
            } else {
                this.commentCache.delete(id);
            }
        } catch (e) {
            console.error('[CommentSystem] Failed to fetch comment for preview:', e);
            this.commentCache.delete(id);
        }
    }

    createTemporaryCommentNode(comment) {
        if (comment.loading) {
            const div = document.createElement('div');
            div.className = 'comment loading-comment';
            div.innerHTML = '<div class="comment-body"><i class="fa-solid fa-spinner fa-spin"></i> Loading...</div>';
            return div;
        }
        const html = this.renderComment(comment, window.f0ckSession?.user, false, true);
        const div = document.createElement('div');
        div.innerHTML = html;
        return div.firstElementChild;
    }

    closePreviewsAboveLevel(level) {
        document.querySelectorAll('.comment-preview-popup').forEach(p => {
            if (parseInt(p.dataset.level || 0) > level) {
                p.remove();
            }
        });
    }

    hideCommentPreview(force = false) {
        if (force) {
            this.closePreviewsAboveLevel(-1);
            return;
        }

        if (this.previewCloseTimer) clearTimeout(this.previewCloseTimer);
        this.previewCloseTimer = setTimeout(() => {
            this.closePreviewsAboveLevel(-1);
        }, 400); // 400ms grace period to move mouse between link and popup
    }

    quoteComment(id, openerEl, body) {
        if (!window.f0ckSession || !window.f0ckSession.logged_in) return;
        // If no reply input open anywhere, open the local one
        let textarea = document.querySelector('.comment-input.reply-input textarea');
        let isNew = false;
        if (!textarea && !body.querySelector('.reply-input')) {
            const div = document.createElement('div');
            div.innerHTML = this.renderInput(id);
            body.appendChild(div.firstElementChild);
            const newForm = body.querySelector('.reply-input');
            this.setupEmojiPicker(newForm);
            textarea = newForm.querySelector('textarea');
            newForm.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
            isNew = true;
        } else if (!textarea) {
            textarea = body.querySelector('.reply-input textarea');
        }

        if (textarea) {
            const author = openerEl.dataset.display || openerEl.dataset.username || 'System';
            const contentEl = body.querySelector('.comment-content');
            if (contentEl) {
                let rawText = (contentEl.dataset.raw || '').trim();
                if (rawText.includes('&gt;') || rawText.includes('&lt;') || rawText.includes('&amp;')) {
                    const txt = document.createElement('textarea');
                    txt.innerHTML = rawText;
                    rawText = txt.value;
                }
                const lines = rawText.split('\n');
                const quote = `>>${id} \n>${author}\n${lines.map(line => `>${line}`).join('\n')}\n`;

                if (isNew) {
                    textarea.value = quote;
                } else {
                    const start = textarea.selectionStart;
                    const end = textarea.selectionEnd;
                    const val = textarea.value;
                    textarea.value = val.substring(0, start) + quote + val.substring(end);
                }
                textarea.focus({ preventScroll: true });
                textarea.selectionStart = textarea.selectionEnd = textarea.value.length;
                textarea.dispatchEvent(new Event('input', { bubbles: true }));
                this._scrollReplyIntoView(textarea);
            }
        }
    }

    _scrollReplyIntoView(textarea) {
        const form = textarea.closest('.reply-input');
        if (!form) return;

        // Smooth scroll immediately
        form.scrollIntoView({ behavior: 'smooth', block: 'nearest' });

        // On mobile, the virtual keyboard takes a moment to appear and resizes the viewport.
        // Doing a delayed scroll guarantees it is centered and fully visible within the resized viewport.
        if (window.innerWidth <= 768) {
            setTimeout(() => {
                form.scrollIntoView({ behavior: 'smooth', block: 'center' });
            }, 300);
        }
    }


    saveMediaState() {
        // Snapshot currently playing or paused-mid-way media elements
        const state = [];
        if (!this.container) return state;
        this.container.querySelectorAll('video, audio').forEach(el => {
            if (!el.paused || el.currentTime > 0) {
                state.push({
                    src: el.src || el.currentSrc,
                    currentTime: el.currentTime,
                    paused: el.paused,
                    muted: el.muted,
                    volume: el.volume
                });
            }
        });
        return state;
    }

    restoreMediaState(state) {
        if (!state || state.length === 0 || !this.container) return;
        state.forEach(snap => {
            const el = this.container.querySelector(`video[src="${snap.src}"], audio[src="${snap.src}"]`);
            if (!el) return;
            el.currentTime = snap.currentTime;
            el.muted = snap.muted;
            el.volume = snap.volume;
            if (!snap.paused) {
                el.play().catch(() => {/* autoplay may be blocked */ });
            }
        });
    }

    render(comments, currentUserId, isSubscribed) {
        // Store for re-rendering when emojis load
        this.lastData = comments;
        this.lastUserId = currentUserId;
        this.lastIsSubscribed = isSubscribed;

        // Build map of who replied to whom for back-references (>>ID)
        this.buildBacklinkMap(comments);

        // Render logic based on display mode
        let renderedHtml = '';
        if (this.displayMode === 1) {
            // Linear Mode: Just sort all comments by date and render them flat
            const sortedComments = [...comments].sort((a, b) => {
                if (this.sort === 'old') return new Date(a.created_at) - new Date(b.created_at);
                return new Date(b.created_at) - new Date(a.created_at);
            });
            renderedHtml = sortedComments.map(c => this.renderComment(c, currentUserId, false, true)).join('');
        } else {
            // Tree Mode: Group by roots and replies
            const map = new Map();
            const roots = [];

            comments.forEach(c => {
                c.replies = [];
                c.replyTo = null; // Username being replied to (for @mentions)
                map.set(c.id, c);
            });

            // Find root parent for any comment
            const findRoot = (comment) => {
                if (!comment.parent_id) return null;
                let current = comment;
                while (current.parent_id && map.has(current.parent_id)) {
                    current = map.get(current.parent_id);
                }
                return current;
            };

            comments.forEach(c => {
                if (!c.parent_id) {
                    roots.push(c);
                } else {
                    const root = findRoot(c);
                    if (root && root !== c) {
                        const directParent = map.get(c.parent_id);
                        if (directParent && directParent.id !== root.id) {
                            c.replyTo = directParent.username;
                        }
                        root.replies.push(c);
                    } else {
                        roots.push(c);
                    }
                }
            });

            // Sort replies by date (oldest first)
            roots.forEach(r => {
                if (r.replies && r.replies.length > 0) {
                    r.replies.sort((a, b) => new Date(a.created_at) - new Date(b.created_at));
                }
            });

            renderedHtml = roots.map(c => this.renderComment(c, currentUserId)).join('');
        }

        // Determine what to show for input
        let inputSection = '';
        if (this.isLocked && !this.isAdmin) {
            inputSection = '<div class="lock-notice">🔒 Comments are disabled on this thread.</div>';
        } else if (currentUserId) {
            inputSection = this.renderInput();
        } else {
            // Guests can read but not comment: an inert look-alike keeps the layout identical
            inputSection = this.renderGuestInput();
        }

        const isLegacy = document.body.classList.contains('layout-legacy') || document.body.classList.contains('legacy-view');
        let scrollToBottomBtn = '';
        if (isLegacy) {
            scrollToBottomBtn = `
                <div class="scroll-nav-wrapper">
                    <div class="scroll-to-bottom" title="Scroll to bottom">
                        <i class="fa-solid fa-arrow-down icon-down"></i>
                        <i class="fa-solid fa-arrow-up icon-up"></i>
                    </div>
                </div>
            `;
        }

        let html = '';
        if (isLegacy) {
            html = `
                <div class="comments-list ${this.displayMode === 1 ? 'linear-view' : ''}">
                    ${scrollToBottomBtn}
                    ${renderedHtml}
                </div>
                ${inputSection}
            `;
        } else {
            html = `
                ${inputSection}
                <div class="comments-list ${this.displayMode === 1 ? 'linear-view' : ''}">
                    ${scrollToBottomBtn}
                    ${renderedHtml}
                </div>
            `;
        }


        // Save xd-score-wrapper before innerHTML wipe.
        // On first load it lives as a previous sibling (server-rendered); afterwards inside.
        let xdWrapperHtml = null;
        const xdInside = this.container.querySelector('.xd-score-wrapper');
        if (xdInside) {
            xdWrapperHtml = xdInside.outerHTML;
        } else {
            const prev = this.container.previousElementSibling;
            if (prev && prev.classList.contains('xd-score-wrapper')) {
                xdWrapperHtml = prev.outerHTML;
                prev.remove();
            }
        }

        const mediaState = this.saveMediaState();
        this.container.innerHTML = html;
        this.restoreMediaState(mediaState);

        // Re-inject xd-score-wrapper between the comment input and the comments list
        if (xdWrapperHtml) {
            const commentsList = this.container.querySelector('.comments-list');
            if (commentsList) {
                const tmp = document.createElement('div');
                tmp.innerHTML = xdWrapperHtml;
                const xdEl = tmp.firstElementChild;
                if (xdEl) this.container.insertBefore(xdEl, commentsList);
            }
        }
        this.syncSubscribeButton(isSubscribed);
        CommentSystem.autoplayConvertedGifs(this.container);
        CommentSystem.playEmojiVideos(this.container);

        // Attach media load listeners to re-stabilize scroll if a hash is active.
        // Only during the initial anchor scroll — never on subsequent renders (tab re-focus,
        // emoji reloads, live comment updates). _anchorScrollDone is set after the first
        // scrollToComment call, so this block only runs on the initial page load.
        if (!this._anchorScrollDone && !this.preservingScroll && window.location.hash && window.location.hash.startsWith('#c')) {
            const hashId = window.location.hash.substring(2);
            this.container.querySelectorAll('img, video, audio').forEach(media => {
                const handler = () => {
                    if (this.preservingScroll) return; // still in suppress window
                    if (this.scrollDebounce) clearTimeout(this.scrollDebounce);
                    this.scrollDebounce = setTimeout(() => this.scrollToComment(hashId, 0, true), 100);
                    media.removeEventListener('load', handler);
                    media.removeEventListener('loadedmetadata', handler);
                };
                media.addEventListener('load', handler);
                media.addEventListener('loadedmetadata', handler);
            });
        }

        const mainInput = this.container.querySelector('.main-input:not(.is-guest)');
        if (mainInput) this.setupEmojiPicker(mainInput);

        // Lazy emoji load: scan the just-rendered comment text for :emoji: patterns.
        // Only fires an API request if at least one shortcode is actually present,
        // avoiding a full round-trip on pages with no custom emojis.
        if (!CommentSystem.emojiCache && !CommentSystem.loadingEmojis) {
            this.loadEmojis(); // will skip internally if no patterns found
        }
    }

    syncSubscribeButton(isSubscribed) {
        this.lastIsSubscribed = isSubscribed;
        document.querySelectorAll('#subscribe-btn').forEach(btn => {
            btn.classList.toggle('active', isSubscribed);
            btn.setAttribute('title', isSubscribed ? 'Subscribed' : 'Subscribe');

            // Update text if it exists (legacy support)
            const textNodes = Array.from(btn.childNodes).filter(node => node.nodeType === Node.TEXT_NODE);
            if (textNodes.length > 0) {
                textNodes[0].textContent = isSubscribed ? 'Subscribed' : 'Subscribe';
            }

            // FA icon: toggle fa-solid / fa-regular
            btn.classList.toggle('fa-solid', isSubscribed);
            btn.classList.toggle('fa-regular', !isSubscribed);
        });
    }

    _isDeeplyIdentical(newComments, currentUserId, isSubscribed) {
        if (!this.lastData) return false;

        // Use lax comparison for user IDs as they might flip between string/int
        const userMatches = (this.lastUserId == currentUserId);
        if (!userMatches) return false;

        if (this.lastIsSubscribed !== isSubscribed) return false;
        if (this.lastData.length !== newComments.length) return false;

        for (let i = 0; i < newComments.length; i++) {
            const a = this.lastData[i];
            const b = newComments[i];

            // Compare essential data that affects the UI
            if (a.id !== b.id ||
                a.content !== b.content ||
                a.is_deleted !== b.is_deleted ||
                a.is_pinned !== b.is_pinned ||
                a.display_name !== b.display_name) {
                return false;
            }
        }
        return true;
    }

    /**
     * DOM Reconciliation: Synchronizes current thread state with fresh data
     * without replacing the entire container, protecting active media (YouTube).
     */
    reconcile(comments, currentUserId, isSubscribed) {
        if (!this.container) return;
        const list = this.container.querySelector('.comments-list');
        if (!list) return this.render(comments, currentUserId, isSubscribed);

        // Update metadata
        this.syncSubscribeButton(isSubscribed);
        this.lastData = comments;
        this.lastUserId = currentUserId;
        this.lastIsSubscribed = isSubscribed;

        // Build map of who replied to whom for back-references (>>ID)
        this.buildBacklinkMap(comments);

        const incomingMap = new Map();
        comments.forEach(c => incomingMap.set(String(c.id), c));

        // 1. Identify missing or edited comments already in DOM
        const existingEls = list.querySelectorAll('[id^="c"]');
        const existingIds = new Set();

        existingEls.forEach(el => {
            const id = el.id.substring(1);
            existingIds.add(id);
            const incoming = incomingMap.get(id);

            if (!incoming) {
                // Comment was deleted/removed from server
                el.classList.add('deleted');
                const contentEl = el.querySelector('.comment-content');
                if (contentEl) contentEl.innerHTML = '<span class="deleted-msg">[deleted]</span>';
                const actions = el.querySelector('.comment-actions');
                if (actions) actions.style.display = 'none';
            } else {
                // Check for edits or state changes using robust data-attributes
                const contentEl = el.querySelector('.comment-content');
                if (contentEl && contentEl.dataset.raw !== incoming.content) {
                    _f0ckDebug(`[CommentSystem] Reconcile: Updating content for #c${id}`);
                    contentEl.innerHTML = this.renderCommentContent(incoming.content, incoming.id);
                    CommentSystem.autoplayConvertedGifs(contentEl);
                    CommentSystem.playEmojiVideos(contentEl);
                    contentEl.dataset.raw = incoming.content;
                }

                // Update pinned status
                el.classList.toggle('pinned', !!incoming.is_pinned);
                const pinIcon = el.querySelector('.pin-icon');
                if (pinIcon) pinIcon.style.display = incoming.is_pinned ? 'block' : 'none';

                if (incoming && incoming.banner_file && incoming.banner_file !== 'null') {
                    el.style.setProperty('--author-banner', `url('/a/${incoming.banner_file}')`);
                    el.style.setProperty('--author-banner-position', incoming.banner_position === 'center' ? 'center top' : (incoming.banner_position || 'center top'));
                    el.style.setProperty('--author-banner-size', (incoming.banner_size && incoming.banner_size !== 'cover') ? incoming.banner_size : '100% auto');
                    el.style.setProperty('--author-banner-repeat', 'no-repeat');
                }
            }
        });

        // 2. Insert new comments
        if (this.displayMode === 1) {
            // Linear Mode Insertion
            const sortedComments = [...comments].sort((a, b) => {
                if (this.sort === 'old') return new Date(a.created_at) - new Date(b.created_at);
                return new Date(b.created_at) - new Date(a.created_at);
            });

            sortedComments.forEach((c, idx) => {
                const idStr = String(c.id);
                if (document.getElementById('c' + idStr)) return;

                _f0ckDebug(`[CommentSystem] Reconcile: Injecting new flat comment #c${idStr}`);
                const html = this.renderComment(c, currentUserId, false, true);
                const tmp = document.createElement('div');
                tmp.innerHTML = html;
                const newEl = tmp.firstElementChild;

                if (idx === 0) {
                    const nav = list.querySelector('.scroll-nav-wrapper');
                    if (nav) nav.insertAdjacentElement('afterend', newEl);
                    else list.prepend(newEl);
                } else {
                    const prevId = String(sortedComments[idx - 1].id);
                    const prevEl = document.getElementById('c' + prevId);
                    if (prevEl) prevEl.insertAdjacentElement('afterend', newEl);
                    else list.appendChild(newEl);
                }
                CommentSystem._animateNewComment(newEl, true);
            });
            return;
        }

        // Tree Mode Insertion (Existing Logic)
        const map = new Map();
        const roots = [];
        comments.forEach(c => {
            c.replies = [];
            c.replyTo = null;
            map.set(c.id, c);
        });

        const findRoot = (comment) => {
            if (!comment.parent_id) return null;
            let current = comment;
            while (current.parent_id && map.has(current.parent_id)) {
                current = map.get(current.parent_id);
            }
            return current;
        };

        comments.forEach(c => {
            if (!c.parent_id) roots.push(c);
            else {
                const root = findRoot(c);
                if (root && root !== c) {
                    const directParent = map.get(c.parent_id);
                    if (directParent && directParent.id !== root.id) c.replyTo = directParent.username;
                    root.replies.push(c);
                } else roots.push(c);
            }
        });

        roots.forEach(r => {
            if (r.replies?.length > 0) r.replies.sort((a, b) => new Date(a.created_at) - new Date(b.created_at));
        });

        // Recursively ensures a set of comments exists in a specific container
        const syncLevel = (levelComments, container, isReply) => {
            levelComments.forEach((c, idx) => {
                const idStr = String(c.id);
                let el = document.getElementById('c' + idStr);

                if (!el) {
                    _f0ckDebug(`[CommentSystem] Reconcile: Injecting new comment #c${idStr}`);
                    const html = this.renderComment(c, currentUserId, isReply);
                    const tmp = document.createElement('div');
                    tmp.innerHTML = html;
                    const newEl = tmp.firstElementChild;

                    // Insertion point: after previous sibling if it exists, else prepend
                    if (idx === 0) {
                        const nav = container.querySelector('.scroll-nav-wrapper');
                        if (nav) nav.insertAdjacentElement('afterend', newEl);
                        else container.prepend(newEl);
                    } else {
                        const prevId = String(levelComments[idx - 1].id);
                        const prevEl = document.getElementById('c' + prevId);
                        // If it's a root comment, we must skip the .comment-replies div of the previous sibling
                        let insertAfter = prevEl;
                        if (prevEl && !isReply) {
                            const next = prevEl.nextElementSibling;
                            if (next && next.classList.contains('comment-replies')) insertAfter = next;
                        }
                        if (insertAfter) insertAfter.insertAdjacentElement('afterend', newEl);
                        else container.appendChild(newEl);
                    }
                    el = newEl;
                    CommentSystem._animateNewComment(el, true);
                }

                // Handle replies container
                if (c.replies?.length > 0) {
                    let repliesWrap = el.nextElementSibling;
                    if (!repliesWrap || !repliesWrap.classList.contains('comment-replies')) {
                        repliesWrap = document.createElement('div');
                        repliesWrap.className = 'comment-replies';
                        el.insertAdjacentElement('afterend', repliesWrap);
                    }
                    syncLevel(c.replies, repliesWrap, true);
                }
            });
        };

        syncLevel(roots, list, false);
    }

    // Maximum characters & lines to fully render in the item view per comment
    static get ITEM_VIEW_MAX_CHARS() { return 500; }
    static get ITEM_VIEW_MAX_LINES() { return 6; }

    renderCommentContent(content, commentId = null, bypassTruncation = false) {
        if (!content) return '';

        // Truncate long comments before any processing
        let truncated = false;
        if (!bypassTruncation) {
            const lines = content.split('\n');
            if (lines.length > CommentSystem.ITEM_VIEW_MAX_LINES) {
                content = lines.slice(0, CommentSystem.ITEM_VIEW_MAX_LINES).join('\n');
                truncated = true;
            }
            if (content.length > CommentSystem.ITEM_VIEW_MAX_CHARS) {
                content = content.substring(0, CommentSystem.ITEM_VIEW_MAX_CHARS);
                truncated = true;
            }
        }

        if (typeof marked === 'undefined') {
            console.warn('Marked.js not loaded, falling back to plain text');
            let text = this.escapeHtml(content)
                .replace(/\*fg\*/gi, '$&');
            return text;
        }

        try {
            // 1. Extract and protect code blocks (```...```) before escaping
            const codeBlocks = [];
            let processed = content.replace(/```([\s\S]*?)```/g, (match) => {
                const placeholder = `BLOCKPORTALX${codeBlocks.length}X`;
                codeBlocks.push(marked.parse(match));
                return placeholder;
            });

            // 2. Initial escaping for the rest of the text. Preserve > for manual blockquote handling.
            let escaped = this.escapeHtml(processed)
                .replace(/&gt;/g, ">");


            const siteOrigin = window.location.origin;
            const renderer = new marked.Renderer();

            renderer.blockquote = quote => {
                let text = (typeof quote === 'string') ? quote : (quote.text || '');
                text = text.replace(/<p>|<\/p>/g, '');
                return text.split('\n').map(line => {
                    if (!line.trim()) return '';
                    return `<span class="greentext">&gt;${line}</span>`;
                }).join('\n');
            };

            renderer.paragraph = text => (typeof text === 'string') ? text : (text.text || '');

            renderer.link = (href, title, text) => {
                if (typeof href === 'object' && href !== null) {
                    title = href.title;
                    text = href.text || text;
                    href = href.href;
                }
                if (!href) return text || '';

                // Strip playlist and tracking params from YouTube URLs so the embed regex always
                // gets a clean URL (e.g. ?list=RDU-... breaks the regex / embed player).
                if (/https?:\/\/(?:www\.)?(?:youtube\.com\/watch|youtu\.be\/)/i.test(href)) {
                    try {
                        const ytUrl = new URL(href);
                        const videoId = ytUrl.searchParams.get('v') || (ytUrl.hostname === 'youtu.be' ? ytUrl.pathname.slice(1) : null);
                        if (videoId) {
                            if (ytUrl.hostname === 'youtu.be') {
                                href = `https://youtu.be/${videoId}`;
                            } else {
                                href = `https://www.youtube.com/watch?v=${videoId}`;
                            }
                        }
                    } catch (e) { /* keep original href on parse failure */ }
                }

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

                let uri = encodeURI(href);
                if (href.startsWith(siteOrigin) || (href.startsWith('/') && !href.startsWith('//'))) {
                    isSameSite = true;
                } else {
                    try {
                        const urlToParse = href.startsWith('//') ? window.location.protocol + href : href;
                        const urlObj = new URL(urlToParse, siteOrigin);
                        isSameSite = (urlObj.hostname === window.location.hostname);
                    } catch (e) { }
                }

                // Shorten internal links if text matches the URL
                let displayText = text;
                if (isSameSite && (text === href || text === href.replace(/^https?:\/\//, '') || text === href.replace(siteOrigin, ''))) {
                    try {
                        const urlToParse = href.startsWith('//') ? window.location.protocol + href : href;
                        const url = new URL(urlToParse.startsWith('http') ? urlToParse : siteOrigin + (urlToParse.startsWith('/') ? '' : '/') + urlToParse);
                        displayText = url.pathname + url.search + url.hash;
                    } catch (e) {
                        console.warn('[CommentSystem] Failed to parse internal URL for shortening:', href);
                    }
                }

                const isMention = href.startsWith('/user/') && text.startsWith('@');
                if (isExternal && !isSameSite) {
                    return `<a href="${href}"${titleAttr} target="_blank" rel="noopener noreferrer">${displayText}<i class="fa-solid fa-arrow-up-right-from-square external-link-icon"></i></a>${extraSuffix}`;
                }
                return `<a href="${href}"${titleAttr}${isMention ? ' class="mention"' : ''}>${displayText}</a>${extraSuffix}`;
            };

            renderer.image = (href, title, text) => {
                const src = (typeof href === 'object' && href !== null) ? (href.href || '') : (href || '');
                const imgHtml = `<img src="${src}" alt="${text || ''}"${title ? ` title="${title}"` : ''}>`;
                if (this.isAdmin && src && src.startsWith('/c/')) {
                    const filename = src.substring(3); // Remove '/c/'
                    return `<span class="image-embed-wrap">${imgHtml}<button class="admin-delete-attachment-btn" data-filename="${filename}" title="Delete attachment">[x]</button></span>`;
                }
                return imgHtml;
            };

            // Pre-compile regexes used in the loop
            const escapedSiteHost = window.location.host.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            const allowedHosts = [escapedSiteHost];
            if (window.f0ckAllowedImages && Array.isArray(window.f0ckAllowedImages)) {
                window.f0ckAllowedImages.forEach(h => {
                    const escapedHost = h.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
                    allowedHosts.push(`(?:[a-z0-9-]+\\.)*${escapedHost}`);
                });
            }
            const hostsRegexPart = allowedHosts.join('|');
            const domainOrRelative = `(?:(?:https?:\\/\\/|\\/\\/)?(?:${hostsRegexPart})|(?:(?<!\\S)|(?<=\\]))(?=\\/[a-zA-Z0-9_\\-]))`;

            // "Safe non-whitespace" — matches any \S except the start of an https?:// boundary.
            // Prevents concatenated URLs (url1.webpurl2.webp) being consumed as one giant src.
            const safeS = `(?:(?!https?:\\/\\/)\\S)`;
            const imageRegex = new RegExp(`(?<![\\(\\[])(${domainOrRelative}(?:\\/${safeS}+\\.(?:jpg|jpeg|png|gif|webp)(?:\\?${safeS}+)?(?:#gif)?))(?![\\)\\]])`, 'gi');
            const rawVideoRegex = new RegExp(`(?<![\\(\\[])(${domainOrRelative}(?:\\/[^\\s\\[\\]\\(\\)]*\\.(?:mp4|webm|ogv|mov)(?:\\?[^\\s\\[\\]\\(\\)]+)?(?:#gif)?))`, 'gi');
            const rawAudioRegex = new RegExp(`(?<![\\(\\[])(${domainOrRelative}(?:\\/[^\\s\\[\\]\\(\\)]*\\.(?:mp3|ogg|wav|flac|aac|opus|m4a)(?:\\?[^\\s\\[\\]\\(\\)]+)?))`, 'gi');
            const mentionRegex = /(?<!\[)@([a-zA-Z0-9_\-\.]+)(?!\])|\[@([^\]]+)\]\(\/user\/([^)]+)\)|\[@([^\]]+)\]/g;

            // Line-by-line processing: handles quotes, mentions, images, and basic markdown safely
            const renderedLines = escaped.split('\n').map(line => {
                const trimmed = line.trimStart();

                // 1. Manual Greentext/Quote handling (avoids recursive blockquote parsing)
                // Exclude only the numeric context links (>>ID) so they can be handled as interactive links.
                // Multiple chevrons (>>text, >>>text) should still be greentext.
                if (trimmed.startsWith('>') && !trimmed.match(/^>>\d+/)) {
                    const quoteContent = line.substring(line.indexOf('>') + 1);
                    const quoteEmojis = window.f0ckSession?.quote_emojis === true;
                    const escapedQuote = quoteContent.replace(/>/g, '&gt;');
                    const renderedContent = quoteEmojis
                        ? escapedQuote.replace(/:([a-z0-9_]+):/g, (m, n) => this.renderEmoji(m, n))
                        : escapedQuote;
                    return `<span class="greentext">&gt;${renderedContent}</span>`;
                }

                // 2. Per-line limit to prevent marked.parse recursion on single giant lines
                if (line.length > 10000) return line;

                if (!line.trim()) return '&nbsp;';

                // 3. Perform replacements on the single line (prevents regex stack overflow on huge strings)
                let processedLine = line;

                // Handle Mentions — use placeholders so the markdown escape pass (step 3)
                // never corrupts usernames that contain underscores or dots.
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
                    return `<a href="#c${id}" class="comment-context-link" data-id="${id}">>>${id}</a>`;
                });

                // Handle Timestamps (e.g., 2:20, 1:02:20)
                const timestampRegex = /\b(?<![\.\/:])(?:(\d{1,2}):)?(\d{1,3}):([0-5]\d)\b/g;
                processedLine = processedLine.replace(timestampRegex, (match, hrs, mins, secs) => {
                    const h = hrs ? parseInt(hrs, 10) : 0;
                    const m = parseInt(mins, 10);
                    const s = parseInt(secs, 10);
                    const totalSeconds = h * 3600 + m * 60 + s;
                    return `<a href="#" class="comment-timestamp" data-time="${totalSeconds}">${match}</a>`;
                });

                // Handle Image Embeds
                processedLine = processedLine.replace(imageRegex, (match, url) => {
                    let fullUrl = url;
                    if (!url.startsWith('http') && !url.startsWith('//') && !url.startsWith('/')) {
                        fullUrl = '//' + url;
                    }
                    return `![image](${fullUrl})`;
                });

                // Handle Raw Video/Audio links so Marked converts them to <a>
                processedLine = processedLine.replace(rawVideoRegex, (match, url) => {
                    let fullUrl = url;
                    if (!url.startsWith('http') && !url.startsWith('//') && !url.startsWith('/')) fullUrl = '//' + url;
                    return `[video](${fullUrl})`;
                });

                processedLine = processedLine.replace(rawAudioRegex, (match, url) => {
                    let fullUrl = url;
                    if (!url.startsWith('http') && !url.startsWith('//') && !url.startsWith('/')) fullUrl = '//' + url;
                    return `[audio](${fullUrl})`;
                });

                // 3. Render Markdown for the line.
                // Protect URLs, already-formed Markdown link/image tokens, AND emoji shortcodes
                // from the italic-prevention escaping pass so that underscores in query params
                // (e.g. ?v=_FcvmypiHg4) and emoji names (e.g. :my_emoji:) are never corrupted.
                const mdProtected = [];
                // Match [text](url) / ![alt](url) tokens, bare http(s) URLs, AND :emoji: shortcodes
                let mdSafe = processedLine.replace(
                    /(!?\[[^\]]*\]\([^)]*\))|https?:\/\/\S+|:[a-z0-9_]+:/g,
                    (match) => {
                        const idx = mdProtected.length;
                        mdProtected.push(match);
                        return `\x02MDURL${idx}\x03`;
                    }
                );
                // Escape * and _ only in the non-URL/non-emoji portions
                mdSafe = mdSafe
                    .replace(/\\/g, '\\\\')
                    .replace(/\*/g, '\\*')
                    .replace(/_/g, '\\_');
                // Restore protected tokens
                mdSafe = mdSafe.replace(/\x02MDURL(\d+)\x03/g, (_, i) => mdProtected[+i]);

                let rendered = marked.parseInline
                    ? marked.parseInline(mdSafe, { renderer: renderer })
                    : marked.parse(mdSafe, { renderer: renderer }).replace(/<p>|<\/p>/g, '');

                // 4. Restore mention placeholders (after markdown so usernames are never escaped)
                rendered = rendered.replace(/\x02MNT(\d+)\x03/g, (_, i) => mentionStore[+i]);

                // 5. Emojis
                rendered = rendered.replace(/:([a-z0-9_]+):/g, (m, n) => this.renderEmoji(m, n));

                return rendered;
            });

            let md = renderedLines.join('\n');

            // YouTube embed: replace anchor links pointing to YouTube with an embedded player
            // Respects per-user preference (session) when logged in; falls back to global config flag for guests.
            const embedYoutube = window.f0ckSession
                ? window.f0ckSession.embed_youtube_in_comments !== false
                : window.f0ckEmbedYoutubeInComments !== false;
            if (embedYoutube) {
                md = md.replace(
                    /<a\s[^>]*href="https?:\/\/(?:www\.)?(?:youtube\.com\/watch\?(?:[^"]*&(?:amp;)?)?v=|youtu\.be\/)([a-zA-Z0-9_\-]{11})[^"]*"[^>]*>([\s\S]*?)<\/a>/gi,
                    (match, videoId) => {
                        return `<span class="yt-embed-wrap"><iframe src="https://www.youtube.com/embed/${videoId}" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture" allowfullscreen referrerpolicy="strict-origin-when-cross-origin"></iframe></span>`;
                    }
                );
            }

            // Vocaroo embed
            md = md.replace(
                /<a\s[^>]*href="https?:\/\/(?:www\.)?(?:voca\.ro|vocaroo\.com)\/([a-zA-Z0-9_-]+)[^"]*"[^>]*>([\s\S]*?)<\/a>/gi,
                (match, vocarooId) => {
                    if (['upload', 'contact', 'privacy', 'tos', 'about'].includes(vocarooId.toLowerCase())) return match;
                    return `<span class="vocaroo-embed-wrap"><iframe src="https://vocaroo.com/embed/${vocarooId}?autoplay=0" width="300" height="60" frameborder="0" allow="autoplay"></iframe></span>`;
                }
            );

            // Abyss label replacement
            md = md.replace(
                /<a\s[^>]*href="(?:https?:\/\/[^\/]+)?\/abyss(?:#|\/)(\d+)"[^>]*>([\s\S]*?)<\/a>/gi,
                (match, abyssId) => {
                    return `<a href="/abyss/${abyssId}" class="abyss-link" data-abyss-id="${abyssId}"><i class="fa-solid fa-dice-d6"></i> /abyss/${abyssId}</a>`;
                }
            );

            const mediaHosts = [escapedSiteHost];
            if (window.f0ckAllowedImages && Array.isArray(window.f0ckAllowedImages)) {
                window.f0ckAllowedImages.forEach(h => {
                    const escaped = h.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
                    mediaHosts.push(`(?:[a-z0-9-]+\\.)*${escaped}`);
                });
            }
            const mediaHostsPart = mediaHosts.join('|');
            const mediaDomainOrRelative = `(?:(?:https?:\\/\\/|\\/\\/)?(?:${mediaHostsPart})|(?=\\/[a-zA-Z0-9_\\-]))`;

            // Video embed: replace anchor links pointing to video files from allowed hosters with a video player
            const videoEmbedRegex = new RegExp(`<a\\s[^>]*href="(${mediaDomainOrRelative}(?:\\/[^\\s\\[\\]\\(\\)]+\\.(?:mp4|webm|ogv|mov)(?:\\?[^\\s\\[\\]\\(\\)]+)?(?:#gif)?))"[^>]*>([\\s\\S]*?)<\\/a>`, 'gi');
            md = md.replace(videoEmbedRegex, (match, url) => {
                const isConvertedGif = url.endsWith('#gif');
                const cleanUrl = url.replace(/#gif$/, '');
                let deleteBtn = '';
                if (this.isAdmin && cleanUrl.startsWith('/c/')) {
                    const filename = cleanUrl.substring(3);
                    deleteBtn = `<button class="admin-delete-attachment-btn" data-filename="${filename}" title="Delete attachment">[x]</button>`;
                }
                if (isConvertedGif) {
                    return `<span class="video-embed-wrap"><video src="${cleanUrl}" class="autoplay-gif" loop muted playsinline preload="auto"></video>${deleteBtn}</span>`;
                }
                return `<span class="video-embed-wrap"><video src="${cleanUrl}" controls loop muted playsinline preload="metadata"></video>${deleteBtn}</span>`;
            });

            // Audio embed: replace anchor links pointing to audio files from allowed hosters with an audio player
            const audioEmbedRegex = new RegExp(`<a\\s[^>]*href="(${mediaDomainOrRelative}(?:\\/[^\\s\\[\\]\\(\\)]+\\.(?:mp3|ogg|wav|flac|aac|opus|m4a)(?:\\?[^\\s\\[\\]\\(\\)]+)?))"[^>]*>([\\s\\S]*?)<\\/a>`, 'gi');
            md = md.replace(audioEmbedRegex, (match, url) => {
                let deleteBtn = '';
                if (this.isAdmin && url.startsWith('/c/')) {
                    const filename = url.substring(3);
                    deleteBtn = `<button class="admin-delete-attachment-btn" data-filename="${filename}" title="Delete attachment">[x]</button>`;
                }
                return `<span class="audio-embed-wrap"><audio src="${url}" controls preload="metadata"></audio>${deleteBtn}</span>`;
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

            // Restore protected code blocks
            md = md.replace(/BLOCKPORTALX(\d+)X/g, (match, index) => {
                return codeBlocks[index] || '';
            });

            if (window.Sanitizer && typeof Sanitizer.clean === 'function') {
                md = Sanitizer.clean(md);
            }

            // Append the "show full comment" button AFTER sanitization — the sanitizer
            // whitelist strips <button> elements for XSS safety, but this button is ours.
            if (truncated) {
                const btnLabel = (window.f0ckI18n?.sidebar_show_full_comment) || 'show full comment';
                md += `<span class="item-comment-truncated-notice"><button class="load-full-comment-btn" type="button">${btnLabel}</button></span>`;
            }

            return md;
        } catch (e) {
            console.error('Markdown error:', e);
            let fallback = this.escapeHtml(content);
            if (truncated) {
                const btnLabel = (window.f0ckI18n?.sidebar_show_full_comment) || 'show full comment';
                fallback += ` <span class="item-comment-truncated-notice"><button class="load-full-comment-btn" type="button">${btnLabel}</button></span>`;
            }
            return fallback;
        }
    }

    /**
     * Force-play videos with autoplay attribute in a container.
     * Browsers often block autoplay on dynamically inserted elements;
     * calling .play() explicitly after DOM insertion resolves this.
     */
    static autoplayConvertedGifs(container) {
        if (!container) return;
        const videos = container.querySelectorAll('video.autoplay-gif');
        videos.forEach(v => {
            v.autoplay = true;
            v.muted = true;
            v.play().catch(() => {
                v.addEventListener('canplay', () => v.play().catch(() => { }), { once: true });
            });
        });
    }

    static playEmojiVideos(container) {
        if (!container) return;
        container.querySelectorAll('video.emoji').forEach(v => {
            v.play().catch(() => {
                v.addEventListener('canplay', () => v.play().catch(() => {}), { once: true });
            });
        });
    }

    buildBacklinkMap(comments) {
        this.backlinkMap = {};
        const process = (c) => {
            if (!c.content) return;
            // Scan for >>ID patterns
            const matches = c.content.matchAll(/(?<!\w)>>(\d+)/g);
            for (const match of matches) {
                const targetId = match[1];
                if (!this.backlinkMap[targetId]) this.backlinkMap[targetId] = new Set();
                this.backlinkMap[targetId].add(c.id);
            }
            // Also treat parent_id as a direct reply
            if (c.parent_id) {
                const targetId = String(c.parent_id);
                if (!this.backlinkMap[targetId]) this.backlinkMap[targetId] = new Set();
                this.backlinkMap[targetId].add(c.id);
            }
        };

        const scan = (list) => {
            list.forEach(c => {
                process(c);
                if (c.replies && c.replies.length > 0) scan(c.replies);
            });
        };
        scan(comments);
    }

    renderEmoji(match, name) {
        if (this.customEmojis && this.customEmojis[name]) {
            const url = this.customEmojis[name];
            if (url.endsWith('.webm')) {
                return `<video src="${url}" class="emoji" autoplay loop muted playsinline title=":${name}:"></video>`;
            }
            return `<img src="${url}" class="emoji" alt="${match}" title="${match}">`;
        }
        return match;
    }

    startLiveTimestamps() {
        // Update timestamps every 30 seconds
        setInterval(() => {
            const timestamps = this.container.querySelectorAll('.comment-time.timeago');
            timestamps.forEach(el => {
                // data-iso stores the raw ISO date for timeAgo calculation;
                // the tooltip attribute holds the human-readable formatted date.
                const isoStr = el.getAttribute('data-iso') || el.getAttribute('tooltip');
                if (isoStr) {
                    el.textContent = this.timeAgo(isoStr);
                    // Keep tooltip in human-readable format
                    if (window.f0ckFormatDateFull) {
                        el.setAttribute('tooltip', window.f0ckFormatDateFull(isoStr));
                    }
                }
            });
        }, 30000);
    }

    updateCommentBacklinks(targetId, replierId) {
        if (!this.backlinkMap) this.backlinkMap = {};
        if (!this.backlinkMap[targetId]) this.backlinkMap[targetId] = new Set();
        this.backlinkMap[targetId].add(replierId);

        const targetEl = document.getElementById('c' + targetId);
        if (targetEl) {
            const headerLeft = targetEl.querySelector('.comment-header-left');
            if (headerLeft) {
                let span = headerLeft.querySelector('.comment-backlinks');
                if (!span) {
                    span = document.createElement('span');
                    span.className = 'comment-backlinks';
                    headerLeft.appendChild(span);
                }
                // Check if already present
                if (!span.querySelector(`a[data-id="${replierId}"]`)) {
                    const link = document.createElement('a');
                    link.href = `#c${replierId}`;
                    link.className = 'comment-context-link';
                    link.dataset.id = replierId;
                    link.textContent = `>>${replierId}`;
                    span.appendChild(document.createTextNode(' '));
                    span.appendChild(link);
                }
            }
        }
    }

    renderComment(comment, currentUserId, isReply = false, isLinear = false) {
        const isDeleted = comment.is_deleted;
        const isPinned = comment.is_pinned;

        // Add @mention prefix if this is a reply to a reply
        const content = isDeleted ? '<span class="deleted-msg">[deleted]</span>' : this.renderCommentContent(comment.content, comment.id);
        const date = new Date(comment.created_at).toLocaleString();

        // Admin buttons
        let adminButtons = '';
        if (this.isAdmin && !isDeleted) {
            const pinIcon = isPinned ? this.icons.unpin : this.icons.pin;
            adminButtons = `<button class="admin-pin-btn ${isPinned ? 'active' : ''}" data-id="${comment.id}" title="${isPinned ? 'Unpin' : 'Pin'}">${pinIcon}</button><button class="admin-edit-btn" data-id="${comment.id}" data-content="${this.escapeHtml(comment.content)}">${this.icons.edit}</button><button class="admin-delete-btn" data-id="${comment.id}">${this.icons.delete}</button>`;
        }

        let userDeleteButton = '';
        if (!this.isAdmin && !isDeleted && window.f0ckSession?.logged_in && window.f0ckSession?.allow_comment_deletion) {
            if (comment.username && comment.username === window.f0ckSession.user) {
                userDeleteButton = `<button class="delete-btn" data-id="${comment.id}" title="Delete Comment">${this.icons.delete}</button>`;
            }
        }

        const pinnedBadge = isPinned ? `<span class="pinned-badge" title="Pinned">${this.icons.pinned}</span>` : '';
        const commentClass = isReply ? 'comment reply' : 'comment';

        // Build replies HTML (only for root comments, max 1 level deep)
        let repliesHtml = '';
        if (!isReply && !isLinear && comment.replies && comment.replies.length > 0) {
            repliesHtml = `<div class="comment-replies">${comment.replies.map(r => this.renderComment(r, currentUserId, true)).join('')}</div>`;
        }

        const timeAgo = this.timeAgo(comment.created_at);
        const isoDate = new Date(comment.created_at).toISOString();
        const fullDate = window.f0ckFormatDateFull
            ? window.f0ckFormatDateFull(comment.created_at)
            : isoDate;

        // Parent context marker removed (redundant with back-references)
        let contextMarker = '';

        // Back-references (replies to this comment)
        let backlinkHtml = '';
        if (this.backlinkMap && this.backlinkMap[comment.id]) {
            const repliers = Array.from(this.backlinkMap[comment.id]);
            if (repliers.length > 0) {
                backlinkHtml = `<span class="comment-backlinks">${repliers.map(rid => `<a href="#c${rid}" class="comment-context-link" data-id="${rid}">>>${rid}</a>`).join(' ')}</span>`;
            }
        }

        const bannerEnabled = window.f0ckCommentBannerEnabled !== false && window.f0ckSession?.comment_banner_enabled !== false;
        let bannerStyle = (bannerEnabled && comment.banner_file && comment.banner_file !== 'null')
            ? `style="--author-banner: url('/a/${comment.banner_file}'); --author-banner-position: ${comment.banner_position === 'center' ? 'center top' : (comment.banner_position || 'center top')}; --author-banner-size: ${(comment.banner_size && comment.banner_size !== 'cover') ? comment.banner_size : '100% auto'}; --author-banner-repeat: no-repeat;"`
            : '';

        const authorUserId = comment.user_id ?? (comment.username && window.f0ckSession && comment.username.toLowerCase() === (window.f0ckSession.user || '').toLowerCase() ? (window.f0ckSession.id || window.f0ckSession.user_id) : null);
        const authorUsernameColor = comment.username_color || (comment.username && window.f0ckSession && comment.username.toLowerCase() === (window.f0ckSession.user || '').toLowerCase() ? window.f0ckSession.username_color : null);

        const isAnonGuest = window.f0ckSession?.is_anonymized ?? (window.f0ckSession?.guest_anonymize && !window.f0ckSession?.logged_in);
        if (isAnonGuest) bannerStyle = '';

        const avatarHtml = isAnonGuest
            ? `<div class="comment-avatar"><img src="/a/default.png"></div>`
            : `<div class="comment-avatar">${comment.username ? `<a href="/user/${comment.username}">` : ''}<img src="${comment.avatar_file ? `/a/${comment.avatar_file}` : (comment.avatar ? `/t/${comment.avatar}.webp` : '/a/default.png')}">${comment.username ? `</a>` : ''}</div>`;

        const isAnon = isAnonGuest || comment.is_anon || comment.anon_fingerprint || comment.username === 'anonymous' || (comment.username && comment.username.startsWith('anon_'));
        // Name without capitals, digits or tall letters ("x"): CSS trims it to the x-height so its top
        // lines up with the avatar. Same rule as the `short_letters` template filter.
        const authorName = comment.display_name || comment.username || '';
        const shortCls = /[a-z]/.test(authorName) && /^[acegmnopqrsuvwxyz _.,-]+$/.test(authorName) ? ' name-short' : '';
        const authorHtml = isAnon
            ? `<span class="comment-author name-short">anonymous</span>`
            : (comment.username
                ? `<a href="/user/${comment.username}" class="comment-author${shortCls}" tooltip="ID: ${authorUserId ?? ''}" ${authorUsernameColor ? `style="color: ${authorUsernameColor}"` : ''}>${this.escapeHtml(comment.display_name || comment.username)}</a>`
                : '<span class="comment-author">System</span>');

        const anonDataAttrs = isAnon ? '' : `data-username="${comment.username}" data-display="${this.escapeHtml(comment.display_name || '')}"`;

        return `<div class="${commentClass} ${isDeleted ? 'deleted' : ''} ${isPinned ? 'pinned' : ''}" id="c${comment.id}" ${bannerStyle}>${avatarHtml}<div class="comment-body"><div class="comment-header"><div class="comment-header-left">${pinnedBadge}${authorHtml}${contextMarker}${backlinkHtml}</div><a href="#c${comment.id}" class="comment-time timeago" tooltip="${fullDate}" data-iso="${isoDate}" data-id="${comment.id}" ${anonDataAttrs}>${timeAgo}</a></div><div class="comment-content" data-raw="${this.escapeHtml(comment.content)}">${content}</div>${this.renderCommentAttachments(comment.files, comment.content)}${this.renderCommentPoll(comment.poll, comment.id, isAnon ? null : comment.username)}<div class="comment-footer"><div class="comment-footer-right"><div class="comment-actions">${!isDeleted ? `${(currentUserId || window.f0ckAnonSSH?.pubkey) ? `<button class="reply-btn" data-id="${comment.id}" data-username="${comment.username}" data-display="${this.escapeHtml(comment.display_name || '')}" title="Reply"><i class="fa-solid fa-reply"></i></button><button class="quote-btn" data-id="${comment.id}" data-username="${comment.username}" data-display="${this.escapeHtml(comment.display_name || '')}" title="Quote with Text"><i class="fa-solid fa-quote-left"></i></button>` : ''}<button class="report-comment-btn" data-id="${comment.id}" title="Report Comment" style="background:none;border:none;color:inherit;cursor:pointer;opacity:0.75;padding:0;"><i class="fa-solid fa-triangle-exclamation"></i></button>` : ''}${adminButtons}${userDeleteButton}</div></div></div></div><a href="#c${comment.id}" class="comment-permalink" title="Permalink" data-id="${comment.id}" ${anonDataAttrs}>#${comment.id}</a></div>${repliesHtml}`;
    }

    timeAgo(date) {
        if (window.f0ckTimeAgo) return window.f0ckTimeAgo(date);
        const seconds = Math.floor((new Date() - new Date(date)) / 1000);
        if (seconds < 5) return 'just now';
        const intervals = [
            { label: 'year', seconds: 31536000 },
            { label: 'month', seconds: 2592000 },
            { label: 'day', seconds: 86400 },
            { label: 'hour', seconds: 3600 },
            { label: 'minute', seconds: 60 },
            { label: 'second', seconds: 1 }
        ];
        for (const interval of intervals) {
            const count = Math.floor(seconds / interval.seconds);
            if (count >= 1) {
                return `${count} ${interval.label}${count !== 1 ? 's' : ''} ago`;
            }
        }
        return 'just now';
    }

    escapeHtml(unsafe) {
        if (!unsafe) return '';
        const div = document.createElement('div');
        div.textContent = unsafe;
        return div.innerHTML.replace(/"/g, '&quot;').replace(/'/g, '&#39;');
    }

    renderCommentAttachments(files, content = '') {
        if (!files || files.length === 0) return '';
        const items = files.map(f => {
            const url = `/c/${f.dest}`;
            if (content.includes(url)) return ''; // Skip if already rendered in content
            if (f.mime.startsWith('image/')) {
                return `<a href="${url}" target="_blank" class="cf-attachment cf-image"><img src="${url}" alt="${this.escapeHtml(f.original_filename || 'image')}" loading="lazy"></a>`;
            } else if (f.mime.startsWith('video/')) {
                return `<div class="cf-attachment cf-video"><video src="${url}" controls preload="metadata"></video></div>`;
            } else if (f.mime.startsWith('audio/')) {
                return `<div class="cf-attachment cf-audio"><audio src="${url}" controls preload="metadata"></audio></div>`;
            }
            return '';
        }).join('');
        return items ? `<div class="comment-attachments">${items}</div>` : '';
    }

    renderCommentPoll(poll, commentId, commentUsername) {
        if (!poll) return '';
        const i18n = window.f0ckI18n || {};
        const session = window.f0ckSession || {};
        const total = poll.total_votes || 0;
        const voted = !!poll.user_vote_option_id;
        const expired = poll.expires_at && new Date(poll.expires_at) < new Date();
        const isAnon = poll.is_anonymous !== false;

        const canDelete = session.logged_in && (session.is_admin || session.is_moderator || session.user === commentUsername);

        const optionsHtml = (poll.options || []).map(opt => {
            const pct = total > 0 ? Math.round((opt.vote_count / total) * 100) : 0;
            const isVoted = poll.user_vote_option_id === opt.id;
            const clickable = session.logged_in && !expired && !voted;
            const voterAvatars = (!isAnon && Array.isArray(opt.voters) && opt.voters.length > 0)
                ? `<div class="poll-option-voters">${opt.voters.map(v => {
                    const u = (v && typeof v === 'object') ? v : { username: String(v || ''), avatar: null, avatar_file: null };
                    const name = String(u.username || '');
                    const src = u.avatar_file ? `/a/${u.avatar_file}` : u.avatar ? `/t/${u.avatar}.webp` : '/a/default.png';
                    return name ? `<a href="/user/${this.escapeHtml(name)}" title="${this.escapeHtml(name)}"><img class="poll-voter-avatar" src="${src}" alt="${this.escapeHtml(name)}" loading="lazy"></a>` : '';
                }).join('')}</div>`
                : '';
            return `<div class="poll-option ${isVoted ? 'poll-option-voted' : ''} ${clickable ? 'poll-option-clickable' : ''}"
                         data-option-id="${opt.id}" data-poll-id="${poll.id}" data-comment-id="${commentId}">
                <div class="poll-option-bar" style="width:${pct}%"></div>
                <span class="poll-option-text">${this.escapeHtml(opt.text)}</span>
                <span class="poll-option-pct">${pct}%</span>
                ${isVoted ? `<i class="fa-solid fa-check poll-vote-check" title="${i18n.poll_voted || 'You voted'}"></i>` : ''}
                ${voterAvatars}
            </div>`;
        }).join('');

        const deleteBtn = canDelete
            ? `<button class="poll-delete-btn" data-poll-id="${poll.id}" title="${i18n.poll_delete || 'Delete poll'}"><i class="fa-solid fa-trash-can"></i></button>`
            : '';

        const anonBadge = isAnon
            ? `<span class="poll-anon-badge" title="${i18n.poll_anonymous || 'Anonymous'}"><i class="fa-solid fa-user-secret"></i></span>`
            : `<span class="poll-anon-badge poll-public-badge" title="${i18n.poll_public || 'Public votes'}"><i class="fa-solid fa-eye"></i></span>`;

        return `<div class="comment-poll" data-poll-id="${poll.id}" data-is-anonymous="${isAnon ? '1' : '0'}">
            <div class="poll-question">${this.escapeHtml(poll.question)}</div>
            <div class="poll-options">${optionsHtml}</div>
            <div class="poll-footer">
                <span class="poll-total">${total} ${total === 1 ? (i18n.poll_vote_single || 'vote') : (i18n.poll_votes || 'votes')}</span>
                ${anonBadge}
                ${expired ? `<span class="poll-expired-badge">${i18n.poll_expired || 'Poll closed'}</span>` : ''}
                ${deleteBtn}
            </div>
        </div>`;
    }

    // Same markup as renderInput() so every theme styles it identically, but inert: readonly, not focusable,
    // no pointer events on the controls, no submit. Clicking it opens the login modal.
    renderGuestInput() {
        const i18n = window.f0ckI18n || {};
        const placeholder = i18n.login_to_comment || 'Log in to comment';
        const postLabel = i18n.post || 'Post';
        return `
            <div class="comment-input main-input is-guest" aria-disabled="true" title="${placeholder}"
                 data-on="click:comments-guest-login">
                <textarea placeholder="${placeholder}" readonly tabindex="-1" aria-disabled="true"></textarea>
                <div class="comment-file-preview"></div>
                <div class="input-actions">
                    <button class="submit-comment" type="button" tabindex="-1" aria-disabled="true"><span class="submit-label">${postLabel}</span><i class="fa-solid fa-spinner fa-spin submit-spinner"></i></button>
                </div>
            </div>
        `;
    }

    renderInput(parentId = null) {
        const i18n = window.f0ckI18n || {};
        const session = window.f0ckSession || {};
        const placeholder = i18n.write_comment || 'Write a comment...';
        const postLabel = i18n.post || 'Post';
        const cancelLabel = i18n.cancel || 'Cancel';
        const attachLabel = i18n.attach_file || 'Attach file';
        const pollLabel = i18n.poll_btn_title || 'Create poll';
        const maxLen = session.comment_max_length;
        const maxLenAttr = (maxLen !== null && maxLen !== undefined) ? ` maxlength="${maxLen}"` : '';
        const counter = (maxLen !== null && maxLen !== undefined)
            ? `<span class="char-counter" data-max="${maxLen}">0 / ${maxLen}</span>`
            : '';
        const anonAttachmentsDisabled = session.is_anon && session.anon_permissions?.comment_attachments === false;
        const fileUploadEnabled = session.logged_in && session.allow_fileupload_comments && !anonAttachmentsDisabled;
        const multiFile = session.fileupload_comments_multifile;
        const attachBtn = fileUploadEnabled
            ? `<button class="comment-attach-btn" title="${attachLabel}" type="button"><i class="fa-solid fa-paperclip"></i></button><input type="file" class="comment-file-input" accept="image/*,video/*,audio/*" ${multiFile ? 'multiple' : ''} style="display:none;">`
            : '';
        const pollEnabled = session.logged_in && session.enable_comment_polls;
        const pollBtn = pollEnabled
            ? `<button class="comment-poll-btn" title="${pollLabel}" type="button"><i class="fa-solid fa-chart-bar"></i></button>`
            : '';
        return `
            <div class="comment-input ${parentId ? 'reply-input' : 'main-input'}" ${parentId ? `data-parent="${parentId}"` : ''}>
                <textarea placeholder="${placeholder}"${maxLenAttr} required></textarea>
                <div class="comment-file-preview"></div>
                <div class="input-actions">
                    ${counter}
                    ${attachBtn}
                    ${pollBtn}
                    ${parentId ? `<button class="cancel-reply" title="${cancelLabel}"><i class="fa-solid fa-xmark"></i></button>` : ''}
                    <button class="submit-comment"><span class="submit-label">${postLabel}</span><i class="fa-solid fa-spinner fa-spin submit-spinner"></i></button>
                </div>
            </div>
        `;
    }

    setupHoverPreviews() {
        if (CommentSystem.hoverPreviewsAttached) return;
        CommentSystem.hoverPreviewsAttached = true;

        // Hover for Comment Context Links (>>ID) - Global delegation for nested previews (inception)
        this.mouseCurrentLevel = -1;
        this.currentHoverLink = null;
        document.addEventListener('mouseover', (e) => {
            const contextLink = e.target.closest('.comment-context-link');
            const popup = e.target.closest('.comment-preview-popup');

            if (contextLink || popup) {
                if (this.previewCloseTimer) {
                    clearTimeout(this.previewCloseTimer);
                    this.previewCloseTimer = null;
                }
            }

            if (contextLink) {
                // Ignore mouseover for previews on mobile touch devices to prevent tap-to-preview
                // We handle mobile previews via the touchstart timer instead.
                if (window.matchMedia('(pointer: coarse)').matches) return;

                if (this.currentHoverLink === contextLink) return;
                this.currentHoverLink = contextLink;

                if (this.previewOpenTimer) clearTimeout(this.previewOpenTimer);
                this.previewOpenTimer = setTimeout(() => {
                    this.showCommentPreview(contextLink, e);
                }, 150); // 150ms dwell time to prevent flickering while moving mouse
            } else {
                if (this.previewOpenTimer) {
                    clearTimeout(this.previewOpenTimer);
                    this.previewOpenTimer = null;
                }
                this.currentHoverLink = null;

                const level = popup ? parseInt(popup.dataset.level || 0) : -1;

                // If we move back to a parent level or blank area of a popup, close its children
                // but use a delay so the user can reach the child popup if they are moving towards it.
                if (popup && !contextLink) {
                    this.previewCloseTimer = setTimeout(() => {
                        this.closePreviewsAboveLevel(level);
                    }, 400);
                }

                this.mouseCurrentLevel = level;
            }
        });

        document.addEventListener('mouseout', (e) => {
            const contextLink = e.target.closest('.comment-context-link');
            const popup = e.target.closest('.comment-preview-popup');

            if (contextLink) {
                if (this.previewOpenTimer) {
                    clearTimeout(this.previewOpenTimer);
                    this.previewOpenTimer = null;
                }
                this.currentHoverLink = null;
            }

            if (contextLink || popup) {
                if (this.previewCloseTimer) clearTimeout(this.previewCloseTimer);
                this.previewCloseTimer = setTimeout(() => {
                    this.closePreviewsAboveLevel(-1);
                    this.mouseCurrentLevel = -1;
                }, 400);
            }
        });

        // Mobile Touch Support: Touch-and-hold to preview
        let touchPreviewTimer = null;
        document.addEventListener('touchstart', (e) => {
            const contextLink = e.target.closest('.comment-context-link');
            if (contextLink) {
                if (touchPreviewTimer) clearTimeout(touchPreviewTimer);
                touchPreviewTimer = setTimeout(() => {
                    this.showCommentPreview(contextLink, e);
                }, 150); // 150ms hold to trigger preview on mobile
            }
        }, { passive: true });

        document.addEventListener('touchmove', () => {
            if (touchPreviewTimer) {
                clearTimeout(touchPreviewTimer);
                touchPreviewTimer = null;
            }
        }, { passive: true });

        document.addEventListener('touchend', () => {
            if (touchPreviewTimer) {
                clearTimeout(touchPreviewTimer);
                touchPreviewTimer = null;
            }
        }, { passive: true });

        // Global click listener for comment interaction (popups & expanding truncated comments in previews)
        document.addEventListener('click', (e) => {
            const target = e.target;

            // Load full comment (expand truncated)
            const loadFullBtn = target.closest('.load-full-comment-btn');
            if (loadFullBtn) {
                const contentEl = loadFullBtn.closest('.comment-content');
                if (contentEl) {
                    if (contentEl.querySelector('.collapse-comment-btn')) return;
                    const commentEl = contentEl.closest('.comment');
                    const commentId = commentEl ? (commentEl.dataset.id || (commentEl.id ? commentEl.id.replace(/^c/, '') : null)) : null;
                    const fullContent = contentEl.dataset.raw || (commentId && this.commentCache ? this.commentCache.get(commentId)?.content : null);
                    if (fullContent) {
                        contentEl.innerHTML = this.renderCommentContent(fullContent, null, true);
                        const seeLessLabel = (window.f0ckI18n?.sidebar_see_less) || 'see less';
                        contentEl.insertAdjacentHTML('beforeend',
                            `<span class="item-comment-truncated-notice"><button class="collapse-comment-btn" type="button">${seeLessLabel}</button></span>`
                        );
                        CommentSystem.playEmojiVideos(contentEl);
                    }
                }
                return;
            }

            // Collapse full comment back to truncated view
            const collapseBtn = target.closest('.collapse-comment-btn');
            if (collapseBtn) {
                const contentEl = collapseBtn.closest('.comment-content');
                if (contentEl) {
                    if (contentEl.querySelector('.load-full-comment-btn')) return;
                    const commentEl = contentEl.closest('.comment');
                    const commentId = commentEl ? (commentEl.dataset.id || (commentEl.id ? commentEl.id.replace(/^c/, '') : null)) : null;
                    const fullContent = contentEl.dataset.raw || (commentId && this.commentCache ? this.commentCache.get(commentId)?.content : null);
                    if (fullContent) {
                        contentEl.innerHTML = this.renderCommentContent(fullContent, null, false);
                        CommentSystem.playEmojiVideos(contentEl);
                    }
                }
                return;
            }

            const isLink = target.closest('.comment-context-link');
            const isPopup = target.closest('.comment-preview-popup');

            if (!isLink && !isPopup) {
                this.closePreviewsAboveLevel(-1);
            }
        });
    }

    setupDelegatedEvents() {
        _f0ckDebug('[DEBUG] Setting up delegated events for container:', this.container);
        if (!this.container) return;

        // Ctrl+Enter to submit comment
        this.container.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && e.ctrlKey) {
                const textarea = e.target.closest('textarea');
                if (!textarea || textarea.disabled) return;
                const wrap = textarea.closest('.comment-input');
                if (!wrap) return;
                const submitBtn = wrap.querySelector('.submit-comment');
                if (submitBtn && !submitBtn.disabled && !submitBtn.classList.contains('loading')) {
                    e.preventDefault();
                    e.stopPropagation();
                    submitBtn.click();
                }
            } else if (e.key === 'Escape') {
                const textarea = e.target.closest('textarea');
                if (textarea) textarea.blur();
            }
        });

        // Live character counter (only active when comment_max_length is set)
        this.container.addEventListener('input', (e) => {
            const textarea = e.target.closest('.comment-input textarea');
            if (!textarea) return;
            const counter = textarea.closest('.comment-input')?.querySelector('.char-counter');
            if (!counter) return;
            const max = parseInt(counter.dataset.max, 10);
            // Exclude quoted lines (starting with '>') from the count —
            // quoted context is capped at 200 chars and shouldn't eat into the user's limit.
            const nonQuotedLen = textarea.value
                .split('\n')
                .filter(line => !line.startsWith('>'))
                .join('\n')
                .length;
            counter.textContent = `${nonQuotedLen} / ${max}`;
            counter.classList.toggle('near-limit', nonQuotedLen >= max * 0.9);
            counter.classList.toggle('at-limit', nonQuotedLen >= max);
        });

        // Single Change Listener for Sort
        this.container.addEventListener('change', (e) => {
            if (e.target.id === 'comment-sort') {
                this.sort = e.target.value;
                this.loadComments();
            }
        });

        // File input change: store the File object locally and show a preview using a
        // local object URL. The actual upload happens at submit time, so no server round-
        // trip occurs here and there is nothing to orphan.
        this.container.addEventListener('change', (e) => {
            if (!e.target.matches('.comment-file-input')) return;
            const fileInput = e.target;
            const wrap = fileInput.closest('.comment-input');
            if (!wrap) return;
            // Clear the attach-pending flag set in the click handler.
            wrap._attachPending = false;
            const textarea = wrap.querySelector('textarea');
            if (!textarea) return;
            const previewArea = wrap.querySelector('.comment-file-preview');
            const session = window.f0ckSession || {};
            const maxSize = session.fileupload_comments_size || (10 * 1024 * 1024);
            const i18n = window.f0ckI18n || {};
            const removeLabel = i18n.remove_file || 'Remove file';

            const maxAttachments = session.fileupload_comments_max || 5;
            const currentCount = previewArea ? previewArea.querySelectorAll('.cf-preview-item').length : 0;
            const slotsLeft = maxAttachments - currentCount;

            if (fileInput.files.length > slotsLeft) {
                if (window.flashMessage) window.flashMessage(
                    `Maximum ${maxAttachments} attachments per comment exceeded`,
                    3000, 'error'
                );
                fileInput.value = '';
                return;
            }

            for (const file of fileInput.files) {
                if (file.size > maxSize) {
                    if (window.flashMessage) window.flashMessage((i18n.file_too_large || 'File too large') + `: ${file.name}`, 3000, 'error');
                    continue;
                }

                // Use saved cursor position (set when attach btn was clicked, before file picker
                // dismissed the keyboard on mobile causing selectionStart to become 0).
                const savedPos = wrap._savedCursorPos ?? textarea.selectionStart;
                const savedEnd = wrap._savedCursorEnd ?? textarea.selectionEnd;
                wrap._savedCursorPos = null;
                wrap._savedCursorEnd = null;

                // Insert a placeholder token so the URL ends up in the right spot in the text
                // once we know the real dest after the server-side upload at submit time.
                const placeholderToken = `[attachment:${file.name}]`;
                const before = textarea.value.substring(0, savedPos);
                const after = textarea.value.substring(savedEnd);
                const sep = before.length > 0 && !/\s$/.test(before) ? ' ' : '';
                textarea.value = before + sep + placeholderToken + after;

                // Build local preview (no server call yet)
                if (previewArea) {
                    const objectUrl = URL.createObjectURL(file);
                    const previewItem = document.createElement('div');
                    previewItem.className = 'cf-preview-item';
                    previewItem.dataset.placeholderToken = placeholderToken;
                    previewItem.dataset.originalFilename = file.name;
                    previewItem.dataset.mime = file.type || 'application/octet-stream';
                    // Store the File object directly on the element for retrieval at submit time
                    previewItem._stagedFile = file;
                    previewItem._objectUrl = objectUrl;

                    if (file.type.startsWith('image/')) {
                        const img = document.createElement('img');
                        img.src = objectUrl;
                        img.loading = 'lazy';
                        previewItem.appendChild(img);
                    } else if (file.type.startsWith('video/')) {
                        const vid = document.createElement('video');
                        vid.src = objectUrl;
                        vid.muted = true;
                        vid.preload = 'metadata';
                        previewItem.appendChild(vid);
                    } else {
                        const icon = document.createElement('i');
                        icon.className = file.type.startsWith('audio/') ? 'fa-solid fa-music' : 'fa-solid fa-file';
                        previewItem.appendChild(icon);
                    }

                    const nameEl = document.createElement('span');
                    nameEl.className = 'cf-filename';
                    nameEl.textContent = file.name;
                    previewItem.appendChild(nameEl);

                    const spoilerBtn = document.createElement('button');
                    spoilerBtn.className = 'cf-spoiler-btn';
                    spoilerBtn.title = 'Toggle spoiler';
                    spoilerBtn.innerHTML = '<i class="fa-solid fa-paperclip"></i><span class="spoiler-attach-badge">S</span>';
                    spoilerBtn.type = 'button';
                    previewItem.appendChild(spoilerBtn);

                    const removeBtn = document.createElement('button');
                    removeBtn.className = 'cf-remove-btn';
                    removeBtn.title = removeLabel;
                    removeBtn.innerHTML = '<i class="fa-solid fa-xmark"></i>';
                    removeBtn.type = 'button';
                    previewItem.appendChild(removeBtn);

                    previewArea.appendChild(previewItem);
                }
            }
            fileInput.value = '';
        });

        this.container.addEventListener('click', async (e) => {
            _f0ckDebug('[DEBUG] Click on container:', e.target);
            const target = e.target;


            // Toggling Scroll Action
            const scrollBtn = target.closest('.scroll-to-bottom');
            if (scrollBtn) {
                if (scrollBtn.classList.contains('is-at-bottom')) {
                    // Scroll to Top of the page
                    window.scrollTo({
                        top: 0,
                        behavior: 'smooth'
                    });
                } else {
                    // Scroll to Bottom of comments
                    const bottomElement = this.container.querySelector('.main-input') || this.container.querySelector('.lock-notice');
                    if (bottomElement) {
                        bottomElement.scrollIntoView({ behavior: 'smooth', block: 'center' });
                    } else {
                        this.container.scrollIntoView({ behavior: 'smooth', block: 'end' });
                    }
                }
                return;
            }

            // Attach file button
            if (target.matches('.comment-attach-btn') || target.closest('.comment-attach-btn')) {
                const wrap = target.closest('.comment-input');
                const textarea = wrap?.querySelector('textarea');
                if (textarea) {
                    wrap._savedCursorPos = textarea.selectionStart;
                    wrap._savedCursorEnd = textarea.selectionEnd;
                }
                // Flag that a file picker is open. Checked by the visibilitychange
                // guard in f0ckm.js so it doesn't call loadComments (and replace the
                // textarea DOM) while the mobile file picker is open.
                // Cleared at the start of the change handler, or left to expire
                // harmlessly if the user cancels the picker.
                if (wrap) wrap._attachPending = true;
                const fileInput = wrap?.querySelector('.comment-file-input');
                if (fileInput) fileInput.click();
                return;
            }

            // Attach file as spoiler: toggle spoiler marker on a staged preview item
            const spoilerToggle = target.closest('.cf-spoiler-btn');
            if (spoilerToggle) {
                const previewItem = spoilerToggle.closest('.cf-preview-item');
                if (previewItem) {
                    // Staged items use a placeholder token in the textarea instead of a real URL.
                    // Toggle a data attribute; the actual [spoiler] wrapping is applied at submit.
                    const isSpoiler = previewItem.classList.toggle('cf-is-spoiler');
                    spoilerToggle.title = isSpoiler ? 'Remove spoiler' : 'Toggle spoiler';
                }
                return;
            }

            // Remove file preview + strip placeholder token from textarea
            if (target.matches('.cf-remove-btn') || target.closest('.cf-remove-btn')) {
                const previewItem = target.closest('.cf-preview-item');
                if (previewItem) {
                    const token = previewItem.dataset.placeholderToken;
                    if (token) {
                        const wrap = previewItem.closest('.comment-input');
                        const textarea = wrap?.querySelector('textarea');
                        if (textarea) {
                            const escaped = token.replace(/[.*+?^${}()|[\\]\\]/g, '\\$&');
                            textarea.value = textarea.value
                                .replace(new RegExp('\\n?' + escaped + '\\n?'), '\n')
                                .replace(/^\n|\n$/g, '');
                        }
                    }
                    // Revoke the local object URL to free memory
                    if (previewItem._objectUrl) URL.revokeObjectURL(previewItem._objectUrl);
                    // No server call needed — file was never uploaded
                    previewItem.remove();
                }
                return;
            }

            // Poll button — toggle poll builder
            if (target.closest('.comment-poll-btn')) {
                const btn = target.closest('.comment-poll-btn');
                const wrap = btn.closest('.comment-input');
                if (!wrap) return;
                const existing = wrap.querySelector('.poll-builder');
                if (existing) {
                    existing.remove();
                    btn.classList.remove('active');
                    return;
                }
                const i18n = window.f0ckI18n || {};
                const builder = document.createElement('div');
                builder.className = 'poll-builder';
                builder.innerHTML = `
                    <input class="poll-question-input" type="text" placeholder="${i18n.poll_question_placeholder || 'Poll question...'}" maxlength="200">
                    <div class="poll-options-list">
                        <input class="poll-option-input" type="text" placeholder="${i18n.poll_option_placeholder || 'Option...'}" maxlength="100">
                        <input class="poll-option-input" type="text" placeholder="${i18n.poll_option_placeholder || 'Option...'}" maxlength="100">
                    </div>
                    <div class="poll-builder-actions">
                        <button class="poll-add-option-btn" type="button"><i class="fa-solid fa-plus"></i> ${i18n.poll_add_option || 'Add option'}</button>
                        <label class="poll-anon-toggle">
                            <input type="checkbox" class="poll-anon-checkbox" checked>
                            <i class="fa-solid fa-user-secret"></i> ${i18n.poll_anonymous || 'Anonymous'}
                        </label>
                        <button class="poll-remove-btn" type="button"><i class="fa-solid fa-xmark"></i> ${i18n.poll_remove || 'Remove poll'}</button>
                    </div>
                `;
                // Insert before input-actions
                const actions = wrap.querySelector('.input-actions');
                if (actions) wrap.insertBefore(builder, actions);
                else wrap.appendChild(builder);
                btn.classList.add('active');
                builder.querySelector('.poll-question-input').focus();
                return;
            }

            // Poll builder — add option
            if (target.closest('.poll-add-option-btn')) {
                const list = target.closest('.poll-builder')?.querySelector('.poll-options-list');
                if (!list) return;
                if (list.querySelectorAll('.poll-option-input').length >= 10) return;
                const i18n = window.f0ckI18n || {};
                const inp = document.createElement('input');
                inp.className = 'poll-option-input';
                inp.type = 'text';
                inp.placeholder = i18n.poll_option_placeholder || 'Option...';
                inp.maxLength = 100;
                list.appendChild(inp);
                inp.focus();
                return;
            }

            // Poll builder — remove
            if (target.closest('.poll-remove-btn')) {
                const builder = target.closest('.poll-builder');
                if (!builder) return;
                const wrap = builder.closest('.comment-input');
                if (wrap) wrap.querySelector('.comment-poll-btn')?.classList.remove('active');
                builder.remove();
                return;
            }

            // Poll option — vote
            if (target.closest('.poll-option-clickable')) {
                const opt = target.closest('.poll-option-clickable');
                const pollId = opt.dataset.pollId;
                const optionId = opt.dataset.optionId;
                const commentId = opt.dataset.commentId;
                if (!pollId || !optionId) return;
                const pollWidget = opt.closest('.comment-poll');
                // Disable all options immediately
                pollWidget?.querySelectorAll('.poll-option-clickable').forEach(o => o.classList.remove('poll-option-clickable'));
                fetch(`/api/polls/${pollId}/vote`, {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/x-www-form-urlencoded',
                        'X-CSRF-Token': window.f0ckSession?.csrf_token
                    },
                    body: `option_id=${optionId}`
                }).then(r => r.json()).then(data => {
                    if (!data.success) {
                        if (window.flashMessage) window.flashMessage(data.message || 'Vote failed', 2500, 'error');
                        return;
                    }
                    // Patch poll widget in-place
                    if (!pollWidget) return;
                    const i18n = window.f0ckI18n || {};
                    const total = data.total_votes || 0;
                    pollWidget.querySelector('.poll-total').textContent = `${total} ${total === 1 ? (i18n.poll_vote_single || 'vote') : (i18n.poll_votes || 'votes')}`;
                    const isAnon = pollWidget.dataset.isAnonymous !== '0';
                    (data.options || []).forEach(updated => {
                        const el = pollWidget.querySelector(`.poll-option[data-option-id="${updated.id}"]`);
                        if (!el) return;
                        const pct = total > 0 ? Math.round((updated.vote_count / total) * 100) : 0;
                        el.querySelector('.poll-option-bar').style.width = pct + '%';
                        el.querySelector('.poll-option-pct').textContent = pct + '%';
                        if (updated.id === data.user_vote_option_id) {
                            el.classList.add('poll-option-voted');
                            if (!el.querySelector('.poll-vote-check')) {
                                el.insertAdjacentHTML('beforeend', `<i class="fa-solid fa-check poll-vote-check" title="${i18n.poll_voted || 'You voted'}"></i>`);
                            }
                        }
                        // Update voter list for public polls
                        if (!isAnon && Array.isArray(updated.voters)) {
                            let votersEl = el.querySelector('.poll-option-voters');
                            if (updated.voters.length > 0) {
                                const html = updated.voters.map(v => {
                                    const u = typeof v === 'object' ? v : { username: v, avatar: null, avatar_file: null };
                                    const src = u.avatar_file ? `/a/${u.avatar_file}` : u.avatar ? `/t/${u.avatar}.webp` : '/a/default.png';
                                    return `<a href="/user/${u.username}" title="${u.username}"><img class="poll-voter-avatar" src="${src}" alt="${u.username}" loading="lazy"></a>`;
                                }).join('');
                                if (votersEl) votersEl.innerHTML = html;
                                else el.insertAdjacentHTML('beforeend', `<div class="poll-option-voters">${html}</div>`);
                            } else if (votersEl) {
                                votersEl.remove();
                            }
                        }
                    });
                }).catch(() => {
                    if (window.flashMessage) window.flashMessage('Network error', 2500, 'error');
                });
                return;
            }

            // Poll delete button
            if (target.closest('.poll-delete-btn')) {
                const btn = target.closest('.poll-delete-btn');
                const pollId = btn.dataset.pollId;
                if (!pollId) return;
                if (!confirm('Delete this poll?')) return;
                fetch(`/api/polls/${pollId}/delete`, {
                    method: 'POST',
                    headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token }
                }).then(r => r.json()).then(data => {
                    if (data.success) {
                        const widget = btn.closest('.comment-poll');
                        if (widget) {
                            widget.style.transition = 'opacity 0.3s';
                            widget.style.opacity = '0';
                            setTimeout(() => widget.remove(), 300);
                        }
                    } else {
                        if (window.flashMessage) window.flashMessage(data.message || 'Delete failed', 2500, 'error');
                    }
                }).catch(() => {
                    if (window.flashMessage) window.flashMessage('Network error', 2500, 'error');
                });
                return;
            }

            // Submit Comment
            if (target.closest('.submit-comment')) {
                this.handleSubmit(e);
                return;
            }

            // Cancel Reply
            if (target.closest('.cancel-reply')) {
                const form = target.closest('.reply-input');
                if (form) form.remove();
                return;
            }

            // Load full comment (expand truncated)
            const loadFullBtn = target.closest('.load-full-comment-btn');
            if (loadFullBtn) {
                const contentEl = loadFullBtn.closest('.comment-content');
                if (contentEl) {
                    if (contentEl.querySelector('.collapse-comment-btn')) return;
                    const commentEl = contentEl.closest('.comment');
                    const commentId = commentEl ? (commentEl.dataset.id || (commentEl.id ? commentEl.id.replace(/^c/, '') : null)) : null;
                    const fullContent = contentEl.dataset.raw || (commentId && this.commentCache ? this.commentCache.get(commentId)?.content : null);
                    if (fullContent) {
                        contentEl.innerHTML = this.renderCommentContent(fullContent, null, true);
                        // Append "see less" button after full content
                        const seeLessLabel = (window.f0ckI18n?.sidebar_see_less) || 'see less';
                        contentEl.insertAdjacentHTML('beforeend',
                            `<span class="item-comment-truncated-notice"><button class="collapse-comment-btn" type="button">${seeLessLabel}</button></span>`
                        );
                        CommentSystem.playEmojiVideos(contentEl);
                    }
                }
                return;
            }

            // Collapse full comment back to truncated view
            const collapseBtn = target.closest('.collapse-comment-btn');
            if (collapseBtn) {
                const contentEl = collapseBtn.closest('.comment-content');
                if (contentEl) {
                    if (contentEl.querySelector('.load-full-comment-btn')) return;
                    const commentEl = contentEl.closest('.comment');
                    const commentId = commentEl ? (commentEl.dataset.id || (commentEl.id ? commentEl.id.replace(/^c/, '') : null)) : null;
                    const fullContent = contentEl.dataset.raw || (commentId && this.commentCache ? this.commentCache.get(commentId)?.content : null);
                    if (fullContent) {
                        contentEl.innerHTML = this.renderCommentContent(fullContent, null, false);
                        CommentSystem.playEmojiVideos(contentEl);
                    }
                }
                return;
            }

            // Comment Context Link (>>ID)
            const contextLink = target.closest('.comment-context-link');
            if (contextLink) {
                e.preventDefault();
                const targetId = contextLink.dataset.id;
                this.scrollToComment(targetId, 0, true);

                // Highlight effect
                const targetEl = document.getElementById('c' + targetId);
                if (targetEl) {
                    targetEl.classList.add('highlight-comment');
                    setTimeout(() => targetEl.classList.remove('highlight-comment'), 2000);
                }
                return;
            }

            // User Delete
            const delBtn = target.closest('.delete-btn');
            if (delBtn) {
                const id = delBtn.dataset.id;
                ModAction.confirm('Delete Comment', `Are you sure you want to delete comment <strong>${id}</strong>?`, async () => {
                    const res = await fetch(`/api/comments/${id}/delete`, {
                        method: 'POST',
                        headers: {
                            'x-csrf-token': window.f0ckSession?.csrf_token
                        }
                    });
                    const json = await res.json();
                    if (json.success) {
                        const sidebarEl = document.getElementById('sc' + id);
                        if (sidebarEl) {
                            sidebarEl.style.transition = 'opacity 0.3s ease, transform 0.3s ease';
                            sidebarEl.style.opacity = '0';
                            sidebarEl.style.transform = 'scale(0.95)';
                            setTimeout(() => sidebarEl.remove(), 300);
                        }
                        if (window._sidebarActivityCache) {
                            window._sidebarActivityCache = window._sidebarActivityCache.filter(c => String(c.id) !== String(id));
                        }

                        const commentEl = document.getElementById(`c${id}`);
                        if (commentEl) {
                            commentEl.classList.add('deleted');
                            const contentEl = commentEl.querySelector('.comment-content');
                            if (contentEl) {
                                contentEl.innerHTML = '<span class="deleted-msg">[deleted]</span>';
                            }
                            const actionsEl = commentEl.querySelector('.comment-actions');
                            if (actionsEl) {
                                actionsEl.innerHTML = ''; // Remove reply/quote/admin buttons
                            }
                            // Remove attachments if present
                            const attachmentsEl = commentEl.querySelector('.comment-attachments, .media-pills');
                            if (attachmentsEl) attachmentsEl.remove();
                        } else {
                            this.loadComments();
                        }
                    } else {
                        throw new Error(json.message || 'Failed to delete');
                    }
                }, { hideReason: true });
                return;
            }

            // Admin Delete
            const adminDelBtn = target.closest('.admin-delete-btn');
            if (adminDelBtn) {
                if (typeof ModAction === 'undefined') return alert('Error: ModAction module not loaded. Are you a moderator?');

                const id = adminDelBtn.dataset.id;
                ModAction.confirm('Delete Comment', `Are you sure you want to delete comment <strong>${id}</strong>?`, async (reason) => {
                    const params = new URLSearchParams();
                    if (reason) params.append('reason', reason);

                    const res = await fetch(`/api/comments/${id}/delete`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
                        body: params
                    });
                    const json = await res.json();
                    if (json.success) {
                        const sidebarEl = document.getElementById('sc' + id);
                        if (sidebarEl) {
                            sidebarEl.style.transition = 'opacity 0.3s ease, transform 0.3s ease';
                            sidebarEl.style.opacity = '0';
                            sidebarEl.style.transform = 'scale(0.95)';
                            setTimeout(() => sidebarEl.remove(), 300);
                        }
                        if (window._sidebarActivityCache) {
                            window._sidebarActivityCache = window._sidebarActivityCache.filter(c => String(c.id) !== String(id));
                        }

                        const commentEl = document.getElementById(`c${id}`);
                        if (commentEl) {
                            commentEl.classList.add('deleted');
                            const contentEl = commentEl.querySelector('.comment-content');
                            if (contentEl) {
                                contentEl.innerHTML = '<span class="deleted-msg">[deleted]</span>';
                            }
                            const actionsEl = commentEl.querySelector('.comment-actions');
                            if (actionsEl) {
                                actionsEl.innerHTML = ''; // Remove reply/quote/admin buttons
                            }
                            // Remove attachments if present
                            const attachmentsEl = commentEl.querySelector('.comment-attachments, .media-pills'); // Try common classes
                            if (attachmentsEl) attachmentsEl.remove();
                        } else {
                            this.loadComments();
                        }
                    } else {
                        throw new Error(json.message || 'Failed to delete');
                    }
                }, { allowEmpty: window.f0ckSession?.is_admin });
                return;
            }

            // Admin Delete Attachment
            const adminDelAttachBtn = target.closest('.admin-delete-attachment-btn');
            if (adminDelAttachBtn) {
                const filename = adminDelAttachBtn.dataset.filename;
                if (filename) {
                    if (confirm('Are you sure you want to delete this attachment?')) {
                        const res = await fetch('/api/comments/attachment/delete', {
                            method: 'POST',
                            headers: {
                                'Content-Type': 'application/x-www-form-urlencoded',
                                'X-CSRF-Token': window.f0ckSession?.csrf_token
                            },
                            body: `filename=${encodeURIComponent(filename)}`
                        });
                        const json = await res.json();
                        if (json.success) {
                            const wrapper = adminDelAttachBtn.closest('.image-embed-wrap') || adminDelAttachBtn.closest('.video-embed-wrap') || adminDelAttachBtn.closest('.audio-embed-wrap');
                            if (wrapper) {
                                const span = document.createElement('span');
                                span.className = 'attachment-removed-text';
                                span.textContent = '[attachment removed]';
                                wrapper.parentNode.replaceChild(span, wrapper);
                            } else {
                                this.loadComments();
                            }
                        } else {
                            alert('Failed to delete attachment: ' + (json.message || 'Error'));
                        }
                    }
                }
                return;
            }

            // Admin Pin
            const adminPinBtn = target.closest('.admin-pin-btn');
            if (adminPinBtn) {
                const id = adminPinBtn.dataset.id;
                const res = await fetch(`/api/comments/${id}/pin`, {
                    method: 'POST',
                    headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token }
                });
                const json = await res.json();
                if (json.success) this.loadComments(id);
                else alert('Failed to pin: ' + (json.message || 'Error'));
                return;
            }

            // Admin Edit
            const adminEditBtn = target.closest('.admin-edit-btn');
            if (adminEditBtn) {
                const id = adminEditBtn.dataset.id;
                const currentContent = adminEditBtn.dataset.content;
                const commentEl = document.getElementById('c' + id);
                const contentEl = commentEl.querySelector('.comment-content');

                const originalHtml = contentEl.innerHTML;
                contentEl.innerHTML = `
                    <textarea class="edit-textarea">${currentContent}</textarea>
                    <div class="input-actions">
                        <button class="cancel-edit-btn" title="Cancel"><i class="fa-solid fa-comment-slash"></i></button>
                        <button class="save-edit-btn">Save</button>
                    </div>
                `;

                this.setupEmojiPicker(contentEl);

                // We don't need to bind events here because delegation handles the new buttons too!
                // But we need to store originalHtml somewhere or handle Cancel specifically?
                // Actually, for Cancel of edit, we need state? 
                // We can store original HTML in a dataset of the container or just re-render/reload?
                // Re-loading is safer for now, or we can use a closure if we attached listener here.
                // Since this is Delegation, "Cancel" needs to know what to restore.
                // Let's attach a "once" listener on the cancel button immediately here? 
                // NO, mixing strategies is confusing.
                // Let's put original HTML in dataset encoded? No, potentially large.
                // Simpler: On Cancel, just reload the comment (fetch or just re-render whole list).
                // Or: Attach a specialized listener strictly for this temporary element.

                const cancelBtn = contentEl.querySelector('.cancel-edit-btn');
                cancelBtn.onclick = () => { contentEl.innerHTML = originalHtml; };

                const saveBtn = contentEl.querySelector('.save-edit-btn');
                saveBtn.onclick = async () => {
                    const newContent = contentEl.querySelector('.edit-textarea').value;
                    if (!newContent.trim()) return alert('Cannot be empty');
                    const params = new URLSearchParams();
                    params.append('content', newContent);
                    const res = await fetch(`/api/comments/${id}/edit`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
                        body: params
                    });
                    const json = await res.json();
                    if (json.success) this.loadComments(id);
                    else alert('Failed to edit: ' + (json.message || 'Error'));
                };
                return;
            }

            // Reply
            // Reply Button (ID only)
            const replyBtn = target.closest('.reply-btn');
            if (replyBtn) {
                const id = replyBtn.dataset.id;
                const commentEl = replyBtn.closest('[id^="c"]');
                const body = commentEl ? commentEl.querySelector('.comment-body') : null;

                if (body) {
                    // Check if any reply input is ALREADY open
                    let textarea = document.querySelector('.comment-input.reply-input textarea');

                    // If none open, open the local one for this comment
                    if (!textarea && !body.querySelector('.reply-input')) {
                        const div = document.createElement('div');
                        div.innerHTML = this.renderInput(id);
                        body.appendChild(div.firstElementChild);
                        const newForm = body.querySelector('.reply-input');
                        this.setupEmojiPicker(newForm);
                        textarea = newForm.querySelector('textarea');
                    } else if (!textarea) {
                        textarea = body.querySelector('.reply-input textarea');
                    }

                    if (textarea) {
                        const quote = `>>${id} `;
                        const start = textarea.selectionStart;
                        const end = textarea.selectionEnd;
                        const val = textarea.value;
                        textarea.value = val.substring(0, start) + quote + val.substring(end);
                        textarea.focus({ preventScroll: true });
                        textarea.selectionStart = textarea.selectionEnd = start + quote.length;
                        textarea.dispatchEvent(new Event('input', { bubbles: true }));
                        this._scrollReplyIntoView(textarea);
                    }
                }
                return;
            }

            // Quote Button (Full Text Quote - Old Style)
            const quoteBtn = target.closest('.quote-btn');
            if (quoteBtn) {
                const id = quoteBtn.dataset.id;
                const body = quoteBtn.closest('.comment-body');
                if (body) {
                    this.quoteComment(id, quoteBtn, body);
                }
                return;
            }

            // Subscribe
            const subBtn = target.closest('#subscribe-btn');
            if (subBtn) {
                const isSubscribed = subBtn.textContent === 'Subscribed';
                subBtn.textContent = 'Wait...';
                try {
                    const res = await fetch(`/api/subscribe/${this.itemId}`, {
                        method: 'POST',
                        headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token }
                    });
                    const json = await res.json();
                    if (json.success) {
                        this.syncSubscribeButton(json.subscribed);
                        window.flashMessage((window.f0ckI18n && (json.subscribed ? window.f0ckI18n.subscribed_thread : window.f0ckI18n.unsubscribed_thread)) || (json.subscribed ? 'SUBSCRIBED TO THREAD' : 'UNSUBSCRIBED FROM THREAD'));
                    } else {
                        subBtn.textContent = isSubscribed ? 'Subscribed' : 'Subscribe';
                        alert('Failed to toggle subscription');
                    }
                } catch (e) {
                    subBtn.textContent = isSubscribed ? 'Subscribed' : 'Subscribe';
                }
                return;
            }

            // Lock
            const lockBtn = target.closest('#lock-thread-btn');
            if (lockBtn) {
                const action = this.isLocked ? 'unlock' : 'lock';
                lockBtn.disabled = true;
                const res = await fetch(`/api/comments/${this.itemId}/lock`, {
                    method: 'POST',
                    headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token }
                });
                const json = await res.json();
                lockBtn.disabled = false;
                if (json.success) {
                    this.isLocked = json.is_locked;
                    lockBtn.title = this.isLocked ? 'Unlock Thread' : 'Lock Thread';
                    window.flashMessage(this.isLocked ? 'THREAD LOCKED' : 'THREAD UNLOCKED');
                    this.loadComments();
                } else {
                    window.flashMessage(json.msg || 'Failed to lock/unlock', 3000, 'error');
                }
                return;
            }

            // Permalinks & Timestamp clicks
            if (target.classList.contains('comment-permalink') || target.closest('.comment-time')) {
                const el = target.closest('.comment-permalink, .comment-time');
                if (el) {
                    if (!window.f0ckSession || !window.f0ckSession.logged_in) {
                        return; // Let standard anchor link behavior scroll/jump to #c{id}
                    }
                    const id = el.dataset.id;
                    const commentEl = el.closest('[id^="c"]');
                    const body = commentEl ? commentEl.querySelector('.comment-body') : null;
                    if (body) {
                        e.preventDefault();
                        e.stopPropagation();
                        e.stopImmediatePropagation();
                        this.quoteComment(id, el, body);
                    }
                }
                return;
            }
        });
    }

    async handleSubmit(e) {
        const wrap = e.target.closest('.comment-input');
        const submitBtn = wrap.querySelector('.submit-comment');
        const textarea = wrap.querySelector('textarea');
        const parentId = wrap.dataset.parent || null;

        // Collect staged (not-yet-uploaded) files from preview items
        const stagedItems = [];
        const previewArea = wrap.querySelector('.comment-file-preview');
        if (previewArea) {
            previewArea.querySelectorAll('.cf-preview-item').forEach(item => {
                if (item._stagedFile) stagedItems.push(item);
            });
        }

        // Collect poll data from builder (if present)
        let pollPayload = null;
        const pollBuilder = wrap.querySelector('.poll-builder');
        if (pollBuilder) {
            const question = pollBuilder.querySelector('.poll-question-input')?.value.trim() || '';
            const options = [...pollBuilder.querySelectorAll('.poll-option-input')]
                .map(i => i.value.trim())
                .filter(Boolean);
            const isAnonymous = pollBuilder.querySelector('.poll-anon-checkbox')?.checked !== false;
            if (question && options.length >= 2) {
                pollPayload = { question, options, is_anonymous: isAnonymous };
            }
        }

        // Read current textarea value (we'll patch placeholder tokens after upload)
        let text = textarea.value.trim();

        if (!text && stagedItems.length === 0 && !pollPayload) {
            textarea.reportValidity();
            return;
        }
        if (submitBtn.classList.contains('loading') || submitBtn.disabled) return;
        if (window.f0ckSession?.is_anon && window.f0ckSession?.anon_permissions?.comment === false) {
            if (window.flashMessage) window.flashMessage('Anonymous commenting is disabled.', 3000, 'error');
            else alert('Anonymous commenting is disabled.');
            return;
        }

        // Captcha first when the site requires one for comments (cap.comments). Covers the attachment
        // uploads below too. The button stays disabled meanwhile so the form can't be sent twice.
        if (window.f0ckCaptchaGate) {
            submitBtn.disabled = true;
            try { await window.f0ckCaptchaGate('comment'); }
            catch { submitBtn.disabled = false; return; } // cancelled
            submitBtn.disabled = false;
        }

        // ── Upload all staged files now (at submit time) ───────────────────────
        const fileIds = [];
        const files = [];

        if (stagedItems.length > 0) {
            submitBtn.classList.add('loading');
            submitBtn.disabled = true;
            textarea.disabled = true;

            const session = window.f0ckSession || {};
            const csrf = session.csrf_token || '';
            const i18n = window.f0ckI18n || {};

            const uploadResults = await Promise.all(stagedItems.map(async (item) => {
                const file = item._stagedFile;
                const fd = new FormData();
                fd.append('file', file);
                try {
                    const res = await fetch('/api/v2/comments/upload', {
                        method: 'POST',
                        headers: { 'X-CSRF-Token': csrf },
                        body: fd
                    });
                    const json = await res.json();
                    if (json.success && json.files && json.files.length > 0) {
                        return { item, fileData: json.files[0] };
                    } else {
                        if (window.flashMessage) window.flashMessage(json.msg || (i18n.upload_failed || 'Upload failed'), 3000, 'error');
                        return { item, fileData: null };
                    }
                } catch (err) {
                    if (window.flashMessage) window.flashMessage((i18n.upload_failed || 'Upload failed') + ': ' + err.message, 3000, 'error');
                    return { item, fileData: null };
                }
            }));

            // Any upload that failed: abort the whole submit so the user can retry
            const anyFailed = uploadResults.some(r => !r.fileData);
            if (anyFailed) {
                submitBtn.classList.remove('loading');
                submitBtn.disabled = false;
                textarea.disabled = false;
                return;
            }

            // Patch placeholder tokens in the textarea with real URLs
            let currentText = textarea.value;
            for (const { item, fileData } of uploadResults) {
                const token = item.dataset.placeholderToken;
                const isSpoiler = item.classList.contains('cf-is-spoiler');
                const rawUrl = `/c/${fileData.dest}${fileData.converted_gif ? '#gif' : ''}`;
                const replacement = isSpoiler ? `[spoiler]${rawUrl}[/spoiler]` : rawUrl;
                if (token) currentText = currentText.replace(token, replacement);

                // Update preview item to reflect its uploaded state
                item.dataset.url = rawUrl;
                item.dataset.fileId = fileData.id;
                item.dataset.dest = fileData.dest;
                item.dataset.mime = fileData.mime;
                delete item._stagedFile;
                if (item._objectUrl) { URL.revokeObjectURL(item._objectUrl); delete item._objectUrl; }

                fileIds.push(String(fileData.id));
                files.push({
                    id: fileData.id,
                    dest: fileData.dest,
                    mime: fileData.mime,
                    original_filename: item.dataset.originalFilename
                });
            }
            textarea.value = currentText;
            text = currentText.trim();
        }

        // Start loading state
        submitBtn.classList.add('loading');
        submitBtn.disabled = true;
        textarea.disabled = true;
        const originalBtnHtml = null; // no longer needed — loading state is CSS-only

        if (window.f0ckDebugSpinner) {
            console.log('[DEBUG] window.f0ckDebugSpinner is true. Freezing spinner state for inspection.');
            return;
        }

        // Mark as pending to prevent state restoration while in flight
        if (parentId) {
            this.pendingSubmissions.add(parentId);
        } else {
            this.isMainSubmitting = true;
        }
        this._pendingSelfCommentTexts.add(text);

        let retryCount = 0;
        const maxRetries = 20; // Allow several minutes of retrying during restart

        const attemptSubmit = async () => {
            try {
                // Capture video timecode at the moment of submission
                const videoEl = document.querySelector('.v0ck video, .v0ck audio');
                const videoTime = (videoEl && isFinite(videoEl.duration) && videoEl.currentTime > 0)
                    ? videoEl.currentTime
                    : null;

                const params = new URLSearchParams();
                params.append('item_id', this.itemId);
                if (parentId) params.append('parent_id', parentId);
                params.append('content', text);
                if (videoTime !== null) params.append('video_time', videoTime.toFixed(3));

                if (fileIds.length > 0) {
                    params.append('file_ids', fileIds.join(','));
                }

                if (pollPayload) {
                    params.append('has_poll', '1');
                }

                let csrfToken = window.f0ckSession?.csrf_token || document.querySelector('meta[name="csrf-token"]')?.content || '';
                if (csrfToken) params.append('csrf_token', csrfToken);

                const fetchHeaders = {
                    'Content-Type': 'application/x-www-form-urlencoded',
                    ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
                };

                if ((!window.f0ckSession?.user || window.f0ckSession?.is_anon) && window.f0ckAnonSSH) {
                    try {
                        const ident = window.f0ckAnonSSH.getIdentity();
                        if (ident && ident.pubkey) {
                            const ts = Date.now();
                            const msg = `anon-auth:${ts}:${ident.pubkey}`;
                            const sig = await window.f0ckAnonSSH.sign(msg);
                            fetchHeaders['X-SSH-Pubkey'] = ident.pubkey;
                            fetchHeaders['X-SSH-Timestamp'] = String(ts);
                            fetchHeaders['X-SSH-Signature'] = sig;
                        }
                    } catch (e) {
                        console.warn('[ANON_COMMENTS] Failed to sign comment:', e);
                    }
                }

                let res = await fetch('/api/comments', {
                    method: 'POST',
                    headers: fetchHeaders,
                    body: params
                });

                // Auto-recovery if CSRF token drifted
                if (res.status === 403) {
                    const errJson = await res.clone().json().catch(() => ({}));
                    if (errJson.msg === 'Invalid CSRF token' || errJson.message === 'Invalid CSRF token') {
                        try {
                            const idRes = await fetch('/api/v2/anon/identity', { credentials: 'same-origin' });
                            const idData = await idRes.json();
                            if (idData && idData.csrf_token) {
                                if (window.f0ckSession) window.f0ckSession.csrf_token = idData.csrf_token;
                                const mCsrf = document.querySelector('meta[name="csrf-token"]');
                                if (mCsrf) mCsrf.content = idData.csrf_token;
                                params.set('csrf_token', idData.csrf_token);
                                fetchHeaders['X-CSRF-Token'] = idData.csrf_token;
                                res = await fetch('/api/comments', {
                                    method: 'POST',
                                    headers: fetchHeaders,
                                    body: params
                                });
                            }
                        } catch (e) {}
                    }
                }

                if (!res.ok) {
                    if (res.status >= 500) {
                        throw new Error(`Server returned ${res.status}`);
                    }
                    // For 4xx errors, we stop and show the error to user (likely validation or auth)
                    const json = await res.json().catch(() => ({}));
                    alert('Error: ' + (json.message || `Status ${res.status}`));
                    this._finishSubmit(submitBtn, originalBtnHtml, parentId, text);
                    return;
                }

                const json = await res.json();
                if (json.success) {
                    // Success cleanup
                    if (parentId) {
                        const formRow = wrap.closest('.reply-input');
                        if (formRow) formRow.remove();
                    } else {
                        if (textarea) textarea.value = '';
                        const counter = wrap.querySelector('.char-counter');
                        if (counter) {
                            counter.textContent = `0 / ${counter.dataset.max}`;
                            counter.classList.remove('near-limit', 'at-limit');
                        }
                        const fpArea = wrap.querySelector('.comment-file-preview');
                        if (fpArea) fpArea.innerHTML = '';
                        // Remove poll builder after posting
                        wrap.querySelector('.poll-builder')?.remove();
                        wrap.querySelector('.comment-poll-btn')?.classList.remove('active');
                    }

                    // If there was a poll, attach it now
                    const commentId = json.comment?.id;
                    if (pollPayload && commentId) {
                        const pfd = new FormData();
                        pfd.append('poll', JSON.stringify(pollPayload));
                        fetch(`/api/polls/attach/${commentId}`, {
                            method: 'POST',
                            headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                            body: new URLSearchParams(pfd)
                        }).then(r => r.json()).then(pData => {
                            if (pData.success && pData.poll) {
                                // Patch poll into newComment and update DOM
                                newComment.poll = pData.poll;
                                const commentEl = document.getElementById('c' + commentId);
                                if (commentEl) {
                                    const attachmentsEl = commentEl.querySelector('.comment-attachments');
                                    const contentEl = commentEl.querySelector('.comment-content');
                                    const insertAfter = attachmentsEl || contentEl;
                                    if (insertAfter) {
                                        const pollHtml = this.renderCommentPoll(pData.poll, commentId, window.f0ckSession?.user);
                                        insertAfter.insertAdjacentHTML('afterend', pollHtml);
                                    }
                                }
                            }
                        }).catch(() => {});
                    }

                    // Notify the right sidebar that a new comment was posted (silent refresh)
                    document.dispatchEvent(new CustomEvent('f0ck:commentPosted', {
                        detail: { item_id: this.itemId, comment_id: json.comment?.id, content: text }
                    }));

                    // ── Optimistic DOM inject ──────────────────────────────────────────
                    // Build a minimal comment object from known session + server data so we
                    // can insert the comment directly without replacing the entire container.
                    const session = window.f0ckSession || {};
                    const currentUsername = session.user || this.user;
                    let resolvedAvatar = null;
                    let resolvedAvatarFile = null;

                    if (session.avatar_file) resolvedAvatarFile = session.avatar_file;
                    else if (session.avatar) resolvedAvatar = session.avatar;

                    if (!resolvedAvatar && !resolvedAvatarFile && this.lastData && currentUsername) {
                        const existingByMe = this.lastData.find(c =>
                            c.username && c.username.toLowerCase() === currentUsername.toLowerCase()
                        );
                        if (existingByMe) {
                            resolvedAvatar = existingByMe.avatar || null;
                            resolvedAvatarFile = existingByMe.avatar_file || null;
                        }
                    }

                    if (!resolvedAvatar && !resolvedAvatarFile && currentUsername) {
                        const existingCommentEl = this.container.querySelector(`.comment-author[href="/user/${currentUsername}"]`);
                        if (existingCommentEl) {
                            const avatarImg = existingCommentEl.closest('.comment-body')?.previousElementSibling?.querySelector('img');
                            if (avatarImg) {
                                const src = avatarImg.getAttribute('src') || '';
                                if (src.startsWith('/a/')) resolvedAvatarFile = src.slice(3);
                                else if (src.startsWith('/t/') && src.endsWith('.webp')) resolvedAvatar = src.slice(3, -5);
                            }
                        }
                    }

                    let resolvedBannerFile = (json.comment?.banner_file && json.comment.banner_file !== 'null') ? json.comment.banner_file : ((session.banner_file && session.banner_file !== 'null') ? session.banner_file : null);
                    let resolvedBannerPosition = (json.comment?.banner_position && json.comment.banner_position !== 'null') ? json.comment.banner_position : ((session.banner_position && session.banner_position !== 'null') ? session.banner_position : null);
                    let resolvedBannerSize = (json.comment?.banner_size && json.comment.banner_size !== 'null') ? json.comment.banner_size : ((session.banner_size && session.banner_size !== 'null') ? session.banner_size : null);
                    let resolvedBannerRepeat = (json.comment?.banner_repeat && json.comment.banner_repeat !== 'null') ? json.comment.banner_repeat : ((session.banner_repeat && session.banner_repeat !== 'null') ? session.banner_repeat : null);

                    if (!resolvedBannerFile && this.lastData && currentUsername) {
                        const existingByMe = this.lastData.find(c =>
                            c.username && c.username.toLowerCase() === currentUsername.toLowerCase() && c.banner_file && c.banner_file !== 'null'
                        );
                        if (existingByMe) {
                            resolvedBannerFile = existingByMe.banner_file || null;
                            resolvedBannerPosition = existingByMe.banner_position || null;
                            resolvedBannerSize = existingByMe.banner_size || null;
                            resolvedBannerRepeat = existingByMe.banner_repeat || null;
                        }
                    }

                    if (!resolvedBannerFile && currentUsername) {
                        const existingCommentEl = document.querySelector(`.comment-author[href="/user/${currentUsername}"], .user-infobox-block[style*="--author-banner"]`);
                        if (existingCommentEl) {
                            const commentDiv = existingCommentEl.closest('.comment') || existingCommentEl.closest('.user-infobox-block');
                            if (commentDiv) {
                                const style = commentDiv.getAttribute('style') || '';
                                const bannerMatch = style.match(/--author-banner:\s*url\(['"]?\/a\/([^'"]+)['"]?\)/);
                                if (bannerMatch) {
                                    resolvedBannerFile = bannerMatch[1];
                                    const posMatch = style.match(/--author-banner-position:\s*([^;]+)/);
                                    if (posMatch) resolvedBannerPosition = posMatch[1].trim();
                                    const sizeMatch = style.match(/--author-banner-size:\s*([^;]+)/);
                                    if (sizeMatch) resolvedBannerSize = sizeMatch[1].trim();
                                    const repeatMatch = style.match(/--author-banner-repeat:\s*([^;]+)/);
                                    if (repeatMatch) resolvedBannerRepeat = repeatMatch[1].trim();
                                }
                            }
                        }
                    }

                    if (resolvedBannerFile && window.f0ckSession) {
                        window.f0ckSession.banner_file = resolvedBannerFile;
                        if (resolvedBannerPosition) window.f0ckSession.banner_position = resolvedBannerPosition;
                        if (resolvedBannerSize) window.f0ckSession.banner_size = resolvedBannerSize;
                        if (resolvedBannerRepeat) window.f0ckSession.banner_repeat = resolvedBannerRepeat;
                    }

                    const newComment = {
                        id: json.comment.id,
                        item_id: this.itemId,
                        parent_id: parentId ? parseInt(parentId, 10) : null,
                        content: text,
                        files: files,
                        created_at: json.comment.created_at || new Date().toISOString(),
                        username: currentUsername,
                        user_id: (json.comment && json.comment.user_id) || session.id || session.user_id || null,
                        display_name: session.display_name || null,
                        avatar: resolvedAvatar,
                        avatar_file: resolvedAvatarFile,
                        username_color: (json.comment && json.comment.username_color) || session.username_color || null,
                        banner_file: resolvedBannerFile,
                        banner_position: resolvedBannerPosition,
                        banner_size: resolvedBannerSize,
                        banner_repeat: resolvedBannerRepeat,
                        is_deleted: false,
                        is_pinned: false,
                        video_time: json.comment.video_time ?? null,
                        replies: [],
                        replyTo: null,
                        poll: null
                    };

                    if (json.comment?.id) {
                        this._selfPostedCommentIds.add(json.comment.id);
                    }
                    this._pendingSelfCommentTexts.delete(text);

                    // Danmaku: fire immediately (one-shot) + add to future rotation
                    if (window.danmakuInstance) {
                        window.danmakuInstance.fire(
                            text,
                            session.display_name || currentUsername || '?',
                            session.username_color || null
                        );
                        window.danmakuInstance.addItem({
                            id: newComment.id,
                            content: text,
                            video_time: newComment.video_time ?? null,
                            display_name: session.display_name || currentUsername || '?',
                            username_color: session.username_color || null
                        });
                    }

                    if (!this.lastData) this.lastData = [];
                    const existingInLastData = this.lastData.find(c => c.id === newComment.id);
                    if (existingInLastData) {
                        Object.assign(existingInLastData, newComment);
                    } else {
                        if (this.sort === 'new') this.lastData.unshift(newComment);
                        else this.lastData.push(newComment);
                    }

                    if (parentId) {
                        const existingReply = document.getElementById('c' + newComment.id);
                        if (existingReply) {
                            const parent = existingReply.parentNode;
                            const commentHtml = this.renderComment(existingInLastData || newComment, this.lastUserId, true);
                            const tmp = document.createElement('div');
                            tmp.innerHTML = commentHtml;
                            const newEl = tmp.firstElementChild;
                            if (newEl) {
                                parent.replaceChild(newEl, existingReply);
                                CommentSystem.playEmojiVideos(newEl);
                                requestAnimationFrame(() => {
                                    CommentSystem._animateNewComment(newEl, true);
                                    newEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
                                });
                            }
                        } else {
                            const parentEl = document.getElementById('c' + parentId);
                            if (parentEl) {
                                let repliesEl = parentEl.nextElementSibling;
                                if (!repliesEl || !repliesEl.classList.contains('comment-replies')) {
                                    repliesEl = document.createElement('div');
                                    repliesEl.className = 'comment-replies';
                                    parentEl.insertAdjacentElement('afterend', repliesEl);
                                }
                                const commentHtml = this.renderComment(newComment, this.lastUserId, true);
                                const tmp = document.createElement('div');
                                tmp.innerHTML = commentHtml;
                                const commentEl = tmp.firstElementChild;
                                if (commentEl) {
                                    repliesEl.appendChild(commentEl);
                                    this._ensureTruncationButton(commentEl, newComment.content);
                                    CommentSystem.playEmojiVideos(commentEl);
                                    requestAnimationFrame(() => {
                                        CommentSystem._animateNewComment(commentEl, true);
                                        commentEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
                                    });
                                }
                            }
                        }
                    } else {
                        const existingTop = document.getElementById('c' + newComment.id);
                        if (existingTop) {
                            const parent = existingTop.parentNode;
                            const commentHtml = this.renderComment(existingInLastData || newComment, this.lastUserId, false);
                            const tmp = document.createElement('div');
                            tmp.innerHTML = commentHtml;
                            const newEl = tmp.firstElementChild;
                            if (newEl) {
                                parent.replaceChild(newEl, existingTop);
                                CommentSystem.playEmojiVideos(newEl);
                                requestAnimationFrame(() => {
                                    CommentSystem._animateNewComment(newEl, true);
                                    newEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
                                });
                            }
                        } else {
                            const list = this.container.querySelector('.comments-list');
                            if (list) {
                                const commentHtml = this.renderComment(newComment, this.lastUserId, false);
                                const tmp = document.createElement('div');
                                tmp.innerHTML = commentHtml;
                                const commentEl = tmp.firstElementChild;
                                if (commentEl) {
                                    if (this.sort === 'new') {
                                        const scrollNav = list.querySelector('.scroll-nav-wrapper');
                                        if (scrollNav) scrollNav.insertAdjacentElement('afterend', commentEl);
                                        else list.prepend(commentEl);
                                    } else {
                                        list.appendChild(commentEl);
                                    }
                                    this._ensureTruncationButton(commentEl, newComment.content);
                                    CommentSystem.playEmojiVideos(commentEl);
                                    requestAnimationFrame(() => {
                                        CommentSystem._animateNewComment(commentEl, true);
                                        commentEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
                                    });
                                }
                            }
                        }
                    }

                    if (json.is_new_subscription) {
                        if (window.flashMessage) window.flashMessage((window.f0ckI18n && window.f0ckI18n.subscribed_thread) || 'SUBSCRIBED TO THREAD');
                        this.syncSubscribeButton(true);
                    }

                    // Update xD score badge immediately from the POST response —
                    // faster and more reliable than waiting for the SSE NOTIFY.
                    if (typeof json.xd_score === 'number' && typeof window.updateXdBadgeFromScore === 'function') {
                        window.updateXdBadgeFromScore(this.itemId, json.xd_score);
                    }

                    this._silentSync();
                    this._finishSubmit(submitBtn, originalBtnHtml, parentId, text);
                } else {
                    alert('Error: ' + json.message);
                    this._finishSubmit(submitBtn, originalBtnHtml, parentId, text);
                }
            } catch (err) {
                console.warn(`[CommentSystem] Submit attempt ${retryCount + 1} failed:`, err);
                if (retryCount < maxRetries) {
                    retryCount++;
                    // Randomized exponential backoff
                    const delay = Math.min(1000 * Math.pow(1.5, retryCount) + (Math.random() * 1000), 10000);
                    _f0ckDebug(`[CommentSystem] Retrying in ${Math.round(delay)}ms...`);
                    setTimeout(attemptSubmit, delay);
                } else {
                    alert('Failed to send comment after multiple attempts. Please check your connection.');
                    this._finishSubmit(submitBtn, originalBtnHtml, parentId, text);
                }
            }
        };

        attemptSubmit();
    }

    // Defensive: ensure the "show full comment" button is present in a rendered comment
    // element whenever the raw content exceeds ITEM_VIEW_MAX_CHARS. Called after every
    // optimistic DOM insert to guarantee the button regardless of rendering path.
    _ensureTruncationButton(commentEl, rawContent) {
        if (!rawContent) return;
        const lineCount = rawContent.split('\n').length;
        if (rawContent.length <= CommentSystem.ITEM_VIEW_MAX_CHARS && lineCount <= CommentSystem.ITEM_VIEW_MAX_LINES) {
            return;
        }
        const contentEl = commentEl.querySelector('.comment-content');
        if (!contentEl) { console.log('[ensureBtn] no .comment-content found'); return; }
        if (contentEl.querySelector('.load-full-comment-btn')) {
            console.log('[ensureBtn] button already present');
            return;
        }
        console.log('[ensureBtn] injecting button');
        const btnLabel = (window.f0ckI18n?.sidebar_show_full_comment) || 'show full comment';
        contentEl.insertAdjacentHTML('beforeend',
            `<span class="item-comment-truncated-notice"><button class="load-full-comment-btn" type="button">${btnLabel}</button></span>`
        );
        contentEl.dataset.raw = rawContent;
        console.log('[ensureBtn] done, button in DOM:', !!contentEl.querySelector('.load-full-comment-btn'));
    }

    _finishSubmit(btn, originalHtml, parentId, submittedText = null) {
        if (submittedText) {
            this._pendingSelfCommentTexts?.delete(submittedText);
        }
        if (parentId) {
            this.pendingSubmissions.delete(parentId);
        } else {
            this.isMainSubmitting = false;
        }

        // Surgical lookup of the active wrap in the live DOM
        let activeWrap = null;
        if (this.container) {
            if (parentId) {
                activeWrap = this.container.querySelector(`#c${parentId} .reply-input`);
            } else {
                activeWrap = this.container.querySelector('.main-input');
            }
        }

        const wrap = activeWrap || (btn ? btn.closest('.comment-input') : null);
        if (wrap) {
            const activeBtn = wrap.querySelector('.submit-comment');
            const activeTextarea = wrap.querySelector('textarea');
            if (activeBtn) {
                activeBtn.classList.remove('loading');
                activeBtn.disabled = false;
            }
            if (activeTextarea) {
                activeTextarea.disabled = false;
            }
        }
    }


    // Silently fetch fresh comment data from the server and update lastData
    // without touching the DOM — used after optimistic inserts to stay in sync.
    async _silentSync() {
        if (!this.itemId) return;
        try {
            const res = await fetch(`/api/comments/${this.itemId}?sort=${this.sort}`);
            const data = await res.json();
            if (data.success) {
                this.lastData = data.comments;
                this.lastUserId = data.user_id;
                this.lastIsSubscribed = data.is_subscribed;
                if (data.is_admin !== undefined) this.isAdmin = data.is_admin;
                if (data.is_locked !== undefined) this.isLocked = data.is_locked;
            }
        } catch (e) {
            // Non-critical: local cache may be slightly stale until next full refresh
            console.warn('[CommentSystem] Silent sync failed:', e);
        }
    }

    setupGlobalListeners() {
        if (CommentSystem.globalListenersAttached) return;
        CommentSystem.globalListenersAttached = true;

        // Refresh comments when the tab becomes visible again (e.g. switching back from another tab).
        // This handles cases where SSE events were missed while the tab was backgrounded,
        // and covers guests where NotificationSystem (which also does this) is absent.
        // Guard: ensure the current commentSystem's container is still in the live DOM
        // (avoids writing to detached nodes left behind by PJAX navigation).
        // Skip if the user is actively typing to avoid clobbering their draft.
        // Note: The visibility listener was moved to f0ckm.js NotificationSystem to centralize
        // tab-activation polling and avoid redundant network requests.

        window.addEventListener('hashchange', () => {
            if (window.commentSystem && location.hash && location.hash.startsWith('#c')) {
                const id = location.hash.substring(2);
                window.commentSystem.scrollToComment(id);
            }
        });

        window.addEventListener('f0ck:emojis_updated', () => {
            const cs = window.commentSystem;
            if (!cs) return;
            CommentSystem.emojiCache = null;
            CommentSystem.emojiPacks = null;
            CommentSystem.loadingEmojis = false;
            cs.loadEmojis(true); // force=true: bypass page-scan, admin just changed emojis
        });

        // Shortcut 'c' to toggle comments
        document.addEventListener('keydown', (e) => {
            if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
            const tag = e.target.tagName.toLowerCase();
            if (tag === 'input' || tag === 'textarea' || e.target.isContentEditable) return;

            if (e.key.toLowerCase() === 'c') {
                if (window.commentSystem) window.commentSystem.toggleComments();
            }
        });

        // Ctrl+. — focus the comment input
        document.addEventListener('keydown', (e) => {
            if (!e.ctrlKey || e.altKey || e.metaKey) return;
            if (e.key !== '.') return;

            const cs = window.commentSystem;
            if (!cs || !cs.container) return;

            e.preventDefault();

            // If comments are hidden, show them first
            const isHidden = cs.container.classList.contains('faded-out') || 
                             cs.container.classList.contains('is-hidden') ||
                             cs.container.style.display === 'none' ||
                             document.body.classList.contains('sidebar-left-hidden') ||
                             document.body.classList.contains('comments-hidden') ||
                             localStorage.getItem('comments_hidden') === 'true';
            if (isHidden) cs.toggleComments();

            // Legacy layout: comments are in the normal page flow under .item-main-content
            // Modern layout: comments are in a fixed-height sidebar column
            const isLegacyLayout = !!cs.container.closest('.item-main-content');

            // Wait a tick so the container is visible before trying to focus
            setTimeout(() => {
                const textarea = cs.container.querySelector('.main-input textarea');
                if (textarea) {
                    // In legacy layout, scroll page to bottom first so the box is fully visible
                    if (isLegacyLayout) {
                        window.scrollTo({ top: document.body.scrollHeight, behavior: 'smooth' });
                    }
                    textarea.focus();
                    if (!isLegacyLayout) {
                        textarea.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
                    }
                }
            }, isHidden ? 320 : 0); // 320ms matches the CSS fade-in transition
        });

        // Global subscribe button listener
        document.addEventListener('click', async (e) => {
            const subBtn = e.target.closest('#subscribe-btn');
            if (subBtn) {
                e.preventDefault();
                if (subBtn.style.opacity === '0.5') return;

                const itemId = subBtn.dataset.itemId || (window.commentSystem ? window.commentSystem.itemId : null);
                if (!itemId) return;

                subBtn.style.opacity = '0.5';
                try {
                    const res = await fetch(`/api/subscribe/${itemId}`, {
                        method: 'POST',
                        headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token }
                    });
                    const json = await res.json();
                    if (json.success && window.commentSystem) {
                        window.commentSystem.syncSubscribeButton(json.subscribed);
                        window.flashMessage((window.f0ckI18n && (json.subscribed ? window.f0ckI18n.subscribed_thread : window.f0ckI18n.unsubscribed_thread)) || (json.subscribed ? 'SUBSCRIBED TO THREAD' : 'UNSUBSCRIBED FROM THREAD'));
                    } else if (!json.success) {
                        alert('Failed to toggle subscription');
                    }
                } catch (e) {
                    console.error(e);
                } finally {
                    subBtn.style.opacity = '1';
                }
            }
        });

        // Global timestamp seek & scroll listener
        document.addEventListener('click', (e) => {
            const timestampLink = e.target.closest('.comment-timestamp');
            if (timestampLink) {
                e.preventDefault();
                e.stopPropagation();
                const seconds = parseFloat(timestampLink.dataset.time);
                if (!isNaN(seconds)) {
                    const mediaElement = document.querySelector('#my-video') || document.querySelector('audio#my-video');
                    if (mediaElement) {
                        mediaElement.currentTime = seconds;
                        if (mediaElement.paused) {
                            mediaElement.play().catch(() => {});
                        }
                        const playerContainer = mediaElement.closest('.v0ck') || mediaElement;
                        if (playerContainer) {
                            playerContainer.scrollIntoView({ behavior: 'smooth', block: 'center' });
                        }
                    }
                }
            }
        });
    }

    toggleComments() {
        if (!this.container) return;

        if (document.body.classList.contains('layout-modern') && !document.body.classList.contains('onara-modal-open')) {
            if (window.innerWidth < 1000) return;
            if (typeof window.toggleSidebarLeft === 'function') {
                window.toggleSidebarLeft();
                return;
            }
        }

        const layout = this.container.closest('.item-layout-container');
        const sidebar = this.container.closest('.item-sidebar-left');
        const siblings = sidebar ? [
            ...sidebar.querySelectorAll('.sidebar-tags-container, .tag-controls')
        ] : [];

        // Check if currently hidden (or slid out)
        const isHidden = this.container.classList.contains('faded-out') || 
                         this.container.classList.contains('is-hidden') ||
                         this.container.style.display === 'none' ||
                         document.body.classList.contains('sidebar-left-hidden') ||
                         document.body.classList.contains('comments-hidden') ||
                         localStorage.getItem('comments_hidden') === 'true';

        // Legacy layout: comments slide up behind the info box (and back down out of it)
        const reduceMotion = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
        if (document.body.classList.contains('layout-legacy') && typeof this.container.animate === 'function' && !reduceMotion) {
            // A slide still running decides the direction, since the classes only flip when it ends
            const running = this._legacySlide && this._legacySlide.el === this.container ? this._legacySlide : null;
            const hiddenNow = running ? running.dir === 'hide' : isHidden;
            this.slideLegacyComments(!hiddenNow, layout, sidebar, siblings);
            return;
        }

        if (isHidden) {
            // SHOW: expand grid first, then slide content in
            if (layout) layout.classList.remove('sidebar-hidden');
            document.body.classList.remove('sidebar-left-hidden', 'comments-hidden');
            this.container.classList.remove('is-hidden');
            this.container.style.removeProperty('display');
            this.container.style.display = '';
            if (sidebar) {
                sidebar.classList.remove('is-hidden');
                sidebar.style.removeProperty('display');
                sidebar.style.display = '';
            }
            localStorage.setItem('comments_hidden', 'false');
            void this.container.offsetWidth; // force reflow so transition fires
            this.container.classList.remove('faded-out');
            siblings.forEach(el => el.classList.remove('faded-out'));
        } else {
            // HIDE: mark state immediately so quick navigations stay hidden, fade out content, then collapse
            localStorage.setItem('comments_hidden', 'true');
            this.container.classList.add('faded-out');
            siblings.forEach(el => el.classList.add('faded-out'));
            setTimeout(() => {
                if (!this.container.classList.contains('faded-out')) return;
                this.container.classList.add('is-hidden');
                this.container.style.setProperty('display', 'none', 'important');
                if (sidebar) {
                    sidebar.classList.add('is-hidden');
                    sidebar.style.setProperty('display', 'none', 'important');
                }
                if (layout) layout.classList.add('sidebar-hidden');
                document.body.classList.add('sidebar-left-hidden', 'comments-hidden');
            }, 300);
        }
    }

    // Legacy layout slide: the container shrinks from the bottom up while its content moves up by the same
    // amount, so the comments look like they retract behind the box above them (reverse on show).
    // Pressing again mid-slide turns it around from where it is.
    slideLegacyComments(hide, layout, sidebar, siblings) {
        const el = this.container;
        const kids = Array.from(el.children);
        // Curtain feel: slow, soft start, gentle settle at the end
        const DURATION = 900;
        const EASING = 'cubic-bezier(0.45, 0, 0.2, 1)';

        let startH = null;
        if (this._legacySlide) {
            // Only turn around from mid-slide on the same item; a slide left over from a previous item is dropped
            if (this._legacySlide.el === el) startH = el.getBoundingClientRect().height;
            this._legacySlide.anims.forEach(a => a.cancel());
            this._legacySlide = null;
        }

        if (!hide) {
            // Put it back in the flow first (collapsed), so its full height can be measured
            if (layout) layout.classList.remove('sidebar-hidden');
            document.body.classList.remove('sidebar-left-hidden', 'comments-hidden');
            // Border stays until the comments are fully down again
            document.body.classList.add('comments-sliding');
            el.classList.remove('is-hidden', 'faded-out');
            el.style.removeProperty('display');
            if (sidebar) {
                sidebar.classList.remove('is-hidden');
                sidebar.style.removeProperty('display');
            }
            siblings.forEach(s => s.classList.remove('faded-out'));
            localStorage.setItem('comments_hidden', 'false');
            if (startH === null) startH = 0;
        } else {
            // Mark state immediately so quick navigations stay hidden
            localStorage.setItem('comments_hidden', 'true');
            document.body.classList.add('comments-sliding');
            if (startH === null) startH = el.getBoundingClientRect().height;
        }

        const fullH = el.scrollHeight;
        const endH = hide ? 0 : fullH;
        // Height + clipping go through a class and a registered custom property (see f0ckm.css): layouts
        // like the Onara modal pin the container with `height: auto !important; overflow: visible
        // !important`, which beats both an animated height and an inline overflow. The class rule is
        // more specific and !important, and its height reads the variable the animation drives.
        el.style.setProperty('--comments-slide-h', endH + 'px');
        el.classList.add('is-comments-sliding');

        const frames = (h0, h1) => [{ transform: 'translateY(' + (h0 - fullH) + 'px)' }, { transform: 'translateY(' + (h1 - fullH) + 'px)' }];
        // A turnaround covers only part of the distance, so it takes proportionally less time (same speed)
        const share = fullH > 0 ? Math.abs(endH - startH) / fullH : 1;
        const opts = { duration: Math.round(DURATION * Math.max(0.3, Math.min(1, share))), easing: EASING };
        const anims = [el.animate([{ '--comments-slide-h': startH + 'px' }, { '--comments-slide-h': endH + 'px' }], opts)];
        kids.forEach(k => anims.push(k.animate(frames(startH, endH), opts)));

        const slide = { dir: hide ? 'hide' : 'show', anims, el };
        this._legacySlide = slide;
        anims[0].onfinish = () => {
            if (this._legacySlide !== slide) return;
            this._legacySlide = null;
            el.classList.remove('is-comments-sliding');
            el.style.removeProperty('--comments-slide-h');
            if (!hide) {
                document.body.classList.remove('comments-sliding');
                return;
            }
            el.classList.add('faded-out', 'is-hidden');
            el.style.setProperty('display', 'none', 'important');
            siblings.forEach(s => s.classList.add('faded-out'));
            if (sidebar) {
                sidebar.classList.add('is-hidden');
                sidebar.style.setProperty('display', 'none', 'important');
            }
            if (layout) layout.classList.add('sidebar-hidden');
            document.body.classList.add('sidebar-left-hidden', 'comments-hidden');
            document.body.classList.remove('comments-sliding');
        };
    }



    escapeHtml(unsafe) {
        return unsafe
            .replace(/&/g, "&amp;")
            .replace(/</g, "&lt;")
            .replace(/>/g, "&gt;")
            .replace(/"/g, "&quot;")
            .replace(/'/g, "&#039;");
    }



    setupEmojiPicker(container) {
        const textarea = container.querySelector('textarea');
        if (!textarea) return;
        if (container.querySelector('.emoji-trigger')) return;

        // Attach mentions
        if (window.MentionAutocomplete) window.MentionAutocomplete.attach(textarea);

        // ── Inline emoji autocomplete ─────────────────────────────────────────
        // Portaled to <body> so it escapes parent stacking contexts (overflow,
        // z-index scopes) and always renders on top of tags/badges.
        const autocomplete = document.createElement('div');
        autocomplete.className = 'emoji-autocomplete';
        autocomplete.style.display = 'none';
        autocomplete.style.overscrollBehavior = 'contain';
        document.body.appendChild(autocomplete);

        let acActiveIdx = -1;
        let acMatches = [];
        let acDisplayedCount = 0;
        const BATCH_SIZE = 20;

        const positionAC = () => {
            const rect = textarea.getBoundingClientRect();
            autocomplete.style.position = 'fixed';
            autocomplete.style.left = rect.left + 'px';
            autocomplete.style.width = rect.width + 'px';
            // layout-modern: input is near the top of the sidebar → open downward
            if (document.body.classList.contains('layout-modern')) {
                autocomplete.style.top = rect.bottom + 'px';
                autocomplete.style.bottom = 'auto';
            } else {
                autocomplete.style.bottom = (window.innerHeight - rect.top) + 'px';
                autocomplete.style.top = 'auto';
            }
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

        const appendMoreItems = () => {
            if (acDisplayedCount >= acMatches.length) return;
            const nextBatch = acMatches.slice(acDisplayedCount, acDisplayedCount + BATCH_SIZE);
            nextBatch.forEach((name, i) => {
                const idx = acDisplayedCount + i;
                const item = document.createElement('div');
                item.className = 'emoji-ac-item';
                item.dataset.idx = idx;
                const url = this.customEmojis[name];
                let preview;
                if (url && url.endsWith('.webm')) {
                    preview = document.createElement('video');
                    preview.src = url;
                    preview.autoplay = true;
                    preview.loop = true;
                    preview.muted = true;
                    preview.playsInline = true;
                } else {
                    preview = document.createElement('img');
                    preview.src = url;
                    preview.alt = name;
                    preview.loading = 'lazy';
                }
                const label = document.createElement('span');
                label.textContent = `:${name}:`;
                item.appendChild(preview);
                item.appendChild(label);
                item.addEventListener('mousedown', ev => {
                    ev.preventDefault(); // don't blur textarea
                    insertEmoji(name);
                });
                autocomplete.appendChild(item);
            });
            acDisplayedCount += nextBatch.length;
        };

        const renderAC = () => {
            const hit = getColon();
            if (!hit || !this.customEmojis) return hideAC();
            const { query } = hit;
            acMatches = Object.keys(this.customEmojis).filter(n => n.includes(query));
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

        const insertEmoji = (name) => {
            const hit = getColon();
            if (!hit) return;
            const before = textarea.value.slice(0, hit.colonPos);
            const after = textarea.value.slice(textarea.selectionStart);
            const insert = `:${name}:`;
            textarea.value = before + insert + after;
            const newPos = hit.colonPos + insert.length;
            textarea.setSelectionRange(newPos, newPos);
            textarea.focus();
            hideAC();
        };

        textarea.addEventListener('input', () => {
            // Lazy-load emojis if user starts typing ':' (emoji autocomplete) but cache is empty
            if (!CommentSystem.emojiCache && !CommentSystem.loadingEmojis && getColon()) {
                this.loadEmojis(true); // force=true: user explicitly wants to pick an emoji
            }
            renderAC();
        });
        textarea.addEventListener('keydown', (e) => {
            if (autocomplete.style.display === 'none') return;
            if (e.key === 'ArrowDown') {
                e.preventDefault();
                // Load more if we are at the end of current displayed items
                if (acActiveIdx === acDisplayedCount - 1 && acDisplayedCount < acMatches.length) {
                    appendMoreItems();
                }
                const nextIdx = (acActiveIdx + 1) % acMatches.length;
                setActive(nextIdx);
            } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                const nextIdx = (acActiveIdx <= 0) ? acMatches.length - 1 : acActiveIdx - 1;
                // If jumping to the end, we must load everything (or enough)
                if (nextIdx > acDisplayedCount - 1) {
                    while (acDisplayedCount < acMatches.length) appendMoreItems();
                }
                setActive(nextIdx);
            } else if ((e.key === 'Enter' || e.key === 'Tab') && acActiveIdx >= 0) {
                e.preventDefault();
                insertEmoji(acMatches[acActiveIdx]);
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

        // Delay so mousedown on an item fires before blur removes the dropdown
        textarea.addEventListener('blur', () => setTimeout(hideAC, 150));
        textarea.addEventListener('click', () => { if (getColon()) renderAC(); });

        // Reposition when textarea is resized (user drags handle) or window resizes
        const ro = new ResizeObserver(() => {
            if (autocomplete.style.display !== 'none') positionAC();
        });
        ro.observe(textarea);
        const onWinResize = () => { if (autocomplete.style.display !== 'none') positionAC(); };
        window.addEventListener('resize', onWinResize);

        // Clean up portal element when this comment system is destroyed
        const _origDestroy = this.destroy?.bind(this);
        this.destroy = () => {
            if (typeof _origDestroy === 'function') _origDestroy();
            if (autocomplete && autocomplete.parentNode) autocomplete.parentNode.removeChild(autocomplete);
            window.removeEventListener('resize', onWinResize);
            ro.disconnect();
        };

        const trigger = document.createElement('button');
        trigger.innerHTML = '<i class="fa-regular fa-face-smile"></i>';
        trigger.className = 'emoji-trigger';

        const actions = container.querySelector('.input-actions');
        if (actions) {
            const referenceNode = actions.querySelector('.cancel-reply') ||
                actions.querySelector('.cancel-edit-btn') ||
                actions.querySelector('.submit-comment') ||
                actions.querySelector('.save-edit-btn');
            actions.insertBefore(trigger, referenceNode);

            // Prevent trigger from blurring the textarea, keeping the keyboard open
            trigger.addEventListener('mousedown', (e) => {
                e.preventDefault();
            });
            trigger.addEventListener('touchstart', (e) => {
                e.preventDefault();
                trigger.click();
            }, { passive: false });
            if (this.isAdmin) {
                const lockBtn = document.createElement('button');
                lockBtn.id = 'lock-thread-btn';
                lockBtn.title = this.isLocked ? 'Unlock Thread' : 'Lock Thread';
                lockBtn.innerHTML = this.isLocked ? this.icons.lock : this.icons.unlock;
                lockBtn.className = 'admin-lock-btn';
                actions.insertBefore(lockBtn, trigger);
            }

            // Create picker once and cache it
            let picker = null;
            let closeHandler = null;

            // ── Shared sticker hold-to-preview overlay ────────────────────────
            if (!window.stickerPreview) {
                // ── Toast notification (singleton) ──
                const toastEl = document.createElement('div');
                toastEl.className = 'emoji-fav-toast';
                document.body.appendChild(toastEl);
                let _toastTimer = null;
                const showToast = (msg) => {
                    clearTimeout(_toastTimer);
                    toastEl.textContent = msg;
                    toastEl.classList.remove('toast-out');
                    toastEl.classList.add('toast-in');
                    _toastTimer = setTimeout(() => {
                        toastEl.classList.replace('toast-in', 'toast-out');
                    }, 2000);
                };

                const overlay = document.createElement('div');
                overlay.className = 'sticker-preview-overlay';
                document.body.appendChild(overlay);
                let _hideTimer = null;
                window.stickerPreview = {
                    isShowing: false,
                    _autoHideTimer: null,
                    show(src, isVideo, name, url) {
                        clearTimeout(this._autoHideTimer);
                        clearTimeout(_hideTimer);
                        overlay.innerHTML = '';
                        const el = isVideo ? document.createElement('video') : document.createElement('img');
                        el.src = src;
                        if (isVideo) { el.autoplay = true; el.loop = true; el.muted = true; el.playsInline = true; }
                        overlay.appendChild(el);
                        // Fav button — always shown in overlay for easy mobile access
                        if (name && url && window._emojiPickerFavUtils) {
                            const isFav = window._emojiPickerFavUtils.has(name);
                            const favBtn = document.createElement('button');
                            favBtn.className = 'sticker-preview-fav-btn' + (isFav ? ' is-fav' : '');
                            favBtn.innerHTML = '<i class="' + (isFav ? 'fa-solid' : 'fa-regular') + ' fa-star"></i>';
                            favBtn.addEventListener('click', (e) => {
                                e.stopPropagation();
                                window._emojiPickerFavUtils.toggle(name, url);
                                const nowFav = window._emojiPickerFavUtils.has(name);
                                // Close preview immediately and show toast
                                window.stickerPreview.hide();
                                showToast(nowFav ? 'Emoji added to Favorites' : 'Emoji removed from Favorites');
                                window._emojiPickerRefresh?.();
                            });
                            favBtn.addEventListener('touchend', (e) => { e.preventDefault(); e.stopPropagation(); favBtn.click(); });
                            overlay.appendChild(favBtn);
                        }
                        overlay.classList.add('visible');
                        this.isShowing = true;
                        if (isVideo) el.play().catch(() => {});
                    },
                    hide() {
                        clearTimeout(this._autoHideTimer);
                        this._autoHideTimer = null;
                        this.isShowing = false;
                        overlay.classList.remove('visible');
                        _hideTimer = setTimeout(() => { overlay.innerHTML = ''; }, 200);
                    },
                    // Auto-close after delay (used on mobile so user has time to tap the fav button)
                    scheduleHide(delay) {
                        clearTimeout(this._autoHideTimer);
                        this._autoHideTimer = setTimeout(() => {
                            this._autoHideTimer = null;
                            this.hide();
                        }, delay);
                    }
                };
                // Expose showToast so fav-tab right-click can use it
                window._showEmojiToast = showToast;
                // Click/tap overlay background to dismiss
                overlay.addEventListener('click', () => window.stickerPreview.hide());
                // Desktop safety net: if mouse released outside the sticker, hide preview
                // (skipped if a mobile scheduleHide is already pending)
                document.addEventListener('mouseup', () => {
                    if (window.stickerPreview?.isShowing && !window.stickerPreview._autoHideTimer) {
                        window.stickerPreview.hide();
                    }
                });
            }

            const buildPickerContent = () => {
                if (!picker) return;
                picker.innerHTML = '';
                picker.classList.remove('emoji-picker-empty');

                const packs = CommentSystem.emojiPacks || [];
                const hasPacks = packs.length > 0;

                if (!this.customEmojis || Object.keys(this.customEmojis).length === 0) {
                    // Nothing to pick: a one-line notice instead of the full-height grid
                    picker.classList.add('emoji-picker-empty');
                    picker.innerHTML = '<div class="emoji-picker-notice">No custom emojis yet.'
                        + (this.isAdmin ? ' <a href="/admin/emojis">Add some in the admin panel</a>' : '')
                        + '</div>';
                    return;
                }

                // Helper: create img or video element for a given emoji URL
                // ── Favorites (singleton utils + context menu) ──────────────
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
                            // Sync to DB for logged-in users (fire-and-forget)
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
                                        // Re-render the open picker so DB favorites appear immediately
                                        window._emojiPickerRefresh?.();
                                    }
                                }).catch(e => console.warn('[EmojiFavs] syncFromDB failed:', e));
                        }
                    };
                    // Seed from DB on first open if logged in
                    if (window.f0ckSession?.logged_in) {
                        window._emojiPickerFavUtils.syncFromDB();
                    }
                }
                if (!window._favContextMenu) {
                    const menu = document.createElement('div');
                    menu.style.cssText = 'position:fixed;z-index:99999;background:var(--dropdown-bg,#222);border:1px solid var(--nav-border-color,#444);border-radius:7px;padding:4px 0;box-shadow:0 4px 20px rgba(0,0,0,0.5);display:none;min-width:180px;';
                    document.body.appendChild(menu);
                    window._favContextMenu = {
                        el: menu,
                        show(x, y, name, url, onToggle) {
                            const isFav = window._emojiPickerFavUtils.has(name);
                            menu.innerHTML = '';
                            const item = document.createElement('div');
                            item.style.cssText = 'padding:9px 16px;cursor:pointer;font-size:0.875em;color:var(--white,#fff);display:flex;align-items:center;gap:8px;border-radius:4px;margin:2px 4px;';
                            item.innerHTML = '<span style="font-size:1.1em;">' + (isFav ? '&#9733;' : '&#9734;') + '</span>' + (isFav ? 'Remove from Favorites' : 'Add to Favorites');
                            item.addEventListener('mouseenter', () => { item.style.background = 'rgba(255,255,255,0.09)'; });
                            item.addEventListener('mouseleave', () => { item.style.background = ''; });
                            item.addEventListener('click', (e) => { e.stopPropagation(); window._emojiPickerFavUtils.toggle(name, url); menu.style.display = 'none'; if (onToggle) onToggle(); });
                            menu.appendChild(item);
                            menu.style.left = x + 'px'; menu.style.top = y + 'px'; menu.style.display = 'block';
                            requestAnimationFrame(() => {
                                const r = menu.getBoundingClientRect();
                                if (r.right  > window.innerWidth)  menu.style.left = (x - r.width)  + 'px';
                                if (r.bottom > window.innerHeight) menu.style.top  = (y - r.height) + 'px';
                            });
                        },
                        hide() { menu.style.display = 'none'; }
                    };
                    document.addEventListener('click', () => window._favContextMenu.hide());
                    document.addEventListener('keydown', (e) => { if (e.key === 'Escape') window._favContextMenu.hide(); });
                }

                const makeEmojiEl = (url, name) => {
                    const isVideo = url && url.endsWith('.webm');
                    // media element (img or video)
                    let media;
                    if (isVideo) {
                        media = document.createElement('video');
                        media.src = url;
                        media.autoplay = true;
                        media.loop = true;
                        media.muted = true;
                        media.playsInline = true;
                    } else {
                        media = document.createElement('img');
                        media.src = url;
                        media.loading = 'lazy';
                    }
                    media.title = `:${name}:`;
                    media.onerror = () => { media.style.display = 'none'; };
                    media._stickerSrc = url;
                    media._stickerIsVideo = isVideo;
                    // Wrapper div — needed to position the fav badge overlay
                    const el = document.createElement('div');
                    el.className = 'ep-sticker-wrap';
                    el._stickerSrc = url;
                    el._stickerIsVideo = isVideo;
                    el.appendChild(media);
                    // Fav star badge
                    if (window._emojiPickerFavUtils?.has(name)) {
                        const badge = document.createElement('i');
                        badge.className = 'fa-solid fa-star ep-fav-badge';
                        el.appendChild(badge);
                    }
                    // Hold-to-preview (400ms) — on mobile, tap the ★ in the overlay to favorite
                    let holdTimer = null;
                    let holdFired = false;
                    let favHoldFired = false;
                    let isTouchHold = false;
                    const startHold = (fromTouch) => {
                        holdFired = false;
                        favHoldFired = false;
                        isTouchHold = !!fromTouch;
                        holdTimer = setTimeout(() => {
                            holdFired = true;
                            window.stickerPreview?.show(url, isVideo, name, url);
                        }, 800);
                    };
                    const cancelHold = () => { clearTimeout(holdTimer); isTouchHold = false; };
                    const endHold = () => {
                        clearTimeout(holdTimer);
                        if (holdFired || favHoldFired) {
                            holdFired = false;
                            favHoldFired = false;
                            el.addEventListener('click', (e) => {
                                e.stopImmediatePropagation();
                                e.preventDefault();
                            }, { once: true, capture: true });
                            if (!isTouchHold) {
                                // Desktop: hide immediately on mouse release
                                window.stickerPreview?.hide();
                            }
                            // Mobile: preview stays open until user taps the overlay
                        } else {
                            window.stickerPreview?.hide();
                        }
                        isTouchHold = false;
                    };
                    media.addEventListener('mousedown', () => startHold(false));
                    media.addEventListener('mouseup', endHold);
                    media.addEventListener('mouseleave', cancelHold);
                    media.addEventListener('mouseenter', () => {
                        if (window.stickerPreview?.isShowing) window.stickerPreview.show(url, isVideo, name, url);
                    });
                    media.addEventListener('touchstart', () => startHold(true), { passive: true });
                    media.addEventListener('touchmove', cancelHold, { passive: true });
                    media.addEventListener('touchend', endHold);
                    media.addEventListener('touchcancel', cancelHold);
                    // Right-click (desktop only) → favorites context menu
                    media.addEventListener('contextmenu', (e) => {
                        e.preventDefault();
                        e.stopPropagation();
                        // Never show the custom menu on touch devices
                        if (navigator.maxTouchPoints > 0) return;
                        if (holdFired || favHoldFired) return;
                        window._favContextMenu?.show(e.clientX, e.clientY, name, url, () => {
                            showTab(activeTabId);
                        });
                    });
                    return el;
                };

                if (!hasPacks) {
                    // Flat list — old behaviour for sites without packs
                    picker.classList.remove('has-tabs');
                    Object.keys(this.customEmojis).forEach(name => {
                        const url = this.customEmojis[name];
                        const el = makeEmojiEl(url, name);
                        el.onclick = (ev) => {
                            ev.stopPropagation();
                            const pos = textarea.selectionStart ?? textarea.value.length;
                            const val = textarea.value;
                            textarea.value = val.slice(0, pos) + `:${name}:` + val.slice(pos);
                            textarea.focus();
                            const newPos = pos + name.length + 2;
                            textarea.setSelectionRange(newPos, newPos);
                        };
                        picker.appendChild(el);
                    });
                    return;
                }

                // ── Tabbed picker with sticker packs ──
                picker.classList.add('has-tabs');

                // Tab bar
                const tabBar = document.createElement('div');
                tabBar.className = 'emoji-picker-tabs';

                // Emoji grid area
                const gridArea = document.createElement('div');
                gridArea.className = 'emoji-picker-grid';

                let activeTabId = null;

                const showTab = (packId) => {
                    activeTabId = packId;
                    window._emojiPickerRefresh = () => showTab(activeTabId);
                    tabBar.querySelectorAll('.ep-tab').forEach(t => t.classList.toggle('active', t.dataset.packId === String(packId ?? 'null')));

                    gridArea.innerHTML = '';
                    gridArea.classList.toggle('favs-active', packId === '__favs__');

                    // ── Favorites tab ──
                    if (packId === '__favs__') {
                        const favs = window._emojiPickerFavUtils?.get() || [];
                        if (!favs.length) {
                            const empty = document.createElement('div');
                            empty.style.cssText = 'grid-column:1/-1;text-align:center;padding:28px 12px;opacity:0.45;font-size:0.82em;line-height:1.5;';
                            empty.textContent = 'No favorites yet. Right-click any sticker to add it here.';
                            gridArea.appendChild(empty);
                        } else {
                            favs.forEach(({ name, url }) => {
                                const el = makeEmojiEl(url, name);
                                el.onclick = (ev) => {
                                    ev.stopPropagation();
                                    const pos = textarea.selectionStart ?? textarea.value.length;
                                    const val = textarea.value;
                                    textarea.value = val.slice(0, pos) + `:${name}:` + val.slice(pos);
                                    textarea.focus();
                                    const newPos = pos + name.length + 2;
                                    textarea.setSelectionRange(newPos, newPos);
                                };
                                gridArea.appendChild(el);
                            });
                        }
                        return;
                    }

                    const pack = packs.find(p => String(p.id ?? null) === String(packId ?? null)) || packs[0];
                    if (!pack) return;

                    pack.emojis.forEach(({ name, url }) => {
                        const el = makeEmojiEl(url, name);
                        el.onclick = (ev) => {
                            ev.stopPropagation();
                            const pos = textarea.selectionStart ?? textarea.value.length;
                            const val = textarea.value;
                            textarea.value = val.slice(0, pos) + `:${name}:` + val.slice(pos);
                            textarea.focus();
                            const newPos = pos + name.length + 2;
                            textarea.setSelectionRange(newPos, newPos);
                        };
                        gridArea.appendChild(el);
                    });

                    // Touch slide: switch preview immediately as finger moves over a new sticker
                    gridArea.addEventListener('touchmove', (e) => {
                        if (!window.stickerPreview?.isShowing) return;
                        const touch = e.touches[0];
                        const target = document.elementFromPoint(touch.clientX, touch.clientY);
                        if (target && target !== gridArea && (target.tagName === 'IMG' || target.tagName === 'VIDEO') && target._stickerSrc) {
                            window.stickerPreview.show(target._stickerSrc, target._stickerIsVideo);
                        }
                    }, { passive: true });
                };

                // Helper: reliable touch handling for tab buttons on mobile (supports hold-to-action)
                let lastHoldTime = 0;
                const addTabTouch = (btn, cb, onHold) => {
                    let ts = null;
                    let holdTimer = null;
                    let holdFired = false;
                    btn.addEventListener('touchstart', (e) => {
                        holdFired = false;
                        ts = { x: e.touches[0].clientX, y: e.touches[0].clientY };
                        if (onHold) {
                            holdTimer = setTimeout(() => {
                                holdFired = true;
                                lastHoldTime = Date.now();
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

                const toggleFavDefault = (tabEl) => {
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

                // Build tab buttons — Favorites first
                const favTab = document.createElement('button');
                favTab.className = 'ep-tab ep-tab-favs';
                favTab.dataset.packId = '__favs__';
                favTab.title = 'Favorites';
                favTab.type = 'button';
                favTab.innerHTML = '<i class="fa-solid fa-star" style="color:var(--emoji-fav-color,#f5c518);font-size:1.1em;"></i>';
                favTab.addEventListener('mousedown', e => e.preventDefault());
                favTab.addEventListener('click', () => showTab('__favs__'));
                addTabTouch(favTab, () => showTab('__favs__'), () => toggleFavDefault(favTab));
                // Right-click on fav tab → set/unset as default
                favTab.addEventListener('contextmenu', (e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (Date.now() - lastHoldTime < 1000) return;
                    toggleFavDefault(favTab);
                });
                // Mark as default if already set
                if (localStorage.getItem('f0ck_emoji_default_tab') === '__favs__') {
                    favTab.classList.add('ep-tab-default');
                }
                tabBar.appendChild(favTab);

                packs.forEach((pack, i) => {
                    const tab = document.createElement('button');
                    tab.className = 'ep-tab';
                    tab.dataset.packId = String(pack.id ?? null);
                    tab.title = pack.name || 'Emojis';
                    tab.type = 'button';
                    // Tab icon — always use a static img (thumb_url from import, or first non-video emoji)
                    if (pack.emojis && pack.emojis.length > 0) {
                        const iconUrl = pack.thumb_url
                            || (pack.emojis.find(e => !e.url.endsWith('.webm')) || pack.emojis[0]).url;
                        const icon = document.createElement('img');
                        icon.src = iconUrl;
                        icon.alt = pack.name;
                        icon.loading = 'lazy';
                        tab.appendChild(icon);
                    } else {
                        tab.textContent = '\u263A';
                    }
                    tab.addEventListener('mousedown', e => e.preventDefault());
                    tab.addEventListener('click', () => showTab(pack.id ?? null));
                    addTabTouch(tab, () => showTab(pack.id ?? null));
                    tabBar.appendChild(tab);
                });

                picker.appendChild(tabBar);
                picker.appendChild(gridArea);

                // Show default tab (favorites if set, otherwise first pack)
                const defaultTab = localStorage.getItem('f0ck_emoji_default_tab');
                showTab(defaultTab === '__favs__' ? '__favs__' : (packs[0]?.id ?? null));
            };

            // Helper: close picker with the out-animation, then hide
            const closePicker = () => {
                if (!picker || picker.style.display === 'none') return;
                trigger.classList.remove('is-active');
                picker.classList.add('picker-closing');
                picker.addEventListener('animationend', () => {
                    picker.style.display = 'none';
                    picker.classList.remove('picker-closing');
                }, { once: true });
            };

            // Bring the whole picker into view with some room below it. Measured once the enter animation has
            // finished (mid-animation it is shifted up and slightly squashed, so it would look smaller than it
            // is) and scrolled on whatever actually scrolls here (Onara modal, index container or the window).
            const revealPicker = () => {
                if (!picker || picker.style.display === 'none') return;
                let done = false;
                const run = () => {
                    if (done || !picker || picker.style.display === 'none') return;
                    done = true;
                    const MARGIN = 24;
                    const sc = window._f0ckScrollerFor ? window._f0ckScrollerFor(picker) : null;
                    const scRect = sc ? sc.getBoundingClientRect() : { top: 0, bottom: window.innerHeight };
                    const view = { top: scRect.top, bottom: scRect.bottom };
                    // A sticky footer (upload modal's submit bar) covers the bottom of the scroll area
                    const footer = picker.closest('form')?.querySelector('.upload-footer');
                    if (footer && getComputedStyle(footer).position === 'sticky') {
                        view.bottom = Math.min(view.bottom, footer.getBoundingClientRect().top);
                    }
                    const r = picker.getBoundingClientRect();
                    let delta = r.bottom + MARGIN - view.bottom;
                    // Never push the picker's own top out of view (very short viewports)
                    delta = Math.min(delta, r.top - view.top - 8);
                    if (delta > 0) (sc || window).scrollBy({ top: delta, behavior: 'smooth' });
                };
                picker.addEventListener('animationend', run, { once: true });
                setTimeout(run, 300);
            };

            trigger.addEventListener('click', (e) => {
                e.preventDefault();

                // Always kick off a load on first click (lazy — noop if already loading/cached)
                if (!CommentSystem.emojiCache && !CommentSystem.loadingEmojis) {
                    this.loadEmojis(true);
                    // onReady listener (set up during picker creation) handles populating
                }

                // If picker already exists, toggle visibility
                if (picker) {
                    const isVisible = picker.style.display !== 'none';
                    if (isVisible) {
                        closePicker();
                    } else {
                        buildPickerContent();
                        picker.style.display = '';
                        // Re-trigger enter animation
                        picker.classList.remove('picker-closing');
                        picker.style.animation = 'none';
                        picker.offsetHeight; // reflow
                        picker.style.animation = '';
                        trigger.classList.add('is-active');
                        revealPicker();
                    }
                    return;
                }

                // Create picker only once
                picker = document.createElement('div');
                picker.className = 'emoji-picker';
                trigger.classList.add('is-active');

                // Prevent picker interactions from blurring the textarea, keeping the keyboard open
                picker.addEventListener('mousedown', (e) => {
                    e.preventDefault();
                });

                // Differentiate touch-scrolling from tap-to-select
                let touchStartX = 0;
                let touchStartY = 0;
                let touchMoved = false;

                picker.addEventListener('touchstart', (e) => {
                    const touch = e.touches[0];
                    touchStartX = touch.clientX;
                    touchStartY = touch.clientY;
                    touchMoved = false;
                }, { passive: true });

                picker.addEventListener('touchmove', (e) => {
                    const touch = e.touches[0];
                    const diffX = Math.abs(touch.clientX - touchStartX);
                    const diffY = Math.abs(touch.clientY - touchStartY);
                    if (diffX > 10 || diffY > 10) {
                        touchMoved = true;
                    }
                }, { passive: true });

                picker.addEventListener('touchend', (e) => {
                    if (touchMoved) {
                        return; // Scroll gesture - let browser handle naturally
                    }
                    // Quick tap - prevent blur to keep keyboard open and click emoji
                    e.preventDefault();
                    const img = e.target.closest('img');
                    if (img) {
                        img.click();
                    }
                }, { passive: false });

                if (CommentSystem.emojiCache) {
                    // Emojis already cached — populate immediately
                    buildPickerContent();
                } else {
                    // Show a loading indicator while fetch is in-flight
                    picker.classList.add('emoji-picker-empty');
                    picker.innerHTML = '<div class="emoji-picker-notice">Loading emojis…</div>';
                    // Rebuild once the fetch completes (loadEmojis was already triggered above)
                    const onReady = () => {
                        window.removeEventListener('f0ck:emojis_ready', onReady);
                        buildPickerContent();
                    };
                    window.addEventListener('f0ck:emojis_ready', onReady, { once: true });
                }

                container.appendChild(picker);
                revealPicker();

            });
        }
    }
}

// Initial load
window.commentSystem = new CommentSystem();

// Re-init on navigation
document.addEventListener('f0ck:contentLoaded', () => {
    if (window.commentSystem && typeof window.commentSystem.destroy === 'function') {
        window.commentSystem.destroy();
    }
    window.commentSystem = new CommentSystem();
});

// If f0ck.js uses custom navigation without valid events, we might need MutationObserver or hook into `getContent`
// Looking at f0ck.js, it seems to just replace innerHTML.

(function initCommentHeightObserver() {
    if (typeof window === 'undefined') return;

    if (window.ResizeObserver) {
        const observer = new ResizeObserver(entries => {
            for (const entry of entries) {
                const height = entry.contentRect ? entry.contentRect.height : entry.target.offsetHeight;
                if (height > 90) {
                    entry.target.classList.add('tall-comment');
                } else {
                    entry.target.classList.remove('tall-comment');
                }
            }
        });

        const observeAll = (root = document) => {
            root.querySelectorAll('.comment').forEach(el => observer.observe(el));
        };

        if (document.readyState === 'loading') {
            document.addEventListener('DOMContentLoaded', () => observeAll());
        } else {
            observeAll();
        }

        if (window.MutationObserver) {
            const mutObs = new MutationObserver(mutations => {
                for (const m of mutations) {
                    m.addedNodes.forEach(node => {
                        if (node.nodeType === 1) {
                            if (node.classList && node.classList.contains('comment')) observer.observe(node);
                            if (node.querySelectorAll) node.querySelectorAll('.comment').forEach(el => observer.observe(el));
                        }
                    });
                }
            });
            mutObs.observe(document.body, { childList: true, subtree: true });
        }
    }
})();
