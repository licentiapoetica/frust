(async () => {
  // Helper to get dynamic context from the DOM
  const getContext = () => {
    const commentsEl = document.querySelector("#comments-container");
    const favoEl = document.querySelector("#a_favo");
    const infoEl = document.querySelector("#a_info");
    const idLinkEl = document.querySelector("a.id-link");

    const rawId = favoEl?.dataset?.localId || commentsEl?.dataset?.itemId || favoEl?.dataset?.itemId || infoEl?.dataset?.itemId || idLinkEl?.dataset?.itemId || idLinkEl?.innerText;
    if (!rawId) return null;

    const tagsContainer = document.querySelector("#tags");
    const inner = tagsContainer ? (tagsContainer.querySelector(".tags-inner") || tagsContainer) : null;
    const currentSub = (window.albumGallery && typeof window.albumGallery.getCurrentSubf0ck === 'function')
      ? window.albumGallery.getCurrentSubf0ck()
      : null;
    const subf0ck_id = currentSub ? (currentSub.slug || currentSub.id) : (tagsContainer?.dataset?.subf0ckSlug || tagsContainer?.dataset?.subf0ckId || null);

    return {
      postid: /^\d+$/.test(String(rawId).trim()) ? parseInt(rawId, 10) : rawId.trim(),
      subf0ck_id,
      poster: document.querySelector("a#a_username")?.innerText,
      tags: inner ? [...inner.querySelectorAll(".badge")].map(t => t.innerText.slice(0, -2)) : []
    };
  };

  const queryapi = async (url, data, method = 'GET') => {
    let req;
    if (method == 'POST') {
      req = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-CSRF-Token": window.f0ckSession?.csrf_token
        },
        body: JSON.stringify(data)
      });
    }
    else {
      let s = [];
      for (const [key, val] of Object.entries(data))
        s.push(encodeURIComponent(key) + "=" + encodeURIComponent(val));
      req = await fetch(url + '?' + s.join('&'));
    }
    return await req.json();
  };

  const get = async (url, data) => queryapi(url, data, 'GET');
  const post = async (url, data) => queryapi(url, data, 'POST');

  const renderTags = (_tags, highlightTag = null) => {
    const tagsContainer = document.querySelector("#tags");
    if (!tagsContainer) return;
    const inner = tagsContainer.querySelector(".tags-inner") || tagsContainer;
    const activePostId = tagsContainer.dataset.itemId || (typeof window.getCurrentItemId === 'function' ? window.getCurrentItemId() : null);

    const canManage = !!(
      document.querySelector('#tags[data-can-manage="true"]') ||
      (window.f0ckSession && (window.f0ckSession.is_admin || window.f0ckSession.is_moderator)) ||
      (window.f0ckSession && window.f0ckSession.user && document.querySelector('#a_username[data-username]')?.dataset?.username?.toLowerCase() === window.f0ckSession.user.toLowerCase())
    );

    // Anonymous uploaders may change the rating of their own item without full manage rights
    const canRate = canManage || !!document.querySelector('#tags[data-can-rate="true"]');

    // Only remove existing dynamically generated tags
    [...inner.querySelectorAll(".badge")].forEach(tag => {
      // Don't remove the one containing the add/toggle buttons, and don't remove the autocomplete input itself
      if (!tag.querySelector('#a_addtag') && !tag.querySelector('#a_toggle') && !tag.classList.contains('tag-ac-wrapper')) {
        tag.parentElement.removeChild(tag);
      }
    });

    // Deduplicate: ensure only at most ONE rating tag exists in the tags list
    const ratingTags = _tags.filter(t => ['sfw', 'nsfw', 'nsfl'].includes(t.normalized));
    const lastRatingTag = ratingTags.length ? ratingTags[ratingTags.length - 1] : null;
    const cleanTags = _tags.filter(t => !['sfw', 'nsfw', 'nsfl'].includes(t.normalized) || t === lastRatingTag);

    const tagsCopy = [...cleanTags];
    tagsCopy.reverse().forEach(tag => {
      const isRating = ['sfw', 'nsfw', 'nsfl'].includes(tag.normalized);
      let contentEl;
      if (isRating) {
        contentEl = document.createElement("span");
        contentEl.className = "rating-label";
        contentEl.textContent = tag.tag;
      } else {
        contentEl = document.createElement("a");
        contentEl.href = "/tag/" + tag.normalized;
        contentEl.className = "tag-name";
        contentEl.textContent = tag.tag;
      }

      const span = document.createElement("span");
      span.classList.add("badge");
      if (highlightTag && (tag.tag === highlightTag || tag.normalized === highlightTag)) {
        span.classList.add('new-tag-glow');
      }

      tag.badge.split(" ").forEach(b => span.classList.add(b));

      if (isRating) {
        span.classList.add('rating-tag', `is-${tag.normalized}`);
        span.dataset.rating = tag.normalized;
        if (activePostId) span.dataset.itemId = activePostId;
        if (canRate) {
          span.classList.add('can-cycle');
        }
      } else {
        span.classList.add('tag-badge');
        span.setAttribute('data-tag-id', tag.id || '');
        span.setAttribute('data-tag', tag.tag);
        span.setAttribute('data-tag-normalized', tag.normalized);
        if (!window.f0ckSession?.is_anonymized && window.f0ckSession?.logged_in && !window.f0ckSession?.is_anon && (tag.display_name || tag.user)) {
          span.setAttribute('tooltip', tag.display_name || tag.user);
        }
        const isExcl = !!tag.is_excluded;
        if (isExcl) span.classList.add('tag-is-excluded');
      }

      span.insertAdjacentElement("beforeend", contentEl);

      if (!isRating && tag.can_exclude) {
        const excludeBtn = document.createElement('button');
        excludeBtn.type = 'button';
        excludeBtn.className = 'tag-exclude-btn';
        excludeBtn.setAttribute('title', tag.is_excluded ? 'Excluded (click to unexclude)' : 'Exclude tag');
        excludeBtn.setAttribute('aria-label', 'Exclude tag');
        excludeBtn.innerHTML = `<i class="fa-solid ${tag.is_excluded ? 'fa-circle-check' : 'fa-ban'}"></i>`;
        span.insertAdjacentElement('beforeend', excludeBtn);
      }

      if (window.f0ckSession && (window.f0ckSession.is_admin || window.f0ckSession.is_moderator) && !isRating) {
        const space = document.createTextNode('\u00A0'); // &nbsp;
        span.appendChild(space);

        const del = document.createElement("a");
        del.className = "removetag admin-deltag";
        del.href = "#"; // same as the server-rendered removetag link; the delegated click handler prevents navigation
        del.innerHTML = '<i class="fa-solid fa-xmark"></i>';
        span.insertAdjacentElement("beforeend", del);
      }

      inner.insertAdjacentElement("afterbegin", span);
    });

    const hasRating = !!lastRatingTag;
    if (!hasRating) {
      const untaggedSpan = document.createElement("span");
      untaggedSpan.className = `badge badge-untagged rating-tag is-untagged${canRate ? ' can-cycle' : ''}`;
      untaggedSpan.dataset.rating = 'untagged';
      if (activePostId) untaggedSpan.dataset.itemId = activePostId;
      const lbl = document.createElement("span");
      lbl.className = "rating-label";
      lbl.textContent = "unrated";
      untaggedSpan.appendChild(lbl);
      inner.insertAdjacentElement("afterbegin", untaggedSpan);
    }

    // Safeguard: remove any extra rating tags in DOM
    const allRatingEls = inner.querySelectorAll('.rating-tag, .badge-success, .badge-danger, .badge-nsfl, .badge-untagged, [data-rating]');
    if (allRatingEls.length > 1) {
      for (let i = 1; i < allRatingEls.length; i++) {
        allRatingEls[i].remove();
      }
    }

    // Update thumbnail data-mode in background grid (ensures div.posts > a > p::before updates in Onara)
    if (activePostId) {
      const modeAttr = lastRatingTag ? lastRatingTag.normalized : 'null';
      document.querySelectorAll(`.posts > a.thumb[data-item-id="${activePostId}"], .posts a[data-item-id="${activePostId}"], .posts a[href$="/${activePostId}"]`).forEach(th => {
        th.setAttribute('data-mode', modeAttr);
      });
      if (document.body.classList.contains('onara-modal-open')) {
        const onaraThumb = document.querySelector('.posts > a.thumb.onara-active');
        if (onaraThumb) onaraThumb.setAttribute('data-mode', modeAttr);
      }
    }

    // Handle show more/less toggle visibility and count
    const allBadges = [...inner.querySelectorAll(".badge")];
    const realTags = allBadges.filter(b => !b.querySelector('#a_addtag') && !b.querySelector('#a_toggle') && !b.classList.contains('tag-ac-wrapper'));
    
    let toggle = tagsContainer.querySelector(".show-tags-toggle");
    
    if (realTags.length > 10) {
      if (!toggle) {
        toggle = document.createElement("a");
        toggle.href = "#";
        toggle.className = "show-tags-toggle";
        tagsContainer.appendChild(toggle);
      }
      const hiddenCount = realTags.length - 10;
      toggle.dataset.count = hiddenCount;
      
      // Auto-expand when rendering new tags (e.g. after adding one) as requested
      tagsContainer.classList.add('tags-expanded');
      toggle.textContent = "show less";

    } else if (toggle) {
      toggle.remove();
      tagsContainer.classList.remove('tags-expanded');
    }
  };

  window.renderTags = renderTags;

  const addtagClick = (e) => {
    if (e) e.preventDefault();
    const ctx = getContext();
    if (!ctx) return;
    const { postid, subf0ck_id, tags } = ctx;
    const anchor = document.querySelector("a#a_addtag");
    if (!anchor) return;

    TagAutocomplete.open({
      postid,
      existingTags: tags,
      anchorEl: anchor,
      onSubmit: async (tag) => {
        const payload = { tagname: tag };
        if (subf0ck_id) payload.subf0ck_id = subf0ck_id;
        const res = await post("/api/v2/tags/" + postid, payload);
        if (res.success) {
          if (window.albumGallery && typeof window.albumGallery.getCurrentSubf0ck === 'function') {
            const cur = window.albumGallery.getCurrentSubf0ck();
            if (cur && res.tags) cur.tags = res.tags;
          }
          if (window.invalidateItemCache) {
            window.invalidateItemCache(postid);
          }
        }
        return res;
      },
      renderTags: (newTags, hl) => {
        if (window.albumGallery && typeof window.albumGallery.getCurrentSubf0ck === 'function') {
          const cur = window.albumGallery.getCurrentSubf0ck();
          if (cur) cur.tags = newTags;
        }
        renderTags(newTags, hl);
      }
    });
  };

  const toggleEvent = async (e) => {
    if (e) e.preventDefault();
    const ratingEl = document.querySelector('.rating-tag.can-cycle, button#a_toggle');
    if (window.cycleRating) {
      window.cycleRating(ratingEl);
    }
  };

  let favSeq = 0;

  const toggleFavEvent = async (e) => {
    // e is the click event or undefined
    if (e && typeof e.preventDefault === 'function') e.preventDefault();
    if (window.f0ckSession?.is_anon && window.f0ckSession?.anon_permissions && window.f0ckSession.anon_permissions.favorite === false) {
      if (typeof window.flashMessage === 'function') {
        window.flashMessage('Anonymous favoriting is disabled.', 3000, 'error');
      }
      return;
    }
    const favoBtns = document.querySelectorAll("#a_favo");
    if (!favoBtns.length) return;
    const firstFavoBtn = favoBtns[0];
    const chanRehostBtn = document.querySelector('#chan-item-rehost-btn');
    const isChan = firstFavoBtn?.dataset?.isChan === 'true' || !!chanRehostBtn;
    const chanUrl = firstFavoBtn?.dataset?.chanUrl || chanRehostBtn?.dataset?.url;
    let localId = firstFavoBtn?.dataset?.localId;

    const ctx = getContext();
    const postid = localId ? Number(localId) : ctx?.postid;
    if (!postid && !chanUrl) return;

    const wasAlreadyFav = favoBtns[0].classList.contains('fa-solid') && !favoBtns[0].classList.contains('fa-spinner');
    const isNowFav = !wasAlreadyFav;

    const setFavoUi = (isFav) => {
      favoBtns.forEach(btn => {
        btn.classList.remove('fa-spinner', 'fa-spin');
        btn.classList.add('iconset', 'fa-heart');
        btn.classList.toggle('fa-solid', isFav);
        btn.classList.toggle('fa-regular', !isFav);
        btn.title = isFav ? (window.f0ckI18n?.fav_remove || 'Remove from favorites') : (window.f0ckI18n?.fav_add || 'Favorite');
      });
    };

    // 1. Instantly apply client UI
    setFavoUi(isNowFav);

    // Micro-animation for tactile pop
    favoBtns.forEach(btn => {
      btn.classList.remove('fav-pop');
      void btn.offsetWidth;
      btn.classList.add('fav-pop');
    });

    // Instant flash message & vibration feedback
    window.flashMessage((window.f0ckI18n && (isNowFav ? window.f0ckI18n.fav_added : window.f0ckI18n.fav_removed)) || (isNowFav ? 'ADDED TO FAVORITES' : 'REMOVED FROM FAVORITES'));
    if (navigator.vibrate) navigator.vibrate(50);

    if (postid && window.invalidateItemCache) {
      window.invalidateItemCache(postid);
    }

    // Optimistically update #favs badge list
    const favcontainer = document.querySelector('#favs');
    const prevFavsHtml = favcontainer ? favcontainer.innerHTML : '';
    const prevFavsHidden = favcontainer ? favcontainer.hidden : true;

    if (favcontainer) {
      const currentUser = (window.f0ckSession?.user || '').toLowerCase();
      const isAnon = !!(window.f0ckSession?.is_anon || window.f0ckSession?.user === 'anonymous');
      const isAnonymized = !!(window.f0ckSession?.is_anonymized ?? (window.f0ckSession?.guest_anonymize && !window.f0ckSession?.logged_in) ?? (window.f0ckSession?.is_anon && window.f0ckSession?.anon_anonymize));

      const findSelfBadge = () => {
        const selfBadges = favcontainer.querySelectorAll('[data-self="true"]');
        if (selfBadges.length) return selfBadges[0];
        if (currentUser && currentUser !== 'anonymous') {
          const links = favcontainer.querySelectorAll('a[href]');
          for (const a of links) {
            const path = a.getAttribute('href') || '';
            if (path.toLowerCase().endsWith('/user/' + currentUser)) return a;
          }
        }
        return null;
      };

      if (!isNowFav) {
        const selfBadge = findSelfBadge();
        if (selfBadge) selfBadge.remove();
        if (favcontainer.children.length === 0) {
          favcontainer.hidden = true;
        }
      } else {
        let selfBadge = findSelfBadge();
        if (!selfBadge) {
          selfBadge = document.createElement('a');
          selfBadge.dataset.self = 'true';
          selfBadge.setAttribute('flow', 'up');
          if (isAnonymized || isAnon) {
            selfBadge.className = 'ghost-fav';
            selfBadge.setAttribute('tooltip', 'anonymous');
            selfBadge.style.cursor = 'default';
            const img = document.createElement('img');
            img.src = '/a/default.png';
            img.style.height = "32px";
            img.style.width = "32px";
            selfBadge.appendChild(img);
          } else {
            selfBadge.href = `/user/${currentUser}`;
            selfBadge.setAttribute('tooltip', window.f0ckSession?.display_name || window.f0ckSession?.user || 'anonymous');
            const img = document.createElement('img');
            const avatarFile = window.f0ckSession?.avatar_file;
            const avatar = window.f0ckSession?.avatar;
            img.src = avatarFile ? `/a/${avatarFile}` : (avatar ? `/t/${avatar}.webp` : '/a/default.png');
            img.style.height = "32px";
            img.style.width = "32px";
            if (window.f0ckSession?.username_color) img.style.borderColor = window.f0ckSession.username_color;
            selfBadge.appendChild(img);
          }
          favcontainer.appendChild(selfBadge);
        }
        favcontainer.hidden = false;
      }
    }

    const mySeq = ++favSeq;

    // 2. Run server operation in background — can take as long as it needs
    (async () => {
      try {
        // If viewing un-rehosted 4chan post, auto-rehost first
        if (isChan && !localId && chanUrl) {
          if (mySeq !== favSeq) return;
          const csrfToken = window.f0ckSession?.csrf_token || '';
          const rehostResp = await fetch('/api/v2/scroller/rehost', {
            method: 'POST',
            headers: {
              'Content-Type': 'application/x-www-form-urlencoded',
              ...(csrfToken ? { 'X-CSRF-Token': csrfToken } : {})
            },
            body: new URLSearchParams({
              url: chanUrl,
              original_filename: firstFavoBtn?.dataset?.filename || chanRehostBtn?.dataset?.filename || '',
              width: firstFavoBtn?.dataset?.width || chanRehostBtn?.dataset?.width || '',
              height: firstFavoBtn?.dataset?.height || chanRehostBtn?.dataset?.height || ''
            })
          });
          const rehostData = await rehostResp.json();
          if (rehostData.success && rehostData.item_id) {
            localId = rehostData.item_id;
            favoBtns.forEach(btn => {
              btn.dataset.itemId = localId;
              btn.dataset.localId = localId;
            });
            const commentsEl = document.querySelector("#comments-container");
            if (commentsEl) commentsEl.dataset.itemId = localId;
            const infoEl = document.querySelector("#a_info");
            if (infoEl) infoEl.dataset.itemId = localId;

            const timeEl = document.querySelector('time.timeago');
            if (timeEl) {
              const nowIso = new Date().toISOString();
              timeEl.dataset.iso = nowIso;
              const fullDate = typeof window.f0ckFormatDateFull === 'function' ? window.f0ckFormatDateFull(nowIso) : new Date().toLocaleString();
              timeEl.setAttribute('tooltip', fullDate);
              timeEl.textContent = (window.f0ckTimeAgo ? window.f0ckTimeAgo(nowIso) : (window.f0ckI18n?.timeago_just_now || 'just now'));
            }

            if (chanRehostBtn) {
              chanRehostBtn.classList.add('rehosted');
              chanRehostBtn.outerHTML = `
                <span class="chan-rehosted-wrap" style="display:inline-flex;gap:4px;align-items:center;">
                  <a href="/${localId}" class="chan-rehost-action-btn rehosted" title="Already rehosted on f0ckm (Post #${localId})" style="display:inline-flex;align-items:center;gap:5px;padding:3px 9px;border-radius:6px;font-size:0.8rem;background:rgba(74,222,128,0.2);color:#4ade80;border:1px solid rgba(74,222,128,0.4);text-decoration:none;font-weight:600;"><i class="fa-solid fa-check"></i> #${localId}</a>
                  <button type="button" class="chan-rehost-edit-btn" data-item-id="${localId}" title="Edit rating" style="display:inline-flex;align-items:center;padding:3px 7px;border-radius:6px;font-size:0.8rem;background:rgba(255,255,255,0.1);color:#fff;border:1px solid rgba(255,255,255,0.2);cursor:pointer;"><i class="fa-solid fa-pen"></i></button>
                </span>
              `;
            }
          } else {
            throw new Error(rehostData.msg || 'Rehost failed');
          }
        }

        const effectivePostId = localId ? Number(localId) : (postid || getContext()?.postid);
        if (!effectivePostId) {
          throw new Error('Missing post ID');
        }

        if (mySeq !== favSeq) return;

        const res = await post('/api/v2/togglefav', {
          postid: effectivePostId,
          chan_url: chanUrl,
          action: isNowFav ? 'add' : 'delete',
          favorited: isNowFav
        });

        if (mySeq !== favSeq) return;

        if (res && res.success) {
          if (window.invalidateItemCache) {
            window.invalidateItemCache(effectivePostId);
          }
          const finalIsFav = (res.favorited !== undefined) ? !!res.favorited : isNowFav;
          setFavoUi(finalIsFav);

          // Render authoritative favs list from server
          const curFavContainer = document.querySelector('#favs');
          if (curFavContainer && Array.isArray(res.favs)) {
            curFavContainer.innerHTML = "";
            if (res.favs.length > 0) {
              const isAnonymized = !!(window.f0ckSession?.is_anonymized ?? (window.f0ckSession?.guest_anonymize && !window.f0ckSession?.logged_in) ?? (window.f0ckSession?.is_anon && window.f0ckSession?.anon_anonymize));
              const fragment = document.createDocumentFragment();
              res.favs.forEach(f => {
                const isSelf = window.f0ckSession && (
                  (window.f0ckSession.id && f.user_id && Number(window.f0ckSession.id) === Number(f.user_id)) ||
                  (window.f0ckSession.user && f.user && window.f0ckSession.user.toLowerCase() === f.user.toLowerCase()) ||
                  (window.f0ckSession.login && f.login && window.f0ckSession.login.toLowerCase() === f.login.toLowerCase())
                );
                const isFavAnon = f.is_anon || f.user === 'anonymous' || (typeof f.user === 'string' && f.user.startsWith('anon_'));

                if (f.hide_fav_badge && !isSelf) {
                  const a = document.createElement('a');
                  a.className = 'ghost-fav';
                  a.setAttribute('tooltip', '?');
                  a.setAttribute('flow', 'up');
                  a.style.cursor = 'default';

                  const img = document.createElement('img');
                  img.src = '/s/img/ghost_fav.svg';
                  img.style.height = "32px";
                  img.style.width = "32px";

                  a.appendChild(img);
                  fragment.appendChild(a);
                } else if (isAnonymized || isFavAnon) {
                  const a = document.createElement('a');
                  a.className = 'ghost-fav';
                  if (isSelf) a.dataset.self = 'true';
                  a.setAttribute('tooltip', 'anonymous');
                  a.setAttribute('flow', 'up');
                  a.style.cursor = 'default';

                  const img = document.createElement('img');
                  img.src = '/a/default.png';
                  img.style.height = "32px";
                  img.style.width = "32px";

                  a.appendChild(img);
                  fragment.appendChild(a);
                } else {
                  const a = document.createElement('a');
                  if (isSelf) a.dataset.self = 'true';
                  a.href = `/user/${(f.user || '').toLowerCase()}`;
                  a.setAttribute('tooltip', f.display_name || f.user || 'anonymous');
                  a.setAttribute('flow', 'up');

                  const img = document.createElement('img');
                  img.src = f.avatar_file ? `/a/${f.avatar_file}` : (f.avatar ? `/t/${f.avatar}.webp` : '/a/default.png');
                  img.style.height = "32px";
                  img.style.width = "32px";
                  if (f.username_color) img.style.borderColor = f.username_color;

                  a.appendChild(img);
                  fragment.appendChild(a);
                }
              });
              curFavContainer.appendChild(fragment);
              curFavContainer.hidden = false;
            } else {
              curFavContainer.hidden = true;
            }
          }
        } else {
          if (mySeq !== favSeq) return;
          setFavoUi(wasAlreadyFav);
          const curFavContainer = document.querySelector('#favs');
          if (curFavContainer) {
            curFavContainer.innerHTML = prevFavsHtml;
            curFavContainer.hidden = prevFavsHidden;
          }
          const errMsg = (res && (res.msg || res.error)) || 'Anonymous favoriting is disabled.';
          if (typeof window.flashMessage === 'function') {
            window.flashMessage(errMsg, 3000, 'error');
          }
        }
      } catch (err) {
        console.error('[CHAN-FAV] Error during background favorite sync:', err);
        if (mySeq !== favSeq) return;
        setFavoUi(wasAlreadyFav);
        const curFavContainer = document.querySelector('#favs');
        if (curFavContainer) {
          curFavContainer.innerHTML = prevFavsHtml;
          curFavContainer.hidden = prevFavsHidden;
        }
        if (typeof window.flashMessage === 'function') {
          window.flashMessage('Failed to update favorite.', 3000, 'error');
        }
      }
    })();
  };

  // Event Delegation
  document.addEventListener("click", e => {
    if (document.querySelector('script[src*="admin.js"]')) return;
    const target = e.target.nodeType === 3 ? e.target.parentElement : e.target;
    if (target.closest("a#a_addtag")) {
      addtagClick(e);
    } else if (target.closest("#a_favo")) {
      toggleFavEvent(e);
    }
  });


})();
