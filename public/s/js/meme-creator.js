/**
 * Meme Creator Logic
 * DYNAMIC MULTIPLE LAYERS
 */

(() => {
    const canvas = document.getElementById('memeCanvas');
    if (canvas) {
        const ctx = canvas.getContext('2d');
        const layersContainer = document.getElementById('textLayersContainer');
        const addTextBtn = document.getElementById('addText');

        const uploadBtn = document.getElementById('uploadMeme');
        
        // Core state
        let textLayers = [];
        let dragOffset = { x: 0, y: 0 };
        let draggingLayer = null; 
        let hoveredLayer = null;
        let img = new Image();
        let hasLoadedImage = window.memeTemplate.id !== 'custom' && window.memeTemplate.category !== 'Custom';

        // Show the local file picker only when no pre-selected template exists
        if (!hasLoadedImage) {
            const customSelector = document.getElementById('customTemplateSelector');
            if (customSelector) customSelector.style.display = '';
        }

        const memeFont = 'Impact, Charcoal, sans-serif';
        const memeLetterSpacing = '0.03em';

        function wrapText(ctx, text, maxWidth) {
            const paragraphs = text.split('\n');
            const lines = [];
            
            paragraphs.forEach(paragraph => {
                if (paragraph.trim() === '') {
                    lines.push('');
                    return;
                }
                
                const words = paragraph.split(' ').filter(w => w !== '');
                let currentLine = '';
                
                words.forEach(word => {
                    const testLine = currentLine ? currentLine + ' ' + word : word;
                    let metrics = ctx.measureText(testLine);
                    
                    if (metrics.width <= maxWidth) {
                        currentLine = testLine;
                    } else {
                        if (currentLine !== '') {
                            lines.push(currentLine);
                            currentLine = '';
                        }
                        
                        metrics = ctx.measureText(word);
                        if (metrics.width <= maxWidth) {
                            currentLine = word;
                        } else {
                            let charLine = '';
                            for (let i = 0; i < word.length; i++) {
                                const char = word[i];
                                const testCharLine = charLine + char;
                                if (ctx.measureText(testCharLine).width > maxWidth && charLine !== '') {
                                    lines.push(charLine);
                                    charLine = char;
                                } else {
                                    charLine = testCharLine;
                                }
                            }
                            currentLine = charLine;
                        }
                    }
                });
                if (currentLine) {
                    lines.push(currentLine);
                }
            });
            
            return lines;
        }


        // Image Setup
        img.crossOrigin = "anonymous";
        img.onload = () => {
            canvas.width = img.width || 800;
            canvas.height = img.height || 600;
            
            const defaultSize = 40;
            
            // Initial layers - only set if we don't have any layers yet and we have loaded an image
            if (textLayers.length === 0 && hasLoadedImage) {
                textLayers = [
                    { id: Date.now(), text: '', x: canvas.width / 2, y: 40, fontSize: defaultSize },
                    { id: Date.now() + 1, text: '', x: canvas.width / 2, y: canvas.height - 100, fontSize: defaultSize }
                ];
            } else if (hasLoadedImage) {
                // Keep the text layers but adjust their coordinates to be in-bounds if they exceed new boundaries
                textLayers.forEach(layer => {
                    if (layer.x > canvas.width) {
                        layer.x = canvas.width / 2;
                    }
                    if (layer.y > canvas.height) {
                        layer.y = canvas.height - 100;
                    }
                });
            }
            
            renderInputs();
            draw();
        };
        img.src = window.memeTemplate.url;

        // Ensure font is loaded before first draw
        if (document.fonts) {
            // Force browser to load the Impact font
            document.fonts.load('bold 40px Impact');
            document.fonts.load('bold 80px Impact');
            document.fonts.load('bold 150px Impact');
            document.fonts.ready.then(() => {
                draw();
            });
        }

        function createSlider(container, min, max, initValue, onChange) {
            container.style.cssText = 'position:relative;height:28px;display:flex;align-items:center;flex:1;cursor:pointer;user-select:none;-webkit-user-select:none;touch-action:none;';

            const track = document.createElement('div');
            track.style.cssText = 'position:absolute;left:8px;right:8px;height:4px;background:#333;border-radius:2px;top:50%;transform:translateY(-50%);';

            const fill = document.createElement('div');
            fill.style.cssText = 'position:absolute;left:0;height:100%;background:var(--accent,#9f0);border-radius:2px;pointer-events:none;';

            const thumb = document.createElement('div');
            thumb.style.cssText = 'position:absolute;width:18px;height:18px;background:var(--accent,#9f0);border-radius:50%;top:50%;transform:translate(-50%,-50%);box-shadow:0 0 6px rgba(0,0,0,.6);pointer-events:none;transition:transform .1s;';

            const setRatio = (r) => {
                fill.style.width = (r * 100) + '%';
                thumb.style.left   = (r * 100) + '%';
            };
            setRatio((initValue - min) / (max - min));

            track.appendChild(fill);
            track.appendChild(thumb);
            container.appendChild(track);

            const valueFromClientX = (clientX) => {
                const rect = track.getBoundingClientRect();
                const r    = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
                setRatio(r);
                return Math.round(min + r * (max - min));
            };

            container.addEventListener('pointerdown', (e) => {
                e.preventDefault(); // block keyboard + scroll takeover
                container.setPointerCapture(e.pointerId);
                thumb.style.transform = 'translate(-50%,-50%) scale(1.25)';
                onChange(valueFromClientX(e.clientX));
            }, { passive: false });

            container.addEventListener('pointermove', (e) => {
                if (!container.hasPointerCapture(e.pointerId)) return;
                onChange(valueFromClientX(e.clientX));
            });

            const onEnd = (e) => {
                if (!container.hasPointerCapture(e.pointerId)) return;
                container.releasePointerCapture(e.pointerId);
                thumb.style.transform = 'translate(-50%,-50%) scale(1)';
            };
            container.addEventListener('pointerup',     onEnd);
            container.addEventListener('pointercancel', onEnd);
        }

        function renderInputs() {
            layersContainer.innerHTML = '';
            textLayers.forEach((layer, index) => {
                const div = document.createElement('div');
                div.className = 'form-group layer-input-group';
                div.style.marginBottom = '20px';
                div.innerHTML = `
                    <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px;">
                        <label style="margin-bottom: 0; font-weight: bold;">${(window.f0ckI18n?.meme?.text_layer) || 'Text Layer'} ${index + 1}</label>
                        <button class="remove-layer" data-id="${layer.id}" style="background: transparent; border: none; color: #ff4444; cursor: pointer; padding: 0 5px;">
                            <i class="fa fa-times"></i>
                        </button>
                    </div>
                    <textarea data-id="${layer.id}" placeholder="${(window.f0ckI18n?.meme?.enter_text) || 'Enter text...'}" rows="2" style="width: 100%; margin-bottom: 8px;">${layer.text}</textarea>
                    
                    <div class="layer-font-size-control" style="display: flex; align-items: center; gap: 10px;">
                        <span style="font-size: 0.8em; color: #888; white-space: nowrap;">${(window.f0ckI18n?.meme?.size_label) || 'Size'}: <span class="layer-fs-val">${layer.fontSize}</span>px</span>
                        <div class="layer-fs-slider" style="flex: 1;"></div>
                    </div>
                `;
                
                const textarea = div.querySelector('textarea');
                textarea.addEventListener('input', (e) => {
                    layer.text = e.target.value;
                    draw();
                });

                const fsSlider = div.querySelector('.layer-fs-slider');
                const fsVal    = div.querySelector('.layer-fs-val');
                createSlider(fsSlider, 10, 200, layer.fontSize, (val) => {
                    layer.fontSize    = val;
                    fsVal.textContent = val;
                    draw();
                });

                const removeBtn = div.querySelector('.remove-layer');
                removeBtn.addEventListener('click', () => {
                    textLayers = textLayers.filter(l => l.id !== layer.id);
                    renderInputs();
                    draw();
                });

                layersContainer.appendChild(div);
            });
        }

        addTextBtn.addEventListener('click', () => {
            if (!hasLoadedImage) {
                window.flashMessage((window.f0ckI18n?.meme?.choose_image_first) || 'Please select an image first!', 3000, 'error');
                return;
            }
            textLayers.push({
                id: Date.now(),
                text: 'NEW TEXT',
                x: canvas.width / 2,
                y: canvas.height / 2,
                fontSize: 40
            });
            renderInputs();
            draw();
        });

        function draw() {
            if (!img.complete) return;
            
            ctx.clearRect(0, 0, canvas.width, canvas.height);
            ctx.drawImage(img, 0, 0);

            ctx.textAlign = 'center';
            ctx.textBaseline = 'top';
            ctx.fillStyle = 'white';
            ctx.strokeStyle = 'black';
            ctx.miterLimit = 2; // Prevent sharp spikes in characters like 'A'
            
            const globalFontSize = 40;

            // Render each layer
            textLayers.forEach((layer) => {
                if (!layer.text) return;
                
                const fontSize = layer.fontSize || 40;
                ctx.font = `bold ${fontSize}px ${memeFont}`;
                ctx.letterSpacing = memeLetterSpacing;

                let displayStr = layer.text.toUpperCase();
                const lines = wrapText(ctx, displayStr, canvas.width * 0.9);
                const h = lines.length * fontSize * 1.25;
                const w = canvas.width * 0.9;

                // Box for the dragged/hovered layer (top-most layer gets preference)
                if (hoveredLayer === layer || draggingLayer === layer) {
                    ctx.save();
                    ctx.setLineDash([5, 5]);
                    ctx.strokeStyle = '#9f0';
                    ctx.lineWidth = 2;
                    ctx.strokeRect(layer.x - w / 2, layer.y - 10, w, h + 20);
                    ctx.restore();
                }

                lines.forEach((line, i) => {
                    const yOffset = i * (fontSize * 1.25);
                    const renderX = Math.round(layer.x);
                    const renderY = Math.round(layer.y + yOffset);
                    
                    ctx.save();
                    ctx.font = `bold ${fontSize}px ${memeFont}`; // Ensure correct font size per line
                    ctx.letterSpacing = memeLetterSpacing;
                    ctx.strokeStyle = 'black';
                    ctx.lineWidth = Math.max(2, fontSize / 8); // Slightly thicker stroke for better legibility
                    ctx.strokeText(line, renderX, renderY);
                    ctx.fillText(line, renderX, renderY);
                    ctx.restore();
                });
            });
        }

        // Options hooks removed as they are now hardcoded and the inputs are gone

        const getEventPos = (e) => {
            const rect = canvas.getBoundingClientRect();
            const clientX = e.clientX || (e.touches && e.touches[0].clientX);
            const clientY = e.clientY || (e.touches && e.touches[0].clientY);
            
            return {
                x: (clientX - rect.left) * (canvas.width / rect.width),
                y: (clientY - rect.top) * (canvas.height / rect.height)
            };
        };

        const isInsideText = (pt, layer) => {
            if (!layer.text) return false;
            const fontSize = layer.fontSize || 40;
            ctx.save();
            ctx.font = `bold ${fontSize}px ${memeFont}`;
            ctx.letterSpacing = memeLetterSpacing;
            const lines = wrapText(ctx, layer.text.toUpperCase(), canvas.width * 0.9);
            ctx.restore();
            const w = canvas.width * 0.95;
            const h = lines.length * fontSize * 1.25;
            
            return pt.x >= layer.x - w / 2 && pt.x <= layer.x + w / 2 && 
                   pt.y >= layer.y - 20 && pt.y <= layer.y + h + 20;
        };

        // POINTER EVENTS
        const onStart = (e) => {
            const pt = getEventPos(e);
            
            // Find layer (start from top-most, reverse of render order)
            draggingLayer = [...textLayers].reverse().find(layer => isInsideText(pt, layer)) || null;

            if (draggingLayer) {
                dragOffset = { x: pt.x - draggingLayer.x, y: pt.y - draggingLayer.y };
                if (e.pointerId) canvas.setPointerCapture(e.pointerId);
                canvas.style.cursor = 'grabbing';
                draw();
                e.preventDefault();
            }
        };

        const onMove = (e) => {
            const pt = getEventPos(e);
            
            if (draggingLayer) {
                draggingLayer.x = pt.x - dragOffset.x;
                draggingLayer.y = pt.y - dragOffset.y;
                draw();
                e.preventDefault();
            } else {
                // Hover logic
                const currentHover = [...textLayers].reverse().find(layer => isInsideText(pt, layer)) || null;
                if (currentHover !== hoveredLayer) {
                    hoveredLayer = currentHover;
                    canvas.style.cursor = hoveredLayer ? 'grab' : 'crosshair';
                    draw();
                }
            }
        };

        const onEnd = (e) => {
            if (draggingLayer) {
                if (e.pointerId) canvas.releasePointerCapture(e.pointerId);
                draggingLayer = null;
                canvas.style.cursor = 'grab';
                draw();
            }
        };

        canvas.addEventListener('pointerdown', onStart);
        window.addEventListener('pointermove', onMove);
        window.addEventListener('pointerup', onEnd);
        canvas.addEventListener('pointercancel', onEnd);
        canvas.addEventListener('mousedown', onStart);
        // Upload
        uploadBtn.addEventListener('click', async () => {
            if (!hasLoadedImage) {
                window.flashMessage((window.f0ckI18n?.meme?.choose_image_first) || 'Please select an image first!', 3000, 'error');
                return;
            }
            const category = (window.memeTemplate && window.memeTemplate.category) ? window.memeTemplate.category.toLowerCase() : '';
            const subCategory = (window.memeTemplate && window.memeTemplate.sub_category) ? window.memeTemplate.sub_category.toLowerCase() : '';
            const templateId = (window.memeTemplate && window.memeTemplate.id) ? window.memeTemplate.id.toLowerCase() : '';
            
            const isOrakelVon10 = subCategory === 'von10';
            const isOrakelUser = subCategory === 'user';
            const isOrakelBingoApu = subCategory === 'bingoapu' || templateId === 'bingoapu';
            const isOrakelNormal = category === 'orakel' && !isOrakelUser && !isOrakelVon10 && !isOrakelBingoApu;

            let uploadCanvas = canvas;

            if (isOrakelNormal || isOrakelUser || isOrakelVon10 || isOrakelBingoApu) {
                // Create an off-screen canvas to apply the orakel answer silently
                uploadCanvas = document.createElement('canvas');
                uploadCanvas.width = canvas.width;
                uploadCanvas.height = canvas.height;
                const uCtx = uploadCanvas.getContext('2d');
                
                // Copy current canvas state
                uCtx.drawImage(canvas, 0, 0);
                
                let result = '';
                let selectedCorner = null;
                if (isOrakelNormal) {
                    const outcomes = ['JA', 'NEIN', 'VIELLEICHT', 'AUF JEDEN FALL', 'NIEMALS', 'SOWAS VON JA', 'VERGISS ES', 'FRAG SPÄTER', 'KOMMT DRAUF AN'];
                    result = outcomes[Math.floor(Math.random() * outcomes.length)];
                } else if (isOrakelUser) {
                    try {
                        const res = await fetch('/api/v2/orakel/user');
                        const data = await res.json();
                        result = (data.success && data.username) ? `${data.display_name || data.username}|||ID: ${data.id}` : 'Anonymous';
                    } catch (e) {
                        result = 'Anonymous';
                    }
                } else if (isOrakelVon10) {
                    result = Math.floor(Math.random() * 11).toString();
                } else if (isOrakelBingoApu) {
                    const corners = [
                        { x: 60, y: 100, label: 'YES' },
                        { x: 573, y: 100, label: 'NO' },
                        { x: 60, y: 615, label: 'NO' },
                        { x: 573, y: 615, label: 'YES' }
                    ];
                    selectedCorner = corners[Math.floor(Math.random() * corners.length)];
                    result = selectedCorner.label;
                }
                
                // Draw Orakel result on the hidden canvas
                uCtx.save();
                if (isOrakelBingoApu) {
                    // Draw "KOPS" in normal meme text style on the selected corner
                    uCtx.font = `bold 28px ${memeFont}`;
                    uCtx.letterSpacing = memeLetterSpacing;
                    uCtx.textAlign = 'center';
                    uCtx.textBaseline = 'middle';
                    uCtx.fillStyle = '#fff';
                    uCtx.strokeStyle = '#000';
                    uCtx.lineWidth = 4;
                    uCtx.miterLimit = 2;
                    
                    uCtx.strokeText('KOPS', selectedCorner.x, selectedCorner.y);
                    uCtx.fillText('KOPS', selectedCorner.x, selectedCorner.y);
                } else {
                    uCtx.font = (isOrakelVon10) ? 'bold 150px Impact' : 'bold 80px Impact'; // Bigger font for the rating
                    uCtx.letterSpacing = memeLetterSpacing;
                    uCtx.textAlign = 'center';
                    uCtx.textBaseline = 'middle';
                    
                    if (isOrakelNormal) {
                        uCtx.shadowBlur = 20;
                        uCtx.shadowColor = 'rgba(101, 37, 212, 1)';
                    } else if (isOrakelVon10) {
                        uCtx.shadowBlur = 0; // No shadow as requested
                    } else {
                        // No shadow for the User Orakel
                        uCtx.shadowBlur = 0;
                    }
                    
                    uCtx.fillStyle = '#fff';
                    uCtx.strokeStyle = '#000';
                    uCtx.lineWidth = 10;
                    uCtx.miterLimit = 2;
                    
                    // Adjust position for user Orakel (reverting to +10 offset)
                    let yPos = Math.round(isOrakelUser ? (uploadCanvas.height / 2 + 10) : (uploadCanvas.height / 2 + 50));
                    
                    if (isOrakelVon10) {
                        yPos = Math.round(uploadCanvas.height / 2 ); // 1px lower
                    }

                    const xPos = Math.round(uploadCanvas.width / 2);
                    
                    // Auto-fit font size for user orakel — shrink until text fits within image width
                    let orakelFontSize = isOrakelVon10 ? 150 : 80;
                    const maxTextWidth = uploadCanvas.width - 80; // 40px padding each side

                    if (isOrakelUser) {
                        const parts = result.split('|||');
                        const namePart = parts[0];
                        const idPart = parts.length > 1 ? `(${parts[1]})` : '';
                        const combinedText = idPart ? `${namePart} ${idPart}` : namePart;

                        // Even tighter threshold for User Orakel (approx 25% total padding)
                        const userMaxWidth = Math.round(uploadCanvas.width * 0.75);

                        let currentFontSize = 74;
                        uCtx.font = `bold ${currentFontSize}px Impact`;
                        uCtx.letterSpacing = memeLetterSpacing;
                        
                        // First attempt: Shrink entire text on one line down to 58px if needed
                        while (uCtx.measureText(combinedText).width > userMaxWidth && currentFontSize > 58) {
                            currentFontSize -= 2;
                            uCtx.font = `bold ${currentFontSize}px Impact`;
                            uCtx.letterSpacing = memeLetterSpacing;
                        }

                        const combinedFits = uCtx.measureText(combinedText).width <= userMaxWidth;

                        if (combinedFits) {
                            // Single line — potentially shrunk for long names
                            uCtx.fillText(combinedText, xPos, yPos);
                        } else {
                            // Two lines — auto-fit just the name, ID below
                            let nameFontSize = 74;
                            uCtx.font = `bold ${nameFontSize}px Impact`;
                            uCtx.letterSpacing = memeLetterSpacing;
                            while (uCtx.measureText(namePart).width > userMaxWidth && nameFontSize > 16) {
                                nameFontSize -= 2;
                                uCtx.font = `bold ${nameFontSize}px Impact`;
                                uCtx.letterSpacing = memeLetterSpacing;
                            }

                            const idFontSize = Math.max(18, Math.round(nameFontSize * 0.45));
                            const lineGap = Math.round(nameFontSize * 1.25);
                            const nameY = Math.round(yPos - lineGap / 2);
                            const idY = Math.round(yPos + lineGap / 2) + 2;

                            uCtx.font = `bold ${nameFontSize}px Impact`;
                            uCtx.letterSpacing = memeLetterSpacing;
                            uCtx.fillText(namePart, xPos, nameY);

                            if (idPart) {
                                uCtx.font = `bold ${idFontSize}px Impact`;
                                uCtx.letterSpacing = memeLetterSpacing;
                                uCtx.fillText(idPart, xPos, idY);
                            }
                        }
                    } else {
                        // Normal / von10 — single line as before
                        uCtx.font = `bold ${orakelFontSize}px Impact`;
                        uCtx.letterSpacing = memeLetterSpacing;
                        uCtx.strokeText(result, xPos, yPos);
                        uCtx.fillText(result, xPos, yPos);
                    }
                }
                uCtx.restore();
            }

            // Captcha first when the site requires one for uploads (cap.uploads)
            if (window.f0ckCaptchaGate) {
                try { await window.f0ckCaptchaGate('upload'); } catch { return; }
            }

            uploadBtn.disabled = true;
            uploadBtn.innerHTML = '<i class="fa fa-spinner fa-spin"></i> ' + (window.f0ckI18n?.uploading || 'Uploading...');

            try {
                const blob = await new Promise(resolve => uploadCanvas.toBlob(resolve, 'image/jpeg', 0.95));
                const formData = new FormData();
                formData.append('file', blob, `meme-${Date.now()}.jpg`);
                const decodeHTMLEntities = (text) => {
                    const textArea = document.createElement('textarea');
                    textArea.innerHTML = text;
                    return textArea.value;
                };

                const defaultTags = decodeHTMLEntities(document.getElementById('tags').value || 'meme');
                const autoTag = window.memeTemplate ? decodeHTMLEntities(window.memeTemplate.name) : '';
                const tags = `${defaultTags}, ${autoTag}`;

                formData.append('rating', 'sfw');
                formData.append('tags', tags);
                formData.append('csrf_token', window.csrf_token);

                const res = await fetch('/api/v2/upload', {
                    method: 'POST',
                    body: formData,
                    headers: { 'X-CSRF-Token': window.csrf_token, 'X-Requested-With': 'XMLHttpRequest' }
                });

                const result = await res.json();
                if (result.success) {
                    if (result.manual_approval) {
                        window.flashMessage(window.f0ckI18n?.upload_pending_approval_patient || 'Upload awaits approval', 3000, 'warning');
                        if (window.loadPageAjax) {
                            window.loadPageAjax('/');
                        } else {
                            window.location.href = '/';
                        }
                    } else {
                        const dest = result.redirect || '/meme';
                        if (window.loadItemAjax) {
                            window.loadItemAjax(dest);
                        } else if (window.loadPageAjax) {
                            window.loadPageAjax(dest);
                        } else {
                            window.location.href = dest;
                        }
                    }
                }
                else {
                    window.flashMessage('Error: ' + result.msg, 3000, 'error');
                    uploadBtn.disabled = false;
                    uploadBtn.innerHTML = `<i class="fa fa-upload"></i> ${(window.f0ckI18n?.meme?.upload_btn) || 'Upload Meme'}`;
                }
            } catch (err) {
                window.flashMessage('Upload failed', 3000, 'error');
                uploadBtn.disabled = false;
            }
        });

        // Custom Local Image Selector logic
        const fileInput = document.getElementById('customTemplateFile');
        const selectFileBtn = document.getElementById('selectCustomFileBtn');
        const canvasWrapper = document.querySelector('.canvas-wrapper');

        const loadLocalImage = (file) => {
            if (file && file.type.startsWith('image/')) {
                const reader = new FileReader();
                reader.onload = (event) => {
                    hasLoadedImage = true;
                    img.src = event.target.result;
                    
                    // Update template metadata
                    window.memeTemplate.name = file.name.replace(/\.[^/.]+$/, "");
                    
                    // Update header title dynamically
                    const headerTitle = document.querySelector('.meme-title');
                    if (headerTitle) {
                        const baseTitle = window.f0ckI18n?.meme?.create_meme || 'Create Meme:';
                        headerTitle.innerHTML = `${baseTitle} ${window.memeTemplate.name}`;
                    }

                    // Update tags input value if tags are present
                    const tagsInput = document.getElementById('tags');
                    if (tagsInput) {
                        tagsInput.value = `meme, Custom, ${window.memeTemplate.name}`;
                    }
                };
                reader.readAsDataURL(file);
            }
        };

        if (selectFileBtn && fileInput) {
            selectFileBtn.addEventListener('click', () => {
                fileInput.click();
            });

            fileInput.addEventListener('change', (e) => {
                const file = e.target.files[0];
                loadLocalImage(file);
            });
        }

        // HTML5 Drag & Drop Support
        if (canvas && canvasWrapper) {
            const preventDefaults = (e) => {
                e.preventDefault();
                e.stopPropagation();
            };

            ['dragenter', 'dragover', 'dragleave', 'drop'].forEach(eventName => {
                canvasWrapper.addEventListener(eventName, preventDefaults, false);
            });

            ['dragenter', 'dragover'].forEach(eventName => {
                canvasWrapper.addEventListener(eventName, () => {
                    canvasWrapper.classList.add('drag-hover');
                }, false);
            });

            ['dragleave', 'drop'].forEach(eventName => {
                canvasWrapper.addEventListener(eventName, () => {
                    canvasWrapper.classList.remove('drag-hover');
                }, false);
            });

            canvasWrapper.addEventListener('drop', (e) => {
                const dt = e.dataTransfer;
                const file = dt.files[0];
                loadLocalImage(file);
            }, false);
        }

        // Initial draw
        setTimeout(draw, 300);
    }
})();

