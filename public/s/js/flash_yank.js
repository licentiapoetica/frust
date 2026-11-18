
(function () {
    'use strict';

    const STORAGE_KEY = 'w0bmFlashFilterSettings_v1';
    const DEFAULT_SETTINGS = {
        yank: 60,           // 0..100 master "yank"
        enabled: false,
        advanced: false,    // if true, use individual params instead of yank mapping
        enableResolution: true, // per-axis degradation toggles (advanced only)
        enableFps: true,
        enablePalette: true,
        internalWidth: 320,
        fps: 12,
        paletteLevels: 4
    };

    let settings = loadSettings();
    let currentConfig = computeConfig();
    let chanLUT = buildChannelLUT(currentConfig.paletteLevels);

    let currentController = null;
    let hotkeyAttached = false;
    let ui = null; // { panel, slider, yankValue, info, toggle }
    const isMobile = /Mobi/i.test(navigator.userAgent);

    function isItemPage() {
        if (document.body?.classList.contains('scroller-active') ||
            document.getElementById('scroller-feed') ||
            /^\/scroller(\/|$)/.test(window.location.pathname)) {
            return false;
        }

        // Fast DOM check: item view or primary media element present
        if (document.getElementById('my-video') || document.querySelector('#main.item-view, .item-view, .item-layout-container, .item-main-content')) {
            return true;
        }

        const path = window.location.pathname;
        const isForbidden = /^\/(s|b|t|ca|a|login|register|settings|about|terms|rules|api|logout|auth|admin|mod|comments|notifications|feed|upload|tags|halls|ranking|abyss|random|scroller)(\/|$)/.test(path);
        if (isForbidden) return false;

        const segments = path.split('/').filter(Boolean);
        if (segments.length === 0) return false;

        // Path ends in /p/123 -> pagination grid, not single item
        const last = segments[segments.length - 1];
        if (/^\d+$/.test(last) && segments.length >= 2 && segments[segments.length - 2] === 'p') {
            return false;
        }
        if (['p', 'tags', 'halls', 'ranking', 'abyss', 'uploads', 'favs', 'f0cks'].includes(last)) {
            return false;
        }

        // Single item path: e.g. /123, /ZLHmYNnj-kK, /tag/foo/ZLHmYNnj-kK, /list/bar/123, etc.
        return segments.some(s => /^[a-zA-Z0-9_-]{11}$/.test(s) || /^\d+$/.test(s));
    }

    // ---------- Settings / Config ----------

    function loadSettings() {
        try {
            const val = localStorage.getItem(STORAGE_KEY);
            if (val) {
                const obj = JSON.parse(val);
                return Object.assign({}, DEFAULT_SETTINGS, obj);
            }
        } catch (e) {
            console.error('Failed to load settings from localStorage', e);
        }
        return Object.assign({}, DEFAULT_SETTINGS);
    }

    function saveSettings() {
        try {
            localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
        } catch (e) {
            console.error('Failed to save settings to localStorage', e);
        }
    }

    // Yank 0..100 => config
    // Higher yank = harsher: lower res, lower FPS, fewer colors
    function computeConfigFromYank(yank) {
        const t = Math.min(1, Math.max(0, yank / 100));
        // Match the ranges used by the advanced sliders:
        // Resolution: 160..800, FPS: 8..30, Palette levels: 2..12
        const widthMax = 800, widthMin = 160;
        const fpsMax = 30, fpsMin = 8;
        const levelsMax = 12, levelsMin = 2; // per channel

        const width = Math.round(widthMax - (widthMax - widthMin) * t);
        const height = Math.round(width * 9 / 16);
        const fps = Math.round(fpsMax - (fpsMax - fpsMin) * t);
        const paletteLevels = Math.round(levelsMax - (levelsMax - levelsMin) * t);

        return {
            internalWidth: width,
            internalHeight: null, // Will be calculated dynamically based on aspect ratio
            fps,
            paletteLevels
        };
    }

    // Update advanced parameters from current yank value
    function updateAdvancedFromYank() {
        const cfg = computeConfigFromYank(settings.yank);
        settings.internalWidth = cfg.internalWidth;
        settings.fps = cfg.fps;
        settings.paletteLevels = cfg.paletteLevels;
    }

    // Derive an approximate yank from current advanced sliders (0..100)
    function updateYankFromAdvanced() {
        const widthMax = 800, widthMin = 160;
        const fpsMax = 30, fpsMin = 8;
        const levelsMax = 12, levelsMin = 2;

        const tWidth = (widthMax - settings.internalWidth) / (widthMax - widthMin);
        const tFps = (fpsMax - settings.fps) / (fpsMax - fpsMin);
        const tPalette = (levelsMax - settings.paletteLevels) / (levelsMax - levelsMin);

        let t = (tWidth + tFps + tPalette) / 3;
        t = Math.min(1, Math.max(0, t));
        settings.yank = Math.round(t * 100);
    }

    // Combine yank mapping with optional advanced overrides
    function computeConfig() {
        if (settings.advanced) {
            const w = clamp(settings.internalWidth || DEFAULT_SETTINGS.internalWidth, 160, 800);
            const fps = clamp(settings.fps || DEFAULT_SETTINGS.fps, 8, 30);
            const pl = clamp(settings.paletteLevels || DEFAULT_SETTINGS.paletteLevels, 2, 12);
            settings.internalWidth = w;
            settings.fps = fps;
            settings.paletteLevels = pl;
            return {
                internalWidth: w,
                internalHeight: null, // Calculated dynamically
                fps,
                paletteLevels: pl
            };
        }

        const cfg = computeConfigFromYank(settings.yank);
        // keep advanced values in sync so toggling advanced inherits current feel
        settings.internalWidth = cfg.internalWidth;
        settings.fps = cfg.fps;
        settings.paletteLevels = cfg.paletteLevels;
        return cfg;
    }

    function clamp(v, min, max) {
        v = Number(v);
        if (isNaN(v)) return min;
        return Math.min(max, Math.max(min, v));
    }

    function applySettingsToRuntime() {
        currentConfig = computeConfig();
        chanLUT = buildChannelLUT(currentConfig.paletteLevels);
        if (currentController && currentController.onConfigChanged) {
            currentController.onConfigChanged();
        }
        updateUIFromSettings();
    }

    // ---------- Palette / Image processing ----------

    function buildChannelLUT(levels) {
        const lut = new Uint8Array(256);
        if (levels <= 1) {
            lut.fill(0);
            return lut;
        }
        const step = 255 / (levels - 1);
        for (let i = 0; i < 256; i++) {
            const idx = Math.round(i / step);
            lut[i] = Math.round(idx * step);
        }
        return lut;
    }

    function applyPalette(imageData, lut) {
        const data = imageData.data;
        for (let i = 0; i < data.length; i += 4) {
            data[i] = lut[data[i]];
            data[i + 1] = lut[data[i + 1]];
            data[i + 2] = lut[data[i + 2]];
            // alpha unchanged
        }
    }

    // ---------- Video controller ----------

    function createCanvasForVideo(video) {
        // Ensure parent is relative for absolute positioning of canvas
        const style = window.getComputedStyle(video.parentNode);
        if (style.position === 'static') {
            video.parentNode.style.position = 'relative';
        }

        const canvas = document.createElement('canvas');
        canvas.style.display = 'block';
        canvas.style.position = 'absolute';
        canvas.style.top = '0';
        canvas.style.left = '0';
        canvas.style.zIndex = '1'; // Lower z-index so controls can sit on top
        canvas.style.pointerEvents = 'none'; // Let clicks pass through to controls

        canvas.style.width = '100%';
        canvas.style.height = '100%';
        canvas.style.backgroundColor = 'transparent';
        canvas.style.imageRendering = 'pixelated';
        canvas.style.objectFit = 'contain';
        video.parentNode.insertBefore(canvas, video.nextSibling);
        return canvas;
    }

    function makeController(video) {
        const canvas = createCanvasForVideo(video);
        const ctx = canvas.getContext('2d', { willReadFrequently: true });

        let enabled = false;
        let intervalId = null;

        function applyConfigToCanvas() {
            const rect = video.getBoundingClientRect();
            const displayWidth = rect.width || video.clientWidth || video.width || 640;
            const displayHeight = rect.height || video.clientHeight || video.height || 360;

            let internalWidth = currentConfig.internalWidth;
            let internalHeight;

            // Calculate aspect ratio
            const videoWidth = video.videoWidth || 640;
            const videoHeight = video.videoHeight || 360;
            const aspectRatio = videoHeight / videoWidth;

            // If advanced resolution scaling is disabled, match display size for best quality
            if (settings.advanced && !settings.enableResolution) {
                internalWidth = Math.round(displayWidth);
                internalHeight = Math.round(displayHeight);
            } else {
                // Calculate height based on fixed width and aspect ratio
                internalHeight = Math.round(internalWidth * aspectRatio);
            }

            canvas.width = internalWidth;
            canvas.height = internalHeight;
            canvas.width = internalWidth;
            canvas.height = internalHeight;
            // The canvas fills the container (which is sized by the hidden video)
            canvas.style.width = '100%';
            canvas.style.height = '100%';
            canvas.style.maxWidth = '';
            canvas.style.aspectRatio = '';
        }

        applyConfigToCanvas();

        function drawFrame(force = false) {
            if (!enabled || (!force && (video.paused || video.ended))) return;
            try {
                const w = canvas.width;
                const h = canvas.height;
                if (!w || !h) return;
                ctx.drawImage(video, 0, 0, w, h);

                // If advanced palette reduction is disabled, skip quantization
                if (!(settings.advanced && !settings.enablePalette)) {
                    const frame = ctx.getImageData(0, 0, w, h);
                    applyPalette(frame, chanLUT);
                    ctx.putImageData(frame, 0, 0);
                }
            } catch (e) {
                if (window.f0ckDebug) window.f0ckDebug("[flash_yank] drawFrame error:", e);
            }
        }

        function startLoop() {
            if (intervalId || !enabled) return;
            const effectiveFps = (settings.advanced && !settings.enableFps) ? 60 : currentConfig.fps;
            intervalId = setInterval(drawFrame, 1000 / effectiveFps);
        }

        function stopLoop() {
            if (intervalId) {
                clearInterval(intervalId);
                intervalId = null;
            }
        }

        function enable() {
            if (enabled) return;
            enabled = true;
            canvas.style.display = 'block';
            video.style.visibility = 'hidden'; // Keep layout space!
            applyConfigToCanvas();
            drawFrame(true);
            if (!video.paused && !video.ended) startLoop();
        }

        function disable() {
            if (!enabled) return;
            enabled = false;
            canvas.style.display = 'none';
            video.style.visibility = '';
            stopLoop();
        }

        function toggle() {
            if (enabled) disable();
            else enable();
        }

        function isEnabled() {
            return enabled;
        }

        function onConfigChanged() {
            const wasEnabled = enabled;
            stopLoop();
            applyConfigToCanvas();
            if (wasEnabled) {
                drawFrame(true);
                if (!video.paused && !video.ended) {
                    startLoop();
                }
            }
        }

        const handleFrameUpdate = () => {
            if (enabled) drawFrame(true);
        };
        const handleMetaLoaded = () => {
            applyConfigToCanvas();
            if (enabled) drawFrame(true);
        };

        function destroy() {
            disable();
            canvas.remove();
            video.removeEventListener('play', startLoop);
            video.removeEventListener('pause', stopLoop);
            video.removeEventListener('ended', stopLoop);
            video.removeEventListener('seeked', handleFrameUpdate);
            video.removeEventListener('loadeddata', handleMetaLoaded);
            video.removeEventListener('loadedmetadata', handleMetaLoaded);
        }

        video.addEventListener('play', startLoop);
        video.addEventListener('pause', stopLoop);
        video.addEventListener('ended', stopLoop);
        video.addEventListener('seeked', handleFrameUpdate);
        video.addEventListener('loadeddata', handleMetaLoaded);
        video.addEventListener('loadedmetadata', handleMetaLoaded);

        return { enable, disable, toggle, isEnabled, destroy, onConfigChanged };
    }

    function setupVideo(video) {
        if (!video) return;

        // Ignore sidebar sticker / emoji preview / modal videos or scroller feed
        if (video.closest && video.closest('.sidebar-activity, .global-sidebar-right, .emoji-preview, .modal, #scroller-feed, .scroll-slide')) {
            return;
        }

        if (!isItemPage()) {
            if (ui) ui.wrapper.style.display = 'none';
            return;
        }

        // Prioritize the main item player (id="my-video" or class "viewer")
        const isPrimary = video.id === 'my-video' || video.classList.contains('viewer') || video.classList.contains('v0ck_video') || (video.closest && !!video.closest('.media-object, .v0ck'));

        if (video.dataset.flashFilterAttached === '1') {
            // If already attached, ensure its currentController is restored if it's the primary one
            if (isPrimary && !currentController) {
                currentController = video.__flashFilterController;
            }
            return;
        }
        
        video.dataset.flashFilterAttached = '1';
        const controller = makeController(video);
        video.__flashFilterController = controller;

        // If this is the primary video, or we don't have one yet, set as current
        if (isPrimary || !currentController) {
            currentController = controller;
        }

        // Always sync with global setting on creation
        if (settings.enabled) {
            controller.enable();
        } else {
            controller.disable();
        }

        // Small delay to ensure v0ck has injected its controls
        setTimeout(() => {
            if (ui) updateUIFromSettings();
        }, 50);
        if (ui && isPrimary) {
            ui.wrapper.style.display = 'block';
        }
    }

    function scanExistingVideos() {
        document.querySelectorAll('video').forEach(setupVideo);
    }

    function startObserver() {
        if (!document.body) return;

        const observer = new MutationObserver((mutations) => {
            for (const m of mutations) {
                m.addedNodes.forEach((node) => {
                    if (node.nodeType !== 1) return;
                    if (node.tagName === 'VIDEO') {
                        setupVideo(node);
                    } else if (node.querySelectorAll) {
                        node.querySelectorAll('video').forEach(setupVideo);
                    }
                });

                m.removedNodes.forEach((node) => {
                    if (node.nodeType !== 1) return;

                    const vids = [];
                    if (node.tagName === 'VIDEO') {
                        vids.push(node);
                    }
                    if (node.querySelectorAll) {
                        node.querySelectorAll('video').forEach(v => vids.push(v));
                    }

                    vids.forEach((v) => {
                        const c = v.__flashFilterController;
                        if (c) {
                            c.destroy();
                            if (currentController === c) {
                                currentController = null;
                            }
                        }
                    });

                    if (ui && document.querySelectorAll('video').length === 0) {
                        ui.wrapper.style.display = 'none';
                    }
                });
            }
        });

        observer.observe(document.body, { childList: true, subtree: true });
    }

    // ---------- UI Panel ----------

    function createOptionsUI() {
        if (ui || !document.body) return;

        const wrapper = document.createElement('div');
        wrapper.style.position = 'fixed';
        wrapper.style.bottom = '10px';
        wrapper.style.right = '10px';
        wrapper.style.zIndex = '99999';
        wrapper.style.fontFamily = 'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
        wrapper.style.fontSize = '12px';
        wrapper.style.color = '#fff';
        wrapper.style.display = 'none'; // Initially hidden
        wrapper.id = 'flash-yank-ui';

        const floatingBadge = document.createElement('div');
        floatingBadge.id = 'w0bm-floating-swf';
        floatingBadge.textContent = 'SWF';
        floatingBadge.style.background = 'rgba(0,0,0,0.9)';
        floatingBadge.style.padding = '2px 8px';
        floatingBadge.style.borderRadius = '3px';
        floatingBadge.style.cursor = 'pointer';
        floatingBadge.style.fontWeight = 'bold';
        floatingBadge.style.letterSpacing = '0.08em';

        const panel = document.createElement('div');
        panel.style.position = 'fixed';
        panel.style.right = '10px';
        panel.style.bottom = '22px';
        panel.style.background = 'rgba(0,0,0,0.9)';
        panel.style.color = '#fff';
        panel.style.padding = '6px 10px';
        panel.style.fontSize = '12px';
        panel.style.borderRadius = '4px';
        panel.style.maxWidth = '320px';
        panel.style.minWidth = '240px';
        panel.style.maxHeight = 'calc(100vh - 40px)'; // Prevent exceeding screen height
        panel.style.overflowY = 'auto'; // Make scrollable
        panel.style.boxSizing = 'border-box';
        panel.style.boxShadow = '0 0 6px rgba(0,0,0,0.7)';
        panel.style.display = 'none';

        panel.innerHTML = `
        <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px;">
            <div id="w0bm-title" style="font-weight:bold; cursor:pointer;">Flash Yank</div>
            <div id="w0bm-close" style="cursor:pointer; font-size:16px; padding: 0 4px;">&times;</div>
        </div>

        <label style="font-size:11px; display:block; margin-bottom:2px;">
        Yank: <span id="w0bm-yank-value"></span>
        </label>
        <input id="w0bm-yank-slider" type="range" min="0" max="100" step="1" style="width:100%;">

        <div id="w0bm-yank-info" style="font-size:10px; margin-top:8px; opacity: 0.8;"></div>
        `;

        wrapper.appendChild(panel);
        wrapper.appendChild(floatingBadge);
        document.body.appendChild(wrapper);

        let panelVisible = false;
        const HOVER_MARGIN = 50; // px around panel before closing

        let activePlayer = null;

        function showPanel(trigger) {
            if (panelVisible) return;
            panelVisible = true;
            panel.style.display = 'block';

            activePlayer = trigger.closest('.v0ck');
            if (activePlayer) activePlayer.classList.add('v0ck_swf_active');
        }

        function hidePanel() {
            if (!panelVisible) return;
            panelVisible = false;
            panel.style.display = 'none';

            if (activePlayer) {
                activePlayer.classList.remove('v0ck_swf_active');
                activePlayer = null;
            }
        }

        function handleBadgeHover(target) {
            if (target.id === 'toggleswf' || target === floatingBadge) {
                // Anchor to trigger element (shared logic for mobile/desktop)
                const rect = target.getBoundingClientRect();
                panel.style.bottom = (window.innerHeight - rect.top + 10) + 'px';
                panel.style.right = (window.innerWidth - rect.right) + 'px';
                panel.style.transform = 'none';
                panel.style.maxWidth = isMobile ? '90vw' : '320px';
                
                showPanel(target);
            }
        }

        document.addEventListener('mouseover', (e) => {
            const target = e.target.closest('#toggleswf') || (e.target === floatingBadge ? floatingBadge : null);
            if (target) {
                handleBadgeHover(target);
            }
        });

        document.addEventListener('mousemove', (e) => {
            if (!panelVisible || isMobile) return; // Don't hide by mousemove on mobile
            const rect = panel.getBoundingClientRect();
            const left = rect.left - HOVER_MARGIN;
            const right = rect.right + HOVER_MARGIN;
            const top = rect.top - HOVER_MARGIN;
            const bottom = rect.bottom + HOVER_MARGIN;

            if (e.clientX < left || e.clientX > right || e.clientY < top || e.clientY > bottom) {
                const overTrigger = e.target.closest && (e.target.closest('#toggleswf') || e.target === floatingBadge);
                if (!overTrigger) {
                    hidePanel();
                }
            }
        });

        const title = panel.querySelector('#w0bm-title');
        const slider = panel.querySelector('#w0bm-yank-slider');
        const yankValue = panel.querySelector('#w0bm-yank-value');
        const info = panel.querySelector('#w0bm-yank-info');

        ui = {
            wrapper,
            floatingBadge,
            panel,
            title,
            slider,
            yankValue,
            info
        };

        slider.addEventListener('input', (e) => setYank(e.target.value));

        function toggleEnabledFromUI(targetController) {
            settings.enabled = !settings.enabled;
            saveSettings();
            
            let controller = targetController || currentController;
            if (!controller) {
                const mainVid = document.getElementById('my-video') || document.querySelector('video.v0ck_video, video.viewer, video');
                if (mainVid) {
                    if (!mainVid.__flashFilterController) setupVideo(mainVid);
                    controller = mainVid.__flashFilterController;
                }
            }
            if (controller) {
                if (settings.enabled) controller.enable();
                else controller.disable();
            }
            
            updateUIFromSettings();

            if (typeof window.flashMessage === 'function') {
                window.flashMessage(`Flash Yank ${settings.enabled ? 'enabled' : 'disabled'}`, 2000, settings.enabled ? 'success' : 'info');
            }
        }

        // Click SWF badge or title to toggle filter enabled/disabled
        document.addEventListener('click', (e) => {
            const swfBtn = e.target.closest('#toggleswf');
            const isBadge = swfBtn || e.target === floatingBadge;
            const isTitle = e.target === title;
            if (isBadge || isTitle) {
                // If clicked a button inside a player, try to get THAT player's controller
                let targetCtrl = null;
                if (swfBtn) {
                    const player = swfBtn.closest('.v0ck') || swfBtn.closest('.media-object') || document.querySelector('.v0ck');
                    const vid = player ? player.querySelector('video') : (document.getElementById('my-video') || document.querySelector('video'));
                    if (vid) {
                        if (!vid.__flashFilterController) {
                            setupVideo(vid);
                        }
                        targetCtrl = vid.__flashFilterController;
                    }
                }
                toggleEnabledFromUI(targetCtrl);
                // Explicitly show options panel on click for both mobile and desktop
                if (swfBtn || floatingBadge) {
                    handleBadgeHover(swfBtn || floatingBadge);
                }
            }
        });

        const closeBtn = panel.querySelector('#w0bm-close');
        if (closeBtn) {
            closeBtn.addEventListener('click', () => {
                hidePanel();
            });
        }

        // Close Flash Yank panel when the main settings menu is closed
        document.addEventListener('v0ck_settings_closed', () => {
            hidePanel();
        });

        updateUIFromSettings();
    }

    function updateUIFromSettings() {
        if (!ui) return;

        const isItem = isItemPage();
        const hasVideos = document.querySelectorAll('video').length > 0;

        if (!isItem || !hasVideos) {
            ui.wrapper.style.display = 'none';
            return;
        }
        ui.slider.value = settings.yank;
        ui.yankValue.textContent = settings.yank + '%';

        const approxColors = Math.pow(currentConfig.paletteLevels, 3);
        ui.info.textContent = currentConfig.internalWidth + 'px width, ' +
            currentConfig.fps + 'fps, ~' + approxColors + ' colors';

        // Enable/disable controls based on enabled state
        const isEnabled = !!settings.enabled;
        ui.slider.disabled = !isEnabled;

        // Visual state: strike-through when disabled
        const swfButtons = Array.from(document.querySelectorAll('#toggleswf, .v0ck_menu_item')).filter(b => b.id === 'toggleswf' || b.textContent.trim() === 'SWF');

        // Handle floating badge visibility — only show as fallback when on an item page
        // with the primary player present but no in-player SWF button (e.g. v0ck not loaded).
        // Never show it just because sidebar .webm stickers exist.
        const primaryVideo = document.getElementById('my-video') ||
            document.querySelector('video.viewer, video.v0ck_video');
        // Retired: the settings live in the sidebar Tuner (Flash Yank tab) now, so no floating fallback badge
        void primaryVideo;
        ui.floatingBadge.style.display = 'none';

        // Style both (if they exist)
        [ui.floatingBadge, ...swfButtons].forEach(b => {
            if (!b) return;
            b.style.textDecoration = isEnabled ? 'none' : 'line-through';
            b.style.opacity = isEnabled ? '1' : '0.6';
            if (b.classList.contains('v0ck_menu_item') || b.id === 'toggleswf') {
                b.style.color = isEnabled ? 'var(--accent, #9f0)' : '#fff';
                b.style.fontWeight = isEnabled ? 'bold' : 'normal';
            }
        });
    }

    // ---------- Hotkey ----------

    function attachHotkey() {
        if (hotkeyAttached) return;
        hotkeyAttached = true;

        document.addEventListener('keydown', (e) => {
            if (document.body?.classList.contains('scroller-active') ||
                document.getElementById('scroller-feed') ||
                /^\/scroller(\/|$)/.test(window.location.pathname)) {
                return;
            }
            if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey || !isItemPage()) return;
            if (e.key.toLowerCase() !== 's') return;

            // Ignore when typing in an input, textarea, or contenteditable
            const tag = document.activeElement?.tagName?.toLowerCase();
            if (tag === 'input' || tag === 'textarea' || document.activeElement?.isContentEditable) return;

            if (!currentController) {
                const mainVid = document.getElementById('my-video') || document.querySelector('video.v0ck_video, video.viewer, video');
                if (mainVid) {
                    if (!mainVid.__flashFilterController) setupVideo(mainVid);
                    currentController = mainVid.__flashFilterController;
                }
            }
            if (!currentController) return;

            setEnabled(!settings.enabled);
        });
    }

    // ---------- Public API (sidebar Tuner → Flash Yank) ----------
    // The in-player "SWF" button is gone; the Tuner pane drives the filter through this API and
    // listens to 'f0ck:flashyank_changed' (also fired by the 's' hotkey) to stay in sync.

    function findController() {
        if (currentController) return currentController;
        const mainVid = document.getElementById('my-video') || document.querySelector('video.v0ck_video, video.viewer, video');
        if (!mainVid) return null;
        if (!mainVid.__flashFilterController) setupVideo(mainVid);
        return mainVid.__flashFilterController || null;
    }

    function getState() {
        return {
            enabled: !!settings.enabled,
            yank: settings.yank,
            width: currentConfig.internalWidth,
            fps: currentConfig.fps,
            colors: Math.pow(currentConfig.paletteLevels, 3),
            available: isItemPage() && document.querySelectorAll('video').length > 0
        };
    }

    function emitChange() {
        window.dispatchEvent(new CustomEvent('f0ck:flashyank_changed', { detail: getState() }));
    }

    function setEnabled(on, { silent = false } = {}) {
        settings.enabled = !!on;
        saveSettings();
        const controller = findController();
        if (controller) {
            if (settings.enabled) controller.enable();
            else controller.disable();
        }
        updateUIFromSettings();
        emitChange();
        if (!silent && typeof window.flashMessage === 'function') {
            window.flashMessage('Flash Yank ' + (settings.enabled ? 'enabled' : 'disabled'), 2000, settings.enabled ? 'success' : 'info');
        }
    }

    function setYank(value) {
        const v = parseInt(value, 10);
        settings.yank = isNaN(v) ? 0 : Math.min(100, Math.max(0, v));
        updateAdvancedFromYank();
        saveSettings();
        applySettingsToRuntime();
        emitChange();
    }

    window.f0ckFlashYank = {
        getState,
        setEnabled: (on) => setEnabled(on),
        toggle: () => setEnabled(!settings.enabled),
        setYank,
        defaults: { yank: DEFAULT_SETTINGS.yank }
    };

    // ---------- Bootstrapping ----------

    function onReady(fn) {
        if (document.readyState === 'loading') {
            document.addEventListener('DOMContentLoaded', fn, { once: true });
        } else {
            fn();
        }
    }

    onReady(() => {
        // firefox mobile check
        const isFirefoxMobile = /android|iphone|ipad|ipod/i.test(navigator.userAgent) && /firefox/i.test(navigator.userAgent);
        if (isFirefoxMobile) {
            window.f0ckDebug("Firefox Mobile detected, disabling Flash Yank script.");
            return;
        }

        applySettingsToRuntime();
        createOptionsUI();
        scanExistingVideos();
        startObserver();
        attachHotkey();

        // Path change handling for AJAX transitions
        const handlePathChange = () => {
             // Reset currentController on path change to force re-discovery
             currentController = null;
             if (!isItemPage()) {
                 if (ui) ui.wrapper.style.display = 'none';
             } else {
                 // Scan after a short delay to allow DOM/v0ck to settle
                 setTimeout(() => {
                    scanExistingVideos();
                    updateUIFromSettings();
                 }, 100);
             }
        };

        window.addEventListener('popstate', handlePathChange);
        document.addEventListener('f0ck:contentLoaded', handlePathChange);
    });
})();
