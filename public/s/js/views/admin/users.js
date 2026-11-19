// Admin user manager (views/admin/users.html, rows: admin/users_list.html).
// Without JavaScript the filters are a GET form, paging uses links and every action is a plain form
// (nojs.rs redirects back). Here the filters apply as you type, the next page loads at the end of the
// list, and the action forms post with fetch; the affected row is then fetched again from the server
// (the same template), so the page never reloads.
(function () {
    'use strict';

    var root = document.getElementById('admin-users');
    var dataEl = document.getElementById('admin-users-data');
    if (!root || !dataEl) return;

    var initial = JSON.parse(dataEl.textContent);
    var state = {
        page: initial.page || 1,
        hasMore: !!initial.hasMore,
        q: initial.q || '',
        legacy: !!initial.onlyLegacy,
        status: initial.status || '',
        role: initial.role || ''
    };

    var filters = document.getElementById('user-filters');
    var searchInput = document.getElementById('user-search');
    var statusFilter = document.getElementById('status-filter');
    var roleFilter = document.getElementById('role-filter');
    var legacyFilter = document.getElementById('legacy-filter');
    var legacyLabel = legacyFilter.closest('label');
    var resetLink = document.getElementById('reset-filters-btn');
    var list = document.getElementById('user-list');
    var emptyMsg = document.getElementById('no-users-msg');
    var countEl = document.getElementById('total-count');
    var labelEl = document.getElementById('total-label');
    var trigger = document.getElementById('loading-trigger');
    var modal = document.getElementById('create-user-modal');

    function flash(msg, type) {
        if (typeof window.showFlash === 'function') window.showFlash(msg, type);
    }

    function csrf() {
        if (typeof window.getCsrfToken === 'function') return window.getCsrfToken();
        return (window.f0ckSession && window.f0ckSession.csrf_token) || '';
    }

    // ── Filters, search, paging ──────────────────────────────────────────

    function filterParams(page) {
        var p = new URLSearchParams();
        if (state.q) p.set('q', state.q);
        if (state.legacy) {
            p.set('legacy', '1');
        } else {
            if (state.status) p.set('status', state.status);
            if (state.role) p.set('role', state.role);
        }
        if (page && page > 1) p.set('page', page);
        return p;
    }

    function syncControls() {
        var qs = filterParams(0).toString();
        history.replaceState(history.state, '', location.pathname + (qs ? '?' + qs : ''));
        resetLink.classList.toggle('adm-users-hidden', !qs);
        statusFilter.disabled = roleFilter.disabled = state.legacy;
        statusFilter.classList.toggle('is-set', !!state.status && !state.legacy);
        roleFilter.classList.toggle('is-set', !!state.role && !state.legacy);
        legacyLabel.classList.toggle('is-set', state.legacy);
    }

    // Each load gets a number; a reply that a newer load overtook is dropped
    var loadSeq = 0;
    var loading = false;

    async function load(page, append) {
        var seq = ++loadSeq;
        loading = true;
        if (!append) filters.classList.add('is-busy');
        try {
            var res = await fetch('/admin/users?' + filterParams(page).toString(), { headers: { 'X-Requested-With': 'XMLHttpRequest' } });
            if (!res.ok) throw new Error('HTTP ' + res.status);
            var html = await res.text();
            if (seq !== loadSeq) return;
            var total = res.headers.get('X-Total-Count');
            if (total !== null) countEl.textContent = total;
            var label = res.headers.get('X-Total-Label');
            if (label) labelEl.textContent = label;
            var empty = res.headers.get('X-Empty-Msg');
            if (empty) emptyMsg.textContent = empty;
            state.hasMore = res.headers.get('X-Has-More') === 'true';
            state.page = page;
            if (append) list.insertAdjacentHTML('beforeend', html);
            else list.innerHTML = html;
            emptyMsg.classList.toggle('adm-users-hidden', list.children.length > 0);
        } catch (e) {
            if (seq === loadSeq) flash('Could not load users: ' + e.message, 'error');
        } finally {
            if (seq === loadSeq) {
                loading = false;
                filters.classList.remove('is-busy');
                if (!append) setTimeout(loadMoreIfNear, 50);
            }
        }
    }

    function refresh() {
        syncControls();
        load(1, false);
    }

    var searchTimer;
    searchInput.addEventListener('input', function () {
        clearTimeout(searchTimer);
        searchTimer = setTimeout(function () {
            var q = searchInput.value.trim();
            if (q === state.q) return;
            state.q = q;
            refresh();
        }, 300);
    });

    statusFilter.addEventListener('change', function () {
        state.status = statusFilter.value;
        refresh();
    });

    roleFilter.addEventListener('change', function () {
        state.role = roleFilter.value;
        refresh();
    });

    legacyFilter.addEventListener('change', function () {
        state.legacy = legacyFilter.checked;
        if (state.legacy) {
            state.status = state.role = '';
            statusFilter.value = roleFilter.value = '';
        }
        refresh();
    });

    // Enter in the search field: apply at once instead of the GET form's page load
    filters.addEventListener('submit', function (e) {
        e.preventDefault();
        clearTimeout(searchTimer);
        state.q = searchInput.value.trim();
        refresh();
    });

    resetLink.addEventListener('click', function (e) {
        e.preventDefault();
        state.q = state.status = state.role = '';
        state.legacy = false;
        searchInput.value = statusFilter.value = roleFilter.value = '';
        legacyFilter.checked = false;
        refresh();
    });

    // Infinite scroll: the next page loads once the end of the list is near (400px ahead), and again
    // right away while it still is (a short page, a tall window), so every user is reachable.
    var triggerNear = false;
    function loadMoreIfNear() {
        if (!triggerNear || !state.hasMore || loading) return;
        load(state.page + 1, true).then(function () {
            setTimeout(loadMoreIfNear, 50);
        });
    }
    new IntersectionObserver(function (entries) {
        triggerNear = entries[entries.length - 1].isIntersecting;
        loadMoreIfNear();
    }, { rootMargin: '0px 0px 400px 0px' }).observe(trigger);

    syncControls();

    // ── Rows ─────────────────────────────────────────────────────────────

    // Fetch one row again (exact-name search) and swap it in, keeping it open
    async function reloadRow(row, login) {
        var p = new URLSearchParams();
        p.set('q', '"' + login + '"');
        if (row.dataset.legacy) p.set('legacy', '1');
        p.set('open', row.dataset.row);
        try {
            var res = await fetch('/admin/users?' + p.toString(), { headers: { 'X-Requested-With': 'XMLHttpRequest' } });
            if (!res.ok) return;
            var tpl = document.createElement('template');
            tpl.innerHTML = await res.text();
            var fresh = tpl.content.getElementById(row.id);
            if (!fresh) {
                // Gone from this listing (a legacy name whose uploads all moved)
                row.remove();
                return;
            }
            row.replaceWith(fresh);
        } catch (e) {
            // the action itself went through; the row just keeps its old look
        }
    }

    list.addEventListener('submit', async function (e) {
        var form = e.target.closest('form.adm-user-form');
        if (!form) return;
        e.preventDefault();
        if (form.classList.contains('is-busy')) return;
        var row = form.closest('.adm-user');
        var body = new URLSearchParams(new FormData(form, e.submitter));
        body.delete('back');
        body.delete('csrf_token');
        form.classList.add('is-busy');
        try {
            var res = await fetch(form.getAttribute('action'), {
                method: 'POST',
                headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'Accept': 'application/json', 'X-CSRF-Token': csrf() },
                body: body
            });
            var data = await res.json().catch(function () { return {}; });
            if (!res.ok || data.success === false) throw new Error(data.msg || data.message || ('HTTP ' + res.status));
            flash(data.msg || data.message || 'Done.', 'success');
            if (form.dataset.kind === 'delete') {
                row.remove();
                countEl.textContent = Math.max(0, (parseInt(countEl.textContent, 10) || 1) - 1);
                emptyMsg.classList.toggle('adm-users-hidden', list.children.length > 0);
                return;
            }
            await reloadRow(row, data.new_login || row.dataset.login);
        } catch (err) {
            flash(err.message || 'Something went wrong.', 'error');
        } finally {
            form.classList.remove('is-busy');
        }
    });

    // ── Move uploads: username suggestions ───────────────────────────────

    var suggestTimer;
    var suggestSeq = 0;

    function suggestBox(input) {
        return input.parentNode.querySelector('.adm-user-suggest-list');
    }

    function closeSuggest(input) {
        var box = suggestBox(input);
        box.classList.remove('is-open');
        box.textContent = '';
    }

    function setActive(box, idx) {
        var items = box.querySelectorAll('.adm-user-suggest-item');
        items.forEach(function (el, i) { el.classList.toggle('is-active', i === idx); });
        if (items[idx]) items[idx].scrollIntoView({ block: 'nearest' });
    }

    function activeIndex(box) {
        var items = Array.prototype.slice.call(box.querySelectorAll('.adm-user-suggest-item'));
        return items.findIndex(function (el) { return el.classList.contains('is-active'); });
    }

    function pick(input, name) {
        input.value = name;
        closeSuggest(input);
        input.focus();
    }

    async function fetchSuggest(input, q) {
        var seq = ++suggestSeq;
        try {
            var res = await fetch('/api/v2/admin/users/search?q=' + encodeURIComponent(q), { headers: { 'X-Requested-With': 'XMLHttpRequest' } });
            var data = await res.json();
            if (seq !== suggestSeq || document.activeElement !== input) return;
            var skip = (input.dataset.suggestSkip || '').toLowerCase();
            var names = (Array.isArray(data) ? data : []).map(function (u) { return String(u.user || ''); })
                .filter(function (n) { return n && n.toLowerCase() !== skip; });
            var box = suggestBox(input);
            box.textContent = '';
            names.forEach(function (n) {
                var el = document.createElement('span');
                el.className = 'adm-user-suggest-item';
                el.setAttribute('role', 'option');
                el.dataset.name = n;
                var icon = document.createElement('i');
                icon.className = 'fa-solid fa-user';
                el.append(icon, document.createTextNode(n));
                box.appendChild(el);
            });
            box.classList.toggle('is-open', names.length > 0);
        } catch (e) {
            // no suggestions; typing the name still works
        }
    }

    list.addEventListener('input', function (e) {
        var input = e.target;
        if (!input.matches('[data-suggest-users]')) return;
        clearTimeout(suggestTimer);
        var q = input.value.trim();
        if (!q) return closeSuggest(input);
        suggestTimer = setTimeout(function () { fetchSuggest(input, q); }, 200);
    });

    list.addEventListener('keydown', function (e) {
        var input = e.target;
        if (!input.matches('[data-suggest-users]')) return;
        var box = suggestBox(input);
        if (!box.classList.contains('is-open')) return;
        var count = box.querySelectorAll('.adm-user-suggest-item').length;
        var idx = activeIndex(box);
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            setActive(box, Math.min(idx + 1, count - 1));
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setActive(box, Math.max(idx - 1, 0));
        } else if (e.key === 'Enter' && idx >= 0) {
            // take the highlighted name instead of submitting
            e.preventDefault();
            pick(input, box.querySelectorAll('.adm-user-suggest-item')[idx].dataset.name);
        } else if (e.key === 'Escape') {
            e.preventDefault();
            closeSuggest(input);
        }
    });

    // mousedown, not click: the field keeps its focus, so the list doesn't close first
    list.addEventListener('mousedown', function (e) {
        var item = e.target.closest('.adm-user-suggest-item');
        if (!item) return;
        e.preventDefault();
        pick(item.closest('.adm-user-suggest').querySelector('[data-suggest-users]'), item.dataset.name);
    });

    list.addEventListener('mouseover', function (e) {
        var item = e.target.closest('.adm-user-suggest-item');
        if (!item) return;
        var box = item.parentNode;
        setActive(box, Array.prototype.indexOf.call(box.children, item));
    });

    list.addEventListener('focusout', function (e) {
        if (e.target.matches('[data-suggest-users]')) closeSuggest(e.target);
    });

    // ── Create user ──────────────────────────────────────────────────────

    var createForm = modal.querySelector('form');

    function openModal() {
        createForm.reset();
        modal.classList.add('is-open');
        setTimeout(function () { createForm.elements.username.focus(); }, 30);
    }

    function closeModal() {
        modal.classList.remove('is-open');
        if (location.hash === '#create-user-modal') history.replaceState(history.state, '', location.pathname + location.search);
    }

    document.getElementById('create-user-open').addEventListener('click', function (e) {
        e.preventDefault();
        openModal();
    });

    modal.addEventListener('click', function (e) {
        if (e.target.closest('a[href="#_"]')) {
            e.preventDefault();
            closeModal();
        }
    });

    // One listener for the whole session (AJAX navigation runs this script again)
    if (!window.__admUsersEsc) {
        window.__admUsersEsc = true;
        document.addEventListener('keydown', function (e) {
            var m = document.getElementById('create-user-modal');
            if (e.key === 'Escape' && m && m.classList.contains('is-open')) {
                m.classList.remove('is-open');
            }
        });
    }

    createForm.addEventListener('submit', async function (e) {
        e.preventDefault();
        if (createForm.classList.contains('is-busy')) return;
        var body = new URLSearchParams(new FormData(createForm));
        body.delete('back');
        body.delete('csrf_token');
        createForm.classList.add('is-busy');
        try {
            var res = await fetch(createForm.getAttribute('action'), {
                method: 'POST',
                headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'Accept': 'application/json', 'X-CSRF-Token': csrf() },
                body: body
            });
            var data = await res.json().catch(function () { return {}; });
            if (!res.ok || data.success === false) throw new Error(data.msg || ('HTTP ' + res.status));
            closeModal();
            flash(data.msg || 'User created.', 'success');
            // Show the new account: search for it exactly
            state.q = '"' + data.username + '"';
            state.legacy = false;
            state.status = state.role = '';
            searchInput.value = state.q;
            statusFilter.value = roleFilter.value = '';
            legacyFilter.checked = false;
            refresh();
        } catch (err) {
            flash(err.message || 'Could not create the user.', 'error');
        } finally {
            createForm.classList.remove('is-busy');
        }
    });

    // Opened through #create-user-modal in the address: the class takes over from :target
    if (location.hash === '#create-user-modal') openModal();
})();
