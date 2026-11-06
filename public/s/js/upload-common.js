/**
 * common upload logic for f0ck
 * shared between /upload page and global drag-and-drop modal
 */
window.F0ckUpload = class {
    constructor(options) {
        this.form = options.form;
        this.config = options.config || {};
        this.onProgress = options.onProgress || (() => {});
        this.onComplete = options.onComplete || (() => {});
        this.onError = options.onError || (() => {});
        this.onStatusChange = options.onStatusChange || (() => {});
        
        this.selectedFile = null;
        this.tags = [];
        this.minTags = options.minTags ?? 0;
        
        this.init();
    }

    init() {
        if (!this.form) return;
        this.bindEvents();
    }

    bindEvents() {
        // Tag input handling (if exists in this form instance)
        const tagInput = this.form.querySelector('.tag-input');
        if (tagInput) {
            tagInput.addEventListener('keydown', (e) => {
                if (e.key === 'Enter' && !e.ctrlKey && !e.metaKey) {
                    e.preventDefault();
                    this.addTag(tagInput.value);
                    tagInput.value = '';
                }
            });
        }
    }

    formatSize(bytes) {
        const units = ['B', 'KB', 'MB', 'GB'];
        let i = 0;
        while (bytes >= 1024 && i < units.length - 1) {
            bytes /= 1024;
            i++;
        }
        return bytes.toFixed(2) + ' ' + units[i];
    }

    validateFile(file) {
        if (!file) return false;
        const allowedMimes = this.config.allowedMimes || [];
        if (allowedMimes.length > 0 && !allowedMimes.includes(file.type)) {
            return { error: `File type ${file.type} is not allowed.` };
        }
        return { success: true };
    }

    addTag(tagName) {
        tagName = tagName.trim();
        if (!tagName || this.tags.some(t => t.toLowerCase() === tagName.toLowerCase())) return;
        if (['sfw', 'nsfw', 'nsfl'].includes(tagName.toLowerCase())) return;
        
        this.tags.push(tagName);
        this.onStatusChange({ type: 'tags_updated', tags: this.tags });
    }

    removeTag(tagName) {
        this.tags = this.tags.filter(t => t !== tagName);
        this.onStatusChange({ type: 'tags_updated', tags: this.tags });
    }

    clearTags() {
        this.tags = [];
        this.onStatusChange({ type: 'tags_updated', tags: this.tags });
    }

    setFile(file) {
        const validation = this.validateFile(file);
        if (validation.error) {
            this.onError(validation.error);
            return false;
        }
        this.selectedFile = file;
        this.onStatusChange({ type: 'file_selected', file: file });
        return true;
    }

    async upload() {
        if (!this.selectedFile) {
            this.onError('No file selected');
            return;
        }
        
        const rating = this.form.querySelector('input[name="rating"]:checked');
        if (!rating) {
            this.onError('Please select a rating');
            return;
        }

        if (this.tags.length < this.minTags) {
            this.onError(`At least ${this.minTags} tag${this.minTags === 1 ? ' is' : 's are'} required`);
            return;
        }

        const formData = new FormData();
        formData.append('file', this.selectedFile);
        formData.append('rating', rating.value);
        formData.append('tags', this.tags.join(','));

        return new Promise((resolve, reject) => {
            const xhr = new XMLHttpRequest();

            xhr.upload.addEventListener('progress', (e) => {
                if (e.lengthComputable) {
                    const percent = Math.round((e.loaded / e.total) * 100);
                    this.onProgress(percent);
                }
            });

            xhr.onload = () => {
                try {
                    const res = JSON.parse(xhr.responseText);
                    if (res.success) {
                        if (res.itemid) {
                            try {
                                const ts = Date.now();
                                const bustedStr = localStorage.getItem('bustedThumbs');
                                const busted = bustedStr ? JSON.parse(bustedStr) : {};
                                busted[res.itemid] = ts;
                                const keys = Object.keys(busted);
                                if (keys.length > 50) delete busted[keys[0]];
                                localStorage.setItem('bustedThumbs', JSON.stringify(busted));
                            } catch(e) {}
                        }
                        this.onComplete(res);
                        resolve(res);
                    } else {
                        this.onError(res.msg, res);
                        reject(res);
                    }
                } catch (err) {
                    this.onError('Upload failed. Server returned invalid response.');
                    reject(err);
                }
            };

            xhr.onerror = (err) => {
                this.onError('Network error occurred during upload.');
                reject(err);
            };

            xhr.open('POST', '/api/v2/upload');
            xhr.setRequestHeader('X-CSRF-Token', window.f0ckSession?.csrf_token || '');
            xhr.send(formData);
        });
    }
};
