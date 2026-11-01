window.escapeHtmlUpload = window.escapeHtmlUpload || ((unsafe) => {
    return (unsafe || '').toString()
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;")
        .replace(/'/g, "&#039;");
});

// Manual approval: after an upload, send the uploader to their status page focused on the new item
window.f0ckGoToPending = window.f0ckGoToPending || ((id) => {
    const target = '/pending' + (id ? '#i' + id : '');
    if (typeof window.loadPageAjax === 'function') {
        window.loadPageAjax(target, false, { bypassCache: true, skipCache: true, skipInherit: true });
    } else {
        window.location.href = target;
    }
});

// ============================================================
// URL Upload Tracker — single panel, active jobs only
// ============================================================
(function () {
    const POLL_INTERVAL = 2000;
    const BC_CHANNEL    = 'f0ck_uut';
    const i18n = () => window.f0ckI18n || {};

    // active jobs: jobId → { url, percent, stage, speed, eta, resolved, pollTimer, _remote }
    const _active = new Map();

    // ── BroadcastChannel (cross-tab sync) ─────────────────────────────────────
    const _bc = typeof BroadcastChannel !== 'undefined' ? new BroadcastChannel(BC_CHANNEL) : null;
    const _broadcast = (msg) => { try { _bc?.postMessage(msg); } catch {} };

    let _closed = false;

    // ── Panel DOM ─────────────────────────────────────────────────────────────
    const _$ = id => document.getElementById(id);

    const _ensurePanel = () => {
        if (_$('url-upload-tracker')) return;
        const el = document.createElement('div');
        el.id = 'url-upload-tracker';
        el.innerHTML = `
            <div class="uut-header">
                <i class="fa-solid fa-arrow-up-from-bracket"></i>
                <span class="uut-title">${i18n().url_tracker_title || 'URL Uploads'}</span>
                <span class="uut-badge" id="uut-badge"></span>
                <button class="uut-btn" id="uut-btn-close" title="Close">×</button>
            </div>
            <div class="uut-body" id="uut-body">
                <div id="uut-active-list"></div>
            </div>
        `;
        document.body.appendChild(el);
        _$('uut-btn-close').addEventListener('click', () => {
            _closed = true;
            // Remove all resolved entries so they don't reappear next time
            for (const [id, j] of _active) { if (j.resolved) _active.delete(id); }
            _sync();
        });
    };

    // ── Visibility + badge ────────────────────────────────────────────────────
    const _sync = () => {
        _ensurePanel();
        const panel = _$('url-upload-tracker');
        panel.classList.toggle('uut-visible', _active.size > 0 && !_closed);

        const badge = _$('uut-badge');
        if (badge) {
            const active = [..._active.values()].filter(j => !j.resolved).length;
            badge.textContent = active || '';
            badge.style.display = active ? '' : 'none';
        }
    };

    // ── Active job row ────────────────────────────────────────────────────────
    const _renderJob = (jobId) => {
        _ensurePanel();
        const job = _active.get(jobId);
        const list = _$('uut-active-list');
        if (!list || !job) return;

        let row = list.querySelector(`[data-jid="${CSS.escape(jobId)}"]`);
        if (!row) {
            row = document.createElement('div');
            row.className = 'uut-job';
            row.dataset.jid = jobId;
            list.appendChild(row);
        }

        const pct = Math.min(Math.max(job.percent || 0, 0), 100);
        const t = i18n();
        const labels = {
            queued:      t.url_tracker_queued      || 'Queued',
            downloading: t.url_tracker_downloading || 'Downloading',
            analyzing:   t.url_tracker_analyzing   || 'Analyzing',
            saving:      t.url_tracker_saving      || 'Saving',
            processing:  t.url_tracker_processing  || 'Processing',
            extracting:  t.url_tracker_extracting  || 'Extracting',
        };
        const label  = labels[job.stage] || job.stage || 'Processing';
        const status = (job.stage === 'downloading' && pct > 0) ? `${label} ${pct.toFixed(1)}%` : label;
        const speed  = (job.speed && job.eta) ? `${job.speed} · ETA ${job.eta}` : (job.speed || '');

        row.innerHTML = `
            <div class="uut-job-row">
                <div class="uut-spinner"></div>
                <div class="uut-job-info">
                    <span class="uut-url" title="${window.escapeHtmlUpload(job.url)}">${window.escapeHtmlUpload(job.url)}</span>
                    <div class="uut-meta">
                        <span class="uut-stage">${status}</span>
                        ${speed ? `<span class="uut-speed">${speed}</span>` : ''}
                    </div>
                </div>
            </div>
            <div class="uut-bar"><div class="uut-bar-fill" style="width:${pct}%"></div></div>
        `;

        if (!job._remote) {
            _broadcast({ type: 'job_update', jobId, state: { url: job.url, percent: job.percent, stage: job.stage, speed: job.speed, eta: job.eta } });
        }
    };

    // ── Polling ───────────────────────────────────────────────────────────────
    const _stopPoll = (jobId) => {
        const j = _active.get(jobId);
        if (j?.pollTimer) { clearTimeout(j.pollTimer); j.pollTimer = null; }
    };

    const _startPolling = (jobId) => {
        const job = _active.get(jobId);
        if (!job || !jobId) return;
        const poll = async () => {
            if (!_active.has(jobId) || job.resolved) return;
            try {
                const resp = await fetch(`/api/v2/upload-url/progress/${jobId}?t=${Date.now()}`, { cache: 'no-store' });
                if (!resp.ok) { job.pollTimer = setTimeout(poll, POLL_INTERVAL); return; }
                const state = await resp.json();
                if (!state.success) { job.pollTimer = setTimeout(poll, POLL_INTERVAL); return; }
                if (state.percent !== undefined) job.percent = state.percent;
                if (state.stage)                job.stage   = state.stage;
                if (state.speed !== undefined)  job.speed   = state.speed;
                if (state.eta   !== undefined)  job.eta     = state.eta;
                _renderJob(jobId);
                if (state.done) {
                    job.pollTimer = null;
                    setTimeout(() => {
                        if (!job.resolved) {
                            _resolveJob(jobId, {
                                type: state.error ? 'upload_error' : 'upload_success',
                                item_id: state.itemId || null,
                                data: state.error ? { msg: state.error } : null
                            });
                        }
                    }, 500);
                    return;
                }
            } catch {}
            job.pollTimer = setTimeout(poll, POLL_INTERVAL);
        };
        job.pollTimer = setTimeout(poll, POLL_INTERVAL);
    };

    // ── Resolve ───────────────────────────────────────────────────────────────
    const _resolveJob = (jobId, notif) => {
        const job = _active.get(jobId);
        if (!job || job.resolved) return;
        job.resolved = true;
        _stopPoll(jobId);
        _broadcast({ type: 'job_resolved', jobId, notif });

        const success = notif.type === 'upload_success';

        if (success) {
            // Success: fade and remove immediately
            const row = _$('uut-active-list')?.querySelector(`[data-jid="${CSS.escape(jobId)}"]`);
            if (row) { row.classList.add('uut-job--fading'); setTimeout(() => row.remove(), 300); }
            setTimeout(() => { _active.delete(jobId); _sync(); }, 350);

            // Toast flash notification
            if (typeof window.flashMessage === 'function') {
                const msg = (window.f0ckI18n && window.f0ckI18n.notif_upload_success) || 'Your background upload is finished!';
                window.flashMessage(msg, 3500, 'success');
            }

            // Refresh notification bell count and items
            if (window.NotificationSystemInstance && typeof window.NotificationSystemInstance.pollDebounced === 'function') {
                window.NotificationSystemInstance.pollDebounced();
            }

            const itemId = notif.item_id;
            if (itemId && window.NotificationSystemInstance && typeof window.NotificationSystemInstance.handleNewItem === 'function') {
                const grid = document.querySelector('.posts');
                if (grid && !grid.querySelector(`a[href$="/${itemId}"]`)) {
                    fetch(`/api/v2/item/${itemId}`)
                        .then(r => r.json())
                        .then(res => {
                            if (res && res.success && res.rows) {
                                const item = res.rows;
                                window.NotificationSystemInstance.handleNewItem({
                                    id: item.id,
                                    dest: item.dest,
                                    mime: item.mime,
                                    username: item.username,
                                    display_name: item.display_name || null,
                                    tag_id: item.rating_tag_id || item.tag_id || 0,
                                    is_oc: !!item.is_oc,
                                    slug: item.slug,
                                    visibility: item.visibility || 0,
                                    is_album: !!item.is_album,
                                    album_count: item.album_count || 0
                                });
                            }
                        })
                        .catch(err => console.error('[UrlTracker] Failed to fetch item for live injection:', err));
                }
            }
        } else {
            // Failure: update the row to show the error, keep until user closes
            const row = _$('uut-active-list')?.querySelector(`[data-jid="${CSS.escape(jobId)}"]`);
            const errMsg = notif.data?.msg || i18n().url_tracker_failed || 'Upload failed';
            if (row) {
                const stageEl = row.querySelector('.uut-stage');
                if (stageEl) {
                    if (notif.item_id) {
                        stageEl.innerHTML = `${window.escapeHtmlUpload(errMsg)} (<a href="/${notif.item_id}" class="repost-link">view existing</a>)`;
                    } else {
                        stageEl.textContent = errMsg;
                    }
                    stageEl.style.color = '#ff5050';
                }
                const spinner = row.querySelector('.uut-spinner');
                if (spinner) spinner.style.display = 'none';
                const barFill = row.querySelector('.uut-bar-fill');
                if (barFill) { barFill.style.width = '100%'; barFill.style.background = '#ff5050'; barFill.style.backgroundImage = 'none'; barFill.style.animation = 'none'; }
            }
            _sync(); // update badge (count stays 0 for resolved jobs)

            if (typeof window.flashMessage === 'function') {
                window.flashMessage('✕ ' + errMsg, 4500, 'error');
            }
            if (window.NotificationSystemInstance && typeof window.NotificationSystemInstance.pollDebounced === 'function') {
                window.NotificationSystemInstance.pollDebounced();
            }
        }
    };

    // ── Public API ────────────────────────────────────────────────────────────
    const addJob = (url, jobId) => {
        if (!jobId) jobId = `local_${Date.now()}`;
        _ensurePanel();
        _closed = false;
        _active.set(jobId, { url, percent: 0, stage: 'queued', speed: null, eta: null, resolved: false, pollTimer: null });
        _renderJob(jobId);
        _sync();
        _startPolling(jobId);
        return jobId;
    };

    const resolveByJobId = (jobId, notif) => { if (_active.has(jobId)) { _resolveJob(jobId, notif); return true; } return false; };
    const resolveByUrl   = (notif, url)   => { for (const [id, j] of _active) { if (!j.resolved && j.url === url) { _resolveJob(id, notif); return true; } } return false; };
    const resolveOldest  = (notif)        => { for (const [id, j] of _active) { if (!j.resolved) { _resolveJob(id, notif); return true; } } return false; };

    // ── SSE Notifications ─────────────────────────────────────────────────────
    document.addEventListener('f0ck:upload_bg_update', (e) => {
        (e.detail || []).forEach(notif => {
            let matched = false;
            const jobId = notif.data?.jobId;
            if (jobId) matched = resolveByJobId(jobId, notif);
            if (!matched) { const url = notif.data?.url; if (url) matched = resolveByUrl(notif, url); }
        });
    });

    // ── BroadcastChannel ──────────────────────────────────────────────────────
    if (_bc) {
        _bc.onmessage = ({ data }) => {
            switch (data.type) {
                case 'hello': {
                    for (const [jobId, job] of _active) {
                        if (!job._remote) _broadcast({ type: 'job_update', jobId, state: { url: job.url, percent: job.percent, stage: job.stage, speed: job.speed, eta: job.eta } });
                    }
                    break;
                }
                case 'job_update': {
                    const { jobId, state } = data;
                    if (_active.has(jobId) && !_active.get(jobId)._remote) break;
                    if (!_active.has(jobId)) {
                        _active.set(jobId, { ...state, resolved: false, pollTimer: null, _remote: true });
                    } else {
                        Object.assign(_active.get(jobId), state);
                    }
                    _closed = false;
                    _renderJob(jobId);
                    _sync();
                    break;
                }
                case 'job_resolved': {
                    const job = _active.get(data.jobId);
                    if (!job?._remote) break;
                    job.resolved = true;
                    const n = data.notif || {};
                    if (n.type === 'upload_success') {
                        const row = _$('uut-active-list')?.querySelector(`[data-jid="${CSS.escape(data.jobId)}"]`);
                        if (row) { row.classList.add('uut-job--fading'); setTimeout(() => row.remove(), 300); }
                        setTimeout(() => { _active.delete(data.jobId); _sync(); }, 350);
                    } else {
                        const row = _$('uut-active-list')?.querySelector(`[data-jid="${CSS.escape(data.jobId)}"]`);
                        if (row) {
                            const errMsg = n.data?.msg || i18n().url_tracker_failed || 'Upload failed';
                            const stageEl = row.querySelector('.uut-stage');
                            if (stageEl) {
                                if (n.item_id) {
                                    stageEl.innerHTML = `${window.escapeHtmlUpload(errMsg)} (<a href="/${n.item_id}" class="repost-link">view existing</a>)`;
                                } else {
                                    stageEl.textContent = errMsg;
                                }
                                stageEl.style.color = '#ff5050';
                            }
                            const spinner = row.querySelector('.uut-spinner');
                            if (spinner) spinner.style.display = 'none';
                            const barFill = row.querySelector('.uut-bar-fill');
                            if (barFill) { barFill.style.width = '100%'; barFill.style.background = '#ff5050'; barFill.style.backgroundImage = 'none'; barFill.style.animation = 'none'; }
                        }
                        _sync();
                    }
                    break;
                }
            }
        };
    }

    // ── Init ──────────────────────────────────────────────────────────────────
    _broadcast({ type: 'hello' });

    window.urlUploadTracker = { addJob, resolveByJobId, resolveByUrl, resolveOldest };
})();





// Throttled queue to capture the first frame of video files asynchronously without blocking the browser
class VideoThumbnailQueue {
    constructor(concurrency = 3) {
        this.concurrency = concurrency;
        this.activeCount = 0;
        this.queue = [];
    }

    add(file, callback) {
        this.queue.push({ file, callback });
        this.next();
    }

    next() {
        if (this.activeCount >= this.concurrency || this.queue.length === 0) return;
        const { file, callback } = this.queue.shift();
        this.activeCount++;
        this.capture(file)
            .then(dataUrl => callback(dataUrl))
            .catch(err => {
                console.warn('[VideoThumbnailQueue] Error capturing thumbnail:', err);
                callback(null);
            })
            .finally(() => {
                this.activeCount--;
                this.next();
            });
    }

    capture(file) {
        return new Promise((resolve, reject) => {
            const video = document.createElement('video');
            const objectUrl = URL.createObjectURL(file);
            video.src = objectUrl;
            video.muted = true;
            video.playsInline = true;
            video.preload = 'metadata';

            let cleanedUp = false;
            const cleanup = () => {
                if (cleanedUp) return;
                cleanedUp = true;
                video.src = '';
                video.load();
                URL.revokeObjectURL(objectUrl);
            };

            video.onloadeddata = () => {
                video.currentTime = 0.1;
            };

            video.onseeked = () => {
                try {
                    const canvas = document.createElement('canvas');
                    const maxDim = 320;
                    let width = video.videoWidth || 160;
                    let height = video.videoHeight || 120;
                    if (width > maxDim || height > maxDim) {
                        if (width > height) {
                            height = Math.round((height * maxDim) / width);
                            width = maxDim;
                        } else {
                            width = Math.round((width * maxDim) / height);
                            height = maxDim;
                        }
                    }
                    canvas.width = width;
                    canvas.height = height;
                    const ctx = canvas.getContext('2d');
                    ctx.drawImage(video, 0, 0, width, height);
                    const dataUrl = canvas.toDataURL('image/jpeg', 0.8);
                    cleanup();
                    resolve(dataUrl);
                } catch (e) {
                    cleanup();
                    reject(e);
                }
            };

            video.onerror = () => {
                cleanup();
                reject(new Error('Video loading failed'));
            };

            setTimeout(() => {
                if (!cleanedUp) {
                    cleanup();
                    reject(new Error('Capture timeout'));
                }
            }, 8000);
        });
    }
}
const videoThumbnailQueue = new VideoThumbnailQueue(3);

window.initUploadForm = (selector) => {
    const form = (typeof selector === 'string') ? document.querySelector(selector) : selector;
    if (!form) return;

    // Prevent double-init
    if (form._f0ckInit) return form._f0ckUploader;
    form._f0ckInit = true;

    let isUploading = false;

    // Use querySelector to find elements within this specific form instance
    const fileInput = form.querySelector('.file-input');
    const dropZone = form.querySelector('.drop-zone');
    const filePreview = form.querySelector('.file-preview');
    const dropZonePrompt = dropZone?.querySelector('.drop-zone-prompt');
    const fileName = form.querySelector('.file-name');
    const fileSize = form.querySelector('.file-size');
    const removeFile = form.querySelector('.btn-remove');
    const tagInput = form.querySelector('.tag-input');
    const tagsList = form.querySelector('.tags-list');
    const tagsHidden = form.querySelector('.tags-hidden');
    const tagCount = form.querySelector('.tag-count');
    const tagSuggestions = form.querySelector('.tag-suggestions');
    const submitBtn = form.querySelector('.btn-upload');
    const progressContainer = form.querySelector('.upload-progress');
    const progressFill = form.querySelector('.progress-fill');
    const progressText = form.querySelector('.progress-text');
    const statusDiv = form.querySelector('.upload-status');
    const thumbSection = form.querySelector('#custom-thumbnail-section');
    const thumbInput = form.querySelector('#upload-thumbnail-input');
    const redirectCheckbox = form.querySelector('input[name="redirect_to_item"], #upload-redirect-checkbox');

    if (redirectCheckbox) {
        const savedRedirect = localStorage.getItem('upload_redirect_to_item');
        if (savedRedirect !== null) {
            redirectCheckbox.checked = (savedRedirect === 'true' || savedRedirect === '1');
        }
        redirectCheckbox.addEventListener('change', () => {
            localStorage.setItem('upload_redirect_to_item', redirectCheckbox.checked ? 'true' : 'false');
        });
    }

    // Capture the SSR-translated initial button text so updateSubmitButton
    // always uses the correct language regardless of window.f0ckI18n state.
    const btnTextEl = submitBtn ? submitBtn.querySelector('.btn-text') : null;
    const ssrSelectFileText = btnTextEl ? btnTextEl.textContent.trim() : null;

    // Close modal when clicking a link in statusDiv (e.g. "View existing")
    if (statusDiv) {
        statusDiv.addEventListener('click', (e) => {
            const link = e.target.closest('a');
            if (link) {
                const dragModal = form.closest('#upload-drag-modal');
                if (dragModal) {
                    dragModal.classList.remove('show');
                    document.body.classList.remove('modal-open');
                    if (form._f0ckUploader && typeof form._f0ckUploader.reset === 'function') {
                        form._f0ckUploader.reset();
                    }
                }
            }
        });
    }


    // URL mode elements
    const urlInput = form.querySelector('#url-upload-input');
    const urlBadge = form.querySelector('#url-type-badge');
    const modeTabs = form.querySelectorAll('.upload-mode-tab');
    const modeFile = form.querySelector('#mode-file');
    const modeUrl = form.querySelector('#mode-url');

    const ytRegex = /(?:youtube\.com\/\S*(?:(?:\/e(?:mbed))?\/|watch\/?\?(?:\S*?&?v\=))|youtu\.be\/)([a-zA-Z0-9_-]{6,11})/i;

    // Dynamically get min tags requirement from DOM
    const minTags = parseInt(form.getAttribute('data-min-tags') || '0');
    const commentMaxLenAttr = form.getAttribute('data-comment-max-length');
    const commentMaxLen = (commentMaxLenAttr && commentMaxLenAttr !== 'null') ? parseInt(commentMaxLenAttr) : null;

    const isShitpost = form.classList.contains('shitpost-mode-active') || !!window.f0ckShitpostMode;
    // Config-driven shitpost overrides
    const shitpostRequireRating = isShitpost && !!window.f0ckShitpostRequireRating;
    const shitpostMinTags = isShitpost ? (parseInt(window.f0ckShitpostMinTags) || 0) : 0;
    let tags = [];
    let autoTags = []; // Track tags suggested from metadata
    let selectedFiles = []; // Array of files for shitpost_mode
    let activeMode = 'file'; // 'file' or 'url'

    // Album mode state and helpers
    const albumChoiceContainer = form.querySelector('#album-choice-container');
    let albumChoiceMode = 'album'; // 'album' | 'batch'

    const isAlbumModeActive = () => {
        if (activeMode === 'album') {
            return selectedFiles.length > 0;
        }
        return activeMode === 'file' && selectedFiles.length > 1 && (albumChoiceMode === 'album' || !isShitpost);
    };

    if (albumChoiceContainer) {
        const btns = albumChoiceContainer.querySelectorAll('.album-choice-btn');
        btns.forEach(btn => {
            btn.addEventListener('click', (e) => {
                e.preventDefault();
                const choice = btn.getAttribute('data-choice');
                if (choice === albumChoiceMode) return;
                albumChoiceMode = choice;
                btns.forEach(b => b.classList.toggle('active', b === btn));
                selectedFiles.forEach(item => { delete item._rendered; });
                if (filePreview) filePreview.innerHTML = '';
                handleFile();
                updateSubmitButton();
            });
        });
    }
    // Shared emoji cache for per-item pickers (fetched once, reused by all items)
    let _emojiCache = null;
    let _emojiCachePromise = null;
    const getEmojis = () => {
        if (_emojiCache) return Promise.resolve(_emojiCache);
        if (_emojiCachePromise) return _emojiCachePromise;
        _emojiCachePromise = fetch('/api/v2/emojis')
            .then(r => r.json())
            .then(data => {
                if (data.success && data.emojis) {
                    _emojiCache = {};
                    data.emojis.forEach(e => { _emojiCache[e.name] = e.url; });
                }
                return _emojiCache || {};
            })
            .catch(() => ({}));
        return _emojiCachePromise;
    };
    const setupItemEmojiPicker = (textarea, triggerBtn) => {
        // Always use standalone picker for per-item comments
        let picker = null;
        triggerBtn.addEventListener('click', async (e) => {
            e.preventDefault();
            e.stopPropagation();
            if (picker) {
                picker.style.display = picker.style.display === 'none' ? '' : 'none';
                return;
            }
            const emojis = await getEmojis();
            if (!emojis || !Object.keys(emojis).length) return;
            picker = document.createElement('div');
            picker.className = 'emoji-picker item-emoji-picker';
            Object.keys(emojis).forEach(name => {
                const img = document.createElement('img');
                img.src = emojis[name];
                img.title = `:${name}:`;
                img.loading = 'lazy';
                img.onerror = () => { img.style.display = 'none'; };
                img.onclick = (ev) => {
                    ev.stopPropagation();
                    textarea.value += ` :${name}: `;
                    textarea.dispatchEvent(new Event('input'));
                    textarea.focus();
                };
                picker.appendChild(img);
            });
            const container = triggerBtn.closest('.item-comment-container');
            container.appendChild(picker);
            const closeHandler = (ev) => {
                if (!picker.contains(ev.target) && ev.target !== triggerBtn) {
                    picker.style.display = 'none';
                    document.removeEventListener('click', closeHandler);
                }
            };
            setTimeout(() => document.addEventListener('click', closeHandler), 0);
        });
    };

    // If shitpost mode is active, the global rating is hidden and per-item ratings are used.
    // We must remove the 'required' attribute from global radios to prevent browser from blocking submission.
    if (isShitpost) {
        form.querySelectorAll('input[name="rating"]').forEach(r => {
            r.required = false;
        });
    }

    // --- Emoji picker for upload comment ---
    const uploadCommentContainer = form.querySelector('.upload-comment-input');
    if (uploadCommentContainer) {
        const initUploadEmoji = async () => {
            // Reuse CommentSystem emoji picker if available
            if (window.commentSystem && typeof window.commentSystem.setupEmojiPicker === 'function') {
                // Ensure emojis are loaded (they won't be on upload page since no comments container)
                if (!CommentSystem.emojiCache || Object.keys(window.commentSystem.customEmojis).length === 0) {
                    await window.commentSystem.loadEmojis();
                }
                window.commentSystem.setupEmojiPicker(uploadCommentContainer);
                // Remove lock-thread button — doesn't belong on upload form
                const lockBtn = uploadCommentContainer.querySelector('#lock-thread-btn');
                if (lockBtn) lockBtn.remove();
            } else {
                // Standalone: load emojis and wire up trigger
                const textarea = uploadCommentContainer.querySelector('textarea');
                if (!textarea) return;

                let emojis = null;
                try {
                    const res = await fetch('/api/v2/emojis');
                    const data = await res.json();
                    if (data.success) {
                        emojis = {};
                        data.emojis.forEach(e => { emojis[e.name] = e.url; });
                    }
                } catch (e) { console.error('Failed to load emojis', e); }

                if (!emojis || Object.keys(emojis).length === 0) return;

                const actions = uploadCommentContainer.querySelector('.input-actions');
                if (!actions) return;

                const trigger = document.createElement('button');
                trigger.type = 'button';
                trigger.innerText = '☺';
                trigger.className = 'emoji-trigger';
                actions.prepend(trigger);

                let picker = null;
                trigger.addEventListener('click', (e) => {
                    e.preventDefault();
                    if (picker) {
                        picker.style.display = picker.style.display === 'none' ? '' : 'none';
                        return;
                    }
                    picker = document.createElement('div');
                    picker.className = 'emoji-picker';
                    Object.keys(emojis).forEach(name => {
                        const img = document.createElement('img');
                        img.src = emojis[name];
                        img.title = `:${name}:`;
                        img.loading = 'lazy';
                        img.onerror = () => { img.style.display = 'none'; };
                        img.onclick = (ev) => {
                            ev.stopPropagation();
                            textarea.value += ` :${name}: `;
                            textarea.focus();
                        };
                        picker.appendChild(img);
                    });
                    uploadCommentContainer.appendChild(picker);
                    const closeHandler = (ev) => {
                        if (!picker.contains(ev.target) && ev.target !== trigger) {
                            picker.style.display = 'none';
                            document.removeEventListener('click', closeHandler);
                        }
                    };
                    setTimeout(() => document.addEventListener('click', closeHandler), 0);
                });
            }
        };
        // Delay slightly to let CommentSystem init
        setTimeout(initUploadEmoji, 100);
    }

    // --- Mode tab switching ---
    if (modeTabs.length > 0) {
        modeTabs.forEach(tab => {
            tab.addEventListener('click', () => {
                const mode = tab.dataset.mode;
                if (mode === activeMode) return;
                const prevMode = activeMode;
                activeMode = mode;

                modeTabs.forEach(t => t.classList.remove('active'));
                tab.classList.add('active');

                if (modeFile) modeFile.style.display = (mode === 'file' || mode === 'album') ? '' : 'none';
                if (modeUrl) modeUrl.style.display = mode === 'url' ? 'block' : 'none';

                if (mode === 'album') {
                    albumChoiceMode = 'album';
                    form.classList.add('album-mode-active');
                    if (albumChoiceContainer) albumChoiceContainer.style.display = 'none';
                    if (fileInput) {
                        try {
                            const mimesObj = JSON.parse(form.getAttribute('data-mimes') || '{}');
                            fileInput.accept = Object.keys(mimesObj).join(',');
                        } catch {}
                    }
                    if (selectedFiles.length > 0) {
                        renderAlbumStaging();
                    }
                } else if (mode === 'file') {
                    if (fileInput) {
                        try {
                            const mimesObj = JSON.parse(form.getAttribute('data-mimes') || '{}');
                            fileInput.accept = Object.keys(mimesObj).join(',');
                        } catch {}
                    }
                    if (!isAlbumModeActive()) {
                        form.classList.remove('album-mode-active');
                        if (prevMode === 'album' && isShitpost && selectedFiles.length > 0) {
                            selectedFiles.forEach(item => { delete item._rendered; });
                            if (filePreview) filePreview.innerHTML = '';
                            handleFile();
                        }
                    }
                } else {
                    form.classList.remove('album-mode-active');
                }

                // Reset status
                if (statusDiv) {
                    statusDiv.textContent = '';
                    statusDiv.className = 'upload-status';
                }
                if (progressContainer) progressContainer.style.display = 'none';

                updateSubmitButton();
            });
        });
    }

    // --- Keyboard shortcuts for Upload Modal ---
    const handleUploadShortcuts = (e) => {
        // Only run if the modal is visible or if we are on the dedicated upload page
        const dragModal = form.closest('#upload-drag-modal');
        const isModalOpen = dragModal && dragModal.classList.contains('show');
        const isUploadPage = window.location.pathname === '/upload';
        
        if (!isModalOpen && !isUploadPage) return;

        // Don't trigger if user is typing in an input or textarea
        const tag = e.target.tagName;
        if (tag === 'INPUT' || tag === 'TEXTAREA' || e.target.isContentEditable) return;

        // One cycle per press
        if (e.repeat) return;

        const key = e.key.toLowerCase();

        // 'p' cycles ratings: SFW -> NSFW -> (NSFL) -> SFW
        if (key === 'p') {
            e.preventDefault();
            e.stopPropagation();

            const ratings = Array.from(form.querySelectorAll('input[name="rating"]'));
            if (ratings.length > 0) {
                const checkedIndex = ratings.findIndex(r => r.checked);
                let nextIndex = (checkedIndex + 1) % ratings.length;
                if (checkedIndex === -1) nextIndex = 0; // Default to first if none checked
                ratings[nextIndex].checked = true;
                ratings[nextIndex].dispatchEvent(new Event('change'));
            }
        }

        // 't' toggles modes: File <-> URL
        if (key === 't') {
            e.preventDefault();
            e.stopPropagation();

            const currentActiveTab = form.querySelector('.upload-mode-tab.active');
            const tabs = Array.from(form.querySelectorAll('.upload-mode-tab'));
            if (tabs.length > 1) {
                const currentIndex = tabs.indexOf(currentActiveTab);
                const nextIndex = (currentIndex + 1) % tabs.length;
                tabs[nextIndex].click();
                
                // Focus URL input if we switched to URL mode
                if (tabs[nextIndex].dataset.mode === 'url' && urlInput) {
                    setTimeout(() => urlInput.focus(), 50);
                }
            }
        }

        // 'Ctrl + .' focuses the comment input
        if (e.key === '.' && (e.ctrlKey || e.metaKey)) {
            const commentInput = form.querySelector('.upload-comment');
            if (commentInput) {
                e.preventDefault();
                e.stopPropagation();
                commentInput.focus();
            }
        }

        // 'i' focuses the tag input
        if (key === 'i') {
            const tagInput = form.querySelector('.tag-input');
            if (tagInput) {
                e.preventDefault();
                e.stopPropagation();
                tagInput.focus();
            }
        }

        // Block 'z' (Random) from triggering in the background
        if (key === 'z') {
            e.preventDefault();
            e.stopPropagation();
        }
    };

    window.addEventListener('keydown', handleUploadShortcuts, true); // Use capture phase to block theme cycling and rating toggle on the item page


    // --- URL type detection & Metadata extraction ---
    let metaDebounce = null;
    let lastFetchedUrl = '';
    // Cache: url -> string[] of suggestion fields (populated by fetchMetadata)
    const metaCache = new Map();

    // Extracts a flat list of suggestion strings from a /api/v2/meta/fetch response meta object
    const extractFieldsFromMeta = (meta) => {
        const fields = [];
        const push = (v) => { if (v && typeof v === 'string' && v.trim()) fields.push(v.trim()); };
        const pushArr = (arr) => { if (Array.isArray(arr)) arr.forEach(push); };

        // Title (prefer og_title, strip site name suffix)
        let bestTitle = meta.og_title || meta.twitter_title || meta.ld_headline || meta.title || null;
        if (bestTitle && meta.site_name) {
            const siteRegex = new RegExp(`\\s*[-|·]\\s*${meta.site_name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`, 'i');
            bestTitle = bestTitle.replace(siteRegex, '').trim();
        }
        push(bestTitle);
        push(meta.site_name);
        push(meta.og_desc || meta.description || meta.twitter_desc || meta.ld_desc);
        push(meta.author || meta.article_author || meta.book_author || meta.music_musician || meta.ld_author || meta.ld_creator);
        push(meta.profile_name);
        push(meta.profile_username);
        pushArr(meta.keywords);
        pushArr(meta.news_keywords);
        pushArr(meta.ld_keywords);
        pushArr(meta.article_tags);
        push(meta.article_section);
        push(meta.category);
        push(meta.genre || meta.ld_genre);
        push(meta.og_type);
        pushArr(meta.video_tags);
        push(meta.music_album);
        push(meta.music_song);
        pushArr(meta.book_tags);
        pushArr(meta.twitter_labels);
        push(meta.twitter_creator);
        push(meta.ld_name);
        push(meta.ld_about);
        if (meta.price_amount) push(`${meta.price_amount}${meta.price_currency ? ' ' + meta.price_currency : ''}`);
        push(meta.language);
        return [...new Set(fields)];
    };

    // Try YouTube oEmbed directly from the client browser (bypasses Tor/proxy consent walls).
    // Returns { success, meta } matching the server's /meta/fetch response shape, or null on failure.
    const tryClientYouTubeOembed = async (url) => {
        try {
            const oembedUrl = `https://www.youtube.com/oembed?url=${encodeURIComponent(url)}&format=json`;
            const resp = await fetch(oembedUrl);
            if (!resp.ok) return null;
            const data = await resp.json();
            if (!data.title) return null;
            const meta = {
                title: data.title,
                site_name: 'youtube.com',
                author: data.author_name || 'Unknown'
            };
            // Push to server cache so future requests (sidebar, other users) don't need Tor
            try {
                const csrf = window.f0ckSession?.csrf_token;
                fetch('/api/v2/meta/cache', {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                        ...(csrf ? { 'X-CSRF-Token': csrf } : {})
                    },
                    body: JSON.stringify({ url, meta })
                }).catch(() => {}); // fire-and-forget
            } catch (_) {}
            return { success: true, meta };
        } catch (e) {
            return null;
        }
    };

    if (urlInput) {
        const fetchMetadata = async (url) => {
            const currentVal = urlInput.value.trim();
            if (!currentVal || currentVal !== url) return;

            const isDirectMedia = /\.(mp4|webm|mp3|ogg|opus|flac|m4a|mkv|jpg|jpeg|png|gif|webp|swf)$/i.test(currentVal);
            if (isDirectMedia) return;

            lastFetchedUrl = currentVal;

            // In shitpost mode: fetch silently (cache only, badge stays hidden)
            if (isShitpost) {
                try {
                    // YouTube: try client-side oEmbed first (avoids Tor consent walls)
                    let data = ytRegex.test(currentVal) ? await tryClientYouTubeOembed(currentVal) : null;
                    if (!data) {
                        const resp = await fetch(`/api/v2/meta/fetch?url=${encodeURIComponent(currentVal)}`);
                        data = await resp.json();
                    }
                    if (data.success && data.meta) {
                        const fields = extractFieldsFromMeta(data.meta);
                        metaCache.set(currentVal, fields);
                        fields.forEach(v => addMetaSuggestion(v));
                    }
                } catch (e) { console.error('[META FETCH ERROR]', e); }
                return;
            }

            if (urlBadge) {
                const originalText = urlBadge.textContent;
                const originalClass = urlBadge.className;
                const originalDisplay = urlBadge.style.display;
                urlBadge.innerHTML = '<span class="loading-spinner"></span> Fetching...';
                urlBadge.className = 'url-type-badge fetching';
                urlBadge.style.display = 'flex';

                try {
                    // YouTube: try client-side oEmbed first (avoids Tor consent walls)
                    let data = ytRegex.test(currentVal) ? await tryClientYouTubeOembed(currentVal) : null;
                    if (!data) {
                        const resp = await fetch(`/api/v2/meta/fetch?url=${encodeURIComponent(currentVal)}`);
                        data = await resp.json();
                    }
                    if (data.success && data.meta) {
                        const fields = extractFieldsFromMeta(data.meta);
                        // Cache the fields for instant use when item is committed
                        metaCache.set(currentVal, fields);
                        fields.forEach(v => addMetaSuggestion(v));

                        urlBadge.innerHTML = '✓ Metadata Fetched';
                        urlBadge.className = 'url-type-badge success';
                        urlBadge.style.display = 'block';
                        setTimeout(() => {
                            if (urlInput.value.trim() === currentVal) {
                                urlBadge.textContent = originalText;
                                urlBadge.className = originalClass;
                                urlBadge.style.display = originalDisplay;
                            }
                        }, 2500);
                    } else {
                        // Restore on fail
                        urlBadge.textContent = originalText;
                        urlBadge.className = originalClass;
                        urlBadge.style.display = originalDisplay;
                    }
                } catch (e) {
                    console.error('[META FETCH ERROR]', e);
                    urlBadge.textContent = originalText;
                    urlBadge.className = originalClass;
                    urlBadge.style.display = originalDisplay;
                }
            }
        };

        urlInput.addEventListener('input', () => {
            const val = urlInput.value.trim();
            if (!val) {
                if (urlBadge) urlBadge.style.display = 'none';
                lastFetchedUrl = '';
                updateSubmitButton();
                return;
            }

            // Update badge for URL type (non-shitpost only)
            if (urlBadge && !isShitpost) {
                if (ytRegex.test(val) && window.f0ckEnableYoutubeUpload !== false) {
                    urlBadge.textContent = '▶ YouTube';
                    urlBadge.className = 'url-type-badge youtube';
                    urlBadge.style.display = 'block';
                    urlBadge.title = 'Click to re-fetch metadata';
                } else if (/^https?:\/\//i.test(val)) {
                    urlBadge.textContent = 'URL';
                    urlBadge.className = 'url-type-badge direct';
                    urlBadge.style.display = 'block';
                    urlBadge.title = 'Click to re-fetch metadata';
                } else {
                    urlBadge.style.display = 'none';
                }
            }

            // Trigger metadata fetch if it looks like a valid URL and not already fetching/fetched
            const isDirectMedia = /\.(mp4|webm|mp3|ogg|opus|flac|m4a|mkv|jpg|jpeg|png|gif|webp|swf)$/i.test(val);
            if (/^https?:\/\//i.test(val) && val !== lastFetchedUrl && !isDirectMedia) {
                clearTimeout(metaDebounce);
                metaDebounce = setTimeout(() => fetchMetadata(val), 800);
            }

            updateSubmitButton();
        });

        // In shitpost mode: auto-commit each pasted URL to the list immediately
        urlInput.addEventListener('paste', (e) => {
            if (!isShitpost) return;
            e.preventDefault();
            const pasted = (e.clipboardData || window.clipboardData).getData('text');
            const lines = pasted.split(/[\n\r]+/).map(u => u.trim()).filter(u => /^https?:\/\//i.test(u));
            if (!lines.length) {
                // Not a URL — let the browser insert it normally into the input
                urlInput.value = pasted.trim();
                urlInput.dispatchEvent(new Event('input'));
                return;
            }
            lines.forEach(url => {
                if (!selectedFiles.some(item => item.type === 'url' && item.url === url)) {
                    selectedFiles.push({ type: 'url', url, rating: '', visibility: '', tags: [], comment: '', title: '', is_oc: false });
                }
            });
            urlInput.value = '';
            if (urlBadge) urlBadge.style.display = 'none';
            handleFile();
        });

        if (urlBadge && !isShitpost) {
            urlBadge.addEventListener('click', () => {
                const val = urlInput.value.trim();
                if (val && /^https?:\/\//i.test(val)) {
                    lastFetchedUrl = ''; // Force retry
                    fetchMetadata(val);
                }
            });
        }

        // Add URL button (shitpost mode) — commits the typed/pasted URL to the list
        const addUrlFn = () => {
            const val = urlInput.value.trim();
            if (!val || !/^https?:\/\//i.test(val)) return;
            if (!selectedFiles.some(item => item.type === 'url' && item.url === val)) {
                selectedFiles.push({ type: 'url', url: val, rating: '', visibility: '', tags: [], comment: '', title: '', is_oc: false });
            }
            urlInput.value = '';
            if (urlBadge) urlBadge.style.display = 'none';
            handleFile();
        };

        const btnAddUrls = form.querySelector('.btn-add-urls');
        if (btnAddUrls) {
            btnAddUrls.addEventListener('click', addUrlFn);
        }

        // Also commit on Enter key inside the URL input in shitpost mode
        if (isShitpost) {
            urlInput.addEventListener('keydown', (e) => {
                if (e.key === 'Enter') { e.preventDefault(); addUrlFn(); }
            });
        }

    }

    const formatSize = (bytes) => {
        const units = ['B', 'KB', 'MB', 'GB'];
        let i = 0;
        while (bytes >= 1024 && i < units.length - 1) {
            bytes /= 1024;
            i++;
        }
        return bytes.toFixed(2) + ' ' + units[i];
    };

    const updateSubmitButton = () => {
        if (isUploading) {
            if (submitBtn) submitBtn.disabled = true;
            return;
        }

        const isShitpost = !!window.f0ckShitpostMode;
        const isAlbum = isAlbumModeActive();
        const isAlbumTab = activeMode === 'album';
        const isAlbumActive = isAlbum || isAlbumTab;

        form.classList.toggle('album-mode-active', isAlbumActive);

        const rating = form.querySelector('input[name="rating"]:checked');
        
        // In Shitpost Mode, ratings are per-item unless album mode is active
        let hasRating = true;
        if (isShitpost && !isAlbumActive && activeMode === 'file') {
            if (shitpostRequireRating) {
                hasRating = selectedFiles.length > 0 && selectedFiles.every(item => ['sfw', 'nsfw', 'nsfl'].includes(item.rating));
            }
        } else {
            hasRating = (rating !== null);
        }
        
        let hasTags = true;
        if (!isShitpost || isAlbumActive) {
            hasTags = tags.length >= minTags;
        } else if (shitpostMinTags > 0 && activeMode === 'file') {
            // In shitpost file mode with min-tags enforced: every queued item must meet the threshold.
            hasTags = selectedFiles.length === 0 || selectedFiles.every(item => (item.tags || []).length >= shitpostMinTags);
        }

        // Toggle visibility of global rating/comment/tag sections
        const ratingSec = form.querySelector('.global-rating-section');
        const commentSec = form.querySelector('.global-comment-section');
        const tagsSec = form.querySelector('.global-tag-section');
        const ocSec = form.querySelector('.global-oc-section');
        const titleSec = form.querySelector('.global-title-section');
        const formActions = form.querySelector('.form-actions');
        if (isShitpost) {
            if (formActions) {
                formActions.style.display = (activeMode === 'url' && !isAlbumActive) ? 'none' : 'block';
            }
            const hide = activeMode === 'file' && !isAlbumActive;
            const disp = hide ? 'none' : 'block';
            
            if (ratingSec) {
                ratingSec.style.display = disp;
                ratingSec.querySelectorAll('input').forEach(i => {
                    i.disabled = hide;
                    i.required = !hide;
                });
            }
            if (commentSec) {
                commentSec.style.display = disp;
                commentSec.querySelectorAll('textarea').forEach(i => i.disabled = hide);
            }
            if (tagsSec) {
                tagsSec.style.display = disp;
                tagsSec.querySelectorAll('input').forEach(i => i.disabled = hide);
            }
            if (ocSec) {
                ocSec.style.display = isAlbumActive ? 'block' : 'none';
                ocSec.querySelectorAll('input').forEach(i => i.disabled = !isAlbumActive);
            }
            if (titleSec) {
                titleSec.style.display = isAlbumActive ? 'block' : 'none';
                titleSec.querySelectorAll('input').forEach(i => i.disabled = !isAlbumActive);
            }
        }

        let hasContent = false;
        if (activeMode === 'album') {
            hasContent = selectedFiles.length >= 2;
        } else if (activeMode === 'file') {
            hasContent = selectedFiles.length > 0;
        } else {
            hasContent = urlInput && urlInput.value.trim().length > 0;
        }

        // Title, rating, visibility, expiry, tags, comment and the redirect checkbox appear once there's
        // something to upload (a file, or a URL)
        const awaiting = activeMode === 'url' ? !hasContent : selectedFiles.length === 0;
        form.querySelectorAll('.global-title-section, .global-rating-section, .global-visibility-section, .global-expiry-section, .global-tag-section, .global-comment-section, .global-redirect-section')
            .forEach(sec => sec.classList.toggle('upload-await-file', awaiting));

        if (submitBtn) submitBtn.disabled = !(hasContent && hasRating && hasTags);

        if (submitBtn) {
            const btnText = submitBtn.querySelector('.btn-text');
            if (btnText) {
                const i18n = window.f0ckI18n || {};
                if (activeMode === 'album' && selectedFiles.length === 0) {
                    btnText.textContent = i18n.album_select_pictures || 'Select files for album';
                    submitBtn.disabled = true;
                } else if (activeMode === 'album' && selectedFiles.length === 1) {
                    btnText.textContent = i18n.album_min_pictures || 'Add at least 2 items for an album';
                    submitBtn.disabled = true;
                } else if (!hasContent) {
                    btnText.textContent = activeMode === 'file'
                        ? (ssrSelectFileText || i18n.select_file || 'Select a file')
                        : (i18n.enter_url   || 'Enter a URL');
                } else if (!hasTags) {
                    // non-shitpost or shitpost with min-tags
                    if (isShitpost && !isAlbumActive && shitpostMinTags > 0) {
                        const remaining = shitpostMinTags - Math.min(...selectedFiles.map(item => (item.tags || []).length));
                        btnText.textContent = `${remaining} more tag${remaining !== 1 ? 's' : ''} required per item`;
                    } else {
                        const remaining = minTags - tags.length;
                        const tpl = i18n.tags_required || '{n} more tag{s} required';
                        btnText.textContent = tpl
                            .replace('{n}', remaining)
                            .replace('{s}', remaining !== 1 ? 's' : '');
                    }
                } else if (!hasRating) {
                    const nsflEnabled = !!form.querySelector('input[name="rating"][value="nsfl"]');
                    if (isShitpost && !isAlbumActive && shitpostRequireRating) {
                        btnText.textContent = 'Select a rating for each item';
                    } else {
                        if (nsflEnabled) {
                            btnText.textContent = i18n.select_rating_nsfl || 'Select SFW, NSFW or NSFL';
                        } else {
                            btnText.textContent = i18n.select_rating || 'Select SFW or NSFW';
                        }
                    }
                } else {
                    if (isAlbumActive) {
                        const tpl = i18n.upload_album || 'Upload Album (%s subf0cks)';
                        btnText.textContent = tpl.replace('%s', selectedFiles.length);
                    } else if (activeMode === 'url' && urlInput && ytRegex.test(urlInput.value.trim()) && window.f0ckEnableYoutubeUpload !== false) {
                        btnText.textContent = i18n.embed_youtube    || 'Embed YouTube Video';
                    } else if (activeMode === 'url') {
                        btnText.textContent = i18n.upload_from_url  || 'Upload from URL';
                    } else {
                        btnText.textContent = i18n.upload           || 'Upload';
                    }
                }
            }
        }

        if (tagCount) {
            if (minTags > 0) {
                tagCount.textContent = '(' + tags.length + '/' + minTags + ' minimum)';
                tagCount.classList.toggle('valid', tags.length >= minTags);
            } else {
                tagCount.style.display = 'none';
            }
        }
    };

    const renderAlbumStaging = () => {
        if (!filePreview) return;
        filePreview.style.display = 'block';
        filePreview.innerHTML = '';

        const stagingCont = document.createElement('div');
        stagingCont.className = 'album-staging-container';

        const stagingHeader = document.createElement('div');
        stagingHeader.className = 'album-staging-header';
        stagingHeader.innerHTML = `
            <div class="album-staging-title">
                <i class="fa-solid fa-layer-group"></i>
                <span>${(window.f0ckI18n && window.f0ckI18n.album_title) || 'Album'} (${selectedFiles.length} subf0cks)</span>
            </div>
            <button type="button" class="btn-add-album-pics">
                <i class="fa-solid fa-plus"></i> ${(window.f0ckI18n && window.f0ckI18n.album_add_more) || 'Add subf0cks'}
            </button>
        `;
        const addBtn = stagingHeader.querySelector('.btn-add-album-pics');
        if (addBtn) {
            addBtn.addEventListener('click', (e) => {
                e.preventDefault();
                if (fileInput) fileInput.click();
            });
        }
        stagingCont.appendChild(stagingHeader);

        const grid = document.createElement('div');
        grid.className = 'album-staging-grid';

        selectedFiles.forEach((item, index) => {
            const file = item.file || item;
            const card = document.createElement('div');
            card.className = 'album-stage-card' + (index === 0 ? ' is-cover' : '');

            const badge = document.createElement('div');
            badge.className = 'album-stage-badge';
            badge.innerHTML = index === 0 ? '<i class="fa-solid fa-star"></i> Cover' : `#${index + 1}`;
            card.appendChild(badge);

            // URL items (YouTube links etc.) — no File/Blob available
            if (item.type === 'url') {
                const urlStr = item.url || '';
                const ytMatch = urlStr.match(/(?:youtube\.com\/watch\?v=|youtu\.be\/)([A-Za-z0-9_-]{11})/);
                if (ytMatch) {
                    const thumb = document.createElement('img');
                    thumb.src = `https://img.youtube.com/vi/${ytMatch[1]}/mqdefault.jpg`;
                    thumb.alt = 'YouTube thumbnail';
                    thumb.style.cssText = 'width:100%;height:100%;object-fit:cover;';
                    card.appendChild(thumb);
                    const mimeBadge = document.createElement('span');
                    mimeBadge.className = 'album-stage-mime-badge';
                    mimeBadge.innerHTML = '<i class="fa-brands fa-youtube"></i>';
                    card.appendChild(mimeBadge);
                } else {
                    const urlPreview = document.createElement('div');
                    urlPreview.className = 'album-stage-audio-preview';
                    const shortUrl = urlStr.replace(/^https?:\/\//, '').substring(0, 40);
                    urlPreview.innerHTML = `
                        <i class="fa-solid fa-link"></i>
                        <span class="album-stage-audio-name" title="${urlStr}">${shortUrl}</span>
                    `;
                    card.appendChild(urlPreview);
                    const mimeBadge = document.createElement('span');
                    mimeBadge.className = 'album-stage-mime-badge';
                    mimeBadge.innerHTML = '<i class="fa-solid fa-link"></i>';
                    card.appendChild(mimeBadge);
                }
            } else {
                const isVideo = (file.type && file.type.startsWith('video/')) || /\.(mp4|webm|mov|mkv)$/i.test(file.name || '');
                const isAudio = (file.type && file.type.startsWith('audio/')) || /\.(mp3|ogg|wav|flac|m4a|aac)$/i.test(file.name || '');

                if (isVideo) {
                    const video = document.createElement('video');
                    video.src = URL.createObjectURL(file);
                    video.muted = true;
                    video.playsInline = true;
                    video.autoplay = false;
                    video.preload = 'metadata';
                    card.appendChild(video);

                    const mimeBadge = document.createElement('span');
                    mimeBadge.className = 'album-stage-mime-badge';
                    mimeBadge.innerHTML = '<i class="fa-solid fa-play"></i>';
                    card.appendChild(mimeBadge);
                } else if (isAudio) {
                    const audioPreview = document.createElement('div');
                    audioPreview.className = 'album-stage-audio-preview';
                    audioPreview.innerHTML = `
                        <i class="fa-solid fa-music"></i>
                        <span class="album-stage-audio-name" title="${file.name || 'Audio'}">${file.name || 'Audio'}</span>
                    `;
                    card.appendChild(audioPreview);

                    const mimeBadge = document.createElement('span');
                    mimeBadge.className = 'album-stage-mime-badge';
                    mimeBadge.innerHTML = '<i class="fa-solid fa-music"></i>';
                    card.appendChild(mimeBadge);
                } else {
                    const img = document.createElement('img');
                    img.src = URL.createObjectURL(file);
                    img.alt = file.name || `Subf0ck ${index + 1}`;
                    card.appendChild(img);
                }
            }

            const actions = document.createElement('div');
            actions.className = 'album-stage-actions';

            // Move Left
            const btnLeft = document.createElement('button');
            btnLeft.type = 'button';
            btnLeft.className = 'album-action-btn btn-album-move-left';
            btnLeft.title = window.f0ckI18n?.album_move_left || 'Move left';
            btnLeft.innerHTML = '<i class="fa-solid fa-arrow-left"></i>';
            if (index === 0) {
                btnLeft.disabled = true;
                btnLeft.style.opacity = '0.3';
                btnLeft.style.cursor = 'not-allowed';
            } else {
                btnLeft.addEventListener('click', (e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    const temp = selectedFiles[index];
                    selectedFiles[index] = selectedFiles[index - 1];
                    selectedFiles[index - 1] = temp;
                    renderAlbumStaging();
                    updateSubmitButton();
                });
            }
            actions.appendChild(btnLeft);

            // Move Right
            const btnRight = document.createElement('button');
            btnRight.type = 'button';
            btnRight.className = 'album-action-btn btn-album-move-right';
            btnRight.title = window.f0ckI18n?.album_move_right || 'Move right';
            btnRight.innerHTML = '<i class="fa-solid fa-arrow-right"></i>';
            if (index === selectedFiles.length - 1) {
                btnRight.disabled = true;
                btnRight.style.opacity = '0.3';
                btnRight.style.cursor = 'not-allowed';
            } else {
                btnRight.addEventListener('click', (e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    const temp = selectedFiles[index];
                    selectedFiles[index] = selectedFiles[index + 1];
                    selectedFiles[index + 1] = temp;
                    renderAlbumStaging();
                    updateSubmitButton();
                });
            }
            actions.appendChild(btnRight);

            // Remove
            const btnRemove = document.createElement('button');
            btnRemove.type = 'button';
            btnRemove.className = 'album-action-btn btn-album-remove';
            btnRemove.title = window.f0ckI18n?.album_remove || 'Remove subf0ck';
            btnRemove.innerHTML = '<i class="fa-solid fa-xmark"></i>';
            btnRemove.addEventListener('click', (e) => {
                e.preventDefault();
                e.stopPropagation();
                selectedFiles.splice(index, 1);
                if (selectedFiles.length === 0) {
                    if (form._f0ckUploader && typeof form._f0ckUploader.reset === 'function') {
                        form._f0ckUploader.reset();
                    }
                } else {
                    handleFile();
                    updateSubmitButton();
                }
            });
            actions.appendChild(btnRemove);

            card.appendChild(actions);

            const caption = document.createElement('div');
            caption.className = 'album-stage-caption';
            caption.title = file.name;
            card.appendChild(caption);

            const tagWrap = document.createElement('div');
            tagWrap.className = 'album-stage-tags-wrap';
            const tagInput = document.createElement('input');
            tagInput.type = 'text';
            tagInput.className = 'album-stage-tags-input';
            tagInput.placeholder = (window.f0ckI18n && window.f0ckI18n.album_subf0ck_tags_placeholder) || 'Tags for this subf0ck (optional)';
            tagInput.value = item.subTags || '';
            tagInput.addEventListener('input', () => {
                item.subTags = tagInput.value;
            });
            tagInput.addEventListener('click', (e) => e.stopPropagation());
            tagWrap.appendChild(tagInput);
            card.appendChild(tagWrap);

            grid.appendChild(card);
        });

        // Add More Card in Grid if under 100 subf0cks
        const maxAlbumItems = 100;
        if (selectedFiles.length < maxAlbumItems) {
            const addMoreCard = document.createElement('div');
            addMoreCard.className = 'album-stage-card album-stage-add-more';
            addMoreCard.innerHTML = `
                <div class="album-add-icon"><i class="fa-solid fa-plus"></i></div>
                <div class="album-add-text">${(window.f0ckI18n && window.f0ckI18n.album_add_more) || 'Add subf0cks'}</div>
                <div class="album-add-sub">(${selectedFiles.length}/${maxAlbumItems})</div>
            `;
            addMoreCard.addEventListener('click', (e) => {
                e.preventDefault();
                if (fileInput) fileInput.click();
            });
            grid.appendChild(addMoreCard);
        }

        stagingCont.appendChild(grid);
        filePreview.appendChild(stagingCont);
    };

    const handleFile = (files) => {
        const isShitpost = !!window.f0ckShitpostMode;
        
        // If files were provided, process them (append or replace)
        if (files && files.length > 0) {
            const isMultiAllowed = isShitpost || activeMode === 'album' || files.length > 1 || selectedFiles.length > 0;
            const filesToProcess = isMultiAllowed ? Array.from(files) : [files[0]];
            if (!isMultiAllowed && selectedFiles.length === 0) {
                selectedFiles = []; // Reset for normal mode single non-image file
                if (filePreview) filePreview.innerHTML = '';
            }

            for (const file of filesToProcess) {
                if (!file) continue;
                
                // Basic validation (MIME/Extension/Size)
                const container = form.closest('.upload-container');
                const mimesSource = form.getAttribute('data-mimes') || (container ? container.getAttribute('data-mimes') : null);
                const swfEnabled = form.getAttribute('data-enable-swf') !== '0';
                let allowedMimes = [];
                let allowedExts = [];
                try {
                    const mimesObj = JSON.parse(mimesSource || '{}');
                    allowedMimes = Object.keys(mimesObj);
                    allowedExts = [...new Set(Object.values(mimesObj))];
                } catch(e) {
                    allowedMimes = ['image/jpeg', 'image/png', 'image/gif', 'image/webp', 'video/mp4', 'video/webm', 'video/quicktime', 'audio/mpeg', 'audio/ogg', 'audio/wav', 'audio/flac', 'audio/x-flac', 'audio/mp4', 'audio/x-m4a', 'audio/aac', 'application/x-shockwave-flash', 'application/vnd.adobe.flash.movie'];
                }

                const fileExt = file.name.split('.').pop().toLowerCase();
                const isSwfFile = fileExt === 'swf' ||
                    file.type === 'application/x-shockwave-flash' ||
                    file.type === 'application/vnd.adobe.flash.movie';

                // Reject SWF when Flash uploads are disabled
                if (isSwfFile && !swfEnabled) {
                    const errorMsg = 'Flash (.swf) uploads are disabled.';
                    if (typeof window.flashMessage === 'function') window.flashMessage('✕ ' + errorMsg, 4000, 'error');
                    else if (window.showFlash) window.showFlash(errorMsg, 'error');
                    else if (statusDiv) { statusDiv.textContent = errorMsg; statusDiv.className = 'upload-status error'; }
                    continue;
                }

                // Reject archives when archive uploads are disabled
                const archiveEnabled = form.getAttribute('data-enable-archive') !== '0';
                if (!archiveEnabled) {
                    const archiveExts = new Set();
                    try {
                        const mimesObj = JSON.parse(mimesSource || '{}');
                        for (const [mime, ext] of Object.entries(mimesObj)) {
                            if (mime.startsWith('application/') && ext !== 'swf' && ext !== 'pdf') {
                                archiveExts.add(ext);
                            }
                        }
                    } catch(e) { /* ignore */ }

                    const isArchiveFile = archiveExts.has(fileExt) ||
                        (file.type && file.type.startsWith('application/') &&
                         file.type !== 'application/x-shockwave-flash' &&
                         file.type !== 'application/vnd.adobe.flash.movie' &&
                         file.type !== 'application/pdf');

                    if (isArchiveFile) {
                        const label = file.type || ('.' + fileExt);
                        const errorMsg = `${label} uploads are disabled.`;
                        if (typeof window.flashMessage === 'function') window.flashMessage('✕ ' + errorMsg, 4000, 'error');
                        else if (window.showFlash) window.showFlash(errorMsg, 'error');
                        else if (statusDiv) { statusDiv.textContent = errorMsg; statusDiv.className = 'upload-status error'; }
                        continue;
                    }
                }

                const mimeOk = !file.type || allowedMimes.includes(file.type);
                const extOk = allowedExts.length > 0 && allowedExts.includes(fileExt);

                if (allowedMimes.length > 0 && !mimeOk && !extOk) {
                    const errorMsg = `File type ${file.type || '.' + fileExt} is not allowed.`;
                    if (window.showFlash) window.showFlash(errorMsg, 'error');
                    else if (statusDiv) { statusDiv.textContent = errorMsg; statusDiv.className = 'upload-status error'; }
                    continue;
                }

                const maxBytesAttr = form.getAttribute('data-max-bytes');
                if (maxBytesAttr) {
                    const maxBytes = parseInt(maxBytesAttr);
                    if (file.size > maxBytes) {
                        const errorMsg = `File is too large: ${formatSize(file.size)} (Limit: ${formatSize(maxBytes)})`;
                        if (window.showFlash) window.showFlash(errorMsg, 'error');
                        else if (statusDiv) { statusDiv.textContent = errorMsg; statusDiv.className = 'upload-status error'; }
                        continue;
                    }
                }

                if (activeMode === 'album' && selectedFiles.length >= 100) {
                    const errorMsg = 'Album limit reached (maximum 100 subf0cks).';
                    if (typeof window.flashMessage === 'function') window.flashMessage('✕ ' + errorMsg, 4000, 'error');
                    else if (window.showFlash) window.showFlash(errorMsg, 'error');
                    else if (statusDiv) { statusDiv.textContent = errorMsg; statusDiv.className = 'upload-status error'; }
                    break;
                }

                if (!selectedFiles.some(f => (f.file || f).name === file.name && (f.file || f).size === file.size)) {
                    selectedFiles.push({ type: 'file', file: file, rating: '', visibility: '', tags: [], comment: '', title: '', is_oc: false });
                }
            }
        }

        // Toggle album choice container
        if (activeMode === 'album') {
            albumChoiceMode = 'album';
        }
        const isAlbumCandidate = activeMode === 'file' && selectedFiles.length > 1;
        if (albumChoiceContainer) {
            albumChoiceContainer.style.display = isAlbumCandidate ? 'flex' : 'none';
        }

        // Rebuild UI state
        if (selectedFiles.length === 0) {
            if (albumChoiceContainer) albumChoiceContainer.style.display = 'none';
            if (filePreview) {
                filePreview.style.display = 'none';
                filePreview.innerHTML = '';
            }
            if (dropZonePrompt) dropZonePrompt.style.display = 'block';
            if (fileInput) {
                fileInput.value = '';
                fileInput.style.display = 'inline-block';
            }
            updateSubmitButton();
            return false;
        }

        if (dropZonePrompt) dropZonePrompt.style.display = 'none';
        if (fileInput) fileInput.style.display = 'none';
        if (filePreview) {
            filePreview.style.display = 'flex';
            // Do NOT clear innerHTML here — existing rendered items stay in place.
            // Only new (un-rendered) items will be appended below.
        }

        if (statusDiv) {
            statusDiv.textContent = '';
            statusDiv.className = 'upload-status';
        }

        // Force 'file' or 'album' mode tab UI if coming from URL mode
        if (activeMode !== 'file' && activeMode !== 'album' && modeTabs.length > 0) {
            modeTabs.forEach(t => t.classList.remove('active'));
            const targetMode = isAlbumModeActive() ? 'album' : 'file';
            const targetTab = form.querySelector(`.upload-mode-tab[data-mode="${targetMode}"]`);
            if (targetTab) targetTab.classList.add('active');
            if (modeFile) modeFile.style.display = '';
            if (modeUrl) modeUrl.style.display = 'none';
            activeMode = targetMode;
        }

        // If Album Mode is active, render Album Staging
        if (isAlbumModeActive()) {
            renderAlbumStaging();
            updateSubmitButton();
            form.dispatchEvent(new CustomEvent('fileReady', { detail: { files: selectedFiles } }));
            return true;
        }

        let lastNewPreviewItem = null;

        // Build preview items — skip items already rendered (append-only)
        selectedFiles.forEach((item, index) => {
            if (item._rendered) return; // already in DOM, don't touch it
            const isUrl = isShitpost && item.type === 'url';
            const file = isUrl ? null : (item.file || item);
            const previewItem = document.createElement('div');
            previewItem.className = 'file-preview-item' + (isUrl ? ' url-item' : '');
            
            let mediaElem;
            // For URL items: wrap media + badge in a column div
            let mediaCol = null;
            let itemBadge = null;
            if (isUrl) {
                const isYtMatch = item.url.match(ytRegex);
                if (isYtMatch && window.f0ckEnableYoutubeUpload !== false) {
                    const videoId = isYtMatch[1];
                    mediaElem = document.createElement('iframe');
                    mediaElem.src = `https://www.youtube.com/embed/${videoId}?autoplay=0&mute=1`;
                    mediaElem.width = "100%";
                    mediaElem.height = "100%";
                    mediaElem.frameBorder = "0";
                    mediaElem.allow = "accelerometer; clipboard-write; encrypted-media; gyroscope; picture-in-picture";
                    mediaElem.allowFullscreen = true;
                    mediaElem.style.borderRadius = '4px';
                    mediaElem.style.pointerEvents = 'auto';

                    // Badge looks like the URL-tab YouTube badge
                    itemBadge = document.createElement('div');
                    itemBadge.className = 'url-type-badge youtube';
                    itemBadge.textContent = '▶ YouTube';
                    itemBadge.title = window.f0ckI18n?.refetch_metadata || 'Click to re-fetch metadata';
                    itemBadge.style.display = 'block';
                    itemBadge.style.cursor = 'pointer';
                    itemBadge.style.marginTop = '6px';
                } else {
                    mediaElem = document.createElement('div');
                    mediaElem.className = 'generic-file-icon';
                    mediaElem.innerHTML = '<i class="fa-solid fa-link"></i>';

                    // Badge looks like the URL-tab direct badge
                    itemBadge = document.createElement('div');
                    itemBadge.className = 'url-type-badge direct';
                    itemBadge.textContent = 'URL';
                    itemBadge.title = window.f0ckI18n?.refetch_metadata || 'Click to re-fetch metadata';
                    itemBadge.style.display = 'block';
                    itemBadge.style.cursor = 'pointer';
                    itemBadge.style.marginTop = '6px';
                }

                // Wrap media + badge in a column
                mediaCol = document.createElement('div');
                mediaCol.className = 'item-media-col';
            } else {
                const fileExt = file.name.split('.').pop().toLowerCase();
                
                if (file.type.startsWith('image/')) {
                    mediaElem = document.createElement('img');
                    mediaElem.src = URL.createObjectURL(file);
                } else if (file.type.startsWith('video/')) {
                    const videoElem = document.createElement('video');
                    videoElem.src = URL.createObjectURL(file);
                    videoElem.muted = true;
                    videoElem.controls = true;
                    videoElem.loop = true;

                    if (isShitpost) {
                        videoElem.autoplay = false;
                        videoElem.preload = 'none';
                        videoElem.classList.add('video-thumbnail-loading');

                        videoThumbnailQueue.add(file, (dataUrl) => {
                            if (dataUrl) {
                                videoElem.poster = dataUrl;
                                videoElem.classList.remove('video-thumbnail-loading');
                            } else {
                                videoElem.classList.remove('video-thumbnail-loading');
                            }
                        });

                        // Wrap immediately so mediaElem is always the rigid container
                        const videoWrapper = document.createElement('div');
                        videoWrapper.className = 'preview-media-small preview-video-wrapper';
                        videoElem.style.cssText = 'width:100%;height:100%;display:block;object-fit:contain;';
                        videoWrapper.appendChild(videoElem);
                        mediaElem = videoWrapper;
                    } else {
                        videoElem.autoplay = true;
                        mediaElem = videoElem;
                    }
                } else if (file.type.startsWith('audio/')) {
                    mediaElem = document.createElement('audio');
                    mediaElem.src = URL.createObjectURL(file);
                    mediaElem.controls = true;
                    mediaElem.style.width = '100%';
                } else if (file.type === 'application/x-shockwave-flash' || file.type === 'application/vnd.adobe.flash.movie' || fileExt === 'swf') {
                    mediaElem = document.createElement('div');
                    mediaElem.className = 'swf-upload-preview';
                    mediaElem.dataset.swfFile = 'pending';
                    // Placeholder shown while Ruffle loads
                    mediaElem.innerHTML = `<div class="swf-upload-placeholder"><span class="swf-upload-placeholder-icon">⚡</span><span class="swf-upload-placeholder-text">Loading Flash preview…</span></div>`;
                    // Sandboxed player (flash-embed.js), started once mediaElem is in the DOM
                    Promise.resolve().then(() => {
                        if (!mediaElem.isConnected) return;
                        if (!window.customElements.get('f0ck-flash')) {
                            mediaElem.innerHTML = `<div class="swf-upload-placeholder"><span class="swf-upload-placeholder-icon">⚡</span><span class="swf-upload-placeholder-text">Flash preview unavailable</span></div>`;
                            return;
                        }
                        const player = document.createElement('f0ck-flash');
                        player.style.borderRadius = '8px';
                        player.style.overflow = 'hidden';
                        const placeholder = mediaElem.querySelector('.swf-upload-placeholder');
                        if (placeholder) placeholder.remove();
                        mediaElem.appendChild(player);
                        player.load({ data: file, volume: 0.5 }).catch((err) => {
                            console.warn('[Flash upload preview]', err);
                            mediaElem.innerHTML = `<div class="swf-upload-placeholder"><span class="swf-upload-placeholder-icon">⚡</span><span class="swf-upload-placeholder-text">Flash preview error</span></div>`;
                        });
                        mediaElem._rufflePlayer = player;

                        // Snapshot button directly below the player (inside .swf-upload-preview, so it travels with each file-preview-item)
                        const snapBtn = document.createElement('button');
                        snapBtn.type = 'button';
                        snapBtn.className = 'btn-ruffle-snapshot';
                        snapBtn.textContent = 'Capture Thumbnail';
                        snapBtn.title = 'Capture the current frame of the Flash preview as the thumbnail';
                        mediaElem.appendChild(snapBtn);
                        snapBtn.addEventListener('click', async (e) => {
                            e.preventDefault();
                            const blob = await player.snapshot({ max: 640, quality: 0.92 });
                            if (!blob) { snapBtn.textContent = '❌ Capture failed'; return; }
                            snapBtn.textContent = 'Capture Thumbnail';
                            const snapFile = new File([blob], 'ruffle-snapshot.jpg', { type: 'image/jpeg' });
                            const dt = new DataTransfer();
                            dt.items.add(snapFile);
                            if (thumbInput) {
                                thumbInput.files = dt.files;
                                thumbInput.dispatchEvent(new Event('change', { bubbles: true }));
                            }
                            // Replace snapshot preview in its grid row (between player and button)
                            const existingPrev = mediaElem.querySelector('.ruffle-snapshot-preview');
                            if (existingPrev) existingPrev.remove();
                            const prevImg = document.createElement('img');
                            prevImg.src = URL.createObjectURL(blob);
                            prevImg.className = 'ruffle-snapshot-preview';
                            mediaElem.insertBefore(prevImg, snapBtn);
                        });
                    });
                } else if (file.type === 'application/pdf' || fileExt === 'pdf') {
                    mediaElem = document.createElement('div');
                    mediaElem.className = 'generic-file-icon pdf-preview-icon';
                    mediaElem.innerHTML = '<i class="fa-solid fa-file-pdf"></i>';
                } else {
                    mediaElem = document.createElement('div');
                    mediaElem.className = 'generic-file-icon';
                    mediaElem.innerHTML = '<i class="fa-solid fa-file"></i>';
                }
            }
            // preview-media-small sizing only applies to standalone (non-col) media
            if (!mediaCol) {
                // Videos in shitpost mode were already wrapped at creation time (above)
                if (!mediaElem.classList.contains('preview-media-small') && !mediaElem.classList.contains('preview-video-wrapper')) {
                    mediaElem.classList.add('preview-media-small');
                }
            }

            // If URL item: finish building the media column (media + badge below)
            if (mediaCol) {
                mediaCol.appendChild(mediaElem);
                if (itemBadge) mediaCol.appendChild(itemBadge);
            }

            const infoRow = document.createElement('div');
            infoRow.className = 'file-meta-row-small';
            
            let ratingSwitch = '';
            let visibilitySwitch = '';
            let expirySwitch = '';
            let tagsUI = '';
            let ocUI = '';
            let commentUI = '';
            let titleUI = '';
            if (isShitpost) {
                const nsflEnabled = !!form.querySelector('input[name="rating"][value="nsfl"]');
                // Build per-item rating HTML
                const ratingValue = item.rating;
                ratingSwitch = `
                    <div class="item-rating-container">
                        <label class="item-rating-option">
                            <input type="radio" name="rating_${index}" value="sfw" ${ratingValue === 'sfw' ? 'checked' : ''}>
                            <span class="item-rating-label sfw">SFW</span>
                        </label>
                        <label class="item-rating-option">
                            <input type="radio" name="rating_${index}" value="nsfw" ${ratingValue === 'nsfw' ? 'checked' : ''}>
                            <span class="item-rating-label nsfw">NSFW</span>
                        </label>
                        ${nsflEnabled ? `
                        <label class="item-rating-option">
                            <input type="radio" name="rating_${index}" value="nsfl" ${ratingValue === 'nsfl' ? 'checked' : ''}>
                            <span class="item-rating-label nsfl">NSFL</span>
                        </label>
                        ` : ''}
                    </div>
                `;

                const hasVisSection = !!form.querySelector('.global-visibility-section');
                if (hasVisSection) {
                    const globalVis = form.querySelector('input[name="visibility"]:checked')?.value || '0';
                    const visValue = (item.visibility !== undefined && item.visibility !== '') ? item.visibility : globalVis;
                    item.visibility = visValue;
                    visibilitySwitch = `
                        <div class="item-visibility-container item-rating-container" style="margin-top: 4px;">
                            <label class="item-rating-option">
                                <input type="radio" name="visibility_${index}" value="0" ${visValue === '0' || visValue === 0 ? 'checked' : ''}>
                                <span class="item-rating-label sfw" style="display: inline-flex; align-items: center; gap: 4px;"><i class="fa-solid fa-globe"></i> Public</span>
                            </label>
                            <label class="item-rating-option">
                                <input type="radio" name="visibility_${index}" value="1" ${visValue === '1' || visValue === 1 ? 'checked' : ''}>
                                <span class="item-rating-label nsfw" style="display: inline-flex; align-items: center; gap: 4px;"><i class="fa-solid fa-link"></i> Unlisted</span>
                            </label>
                            <label class="item-rating-option">
                                <input type="radio" name="visibility_${index}" value="2" ${visValue === '2' || visValue === 2 ? 'checked' : ''}>
                                <span class="item-rating-label nsfl" style="display: inline-flex; align-items: center; gap: 4px;"><i class="fa-solid fa-lock"></i> Private</span>
                            </label>
                        </div>
                    `;
                } else {
                    visibilitySwitch = '';
                }

                const globalExpiryEl = form.querySelector('select[name="expiry"], input[name="expiry"]');
                const hasExpirySection = !!globalExpiryEl || window.f0ckEnableExpiringUploads !== false;
                if (hasExpirySection && globalExpiryEl) {
                    const globalExpiry = globalExpiryEl.value || 'permanent';
                    const expiryValue = (item.expiry !== undefined && item.expiry !== '') ? item.expiry : globalExpiry;
                    item.expiry = expiryValue;

                    expirySwitch = `
                        <div class="item-expiry-container" style="margin-top: 4px;">
                            <select class="item-expiry-select" style="width: 100%; padding: 4px 8px; background: rgba(0,0,0,0.3); color: #fff; border: 1px solid var(--nav-border-color, rgba(255,255,255,0.1)); border-radius: 4px; font-size: 0.85em; cursor: pointer;">
                                <option value="permanent" ${expiryValue === 'permanent' ? 'selected' : ''}>Permanent (Never expires)</option>
                                <option value="30minutes" ${expiryValue === '30minutes' ? 'selected' : ''}>30 Minutes</option>
                                <option value="1hour" ${expiryValue === '1hour' ? 'selected' : ''}>1 Hour</option>
                                <option value="24hours" ${expiryValue === '24hours' ? 'selected' : ''}>24 Hours</option>
                                <option value="1week" ${expiryValue === '1week' ? 'selected' : ''}>1 Week</option>
                                <option value="1month" ${expiryValue === '1month' ? 'selected' : ''}>1 Month</option>
                            </select>
                        </div>
                    `;
                } else {
                    expirySwitch = '';
                }



                const tagsPlaceholder = window.f0ckI18n?.upload_tags_placeholder || 'Tags...';
                const minTagsHint = shitpostMinTags > 0 ? ` (min ${shitpostMinTags})` : '';
                tagsUI = `
                    <div class="item-tags-container">
                        <div class="item-tags-list"></div>
                        <input type="text" class="item-tag-input" placeholder="${window.escapeHtmlUpload(tagsPlaceholder + minTagsHint)}" enterkeyhint="done">
                        <div class="tag-suggestions" style="display:none;"></div>
                        <div class="item-meta-suggestions" style="display:none; margin-top:5px; font-size:0.7rem; opacity:0.6;"></div>
                    </div>
                `;

                if (window.f0ckEnableItemTitle !== false) {
                    titleUI = `
                        <div class="item-title-container">
                            <input type="text" class="item-title-input" placeholder="Add Title..." maxlength="500" value="${window.escapeHtmlUpload(item.title || '')}">
                        </div>
                    `;
                }

                const commentPlaceholder = window.f0ckI18n?.upload_comment_placeholder || 'AddComment...';
                const maxLenHtml = (commentMaxLen !== null && !isNaN(commentMaxLen)) ? ` maxlength="${commentMaxLen}"` : '';
                commentUI = `
                    <div class="item-comment-container">
                        <textarea class="item-comment-input" placeholder="${window.escapeHtmlUpload(commentPlaceholder)}"${maxLenHtml}>${window.escapeHtmlUpload(item.comment || '')}</textarea>
                        <div class="item-comment-actions">
                            <button type="button" class="item-emoji-trigger" title="Emoji">&#x263A;</button>
                        </div>
                    </div>
                `;
            }

            const fileNameStr = isUrl ? item.url : file.name;
            const fileSizeStr = isUrl
                ? ((item.url.match(ytRegex) && window.f0ckEnableYoutubeUpload !== false) ? 'YouTube' : 'URL')
                : formatSize(file.size);

            infoRow.innerHTML = `
                <div class="file-info-small">
                    <span class="file-name-small" title="${window.escapeHtmlUpload(fileNameStr)}">${window.escapeHtmlUpload(fileNameStr)}</span>
                    <span class="file-size-small">${fileSizeStr}</span>
                </div>
                ${titleUI}
                ${ratingSwitch}
                ${visibilitySwitch}
                ${expirySwitch}
                ${tagsUI}
                ${commentUI}
            `;

            if (isShitpost) {
                // Handle Rating
                infoRow.querySelectorAll('.item-rating-container:not(.item-visibility-container) input').forEach(radio => {
                    radio.onchange = () => { 
                        item.rating = radio.value; 
                        updateSubmitButton();
                    };
                });

                // Handle Visibility
                infoRow.querySelectorAll('.item-visibility-container input').forEach(radio => {
                    radio.onchange = () => {
                        item.visibility = radio.value;
                    };
                });

                // Handle Expiry
                const expirySelect = infoRow.querySelector('.item-expiry-select');
                if (expirySelect) {
                    expirySelect.onchange = () => {
                        item.expiry = expirySelect.value;
                    };
                }


                // Handle Comment
                const commentInput = infoRow.querySelector('.item-comment-input');
                const emojiTrigger = infoRow.querySelector('.item-emoji-trigger');
                if (commentInput) {
                    commentInput.oninput = () => { item.comment = commentInput.value; };
                    if (emojiTrigger) setupItemEmojiPicker(commentInput, emojiTrigger);
                }

                // Handle Title
                const titleInput = infoRow.querySelector('.item-title-input');
                if (titleInput) {
                    titleInput.oninput = () => { item.title = titleInput.value.trim(); };
                }

                // Handle Tags
                const tagList = infoRow.querySelector('.item-tags-list');
                const tagInput = infoRow.querySelector('.item-tag-input');
                const tagSuggestions = infoRow.querySelector('.tag-suggestions');
                const suggCont = infoRow.querySelector('.item-meta-suggestions');

                const renderTags = () => {
                    if (!tagList) return;
                    tagList.innerHTML = '';
                    item.tags.forEach((t, i) => {
                        const chip = document.createElement('span');
                        chip.className = 'item-tag-chip';
                        chip.innerHTML = `${window.escapeHtmlUpload(t)} <span class="item-tag-remove">✕</span>`;
                        chip.querySelector('.item-tag-remove').onclick = () => {
                            item.tags.splice(i, 1);
                            renderTags();
                            updateSubmitButton();
                        };
                        tagList.appendChild(chip);
                    });
                };
                renderTags();

                const addItemTag = (val) => {
                    val = val.trim();
                    if (val && !item.tags.includes(val)) {
                        item.tags.push(val);
                        renderTags();
                        updateSubmitButton();
                    }
                };

                // Initialize autocomplete for THIS item
                if (tagInput && tagSuggestions) {
                    window.f0ckInitTagAutocomplete(tagInput, tagSuggestions, addItemTag, () => item.tags);
                }

                // Metadata Suggestions for THIS item
                const addMetaSuggestionToItem = (val) => {
                    if (!suggCont) return;
                    suggCont.style.display = 'flex';
                    const sugg = document.createElement('div');
                    sugg.className = 'item-meta-suggestion';
                    sugg.innerHTML = `<i class="fa fa-plus-circle" style="user-select:none"></i> <span style="user-select:text">${window.escapeHtmlUpload(val)}</span>`;
                    
                    sugg.addEventListener('mouseup', (ev) => {
                        const sel = window.getSelection?.()?.toString().trim();
                        if (!sel || sel === val) return;
                        sugg.addEventListener('click', (e) => e.stopImmediatePropagation(), { once: true, capture: true });
                        ev.stopPropagation();
                        window._showSelTagPopover?.(sel, sugg, (confirmed) => {
                            window.getSelection?.()?.removeAllRanges();
                            addItemTag(confirmed);
                        });
                    });

                    sugg.onclick = () => {
                        addItemTag(val);
                        sugg.classList.add('selected');
                        const icon = sugg.querySelector('i');
                        if (icon) icon.className = 'fa fa-check-circle';
                    };
                    suggCont.appendChild(sugg);
                };

                // Trigger Extraction for this item
                if (isUrl) {
                    // Helper: populate suggestions for this item from a fields array
                    const populateSuggestionsFromFields = (fields) => {
                        fields.forEach(val => addMetaSuggestionToItem(val));
                    };

                    // The itemBadge IS the refetch button — same badge as the URL tab
                    const isYt = ytRegex.test(item.url) && window.f0ckEnableYoutubeUpload !== false;
                    const badgeLabel    = isYt ? '▶ YouTube' : 'URL';
                    const badgeClass    = isYt ? 'url-type-badge youtube' : 'url-type-badge direct';

                    // Helper: drive the badge through fetching → success → restore
                    const setBadgeFetching = () => {
                        if (!itemBadge) return;
                        itemBadge.innerHTML = '<span class="loading-spinner"></span> Fetching...';
                        itemBadge.className = 'url-type-badge fetching';
                        itemBadge.style.display = 'flex';
                        itemBadge.style.pointerEvents = 'none';
                    };
                    const setBadgeSuccess = () => {
                        if (!itemBadge) return;
                        itemBadge.innerHTML = '✓ Metadata Fetched';
                        itemBadge.className = 'url-type-badge success';
                        itemBadge.style.display = 'block';
                        setTimeout(() => {
                            if (itemBadge) {
                                itemBadge.textContent = badgeLabel;
                                itemBadge.className = badgeClass;
                                itemBadge.style.pointerEvents = 'auto';
                            }
                        }, 2500);
                    };
                    const setBadgeRestored = () => {
                        if (!itemBadge) return;
                        itemBadge.textContent = badgeLabel;
                        itemBadge.className = badgeClass;
                        itemBadge.style.display = 'block';
                        itemBadge.style.pointerEvents = 'auto';
                    };

                    // Helper: fetch metadata for this item
                    const fetchItemMeta = async (forceFetch = false) => {
                        if (suggCont) suggCont.style.display = 'flex';

                        // Cache hit → instant, no network call needed
                        if (!forceFetch && metaCache.has(item.url)) {
                            populateSuggestionsFromFields(metaCache.get(item.url));
                            setBadgeSuccess();
                            return;
                        }

                        // Dim existing suggestions in-place — no removal, no layout jump
                        const staleChips = suggCont
                            ? Array.from(suggCont.querySelectorAll('.item-meta-suggestion'))
                            : [];
                        staleChips.forEach(s => { s.style.opacity = '0.3'; s.style.pointerEvents = 'none'; });

                        setBadgeFetching();
                        try {
                            // YouTube: try client-side oEmbed first (avoids Tor consent walls)
                            let data = ytRegex.test(item.url) ? await tryClientYouTubeOembed(item.url) : null;
                            if (!data) {
                                const resp = await fetch(`/api/v2/meta/fetch?url=${encodeURIComponent(item.url)}`, {
                                    headers: { 'X-Requested-With': 'XMLHttpRequest' }
                                });
                                data = await resp.json();
                            }
                            if (data.success && data.meta) {
                                const fields = extractFieldsFromMeta(data.meta);
                                metaCache.set(item.url, fields);
                                // Now it's safe to clear — new content is ready to fill the space
                                staleChips.forEach(s => s.remove());
                                populateSuggestionsFromFields(fields);
                                setBadgeSuccess();
                            } else {
                                // Restore stale chips on failure
                                staleChips.forEach(s => { s.style.opacity = ''; s.style.pointerEvents = ''; });
                                setBadgeRestored();
                            }
                        } catch (e) {
                            console.warn('[URL ITEM META SYNC ERROR]', e.message);
                            staleChips.forEach(s => { s.style.opacity = ''; s.style.pointerEvents = ''; });
                            setBadgeRestored();
                        }
                    };

                    // Badge click = re-fetch
                    if (itemBadge) {
                        itemBadge.addEventListener('click', (e) => {
                            e.stopPropagation();
                            metaCache.delete(item.url);
                            fetchItemMeta(true);
                        });
                    }

                    // Populate suggestions (instant if cached, otherwise network)
                    fetchItemMeta(false);
                } else {
                    // Add filename as first suggestion
                    if (file.name) {
                        let baseName = file.name;
                        const lastDotIndex = baseName.lastIndexOf('.');
                        if (lastDotIndex > 0) baseName = baseName.substring(0, lastDotIndex);
                        baseName = baseName.trim();
                        if (baseName) addMetaSuggestionToItem(baseName);
                    }

                    if (files && Array.from(files).includes(file)) {
                        (async () => {
                            const chunkSize = (file.type && file.type.startsWith('image/')) ? 512 * 1024 : 4 * 1024 * 1024;
                            const chunk = file.slice(0, chunkSize);
                            const formData = new FormData();
                            formData.append('file', chunk, file.name);
                            try {
                                const resp = await fetch('/api/v2/meta/extract-file', {
                                    method: 'POST',
                                    headers: {
                                        'X-CSRF-Token': window.f0ckSession?.csrf_token || document.querySelector('input[name="csrf_token"]')?.value || '',
                                        'X-Requested-With': 'XMLHttpRequest'
                                    },
                                    body: formData
                                });
                                const data = await resp.json();
                                if (data.success && data.fields) {
                                    data.fields.forEach(val => addMetaSuggestionToItem(val));
                                }
                                if (data.hasGpsData) showGpsPrivacyWarning(file);
                            } catch (e) { console.warn('[ITEM META SYNC ERROR]', e.message); }
                        })();
                    }
                }
            }

            const removeBtn = document.createElement('button');
            removeBtn.type = 'button';
            removeBtn.className = 'btn-remove-small';
            removeBtn.innerHTML = '✕';
            removeBtn.onclick = (e) => {
                e.stopPropagation();
                // Remove by reference (index may have shifted after prior removals)
                const idx = selectedFiles.indexOf(item);
                if (idx !== -1) selectedFiles.splice(idx, 1);
                item._rendered = false;
                // Clean up Ruffle player and blob URL if this was a SWF preview
                const swfPreview = previewItem.querySelector('.swf-upload-preview');
                if (swfPreview) {
                    if (swfPreview._rufflePlayer) { try { swfPreview._rufflePlayer.pause(); swfPreview._rufflePlayer.remove(); } catch {} swfPreview._rufflePlayer = null; }
                    if (swfPreview._swfObjectUrl) { URL.revokeObjectURL(swfPreview._swfObjectUrl); swfPreview._swfObjectUrl = null; }
                }
                previewItem.remove();

                if (selectedFiles.length === 0) {
                    // Reset to empty state
                    if (filePreview) { filePreview.style.display = 'none'; filePreview.innerHTML = ''; }
                    if (dropZonePrompt) dropZonePrompt.style.display = 'block';
                    if (fileInput) { fileInput.value = ''; fileInput.style.display = 'inline-block'; }
                } else {
                    // Ensure the "Add more" button is still last
                    const addMoreEl = filePreview?.querySelector('.add-more-item');
                    if (addMoreEl) filePreview.appendChild(addMoreEl);
                }
                updateSubmitButton();
            };

            item._rendered = true;

            // Append media column (URL items) or raw media element (file items)
            if (mediaCol) {
                previewItem.appendChild(mediaCol);
            } else {
                previewItem.appendChild(mediaElem);
            }
            previewItem.appendChild(infoRow);
            previewItem.appendChild(removeBtn);
            if (filePreview) filePreview.appendChild(previewItem);

            if (isShitpost) {
                lastNewPreviewItem = previewItem;
            }
        });

        // "Add more" button for Shitpost Mode — reuse existing or create once, always move to end
        if (isShitpost && filePreview) {
            let addMoreItem = filePreview.querySelector('.add-more-item');
            if (!addMoreItem) {
                addMoreItem = document.createElement('div');
                addMoreItem.className = 'file-preview-item add-more-item';
                addMoreItem.innerHTML = `<span>${window.f0ckI18n?.upload_add_more || 'Add more'}</span>`;
                addMoreItem.onclick = () => fileInput && fileInput.click();
            }
            filePreview.appendChild(addMoreItem); // moves it to end if already present
        }

        // Legacy Global Meta Sync (Non-Shitpost Mode)
        if (!isShitpost && selectedFiles.length > 0 && files && files.length > 0) {
            const primaryFile = selectedFiles[0].file || selectedFiles[0];
            autoTags = [];
            const metaCont = form.querySelector('.meta-suggestions-container');
            const metaList = form.querySelector('.meta-suggestions-list');
            if (metaList) metaList.innerHTML = '';
            if (metaCont) metaCont.style.display = 'none';
            form.querySelector('.gps-privacy-warning')?.remove();

            if (primaryFile.name) {
                let baseName = primaryFile.name;
                const lastDotIndex = baseName.lastIndexOf('.');
                if (lastDotIndex > 0) baseName = baseName.substring(0, lastDotIndex);
                baseName = baseName.trim();
                if (baseName && !autoTags.includes(baseName)) {
                    autoTags.push(baseName);
                    addMetaSuggestion(baseName);
                }
            }

            const isMedia = (primaryFile.type && primaryFile.type.startsWith('video/')) || 
                           (primaryFile.type && primaryFile.type.startsWith('audio/')) || 
                           (primaryFile.type && primaryFile.type.startsWith('image/')) ||
                           /\.(mp4|webm|mp3|ogg|wav|m4a|flac|jpg|jpeg|png|gif|webp|tiff?)$/i.test(primaryFile.name);
            
            if (isMedia) {
                const extractTags = async () => {
                    const chunkSize = (primaryFile.type && primaryFile.type.startsWith('image/')) ? 512 * 1024 : 4 * 1024 * 1024;
                    const chunk = primaryFile.slice(0, chunkSize);
                    const formData = new FormData();
                    formData.append('file', chunk, primaryFile.name);
                    const status = form.querySelector('.tag-input-container .sync-spinner');
                    if (status) status.classList.add('active');

                    try {
                        const controller = new AbortController();
                        const timeoutId = setTimeout(() => controller.abort(), 10000);
                        const resp = await fetch('/api/v2/meta/extract-file', {
                            method: 'POST',
                            headers: {
                                'X-CSRF-Token': window.f0ckSession?.csrf_token || document.querySelector('input[name="csrf_token"]')?.value || '',
                                'X-Requested-With': 'XMLHttpRequest'
                            },
                            body: formData,
                            signal: controller.signal
                        });
                        clearTimeout(timeoutId);
                        const data = await resp.json();
                        if (data.success && data.fields) {
                            data.fields.forEach(val => {
                                if (!autoTags.includes(val)) {
                                    autoTags.push(val);
                                    addMetaSuggestion(val);
                                }
                            });
                        }
                        if (data.hasGpsData) showGpsPrivacyWarning(primaryFile);
                    } catch (e) {
                        console.warn('[META SYNC ERROR]', e.message);
                    } finally {
                        if (status) status.classList.remove('active');
                    }
                };
                extractTags();
            }
        }

        // Hide thumbSection for SWF (snapshot button now lives inside each file-preview-item)
        if (thumbSection) {
            thumbSection.style.display = 'none';
        }

        updateSubmitButton();
        form.dispatchEvent(new CustomEvent('fileReady', { detail: { files: selectedFiles } }));

        if (lastNewPreviewItem) {
            setTimeout(() => {
                lastNewPreviewItem.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
            }, 100);
        }

        return true;
    };

    if (dropZone) {
        const preventDefaults = (e) => {
            e.preventDefault();
            e.stopPropagation();
        };

        ['dragenter', 'dragover', 'dragleave', 'drop'].forEach(eventName => {
            dropZone.addEventListener(eventName, preventDefaults, false);
        });

        ['dragenter', 'dragover'].forEach(eventName => {
            dropZone.addEventListener(eventName, () => dropZone.classList.add('dragover'), false);
        });

        ['dragleave', 'drop'].forEach(eventName => {
            dropZone.addEventListener(eventName, () => dropZone.classList.remove('dragover'), false);
        });

        dropZone.addEventListener('drop', (e) => {
            const dt = e.dataTransfer;
            const files = dt.files;
            handleFile(files);
        });
    }

    if (fileInput) {
        fileInput.addEventListener('change', (e) => handleFile(e.target.files));
    }

    if (removeFile) {
        removeFile.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            // Clean up any Ruffle preview players and blob URLs
            filePreview?.querySelectorAll('.swf-upload-preview').forEach(el => {
                if (el._rufflePlayer) { try { el._rufflePlayer.pause(); el._rufflePlayer.remove(); } catch {} el._rufflePlayer = null; }
                if (el._swfObjectUrl) { URL.revokeObjectURL(el._swfObjectUrl); el._swfObjectUrl = null; }
            });
            selectedFiles = [];
            if (albumChoiceContainer) albumChoiceContainer.style.display = 'none';
            form.querySelector('.gps-privacy-warning')?.remove();
            if (fileInput) fileInput.value = '';
            if (dropZonePrompt) dropZonePrompt.style.display = 'block';
            if (fileInput) fileInput.style.display = 'inline-block'; 
            if (filePreview) filePreview.style.display = 'none';
            const media = filePreview?.querySelector('.preview-media');
            if (media) media.remove();

            if (thumbSection) {
                thumbSection.style.display = 'none';
                thumbSection.querySelector('.btn-ruffle-snapshot')?.remove();
                thumbSection.querySelector('.ruffle-snapshot-preview')?.remove();
            }
            if (thumbInput) thumbInput.value = '';

            updateSubmitButton();
        });
    }

    const addTag = (tagName) => {
        tagName = tagName.trim();
        if (!tagName || tags.some(t => t.toLowerCase() === tagName.toLowerCase())) return;
        if (['sfw', 'nsfw', 'nsfl'].includes(tagName.toLowerCase())) return;
        if (/^https?:\/\//i.test(tagName)) {
            if (typeof window.showFlash === 'function') {
                window.showFlash('Post that in the comments', 'error');
            }
            if (tagInput) tagInput.value = '';
            return;
        }

        tags.push(tagName);

        const chip = document.createElement('span');
        chip.className = 'tag-chip';
        chip.style.cursor = 'pointer';
        chip.title = 'Click to edit prefix or tag';
        chip.innerHTML = `<span class="tag-text">${window.escapeHtmlUpload(tagName)}</span><button type="button">&times;</button>`;
        
        // Remove button logic
        chip.querySelector('button').addEventListener('click', (e) => {
            e.stopPropagation();
            tags = tags.filter(t => t !== tagName);
            chip.remove();
            
            syncMetaSuggestions();
            updateSubmitButton();
        });

        // Edit logic: clicking the text moves it back to input
        chip.querySelector('.tag-text').addEventListener('click', () => {
            tags = tags.filter(t => t !== tagName);
            chip.remove();
            if (tagInput) {
                tagInput.value = tagName;
                tagInput.focus();
            }
            syncMetaSuggestions();
            updateSubmitButton();
        });

        if (tagsList) tagsList.appendChild(chip);
        if (tagsHidden) tagsHidden.value = tags.join(',');
        if (tagInput) tagInput.value = '';
        if (tagSuggestions) {
            tagSuggestions.innerHTML = '';
            tagSuggestions.style.display = 'none';
        }
        syncMetaSuggestions();
        updateSubmitButton();
    };

    const showGpsPrivacyWarning = (originalFile) => {
        // Remove any existing warning
        const existing = form.querySelector('.gps-privacy-warning');
        if (existing) existing.remove();

        const warn = document.createElement('div');
        warn.className = 'gps-privacy-warning';
        warn.innerHTML = `
            <i class="fa-solid fa-location-dot"></i>
            <span><strong>Location data detected.</strong> This image contains GPS coordinates that reveal where it was taken.</span>
            <button type="button" class="gps-strip-btn">Strip GPS data</button>
            <button type="button" class="gps-dismiss-btn" title="Dismiss">×</button>
        `;

        const stripBtn = warn.querySelector('.gps-strip-btn');
        const dismissBtn = warn.querySelector('.gps-dismiss-btn');

        dismissBtn.addEventListener('click', () => warn.remove());

        stripBtn.addEventListener('click', async () => {
            stripBtn.disabled = true;
            stripBtn.textContent = 'Stripping…';

            try {
                const formData = new FormData();
                formData.append('file', originalFile, originalFile.name);

                const resp = await fetch('/api/v2/meta/strip-gps', {
                    method: 'POST',
                    headers: {
                        'X-CSRF-Token': window.f0ckSession?.csrf_token || document.querySelector('input[name="csrf_token"]')?.value || '',
                        'X-Requested-With': 'XMLHttpRequest',
                    },
                    body: formData,
                });

                if (!resp.ok) throw new Error(`HTTP ${resp.status}`);

                const blob = await resp.blob();
                const cleanFile = new File([blob], originalFile.name, { type: originalFile.type || blob.type });

                // Replace the selected file in the array so the upload uses the clean version
                const isShitpost = !!window.f0ckShitpostMode;
                const idx = selectedFiles.findIndex(f => (f.file || f) === originalFile);
                if (idx !== -1) {
                    if (isShitpost && typeof selectedFiles[idx] === 'object') {
                        selectedFiles[idx].file = cleanFile;
                    } else {
                        selectedFiles[idx] = cleanFile;
                    }
                }

                warn.innerHTML = `<i class="fa-solid fa-check"></i> <span>GPS data stripped. Location will not be embedded in the uploaded image.</span>`;
                warn.classList.add('gps-stripped');
                setTimeout(() => warn.remove(), 5000);
            } catch (e) {
                console.warn('[GPS STRIP ERROR]', e.message);
                stripBtn.disabled = false;
                stripBtn.textContent = 'Strip GPS data';
                if (typeof window.showFlash === 'function') window.showFlash('Failed to strip GPS data', 'error');
                console.error('[GPS STRIP ERROR]', err);
            }
        });

        // Insert above the tag controls area
        const tagControls = form.querySelector('.tag-controls') || form.querySelector('.tag-input-container');
        if (tagControls) {
            tagControls.parentNode.insertBefore(warn, tagControls);
        } else {
            form.appendChild(warn);
        }
    };

    const addMetaSuggestion = (text) => {
        text = text.trim();
        if (!text) return;

        const metaCont = form.querySelector('.meta-suggestions-container');
        const metaList = form.querySelector('.meta-suggestions-list');
        if (!metaCont || !metaList) return;

        // Don't suggest duplicates in the suggestions list
        if (Array.from(metaList.children).some(el => el.getAttribute('data-text') === text)) return;

        metaCont.style.display = 'block';

        const sug = document.createElement('div');
        sug.className = 'meta-suggestion';
        sug.setAttribute('data-text', text);
        sug.innerHTML = `<i class="fa fa-plus-circle" style="user-select:none"></i> <span>${window.escapeHtmlUpload(text)}</span>`;

        sug.addEventListener('mouseup', (ev) => {
            const sel = window.getSelection?.()?.toString().trim();
            if (!sel) return;
            sug.addEventListener('click', (e) => e.stopImmediatePropagation(), { once: true, capture: true });
            ev.stopPropagation();
            window._showSelTagPopover?.(sel, sug, (confirmed) => {
                window.getSelection?.()?.removeAllRanges();
                addTag(confirmed);
            });
        });

        sug.addEventListener('click', () => {
            addTag(text);
        });

        metaList.appendChild(sug);
        syncMetaSuggestions();
    };

    const syncMetaSuggestions = () => {
        const metaList = form.querySelector('.meta-suggestions-list');
        if (!metaList) return;

        Array.from(metaList.children).forEach(sug => {
            const text = sug.getAttribute('data-text');
            if (tags.some(t => t.toLowerCase() === text.toLowerCase())) {
                sug.classList.add('selected');
                sug.querySelector('i').className = 'fa fa-check-circle';
            } else {
                sug.classList.remove('selected');
                sug.querySelector('i').className = 'fa fa-plus-circle';
            }
        });
    };

    /**
     * Reusable Tag Autocomplete Logic
     */
    window.f0ckInitTagAutocomplete = (tagInput, tagSuggestions, onAdd, getExistingTags) => {
        if (!tagInput || !tagSuggestions) return;

        let currentFocus = -1;
        let debounceTimer;

        const addActive = (x) => {
            if (!x) return false;
            removeActive(x);
            if (currentFocus >= x.length) currentFocus = 0;
            if (currentFocus < 0) currentFocus = (x.length - 1);
            x[currentFocus].classList.add("active");
            x[currentFocus].scrollIntoView({ block: 'nearest' });
            
            // Optional: update input value to highlighted tag
            // const tagName = x[currentFocus].querySelector('.tag-suggestion-name').textContent;
            // tagInput.value = tagName;
        };

        const removeActive = (x) => {
            for (let i = 0; i < x.length; i++) {
                x[i].classList.remove("active");
            }
        };

        tagInput.addEventListener('keydown', (e) => {
            const x = tagSuggestions.getElementsByClassName("tag-suggestion-item");
            if (e.key === 'ArrowDown') {
                currentFocus++;
                addActive(x);
            } else if (e.key === 'ArrowUp') {
                currentFocus--;
                addActive(x);
            } else if ((e.key === 'Enter' || e.key === 'Tab') && !e.ctrlKey && !e.metaKey) {
                if (currentFocus > -1) {
                    e.preventDefault();
                    if (x && x[currentFocus] && x[currentFocus]._f0ckSelect) {
                        x[currentFocus]._f0ckSelect();
                    }
                } else if (e.key === 'Enter' && tagInput.value.trim().length > 0) {
                    e.preventDefault();
                    onAdd(tagInput.value);
                    tagInput.value = '';
                }
            } else if (e.key === 'Escape') {
                if (tagSuggestions.style.display === 'block') {
                    e.preventDefault();
                    e.stopPropagation();
                }
                tagSuggestions.style.display = 'none';
                currentFocus = -1;
            }
        });

        tagInput.addEventListener('input', () => {
            clearTimeout(debounceTimer);
            const query = tagInput.value.trim();
            currentFocus = -1;

            if (query.length < 1) {
                tagSuggestions.style.display = 'none';
                return;
            }

            debounceTimer = setTimeout(async () => {
                try {
                    const res = await fetch('/api/v2/tags/suggest?q=' + encodeURIComponent(query));
                    const data = await res.json();
                    if (data.success && data.suggestions && data.suggestions.length > 0) {
                        const existing = getExistingTags();
                        const filtered = (data.suggestions || []).filter(s => s && s.tag && !existing.some(t => t.toLowerCase() === s.tag.toLowerCase()));
                        let html = '';
                        const maxSuggestions = Math.min(8, filtered.length);
                        for (let i = 0; i < maxSuggestions; i++) {
                            const s = filtered[i];
                            const scoreStr = typeof s.score === 'number' ? s.score.toFixed(2) : '0.00';
                            html += `
                                <div class="tag-suggestion-item">
                                    <span class="tag-suggestion-name">${window.escapeHtmlUpload(s.tag)}</span>
                                    <span class="tag-suggestion-meta">${s.tagged || 0}× · ${scoreStr}</span>
                                </div>
                            `;
                        }
                        
                        if (maxSuggestions > 0) {
                            tagSuggestions.innerHTML = html;
                            tagSuggestions.style.display = 'block';

                            const items = tagSuggestions.querySelectorAll('.tag-suggestion-item');
                            items.forEach((el, idx) => {
                                const select = (ev) => {
                                    if (ev) {
                                        // Only stop if we actually have a selection (handled in mouseup)
                                        const sel = window.getSelection?.()?.toString().trim();
                                        if (sel && sel !== filtered[idx].tag) {
                                            ev.preventDefault();
                                            ev.stopPropagation();
                                            return;
                                        }
                                        ev.stopPropagation();
                                    }
                                    onAdd(filtered[idx].tag);
                                    tagInput.value = '';
                                    tagSuggestions.style.display = 'none';
                                    tagInput.focus();
                                };
                                
                                // Partial Selection Support
                                el.addEventListener('mouseup', (ev) => {
                                    const sel = window.getSelection?.()?.toString().trim();
                                    if (!sel || sel === filtered[idx].tag) return;
                                    
                                    // Block the immediate click action if we have a selection
                                    el.addEventListener('click', (e) => e.stopImmediatePropagation(), { once: true, capture: true });
                                    ev.stopPropagation();
                                    
                                    window._showSelTagPopover?.(sel, el, (confirmed) => {
                                        window.getSelection?.()?.removeAllRanges();
                                        onAdd(confirmed);
                                        tagSuggestions.style.display = 'none';
                                        tagInput.value = '';
                                        tagInput.focus();
                                    });
                                });

                                el.addEventListener('click', select);
                                
                                // Mobile support
                                let touchStartX = 0, touchStartY = 0;
                                el.addEventListener('touchstart', (ev) => {
                                    touchStartX = ev.touches[0].clientX;
                                    touchStartY = ev.touches[0].clientY;
                                }, { passive: true });
                                el.addEventListener('touchend', (ev) => {
                                    const dx = Math.abs(ev.changedTouches[0].clientX - touchStartX);
                                    const dy = Math.abs(ev.changedTouches[0].clientY - touchStartY);
                                    if (dx < 10 && dy < 10) {
                                        select(ev);
                                    }
                                });
                                el._f0ckSelect = select;
                            });
                        } else {
                            tagSuggestions.style.display = 'none';
                        }
                    } else {
                        tagSuggestions.style.display = 'none';
                    }
                } catch (err) {
                    console.error('[UPLOAD_TAGS] Error fetching suggestions:', err);
                }
            }, 200);
        });

        // Close when clicking outside
        const closeSuggestions = (e) => {
            if (!tagInput.contains(e.target) && !tagSuggestions.contains(e.target)) {
                tagSuggestions.style.display = 'none';
            }
        };
        document.addEventListener('mousedown', closeSuggestions);
        document.addEventListener('touchstart', closeSuggestions, { passive: true });
    };

    if (tagInput && tagSuggestions) {
        window.f0ckInitTagAutocomplete(tagInput, tagSuggestions, addTag, () => tags);
    }

    // Prevent Enter in the title input from submitting the form and
    // accidentally flushing whatever is currently typed in the tag input as a tag.
    const titleInputEl = form.querySelector('.upload-title-input');
    if (titleInputEl) {
        titleInputEl.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') e.preventDefault();
        });
    }

    form.querySelectorAll('input[name="rating"]').forEach(radio => {
        radio.addEventListener('change', updateSubmitButton);
    });

    const performUpload = async (e) => {
        if (e && e.preventDefault) e.preventDefault();

        // If already uploading, don't start again
        if (isUploading) {
            return;
        }

        const isFileMode = activeMode === 'file';
        const isAlbum = isAlbumModeActive() || activeMode === 'album';

        const globalRatingEl = form.querySelector('input[name="rating"]:checked');

        // Validation
        if (isShitpost && isFileMode && !isAlbum) {
            if (selectedFiles.length === 0) {
                if (window.showFlash) window.showFlash('No files selected', 'error');
                return;
            }
            // No tag or rating requirement in shitpost mode — untagged items are allowed
        } else {
            if (isAlbum && selectedFiles.length < 2) {
                if (window.showFlash) window.showFlash('Add at least 2 pictures for an album', 'error');
                return;
            }
            if (!globalRatingEl) {
                if (window.showFlash) window.showFlash('Please select a rating', 'error');
                return;
            }
            if (tags.length < minTags) {
                if (window.showFlash) window.showFlash(`At least ${minTags} tags are required`, 'error');
                return;
            }
        }

        // Captcha first when the site requires one for uploads (cap.uploads)
        if (window.f0ckCaptchaGate) {
            if (performUpload.captchaOpen) return;
            performUpload.captchaOpen = true;
            try { await window.f0ckCaptchaGate('upload'); }
            catch { return; } // cancelled
            finally { performUpload.captchaOpen = false; }
        }

        const dragModal = form.closest('#upload-drag-modal');
        const comment = form.querySelector('.upload-comment')?.value.trim() || '';
        const isOc = form.querySelector('#upload-oc-checkbox')?.checked || false;
        const titleVal = form.querySelector('.upload-title-input')?.value.trim() || '';

        const setBtnLoading = (text) => {
            isUploading = true;
            if (!submitBtn) return;
            submitBtn.disabled = true;
            const btnText = submitBtn.querySelector('.btn-text');
            const btnLoading = submitBtn.querySelector('.btn-loading');
            if (btnText) btnText.style.display = 'none';
            if (btnLoading) {
                btnLoading.style.display = 'inline';
                btnLoading.textContent = text;
            }
        };

        const restoreBtn = () => {
            isUploading = false;
            if (!submitBtn) return;
            submitBtn.disabled = false;
            const btnText = submitBtn.querySelector('.btn-text');
            const btnLoading = submitBtn.querySelector('.btn-loading');
            if (btnText) btnText.style.display = 'inline';
            if (btnLoading) btnLoading.style.display = 'none';
            updateSubmitButton();
        };

        if (activeMode === 'url') {
            // --- URL Upload ---
            let urls = [urlInput?.value.trim()];
            if (isShitpost && urlInput?.tagName === 'TEXTAREA') {
                urls = urlInput.value.split('\n').map(u => u.trim()).filter(u => u.length > 0);
            }
            if (urls.length === 0 || !urls[0]) return;

            setBtnLoading(isShitpost ? `Processing 1/${urls.length}...` : 'Processing...');
            if (statusDiv) {
                statusDiv.textContent = '';
                statusDiv.className = 'upload-status';
            }

            let successCount = 0;
            let lastData = null;

            for (let i = 0; i < urls.length; i++) {
                const url = urls[i];
                if (isShitpost) {
                    const statusMsg = window.f0ckI18n?.upload_shitposting_status || 'Shitposting';
                    setBtnLoading(`[${i + 1}/${urls.length}] ${statusMsg}...`);
                }

                try {
                    const globalVisEl = form.querySelector('input[name="visibility"]:checked');
                    const visibilityVal = globalVisEl ? globalVisEl.value : '0';
                    const globalExpiryEl = form.querySelector('select[name="expiry"], input[name="expiry"]');
                    const expiryVal = globalExpiryEl ? globalExpiryEl.value : 'permanent';

                    const resp = await fetch('/api/v2/upload-url', {
                        method: 'POST',
                        headers: {
                            'Content-Type': 'application/json',
                            'X-CSRF-Token': window.f0ckSession?.csrf_token || document.querySelector('input[name="csrf_token"]')?.value || '',
                            'X-Requested-With': 'XMLHttpRequest'
                        },
                        body: JSON.stringify({ 
                            url, 
                            rating: globalRatingEl ? globalRatingEl.value : 'sfw', 
                            visibility: visibilityVal,
                            expiry: expiryVal,
                            tags: tags.join(','),
                            comment: comment,
                            is_oc: isOc,
                            title: titleVal || undefined
                        })
                    });

                    const text = await resp.text();
                    let data;
                    try {
                        data = JSON.parse(text);
                    } catch (e) {
                        const statusText = resp.statusText || 'Unknown Error';
                        throw new Error(`Server returned non-JSON response (Status: ${resp.status} ${statusText})`);
                    }

                    if (data.success) {
                        successCount++;
                        lastData = data;
                        // If this is a background URL upload, register it with the tracker widget
                        if (data.pending && window.urlUploadTracker) {
                            window.urlUploadTracker.addJob(url, data.jobId || null);
                        }
                        if (data.itemid) {
                            try {
                                const ts = Date.now();
                                const busted = JSON.parse(localStorage.getItem('bustedThumbs') || '{}');
                                busted[data.itemid] = ts;
                                const keys = Object.keys(busted);
                                if (keys.length > 50) delete busted[keys[0]];
                                localStorage.setItem('bustedThumbs', JSON.stringify(busted));
                            } catch(e) {}
                        }
                    } else if (!isShitpost) {
                        const err = new Error(data.msg || 'Upload failed');
                        if (data.repost) err.repost = data.repost;
                        throw err;
                    }
                } catch (err) {
                    console.error('[UPLOAD ERROR]', err);
                    if (!isShitpost) {
                        if (err.repost) {
                            statusDiv.innerHTML = '✕ ' + window.escapeHtmlUpload(err.message) + ` (<a href="/${err.repost}" class="repost-link">view existing</a>)`;
                        } else {
                            statusDiv.textContent = '✕ ' + err.message;
                        }
                        statusDiv.className = 'upload-status error';
                        restoreBtn();
                        return;
                    }
                }
            }

            if (successCount > 0) {
                if (lastData?.pending) {
                    // Background URL upload: close modal gently — delay scroll reset
                    // so queued pointer/keyboard events don't fire on the underlying page
                    if (dragModal) dragModal.classList.remove('show');
                    setTimeout(() => {
                        if (window.resetGlobalScrollState) window.resetGlobalScrollState();
                        if (window.hideAllModals) window.hideAllModals();
                    }, 400);
                } else {
                    if (dragModal) dragModal.classList.remove('show');
                    if (window.resetGlobalScrollState) window.resetGlobalScrollState();
                    if (window.hideAllModals) window.hideAllModals();
                }

                form._f0ckUploader.reset();

                if (isShitpost) {
                    if (lastData?.manual_approval && typeof window.flashMessage === 'function') {
                        window.flashMessage(window.f0ckI18n?.upload_pending_approval_patient || 'Upload awaits approval', 3000, 'warning');
                    }
                } else {
                    if (lastData?.manual_approval) {
                        if (typeof window.flashMessage === 'function') {
                            window.flashMessage(window.f0ckI18n?.upload_pending_approval_patient || 'Upload awaits approval', 3000, 'warning');
                        }
                    } else if (!lastData?.pending && !dragModal && statusDiv) {
                        statusDiv.innerHTML = '✓ ' + (lastData?.msg || 'Upload successful');
                        statusDiv.className = 'upload-status success';
                    }
                }

                if (lastData?.manual_approval && lastData?.itemid) window.f0ckGoToPending(lastData.itemid);

                // URL uploads: redirect or stay based on user preference (pending/async jobs skip redirect)
                if (!lastData?.pending && !lastData?.manual_approval) {
                    if (lastData?.itemid && window.NotificationSystemInstance && typeof window.NotificationSystemInstance.handleNewItem === 'function') {
                        window.NotificationSystemInstance.handleNewItem({
                            id: lastData.itemid,
                            dest: lastData.dest,
                            mime: lastData.mime,
                            username: lastData.username || window.f0ckSession?.user || '',
                            display_name: lastData.display_name || window.f0ckSession?.display_name || null,
                            tag_id: lastData.tag_id ?? 0,
                            is_oc: !!lastData.is_oc,
                            slug: lastData.slug,
                            visibility: lastData.visibility || 0,
                            is_album: !!lastData.is_album,
                            album_count: lastData.album_count || 0
                        });
                    }

                    const shouldRedirectToItem = redirectCheckbox
                        ? redirectCheckbox.checked
                        : (localStorage.getItem('upload_redirect_to_item') !== 'false');
                    if (shouldRedirectToItem && lastData?.itemid) {
                        const isMultiNonAlbumShitpost = isShitpost && (successCount > 1 || urls.length > 1);
                        const targetUrl = isMultiNonAlbumShitpost ? '/' : (lastData.slug ? `/${lastData.slug}` : `/${lastData.itemid}`);
                        if (typeof window.loadPageAjax === 'function') {
                            window.loadPageAjax(targetUrl, false, { bypassCache: true, skipCache: true, skipInherit: true });
                        } else {
                            window.location.href = targetUrl;
                        }
                    }
                    // else: stay on current page
                }
            } else {
                restoreBtn();
            }
        } else {
            // --- File Upload ---
            if (selectedFiles.length === 0) return;

            const isAlbum = isAlbumModeActive() || activeMode === 'album';
            if (isAlbum) {
                const statusMsg = window.f0ckI18n?.uploading_album || `Uploading album (${selectedFiles.length} pictures)...`;
                setBtnLoading(statusMsg);
                if (progressContainer) progressContainer.style.display = 'flex';
                if (statusDiv) {
                    statusDiv.textContent = '';
                    statusDiv.className = 'upload-status';
                }

                const globalRatingEl = form.querySelector('input[name="rating"]:checked');
                const globalVisEl = form.querySelector('input[name="visibility"]:checked');
                const globalExpiryEl = form.querySelector('select[name="expiry"], input[name="expiry"]');

                const formData = new FormData();
                formData.append('is_album', 'true');
                formData.append('rating', globalRatingEl ? globalRatingEl.value : 'sfw');
                formData.append('visibility', globalVisEl ? globalVisEl.value : '0');
                formData.append('expiry', globalExpiryEl ? globalExpiryEl.value : 'permanent');
                formData.append('tags', tags.join(','));
                formData.append('is_oc', isOc ? 'true' : 'false');
                if (titleVal) formData.append('title', titleVal);
                if (comment) formData.append('comment', comment);

                for (let i = 0; i < selectedFiles.length; i++) {
                    const item = selectedFiles[i];
                    if (item.type === 'url') {
                        // URL items (YouTube links etc.) — send as a URL field, not a binary file
                        formData.append(`subf0ck_url_${i}`, item.url || '');
                    } else {
                        const f = item.file || item;
                        formData.append('files', f);
                    }
                    const subTags = item.subTags || '';
                    if (subTags) {
                        formData.append(`subf0ck_tags_${i}`, subTags);
                    }
                }

                try {
                    const res = await new Promise((resolve, reject) => {
                        const xhr = new XMLHttpRequest();
                        xhr.upload.addEventListener('progress', (e) => {
                            if (e.lengthComputable) {
                                const percent = Math.round((e.loaded / e.total) * 100);
                                if (progressFill) progressFill.style.width = percent + '%';
                                if (progressText) progressText.textContent = percent + '%';
                            }
                        });

                        xhr.onload = () => {
                            try {
                                const data = JSON.parse(xhr.responseText);
                                resolve(data);
                            } catch(e) {
                                let msg = 'Server error';
                                if (xhr.status === 413) msg = 'File too large';
                                try {
                                    const errData = JSON.parse(xhr.responseText);
                                    if (errData.msg) msg = errData.msg;
                                } catch(e2) {}
                                reject(new Error(msg));
                            }
                        };
                        xhr.onerror = () => reject(new Error('Connection error'));
                        xhr.open('POST', '/api/v2/upload');
                        const csrf = window.f0ckSession?.csrf_token || document.querySelector('input[name="csrf_token"]')?.value || '';
                        xhr.setRequestHeader('X-CSRF-Token', csrf);
                        xhr.send(formData);
                    });

                    if (res.success) {
                        if (dragModal) dragModal.classList.remove('show');
                        const dropModal = document.getElementById('upload-drag-modal');
                        if (dropModal) dropModal.classList.remove('show');
                        form._f0ckUploader.reset();

                        const successMsg = res.msg || `Album (${res.album_count || selectedFiles.length} subf0cks) uploaded successfully!`;
                        if (typeof window.flashMessage === 'function') {
                            window.flashMessage(successMsg, 3000, 'success');
                        } else if (!dragModal && statusDiv) {
                            statusDiv.innerHTML = '✓ ' + successMsg;
                            statusDiv.className = 'upload-status success';
                        }

                        if (res.itemid && window.NotificationSystemInstance && typeof window.NotificationSystemInstance.handleNewItem === 'function') {
                            window.NotificationSystemInstance.handleNewItem({
                                id: res.itemid,
                                dest: res.dest,
                                mime: res.mime,
                                username: res.username || window.f0ckSession?.user || '',
                                display_name: res.display_name || window.f0ckSession?.display_name || null,
                                tag_id: res.tag_id ?? 0,
                                is_oc: !!res.is_oc,
                                slug: res.slug,
                                visibility: res.visibility || 0,
                                is_album: !!res.is_album,
                                album_count: res.album_count || 0
                            });
                        }

                        const shouldRedirectToItem = redirectCheckbox
                            ? redirectCheckbox.checked
                            : (localStorage.getItem('upload_redirect_to_item') !== 'false');

                        if (shouldRedirectToItem) {
                            const targetUrl = res.slug ? `/${res.slug}` : (res.itemid ? `/${res.itemid}` : '/');
                            if (typeof window.loadPageAjax === 'function') {
                                window.loadPageAjax(targetUrl, false, { bypassCache: true, skipCache: true, skipInherit: true });
                            } else {
                                window.location.href = targetUrl;
                            }
                        }
                        // else: stay on current page
                        return;
                    } else {
                        const errMsg = res.msg || 'Upload failed';
                        const err = new Error(errMsg);
                        if (res.repost) err.repost = res.repost;
                        throw err;
                    }
                } catch (err) {
                    console.error('[ALBUM UPLOAD ERROR]', err);
                    if (err.repost) {
                        statusDiv.innerHTML = '✕ ' + window.escapeHtmlUpload(err.message) + ` (<a href="/${err.repost}" class="repost-link">view existing</a>)`;
                    } else {
                        statusDiv.textContent = '✕ ' + err.message;
                    }
                    statusDiv.className = 'upload-status error';
                    if (progressContainer) progressContainer.style.display = 'none';
                    restoreBtn();
                    return;
                }
            }

            setBtnLoading(isShitpost ? `Uploading 1/${selectedFiles.length}...` : 'Uploading...');
            if (progressContainer) progressContainer.style.display = 'flex';
            if (statusDiv) {
                statusDiv.textContent = '';
                statusDiv.className = 'upload-status';
            }

            let successCount = 0;
            let lastData = null;
            const uploadedResults = [];

            for (let i = 0; i < selectedFiles.length; i++) {
                const item = selectedFiles[i];
                // With cap.pass_minutes: 0 every file needs its own captcha (the first was solved
                // above). Otherwise the pass is still valid and this returns right away.
                if (i > 0 && window.f0ckCaptchaGate) {
                    try { await window.f0ckCaptchaGate('upload'); }
                    catch { break; } // cancelled: keep what's uploaded, skip the rest
                }
                const globalVisEl = form.querySelector('input[name="visibility"]:checked');
                const globalExpiryEl = form.querySelector('select[name="expiry"], input[name="expiry"]');
                const isUrlItem = isShitpost && item.type === 'url';
                const file = !isUrlItem ? (item.file || item) : null;
                const fileRating = isShitpost ? item.rating : (globalRatingEl ? globalRatingEl.value : 'sfw');
                const fileVisibility = isShitpost ? (item.visibility || globalVisEl?.value || '0') : (globalVisEl?.value || '0');
                const fileExpiry = isShitpost ? (item.expiry || globalExpiryEl?.value || 'permanent') : (globalExpiryEl?.value || 'permanent');
                const fileTags = isShitpost ? item.tags : tags;
                const fileComment = isShitpost ? item.comment : comment;
                const fileTitle = isShitpost ? (item.title || '') : titleVal;

                if (isShitpost) {
                    const statusMsg = window.f0ckI18n?.upload_shitposting_status || 'Shitposting';
                    setBtnLoading(`[${i + 1}/${selectedFiles.length}] ${statusMsg}...`);
                }

                const formData = new FormData();
                if (isUrlItem) {
                    formData.append('url', item.url);
                } else {
                    formData.append('file', file);
                }
                formData.append('rating', fileRating);
                formData.append('visibility', fileVisibility);
                formData.append('expiry', fileExpiry);
                formData.append('tags', fileTags.join(','));
                formData.append('is_oc', (isShitpost ? item.is_oc : isOc) ? 'true' : 'false');
                if (isShitpost) formData.append('is_shitpost', 'true');
                if (fileTitle) formData.append('title', fileTitle);

                // Add custom thumbnail if provided (only for single SWF files)
                if (selectedFiles.length === 1 && thumbInput && thumbInput.files && thumbInput.files[0]) {
                    formData.append('thumbnail', thumbInput.files[0]);
                }
                
                // Send the specific comment for this file
                if (fileComment) {
                    formData.append('comment', fileComment);
                }

                try {
                    const res = await new Promise((resolve, reject) => {
                        const xhr = new XMLHttpRequest();
                        xhr.upload.addEventListener('progress', (e) => {
                            if (e.lengthComputable) {
                                const percent = Math.round((e.loaded / e.total) * 100);
                                if (progressFill) progressFill.style.width = percent + '%';
                                if (progressText) progressText.textContent = (isShitpost ? `[${i+1}/${selectedFiles.length}] ` : '') + percent + '%';
                            }
                        });

                        xhr.onload = () => {
                            try {
                                const data = JSON.parse(xhr.responseText);
                                resolve(data);
                            } catch(e) {
                                let msg = 'Server error';
                                if (xhr.status === 413) msg = 'File too large';
                                try {
                                    const errData = JSON.parse(xhr.responseText);
                                    if (errData.msg) msg = errData.msg;
                                } catch(e2) {}
                                reject(new Error(msg));
                            }
                        };
                        xhr.onerror = () => reject(new Error('Connection error'));
                        const endpoint = isUrlItem ? '/api/v2/upload-url' : '/api/v2/upload';
                        xhr.open('POST', endpoint);
                        const csrf = window.f0ckSession?.csrf_token || document.querySelector('input[name="csrf_token"]')?.value || '';
                        xhr.setRequestHeader('X-CSRF-Token', csrf);
                        
                        if (isUrlItem) {
                            xhr.setRequestHeader('Content-Type', 'application/json');
                            xhr.send(JSON.stringify({
                                url: item.url,
                                rating: fileRating,
                                visibility: fileVisibility,
                                expiry: fileExpiry,
                                tags: fileTags.join(','),
                                is_oc: (isShitpost ? item.is_oc : isOc),
                                comment: fileComment,
                                is_shitpost: isShitpost ? true : undefined,
                                title: fileTitle || undefined
                            }));
                        } else {
                            xhr.send(formData);
                        }
                    });

                    if (res.success) {
                        successCount++;
                        lastData = res;
                        uploadedResults.push(res);
                        if (res.pending) {
                            // Background URL download — show tracker widget entry
                            if (window.urlUploadTracker) {
                                window.urlUploadTracker.addJob(item.url, res.jobId || null);
                            }
                        }
                        if (res.itemid) {
                            try {
                                const ts = Date.now();
                                const busted = JSON.parse(localStorage.getItem('bustedThumbs') || '{}');
                                busted[res.itemid] = ts;
                                const keys = Object.keys(busted);
                                if (keys.length > 50) delete busted[keys[0]];
                                localStorage.setItem('bustedThumbs', JSON.stringify(busted));
                            } catch(e) {}
                        }
                    } else {
                        // Server returned an error — always surface it visibly
                        const errMsg = res.msg || 'Upload failed';
                        if (isShitpost) {
                            // In shitpost mode there's no persistent statusDiv — use flash
                            let flashMsg = `✕ ${errMsg}`;
                            if (res.repost) {
                                flashMsg += ` (<a href="/${res.repost}" style="color:#1fb2b0;text-decoration:underline;">view existing</a>)`;
                            }
                            if (typeof window.flashMessage === 'function') {
                                window.flashMessage(flashMsg, 5000, 'error');
                            } else if (typeof window.showFlash === 'function') {
                                window.showFlash(res.repost ? errMsg + ` (<a href="/${res.repost}" style="color:#1fb2b0;text-decoration:underline;">view existing</a>)` : errMsg, 'error');
                            }
                        } else {
                            const err = new Error(errMsg);
                            if (res.repost) err.repost = res.repost;
                            throw err;
                        }
                    }
                } catch (err) {
                    console.error('[UPLOAD ERROR]', err);
                    if (!isShitpost) {
                        if (err.repost) {
                            statusDiv.innerHTML = '✕ ' + window.escapeHtmlUpload(err.message) + ` (<a href="/${err.repost}" class="repost-link">view existing</a>)`;
                        } else {
                            statusDiv.textContent = '✕ ' + err.message;
                        }
                        statusDiv.className = 'upload-status error';
                        if (progressContainer) progressContainer.style.display = 'none';
                        restoreBtn();
                        return;
                    } else {
                        // Shitpost mode: show via flash toast for general exceptions
                        let flashMsg = `✕ ${err.message}`;
                        if (err.repost) {
                            flashMsg += ` (<a href="/${err.repost}" style="color:#1fb2b0;text-decoration:underline;">view existing</a>)`;
                        }
                        if (typeof window.flashMessage === 'function') {
                            window.flashMessage(flashMsg, 5000, 'error');
                        } else if (typeof window.showFlash === 'function') {
                            window.showFlash(err.repost ? err.message + ` (<a href="/${err.repost}" style="color:#1fb2b0;text-decoration:underline;">view existing</a>)` : err.message, 'error');
                        }
                    }
                }
            }

            if (successCount > 0) {
                const totalFilesToUpload = selectedFiles.length;
                const isBgUrlUpload = lastData?.pending && selectedFiles.every(i => i.type === 'url');
                if (dragModal) dragModal.classList.remove('show');
                const dropModal = document.getElementById('upload-drag-modal');
                if (dropModal) dropModal.classList.remove('show');
                if (window.resetGlobalScrollState) window.resetGlobalScrollState();
                if (window.hideAllModals) window.hideAllModals();
                form._f0ckUploader.reset();

                if (isBgUrlUpload) {
                    if (typeof window.flashMessage === 'function') {
                        window.flashMessage((window.f0ckI18n && window.f0ckI18n.url_upload_started) || 'Background upload started', 3000, 'info');
                    }
                } else {
                    if (lastData?.manual_approval) {
                        if (typeof window.flashMessage === 'function') {
                            window.flashMessage(window.f0ckI18n?.upload_pending_approval_patient || 'Upload awaits approval', 3000, 'warning');
                        }
                    } else {
                        const successMsg = successCount > 1 
                            ? `${successCount} items uploaded successfully!` 
                            : (lastData?.msg || 'Upload successful');
                        if (typeof window.flashMessage === 'function') {
                            window.flashMessage(successMsg, 3000, 'success');
                        } else if (!dragModal && statusDiv) {
                            statusDiv.innerHTML = '✓ ' + successMsg;
                            statusDiv.className = 'upload-status success';
                        }
                    }

                    const shouldRedirectToItem = redirectCheckbox
                        ? redirectCheckbox.checked
                        : (localStorage.getItem('upload_redirect_to_item') !== 'false');

                    // Prepend items to live grid seamlessly
                    if (window.NotificationSystemInstance && typeof window.NotificationSystemInstance.handleNewItem === 'function') {
                        for (const up of uploadedResults) {
                            if (!up.manual_approval && !up.pending && up.itemid) {
                                window.NotificationSystemInstance.handleNewItem({
                                    id: up.itemid,
                                    dest: up.dest,
                                    mime: up.mime,
                                    username: up.username || window.f0ckSession?.user || '',
                                    display_name: up.display_name || window.f0ckSession?.display_name || null,
                                    tag_id: up.tag_id ?? 0,
                                    is_oc: !!up.is_oc,
                                    slug: up.slug,
                                    visibility: up.visibility || 0,
                                    is_album: !!up.is_album,
                                    album_count: up.album_count || 0
                                });
                            }
                        }
                    }

                    if (shouldRedirectToItem && !lastData?.manual_approval) {
                        const isMultiNonAlbumShitpost = isShitpost && !isAlbum && (uploadedResults.length > 1 || successCount > 1 || totalFilesToUpload > 1);
                        let targetUrl;
                        if (isMultiNonAlbumShitpost) {
                            targetUrl = '/';
                        } else {
                            const targetItem = lastData || (uploadedResults && uploadedResults[0]);
                            targetUrl = targetItem ? (targetItem.slug ? `/${targetItem.slug}` : (targetItem.itemid ? `/${targetItem.itemid}` : '/')) : '/';
                        }
                        if (typeof window.loadPageAjax === 'function') {
                            window.loadPageAjax(targetUrl, false, { bypassCache: true, skipCache: true, skipInherit: true });
                        } else {
                            window.location.href = targetUrl;
                        }
                    }
                    if (lastData?.manual_approval) {
                        const pendingItem = lastData || (uploadedResults && uploadedResults[0]);
                        window.f0ckGoToPending(pendingItem && pendingItem.itemid);
                    }
                    // else (redirect disabled): stay on current page
                }
            } else {
                restoreBtn();
                if (progressContainer) progressContainer.style.display = 'none';
            }
        }
    };

    form.addEventListener('submit', performUpload);
    updateSubmitButton();
    
    // Return an object with methods to control the form externally if needed
    form._f0ckUploader = {
        handleFile: handleFile,
        performUpload: performUpload,
        reset: () => {
            isUploading = false;
            form.reset();
            if (redirectCheckbox) {
                const savedRedirect = localStorage.getItem('upload_redirect_to_item');
                if (savedRedirect !== null) {
                    redirectCheckbox.checked = (savedRedirect === 'true' || savedRedirect === '1');
                }
            }
            tags = [];
            selectedFiles = [];
            if (albumChoiceContainer) albumChoiceContainer.style.display = 'none';
            albumChoiceMode = 'album';
            if (albumChoiceContainer) {
                albumChoiceContainer.querySelectorAll('.album-choice-btn').forEach(b => {
                    b.classList.toggle('active', b.getAttribute('data-choice') === 'album');
                });
            }
            if (tagsList) tagsList.innerHTML = '';
            if (tagsHidden) tagsHidden.value = '';
            if (fileInput) fileInput.style.display = 'inline-block';
            if (dropZonePrompt) dropZonePrompt.style.display = 'block';
            if (filePreview) {
                filePreview.style.display = "none";
                filePreview.innerHTML = "";
            }
            if (urlInput) urlInput.value = '';
            if (urlBadge) urlBadge.style.display = 'none';
            if (progressContainer) { progressContainer.style.display = 'none'; }
            if (progressFill) { progressFill.style.width = '0%'; }
            if (progressText) { progressText.textContent = '0%'; }
            if (statusDiv) { statusDiv.textContent = ''; statusDiv.className = 'upload-status'; }

            if (thumbSection) thumbSection.style.display = 'none';
            if (thumbInput) thumbInput.value = '';
            if (submitBtn) {
                const btnText = submitBtn.querySelector('.btn-text');
                const btnLoading = submitBtn.querySelector('.btn-loading');
                if (btnText) btnText.style.display = 'inline';
                if (btnLoading) {
                    btnLoading.style.display = 'none';
                }
            }

            // Reset mode to 'file'
            activeMode = 'file';
            form.classList.remove('album-mode-active');
            if (modeTabs.length > 0) {
                modeTabs.forEach(t => {
                    if (t.dataset.mode === 'file') t.classList.add('active');
                    else t.classList.remove('active');
                });
            }
            if (modeFile) modeFile.style.display = '';
            if (modeUrl) modeUrl.style.display = 'none';

            // Clear GPS warning and meta suggestions
            form.querySelector('.gps-privacy-warning')?.remove();
            const metaContReset = form.querySelector('.meta-suggestions-container');
            const metaListReset = form.querySelector('.meta-suggestions-list');
            if (metaListReset) metaListReset.innerHTML = '';
            if (metaContReset) metaContReset.style.display = 'none';
            autoTags = [];

            updateSubmitButton();
        }
    };

    return form._f0ckUploader;
};

// Global Keyboard Shortcut: Ctrl+Enter
document.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter' || (!e.ctrlKey && !e.metaKey)) return;

    // If the focused element is a textarea or input that is NOT inside an upload form,
    // do not interfere — it belongs to comments, chat, or another feature.
    const active = document.activeElement;
    if (active && (active.tagName === 'TEXTAREA' || active.tagName === 'INPUT')) {
        if (!active.closest('.upload-form')) return;
    }

    // 1. Try the element that currently has focus
    // 2. Fall back to the modal form if it's visible
    // 3. Fall back to any visible upload form on the page
    let targetForm = active?.closest('.upload-form');

    if (!targetForm || !targetForm._f0ckUploader) {
        const modal = document.getElementById('upload-drag-modal');
        if (modal && modal.classList.contains('show')) {
            targetForm = modal.querySelector('.upload-form');
        }
    }

    if (!targetForm || !targetForm._f0ckUploader) {
        targetForm = document.querySelector('.upload-form');
    }

    if (!targetForm || !targetForm._f0ckUploader) return;

    e.preventDefault();
    e.stopPropagation();

    const uploader = targetForm._f0ckUploader;
    const isShitpost = targetForm.classList.contains('shitpost-mode-active') || !!window.f0ckShitpostMode;
    const isUrlTab = targetForm.querySelector('.upload-mode-tab[data-mode="url"]')?.classList.contains('active');

    if (isShitpost && isUrlTab) {
        const btnAddUrls = targetForm.querySelector('.btn-add-urls');
        if (btnAddUrls) btnAddUrls.click();
    } else {
        uploader.performUpload();
    }
});

// Global multi-instance auto-init
window.autoInitUploadForms = () => {
    document.querySelectorAll('.upload-form').forEach(form => {
        window.initUploadForm(form);
    });
};

document.addEventListener('DOMContentLoaded', window.autoInitUploadForms);
