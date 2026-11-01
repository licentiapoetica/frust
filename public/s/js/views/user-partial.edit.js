    F0ckActions.define({
        toggleBannerEditMenu: () => {
            const menu = document.getElementById('banner-edit-menu');
            menu.style.display = getComputedStyle(menu).display === 'none' ? 'block' : 'none';
        },
        closeBannerEditMenu: () => { document.getElementById('banner-edit-menu').style.display = 'none'; },
        chooseBannerFile: () => { document.getElementById('banner-choose-btn').click(); },
        toggleProfileEditMode: (el) => {
            const box = document.getElementById('live-profile-preview-box');
            const on = box.classList.toggle('edit-mode-active');
            el.classList.toggle('is-active', on);
            el.innerHTML = on ? '<i class="fa-solid fa-check"></i>' : '<i class="fa-solid fa-pen"></i>';
        },
    });

    (function() {
        const DATA = JSON.parse(document.getElementById('user-partial-edit-data').textContent);


        const avatarEditBtn = document.getElementById('inline-avatar-btn');
        const avatarFileInput = document.getElementById('inline-avatar-file');
        if (avatarEditBtn && avatarFileInput) {
            avatarEditBtn.addEventListener('click', () => {
                avatarFileInput.click();
            });
            avatarFileInput.addEventListener('change', async (e) => {
                const file = e.target.files[0];
                if (!file) return;
                
                avatarEditBtn.innerHTML = '<i class="fa-solid fa-spinner fa-spin user-profile-avatar-edit-icon-sm"></i>';
                avatarEditBtn.disabled = true;

                try {
                    const formData = new FormData();
                    formData.append('file', file);
                    const res = await fetch('/api/v2/settings/uploadAvatar', {
                        method: 'POST',
                        headers: { 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                        body: formData
                    });
                    const data = await res.json();
                    if (data.success) {
                        let objectUrl = '/a/' + (data.avatar_file || data.file) + '?t=' + Date.now();
                        let wrapper = document.querySelector('.avatar-image-wrapper');
                        if (wrapper) {
                            let placeholder = wrapper.querySelector('.avatar-placeholder');
                            if (placeholder) {
                                let img = document.createElement('img');
                                img.className = 'profile-avatar-img';
                                img.src = objectUrl;
                                wrapper.insertBefore(img, placeholder);
                                placeholder.remove();
                            } else {
                                let img = wrapper.querySelector('.profile-avatar-img');
                                if (img) img.src = objectUrl;
                            }
                        }
                        
                        document.querySelectorAll('.nav-avatar-img').forEach(img => img.src = objectUrl);
                        document.querySelectorAll('.comment-avatar img, .sidebar-avatar, .poll-voter-avatar, .rank-avatar').forEach(img => {
                            // Only replace if it matches the user's old avatar file, or if they didn't have one, just match if it links to their profile
                            const oldFile = DATA.avatar_file;
                            const oldId = DATA.avatar;
                            const isOldImage = (oldFile && img.src.includes('/a/' + oldFile)) || (oldId && img.src.includes('/t/' + oldId + '.webp'));
                            const link = img.closest('a');
                            const isUserLink = link && link.href.includes('/user/' + DATA.user);
                            
                            if (isOldImage || isUserLink) {
                                img.src = objectUrl;
                            }
                        });
                        avatarEditBtn.innerHTML = '<i class="fa-solid fa-pen user-profile-avatar-edit-icon-sm"></i>';
                        avatarEditBtn.disabled = false;
                    } else {
                        alert(data.msg || 'Failed to upload avatar');
                        avatarEditBtn.innerHTML = '<i class="fa-solid fa-pen user-profile-avatar-edit-icon-sm"></i>';
                        avatarEditBtn.disabled = false;
                    }
                } catch (err) {
                    alert('Network error');
                    avatarEditBtn.innerHTML = '<i class="fa-solid fa-pen user-profile-avatar-edit-icon-sm"></i>';
                    avatarEditBtn.disabled = false;
                }
            });
        }

        const nameEditBtn = document.getElementById('inline-name-edit-btn');
        const nameContainer = document.getElementById('profile-name-container');
        const nameEditContainer = document.getElementById('profile-name-edit-container');
        const nameCancelBtn = document.getElementById('inline-name-cancel-btn');
        const nameSaveBtn = document.getElementById('inline-name-save-btn');
        const inlineDisplayName = document.getElementById('inline-display-name');
        const inlineUsernameColor = document.getElementById('inline-username-color');
        
        if (nameEditBtn && nameEditContainer) {
            nameEditBtn.addEventListener('click', () => {
                nameContainer.style.display = 'none';
                nameEditContainer.style.display = 'flex';
            });
            nameCancelBtn.addEventListener('click', () => {
                nameContainer.style.display = 'flex';
                nameEditContainer.style.display = 'none';
            });
            nameSaveBtn.addEventListener('click', async () => {
                nameSaveBtn.disabled = true;
                const displayName = inlineDisplayName.value;
                const color = inlineUsernameColor.value;

                try {
                    let dPayload = {}; dPayload["display_name"] = displayName;
                    await fetch('/api/v2/settings/display_name', {
                        method: 'PUT',
                        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                        body: JSON.stringify(dPayload)
                    });
                    let cPayload = {}; cPayload["color"] = color;
                    await fetch('/api/v2/settings/username_color', {
                        method: 'PUT',
                        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                        body: JSON.stringify(cPayload)
                    });
                    nameSaveBtn.disabled = false;
                    nameContainer.style.display = 'flex';
                    nameEditContainer.style.display = 'none';
                    
                    const updateNameNode = (el, text) => {
                        if (!el) return;
                        for (let i = el.childNodes.length - 1; i >= 0; i--) {
                            let node = el.childNodes[i];
                            if (node.nodeType === Node.TEXT_NODE && node.nodeValue.trim().length > 0) {
                                const match = node.nodeValue.match(new RegExp('^(\\s*)(.*)'));
                                node.nodeValue = (match ? match[1] : '') + text;
                                break;
                            }
                        }
                    };
                    const targetName = displayName ? displayName : DATA.user;
                    const profileSpan = document.getElementById('profile-display-name');
                    if (profileSpan) {
                        profileSpan.style.color = color;
                        updateNameNode(profileSpan, targetName);
                    }
                    const navSpan = document.getElementById('nav-display-name');
                    if (navSpan) {
                        navSpan.style.color = color;
                        updateNameNode(navSpan, targetName);
                    }
                    const bracket = document.getElementById('username-bracket');
                    if (bracket) {
                        bracket.style.display = displayName ? 'inline' : 'none';
                    }
                } catch (err) {
                    alert('Error saving name/color');
                    nameSaveBtn.disabled = false;
                }
            });
        }

        const descEditBtn = document.getElementById('inline-desc-edit-btn');
        const descDisplay = document.getElementById('profile-desc-display');
        const descEditContainer = document.getElementById('profile-desc-edit-container');
        const descCancelBtn = document.getElementById('inline-desc-cancel-btn');
        const descSaveBtn = document.getElementById('inline-desc-save-btn');
        const inlineDescInput = document.getElementById('inline-desc-input');

        if (descEditBtn && descEditContainer) {
            descEditBtn.addEventListener('click', () => {
                descEditContainer.style.display = 'flex';
                document.body.classList.add('modal-open');
            });
            descCancelBtn.addEventListener('click', () => {
                descEditContainer.style.display = 'none';
                document.body.classList.remove('modal-open');
            });
            descSaveBtn.addEventListener('click', async () => {
                descSaveBtn.disabled = true;
                const description = inlineDescInput.value;
                try {
                    let p = {}; p["description"] = description;
                    const res = await fetch('/api/v2/settings/description', {
                        method: 'PUT',
                        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': window.f0ckSession?.csrf_token },
                        body: JSON.stringify(p)
                    });
                    const data = await res.json();
                    if (data.success) {
                        descSaveBtn.disabled = false;
                        descEditContainer.style.display = 'none';
                        document.body.classList.remove('modal-open');
                        const descTextDiv = descDisplay.querySelector('.desc-text');
                        if (description.trim() === '') {
                            descTextDiv.innerHTML = '<em class="user-profile-desc-empty">No description</em>';
                        } else {
                            const tempDiv = document.createElement('div');
                            tempDiv.textContent = description;
                            descTextDiv.innerHTML = tempDiv.innerHTML.replace(new RegExp('\\\\n', 'g'), '<br>');
                        }
                    } else {
                        alert(data.msg || 'Failed to update description');
                        descSaveBtn.disabled = false;
                    }
                } catch (err) {
                    alert('Error saving description');
                    descSaveBtn.disabled = false;
                }
            });
        }
    })();
