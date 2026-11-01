/**
 * Global Drag and Drop Initialization - ROBUST VERSION
 */
(() => {
    const dropOverlay = document.getElementById('drop-overlay');
    const dragModal = document.getElementById('upload-drag-modal');
    const dragModalClose = document.getElementById('drag-modal-close');
    const dragForm = dragModal ? dragModal.querySelector('.upload-form') : null;

    const navUploadLink = document.getElementById('nav-upload-link');

    // Dropdown placement: right under the navbar "Upload" link, left-aligned with it and kept inside the
    // window. Without a visible link (phone: it sits in the collapsed menu) it drops from the navbar.
    const positionModal = () => {
        if (!dragModal || !dragModal.classList.contains('show')) return;
        const content = dragModal.querySelector('.modal-content');
        if (!content) return;
        const vw = document.documentElement.clientWidth;
        const linkRect = navUploadLink ? navUploadLink.getBoundingClientRect() : null;
        const useLink = !!(linkRect && linkRect.width > 0 && linkRect.height > 0);
        const nav = document.querySelector('body > nav.navbar, nav.navbar');
        const top = Math.round((useLink ? linkRect.bottom : (nav ? nav.getBoundingClientRect().bottom : 0)) + 6);
        content.style.top = top + 'px';
        content.style.maxHeight = Math.max(200, window.innerHeight - top - 12) + 'px';
        const w = content.offsetWidth;
        let left = useLink ? linkRect.left : (vw - w) / 2;
        left = Math.max(8, Math.min(left, vw - w - 8));
        content.style.left = Math.round(left) + 'px';
    };
    window.addEventListener('resize', positionModal, { passive: true });

    // The navbar link shows when the dropdown is open. Watched on the modal itself because it gets closed
    // from several places (close button, Escape, toggle, finished upload, hideAllModals).
    if (dragModal && navUploadLink) {
        const syncLink = () => {
            const open = dragModal.classList.contains('show');
            navUploadLink.classList.toggle('is-active', open);
            navUploadLink.setAttribute('aria-expanded', open ? 'true' : 'false');
        };
        new MutationObserver(syncLink).observe(dragModal, { attributes: true, attributeFilter: ['class'] });
        syncLink();
    }
    const navEl = document.querySelector('body > nav.navbar, nav.navbar');
    if (navEl && window.ResizeObserver) new ResizeObserver(positionModal).observe(navEl);

    const showModal = () => {
        if (!dragModal) return;
        dragModal.classList.add('show', 'is-dropdown');
        document.body.classList.add('modal-open');
        positionModal();
        // Reset scroll position so it always starts at the top
        dragModal.scrollTop = 0;
        const modalContent = dragModal.querySelector('.modal-content');
        if (modalContent) modalContent.scrollTop = 0;
        const modalBody = dragModal.querySelector('.modal-body');
        if (modalBody) modalBody.scrollTop = 0;
    };

    // Hide without resetting: a picked file / typed tags survive, reopening continues there.
    // (The close button and Escape still close + reset, see below.)
    const hideModal = () => {
        if (!dragModal) return;
        dragModal.classList.remove('show');
        document.body.classList.remove('modal-open');
    };

    // Navbar upload link — always attached so it works even if drag-drop can't init.
    // Toggles the dropdown when available; falls back to /upload navigation otherwise.
    if (navUploadLink) {
        navUploadLink.addEventListener('click', (e) => {
            if (!dragModal) return; // no modal → fall back to href navigation
            e.preventDefault();
            if (dragModal.classList.contains('show')) hideModal();
            else showModal();
        });
    }

    if (!dropOverlay || !dragModal || !dragForm) {
        console.warn('[f0ck] Quick Upload Modal or Form missing, global drag-drop disabled.');
        return;
    }

    let dragCounter = 0;
    let uploader = null;

    // Use the consolidated initUploadForm from upload.js
    if (window.initUploadForm) {
        uploader = window.initUploadForm(dragForm);
    } else {
        console.error('[f0ck] window.initUploadForm is missing! upload.js not loaded?');
        return;
    }

    // Mark internal drags to prevent them from triggering the dropzone
    window.addEventListener('dragstart', (e) => {
        if (e.dataTransfer) {
            e.dataTransfer.setData('application/x-f0ck-internal', 'true');
        }
    });

    // Global Drag Events
    window.addEventListener('dragenter', (e) => {
        e.preventDefault();
        if (window.location.pathname === '/upload') return;
        if (e.dataTransfer && e.dataTransfer.types.includes('application/x-f0ck-internal')) return;
        if (e.dataTransfer && e.dataTransfer.types.includes('Files')) {
            dragCounter++;
            dropOverlay.classList.add('active');
        }
    });

    window.addEventListener('dragover', (e) => {
        e.preventDefault();
        if (window.location.pathname === '/upload') return;
        if (e.dataTransfer && e.dataTransfer.types.includes('application/x-f0ck-internal')) {
            // Change dropEffect to none to indicate this dropzone doesn't accept internal elements
            e.dataTransfer.dropEffect = 'none';
        }
    });

    window.addEventListener('dragleave', (e) => {
        if (window.location.pathname === '/upload') return;
        // dragleave doesn't always have reliable dataTransfer.types, but if it does, check it
        if (e.dataTransfer && e.dataTransfer.types.includes('application/x-f0ck-internal')) return;
        
        dragCounter--;
        if (dragCounter <= 0) {
            dragCounter = 0;
            dropOverlay.classList.remove('active');
        }
    });

    window.addEventListener('drop', (e) => {
        if (window.location.pathname === '/upload') return;
        if (e.dataTransfer && e.dataTransfer.types.includes('application/x-f0ck-internal')) return;
        
        e.preventDefault();
        dragCounter = 0;
        dropOverlay.classList.remove('active');

        const files = e.dataTransfer.files;
        if (files && files.length > 0) {
            if (uploader && uploader.handleFile) {
                const ok = uploader.handleFile(files);
                if (ok !== false) {
                    showModal();
                }
            }
        }
    });

    // Modal Close
    dragModalClose.onclick = () => {
        dragModal.classList.remove('show');
        document.body.classList.remove('modal-open');
        if (uploader && uploader.reset) {
            uploader.reset();
        }
    };

    // Close on ESC
    window.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && dragModal.classList.contains('show')) {
            dragModalClose.onclick();
        }
    });

    // Toggle on 'u' shortcut (not on /upload, not when typing). Like the navbar link, closing
    // keeps the picked file / typed tags.
    window.addEventListener('keydown', (e) => {
        if (e.key === 'u' && !e.ctrlKey && !e.altKey && !e.metaKey) {
            const tag = document.activeElement?.tagName;
            if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || document.activeElement?.isContentEditable) return;
            if (window.location.pathname === '/upload') return;

            e.preventDefault();
            if (dragModal.classList.contains('show')) hideModal();
            else showModal();
        }
    });

    // Global Paste Event for Clipboard Images/Videos/URLs
    window.addEventListener('paste', (e) => {
        const activeTag = document.activeElement?.tagName;
        const isTyping = activeTag === 'INPUT' || activeTag === 'TEXTAREA' || activeTag === 'SELECT' || document.activeElement?.isContentEditable;
        
        const isModalOpen = dragModal.classList.contains('show');
        const isUploadPage = window.location.pathname === '/upload';
        
        // Items loop for files
        const items = e.clipboardData?.items;
        let file = null;
        if (items) {
            for (let i = 0; i < items.length; i++) {
                const item = items[i];
                if (item.kind === 'file' && (item.type.startsWith('image/') || item.type.startsWith('video/'))) {
                    file = item.getAsFile();
                    break;
                }
            }
        }

        if (file) {
            // Find target uploader
            let targetUploader = null;
            if (isModalOpen) {
                targetUploader = uploader;
            } else if (isUploadPage) {
                const pageForm = document.querySelector('.pagewrapper .upload-form') || document.querySelector('#main .upload-form');
                targetUploader = (pageForm && pageForm._f0ckUploader) ? pageForm._f0ckUploader : uploader;
            } else {
                targetUploader = uploader;
            }

            if (targetUploader && targetUploader.handleFile) {
                if (isUploadPage || isModalOpen) {
                    targetUploader.handleFile([file]);
                    e.preventDefault();
                } else if (!isTyping) {
                    e.preventDefault();
                    showModal();
                    targetUploader.handleFile([file]);
                }
            }
            return;
        }

        // Handle URL paste (only if NOT typing)
        const text = e.clipboardData.getData('text')?.trim();
        if (text && (text.startsWith('http://') || text.startsWith('https://') || text.includes('youtube.com/') || text.includes('youtu.be/'))) {
            if (!isTyping) {
                e.preventDefault();

                // Context-aware target selection
                let targetContainer = dragModal;
                if (!isModalOpen && isUploadPage) {
                    targetContainer = document.querySelector('.pagewrapper .upload-form') || 
                                      document.querySelector('#main .upload-form') || 
                                      dragModal;
                }
                
                if (targetContainer === dragModal && !isModalOpen && !isUploadPage) {
                    showModal();
                }

                // Switch to URL tab
                const urlTab = targetContainer.querySelector('.upload-mode-tab[data-mode="url"]');
                if (urlTab) urlTab.click();

                // Fill input
                const urlInput = targetContainer.querySelector('#url-upload-input');
                if (urlInput) {
                    urlInput.value = text;
                    urlInput.dispatchEvent(new Event('input', { bubbles: true }));
                }
            }
        }
    });
})();
