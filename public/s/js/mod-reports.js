
var CATEGORY_META = {
    wrong_rating: { label: 'Wrong Rating', bg: '#ffc107', color: '#000' },
    spam:         { label: 'Spam',         bg: '#6c757d', color: '#fff' },
    duplicate:    { label: 'Duplicate',    bg: '#17a2b8', color: '#fff' },
    copyright:    { label: 'Copyright',    bg: '#fd7e14', color: '#fff' },
    illegal:      { label: 'Illegal',      bg: '#dc3545', color: '#fff' },
    other:        { label: 'Other',        bg: '#495057', color: '#fff' }
};

function catBadge(c) {
    var m = CATEGORY_META[c] || { label: c, bg: '#444', color: '#fff' };
    return '<span class="rp-cat" style="background:' + m.bg + ';color:' + m.color + ';">' + m.label + '</span>';
}

function rpEsc(s) {
    return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

function relTime(d) {
    var diff = (Date.now() - new Date(d)) / 1000;
    if (diff < 60) return 'just now';
    if (diff < 3600) return Math.floor(diff/60) + 'm ago';
    if (diff < 86400) return Math.floor(diff/3600) + 'h ago';
    return new Date(d).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

window.currentPage = window.currentPage || 1;

// Report card / pagination buttons: data-on="click:rp-call" data-args='["fnName", ...args]' calls window[fnName](...args)
var RP_CALLABLE = ['resolveReport', 'adminDeleteItem', 'modToggleUnavailable', 'adminDeleteComment', 'modWarnUser', 'adminBanUser', 'loadReports'];
F0ckActions.define('rp-call', function(el, e, fn) {
    if (RP_CALLABLE.indexOf(fn) === -1 || typeof window[fn] !== 'function') return;
    window[fn].apply(window, Array.prototype.slice.call(arguments, 3));
});

window.loadReports = async function(page) {
    page = page || 1;
    window.currentPage = page;
    var status = document.getElementById('report-status-filter').value;
    var feed = document.getElementById('reports-feed');
    var pag  = document.getElementById('reports-pagination');

    feed.innerHTML = '<div class="rp-state-msg"><i class="fa-solid fa-spinner fa-spin"></i> Loading...</div>';
    pag.innerHTML = '';

    try {
        var res  = await fetch('/api/v2/mod/reports?status=' + status + '&page=' + page);
        var data = await res.json();

        if (!data.success) {
            feed.innerHTML = '<div class="rp-state-msg" style="color:#dc3545;">Error: ' + rpEsc(data.msg) + '</div>';
            return;
        }

        window.currentReports = data.reports;
        window.emojiMap = new Map();
        if (data.emojis) data.emojis.forEach(function(e) { window.emojiMap.set(e.name.toLowerCase(), e.url); });

        if (!data.reports.length) {
            feed.innerHTML = '<div class="rp-state-msg">No reports found.</div>';
            return;
        }

        feed.innerHTML = '';
        var isAdmin = window.f0ckSession && window.f0ckSession.admin;

        data.reports.forEach(function(r) {
            var card = document.createElement('div');
            var cats = Array.isArray(r.categories) ? r.categories : [];
            card.className = 'rp-card' + (cats.indexOf('illegal') !== -1 ? ' illegal-flag' : '');

            var statusBadge = '<span class="rp-status-badge rp-status-' + status + '">' + status + '</span>';

            var reporter = r.reporter_name
                ? '<a href="/user/' + rpEsc(r.reporter_name) + '" class="rp-reporter-link">' + rpEsc(r.reporter_name) + '</a>' + (r.reporter_ip ? ' <span class="rp-reporter-ip">(' + rpEsc(r.reporter_ip) + ')</span>' : '')
                : '<span style="color:#666;font-style:italic;">Guest' + (r.reporter_ip ? ' (' + rpEsc(r.reporter_ip) + ')' : '') + '</span>';

            var targetHtml = '';
            var itemLink = '';
            if (r.comment_id) {
                targetHtml = '<span style="color:#888;font-size:0.85em;">comment #' + r.comment_id + '</span>';
                if (r.resolved_item_id) itemLink = '<a href="/' + r.resolved_item_id + '" target="_blank" class="rp-open-link"><i class="fa-solid fa-arrow-up-right-from-square"></i> item #' + r.resolved_item_id + '</a>';
            } else if (r.resolved_item_id) {
                targetHtml = '<span style="color:#888;font-size:0.85em;">item</span>';
                itemLink = '<a href="/' + r.resolved_item_id + '" target="_blank" class="rp-open-link"><i class="fa-solid fa-arrow-up-right-from-square"></i> #' + r.resolved_item_id + '</a>';
            } else if (r.reported_user_name) {
                targetHtml = 'user <a href="/user/' + rpEsc(r.reported_user_name) + '" class="rp-target-link">' + rpEsc(r.reported_user_name) + '</a>';
            }

            var previewHtml = '';
            var isItem = !!r.resolved_item_id && r.resolved_item_dest;
            var isComment = !!r.comment_id;
            if (isItem && !isComment) {
                var mime = r.resolved_item_mime || '';
                var src  = '/b/' + r.resolved_item_dest;
                var href = '/' + r.resolved_item_id;
                if (mime === 'video/youtube') {
                    var ytId = r.resolved_item_dest.replace('yt:', '');
                    previewHtml = '<div class="rp-preview"><img src="https://img.youtube.com/vi/' + ytId + '/mqdefault.jpg" loading="lazy"><a href="' + href + '" target="_blank" class="rp-preview-link"><i class="fa-brands fa-youtube"></i></a></div>';
                } else if (mime.indexOf('image/') === 0) {
                    previewHtml = '<div class="rp-preview"><img src="' + src + '" loading="lazy"><a href="' + href + '" target="_blank" class="rp-preview-link"><i class="fa-solid fa-expand"></i></a></div>';
                } else if (mime.indexOf('video/') === 0) {
                    previewHtml = '<div class="rp-preview"><video src="' + src + '" muted playsinline preload="metadata"></video><a href="' + href + '" target="_blank" class="rp-preview-link"><i class="fa-solid fa-play"></i></a></div>';
                } else if (mime.indexOf('audio/') === 0) {
                    previewHtml = '<div class="rp-preview"><div class="rp-no-preview"><i class="fa-solid fa-music"></i></div><a href="' + href + '" target="_blank" class="rp-preview-link"><i class="fa-solid fa-expand"></i></a></div>';
                } else {
                    previewHtml = '<div class="rp-preview"><div class="rp-no-preview"><i class="fa-solid fa-file"></i></div><a href="' + href + '" target="_blank" class="rp-preview-link"><i class="fa-solid fa-expand"></i></a></div>';
                }
            } else if (isComment) {
                var body = (r.comment_body || '[deleted]').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
                if (window.emojiMap) {
                    body = body.replace(/:([a-z0-9_]+):/g, function(match, code) {
                        var url = window.emojiMap.get(code.toLowerCase());
                        return url ? '<img src="' + url + '" style="height:18px;vertical-align:middle;" title=":' + code + ':">' : match;
                    });
                }
                previewHtml = '<div class="rp-preview" style="background:rgba(0,0,0,0.4);width:180px;min-width:180px;align-items:flex-start;padding:12px;overflow-y:auto;font-size:0.78em;color:#ccc;font-family:monospace;line-height:1.5;white-space:pre-wrap;height:140px;">' + body + '</div>';
            } else {
                previewHtml = '<div class="rp-preview"><div class="rp-no-preview"><i class="fa-solid fa-user"></i></div></div>';
            }

            var catsHtml = cats.length ? '<div class="rp-cats">' + cats.map(catBadge).join('') + '</div>' : '';
            var reasonHtml = r.reason ? '<div class="rp-reason">' + rpEsc(r.reason) + '</div>' : '';

            var actions = '';
            if (status === 'pending') {
                actions += '<button class="rp-btn rp-btn-resolve" data-on="click:rp-call" data-args=\'["resolveReport",' + r.id + ',"resolved"]\'><i class="fa-solid fa-check"></i> Resolve</button>';
                actions += '<button class="rp-btn rp-btn-reject" data-on="click:rp-call" data-args=\'["resolveReport",' + r.id + ',"rejected"]\'><i class="fa-solid fa-xmark"></i> Reject</button>';
                actions += '<span class="rp-sep"></span>';
            }
            if (isItem && !isComment) {
                var isUnav = r.resolved_item_visibility === 3;
                actions += '<button class="rp-btn rp-btn-delete" data-on="click:rp-call" data-args=\'["adminDeleteItem",' + r.resolved_item_id + ']\'><i class="fa-solid fa-trash"></i> Delete</button>';
                actions += '<button class="rp-btn ' + (isUnav ? 'rp-btn-avail' : 'rp-btn-unavail') + '" data-on="click:rp-call" data-args=\'["modToggleUnavailable",' + r.resolved_item_id + ',' + (r.resolved_item_visibility||0) + ']\'>' + (isUnav ? '<i class="fa-solid fa-eye"></i> Restore' : '<i class="fa-solid fa-ban"></i> 451') + '</button>';
            }
            if (isComment) {
                actions += '<button class="rp-btn rp-btn-delete" data-on="click:rp-call" data-args=\'["adminDeleteComment",' + r.comment_id + ']\'><i class="fa-solid fa-trash"></i> Del Comment</button>';
            }
            if (r.reported_user_id && (isAdmin || !r.reported_user_is_admin)) {
                var who = r.reported_user_name ? rpEsc(r.reported_user_name) : 'user';
                actions += '<span class="rp-sep"></span>';
                actions += '<button class="rp-btn rp-btn-warn" data-on="click:rp-call" data-args=\'["modWarnUser",' + r.reported_user_id + ']\'><i class="fa-solid fa-triangle-exclamation"></i> Warn ' + who + '</button>';
                actions += '<button class="rp-btn rp-btn-ban" data-on="click:rp-call" data-args=\'["adminBanUser",' + r.reported_user_id + ']\'><i class="fa-solid fa-gavel"></i> Ban ' + who + '</button>';
            } else if (!r.reported_user_id) {
                actions += '<span class="rp-anon-note">Anonymous reporter</span>';
            }
            if (r.reporter_id && r.reporter_name) {
                actions += '<button class="rp-btn rp-btn-secondary" data-on="click:rp-call" data-args=\'["modWarnUser",' + r.reporter_id + ']\'>Warn Reporter</button>';
            }

            var ts = new Date(r.created_at).toLocaleString();
            var rt = relTime(r.created_at);

            var resolverHtml = (status !== 'pending' && r.resolver_name)
                ? '<span style="font-size:0.78em;color:#888;margin-left:6px;"><i class="fa-solid fa-' + (status === 'resolved' ? 'check' : 'xmark') + '" style="margin-right:3px;color:' + (status === 'resolved' ? '#28a745' : '#6c757d') + ';"></i>by <a href="/user/' + rpEsc(r.resolver_name) + '" style="color:#888;font-weight:600;text-decoration:none;">' + rpEsc(r.resolver_name) + '</a></span>'
                : '';

            card.innerHTML =
                '<div class="rp-card-bar">' +
                    '<div class="rp-card-bar-left">' +
                        '<span class="rp-card-id">#' + r.id + '</span>' +
                        statusBadge +
                        resolverHtml +
                        '<span style="font-size:0.83em;color:#aaa;margin-left:6px;">from ' + reporter + '</span>' +
                        (targetHtml ? '<span style="font-size:0.83em;color:#777;">&rarr; ' + targetHtml + '</span>' : '') +
                        itemLink +
                    '</div>' +
                    '<span class="rp-card-time" title="' + rpEsc(ts) + '">' + rt + '</span>' +
                '</div>' +
                '<div class="rp-card-body">' +
                    previewHtml +
                    '<div class="rp-info">' + catsHtml + reasonHtml + '</div>' +
                '</div>' +
                '<div class="rp-card-actions">' + actions + '</div>';

            feed.appendChild(card);
        });

        pag.innerHTML = '';
        if (data.pages > 1) {
            if (data.page > 1)
                pag.innerHTML += '<button data-on="click:rp-call" data-args=\'["loadReports",' + (data.page-1) + ']\'><i class="fa-solid fa-chevron-left"></i> Prev</button>';
            pag.innerHTML += '<span class="rp-page-info">Page ' + data.page + ' of ' + data.pages + '</span>';
            if (data.page < data.pages)
                pag.innerHTML += '<button data-on="click:rp-call" data-args=\'["loadReports",' + (data.page+1) + ']\'>Next <i class="fa-solid fa-chevron-right"></i></button>';
        }

    } catch(e) {
        feed.innerHTML = '<div class="rp-state-msg" style="color:#dc3545;"><i class="fa-solid fa-circle-exclamation"></i> Network error</div>';
    }
};

window.resolveReport = function(id, action) {
    var label = action === 'resolved' ? 'Resolve' : 'Reject';
    var desc = action === 'resolved'
        ? 'Mark report #' + id + ' as resolved.'
        : 'Reject report #' + id + '. The content will remain as-is.';
    window.ModAction.confirm(label + ' Report #' + id, desc, async function() {
        var params = new URLSearchParams();
        params.append('action', action);
        var csrfToken = window.f0ckSession && window.f0ckSession.csrf_token;
        var res = await fetch('/api/v2/mod/reports/' + id + '/resolve', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/x-www-form-urlencoded',
                'X-CSRF-Token': csrfToken
            },
            body: params
        });
        var data = await res.json();
        if (data.success) {
            window.loadReports(window.currentPage);
            if (window.NotificationSystemInstance && typeof window.NotificationSystemInstance.pollDebounced === 'function')
                window.NotificationSystemInstance.pollDebounced();
        } else {
            throw new Error(data.msg || 'Failed to update report');
        }
    }, { hideReason: true });
};

window.adminDeleteComment = function(id) {
    window.ModAction.confirm('Delete Comment #' + id, 'Are you sure you want to delete this comment? This action is permanent.', async (reason) => {
        var params = new URLSearchParams();
        params.append('reason', reason);
        var res = await fetch('/api/comments/' + id + '/delete', { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: params });
        var data = await res.json();
        if (data.success) { if (window.showFlash) window.showFlash('comment deleted', 'success'); }
        else throw new Error(data.msg || 'Unknown error');
    });
};

window.adminDeleteItem = function(id) {
    window.ModAction.confirm('Delete Item #' + id, 'Are you sure you want to delete this item? This action is permanent.', async (reason) => {
        var params = new URLSearchParams();
        params.append('postid', id); params.append('reason', reason);
        var res = await fetch('/api/v2/admin/deletepost', { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: params });
        var data = await res.json();
        if (data.success) { if (window.showFlash) window.showFlash('item deleted', 'success'); }
        else throw new Error(data.msg || 'Unknown error');
    });
};

window.modToggleUnavailable = function(id, currentVis) {
    var willBeUnavailable = currentVis !== 3;
    var targetVis = willBeUnavailable ? 3 : 0;
    var actionText = willBeUnavailable ? 'Make Unavailable (HTTP 451)' : 'Make Available (Public)';
    window.ModAction.confirm('Item Visibility', actionText + ' for item #' + id + '?', async () => {
        var params = new URLSearchParams();
        params.append('postid', id); params.append('id', id); params.append('visibility', targetVis);
        var csrfToken = window.f0ckSession && window.f0ckSession.csrf_token;
        var res = await fetch('/api/v2/item/visibility', { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'X-CSRF-Token': csrfToken }, body: params });
        var data = await res.json();
        if (data.success) {
            if (window.showFlash) window.showFlash(willBeUnavailable ? 'Item marked unavailable (451)' : 'Item restored to public', 'success');
            var item = window.currentReports.find(function(x) { return x.resolved_item_id === id; });
            if (item) item.resolved_item_visibility = targetVis;
            window.loadReports(window.currentPage);
        } else throw new Error(data.msg || 'Failed to update visibility');
    });
};

window.modWarnUser = function(userId) {
    window.ModAction.confirm('Warn User ID ' + userId, '', async (reason) => {
        var params = new URLSearchParams();
        params.append('user_id', userId); params.append('reason', reason);
        var res = await fetch('/api/v2/mod/warnings/issue', { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: params });
        var data = await res.json();
        if (data.success) { if (window.showFlash) window.showFlash('user has been warned', 'success'); }
        else throw new Error(data.msg || 'Unknown error');
    });
};

window.adminBanUser = function(userId) {
    var isAdmin = window.f0ckSession && window.f0ckSession.admin;
    var promptHtml =
        '<p>This will restrict the user from accessing their account and performing most actions.</p>' +
        '<div style="margin-top:10px;"><label>Ban Duration:</label>' +
        '<select id="ban-duration-select" class="form-control" style="margin-top:5px;">' +
        (isAdmin ? '<option value="permanent">Permanent</option>' : '') +
        '<option value="1">1 Hour</option><option value="6">6 Hours</option><option value="24">24 Hours (1 Day)</option>' +
        (!isAdmin ? '<option value="48">48 Hours (2 Days)</option>' : '') +
        (isAdmin ? '<option value="168">168 Hours (1 Week)</option>' : '') +
        (isAdmin ? '<option value="720">720 Hours (1 Month)</option>' : '') +
        '</select></div>';
    window.ModAction.confirm('Ban User ID ' + userId, promptHtml, async (reason) => {
        var duration = document.getElementById('ban-duration-select').value;
        var params = new URLSearchParams();
        params.append('user_id', userId); params.append('reason', reason); params.append('duration', duration);
        var res = await fetch('/api/v2/admin/ban', { method: 'POST', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: params });
        var data = await res.json();
        if (data.success) { if (window.showFlash) window.showFlash('User banned cleanly.', 'success'); }
        else throw new Error(data.msg || 'Unknown error');
    });
};

(function() {
    var filter = document.getElementById('report-status-filter');
    if (filter) filter.onchange = function() { window.loadReports(1); };
    window.loadReports(1);
})();
