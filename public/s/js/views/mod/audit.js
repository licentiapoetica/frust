(function () {
    var DATA = JSON.parse(document.getElementById('mod-audit-data').textContent);
    var grid = document.getElementById('audit-grid');
    var loader = document.getElementById('audit-loading');
    var pagination = document.getElementById('audit-pagination');

    var escapeHtml = function (unsafe) {
        return (unsafe || '')
            .replace(/&/g, "&amp;")
            .replace(/</g, "&lt;")
            .replace(/>/g, "&gt;")
            .replace(/"/g, "&quot;")
            .replace(/'/g, "&#39;");
    };

    var renderDiff = function (oldText, newText) {
        var isEmpty = function (t) { return !t || t === 'null' || t === 'undefined'; };
        if (isEmpty(oldText) && isEmpty(newText)) return '';

        var html = '<div class="audit-diff">';
        if (!isEmpty(oldText)) html += '<div class="diff-removed">- ' + escapeHtml(oldText) + '</div>';
        if (!isEmpty(newText)) html += '<div class="diff-added">+ ' + escapeHtml(newText) + '</div>';
        html += '</div>';
        return html;
    };

    var currentPage = Number(DATA.page) || 1;
    var totalPages = Number(DATA.pages) || 1;
    var loading = false;
    var hasMore = currentPage < totalPages;

    // Read active filters so infinite scroll preserves them
    var activeFilterAction = DATA.filterAction || '';
    var activeFilterUser   = DATA.filterUser   || '';

    if (pagination) pagination.style.display = 'none';

    window.addEventListener('scroll', function () {
        if (loading || !hasMore) return;

        var scrollPosition = window.innerHeight + window.scrollY;
        var threshold = document.documentElement.scrollHeight - 500;

        if (scrollPosition > threshold) {
            loadMore();
        }
    });

    async function loadMore() {
        if (loading || !hasMore) return;
        loading = true;
        if (loader) loader.style.display = 'block';

        try {
            var next = currentPage + 1;
            var url = '/mod/audit?page=' + next;
            if (activeFilterAction) url += '&action=' + encodeURIComponent(activeFilterAction);
            if (activeFilterUser)   url += '&user='   + encodeURIComponent(activeFilterUser);

            var res = await fetch(url, {
                headers: { 'X-Requested-With': 'XMLHttpRequest' }
            });
            var data = await res.json();

            if (data.success && data.logs && data.logs.length) {
                data.logs.forEach(function (log) {
                    var card = document.createElement('div');
                    card.className = 'audit-card';

                    var reason = log.reason;

                    var html = '<div class="audit-card-header">' +
                        '<div class="audit-card-user">' +
                        '<a href="/user/' + encodeURIComponent(log.username) + '">' + log.username + '</a>' +
                        '</div>' +
                        '<div class="audit-card-header-right">' +
                        '<div class="audit-card-time">' + (log.created_at || '') + '</div>' +
                        '<span class="audit-entry-id">#<span class="audit-entry-id-val">' + (log.id || '') + '</span></span>' +
                        '</div>' +
                        '</div>' +
                        '<div class="audit-card-body">' +
                        '<div class="audit-card-row">' +
                        '<span class="audit-label">Action:</span>' +
                        '<span class="badge badge-secondary badge-action">' + (log.action || '') + '</span>' +
                        '</div>' +
                        '<div class="audit-card-row">' +
                        '<span class="audit-label">Target:</span>' +
                        '<span class="audit-value">' +
                        '<strong>' + (log.target_type || '') + '</strong> ';

                    if (log.target_type === 'comment') {
                        if (log.item_id) {
                            html += '<a href="/' + log.item_id + '#c' + log.target_id + '">#c' + log.target_id + '</a>';
                        } else {
                            html += '#c' + (log.target_id || '');
                        }
                    } else if (log.target_type === 'item') {
                        html += '<a href="/' + log.target_id + '">/' + log.target_id + '</a>';
                    } else {
                        html += (log.target_id || '');
                    }
                    html += '</span></div>';

                    if (reason) {
                        html += '<div class="audit-card-row">' +
                            '<span class="audit-label">Reason:</span>' +
                            '<span class="audit-value">' + escapeHtml(reason) + '</span>' +
                            '</div>';
                    }

                    if (log.uploader_info) {
                        html += '<div class="audit-card-row">' +
                            '<span class="audit-label">Info:</span>' +
                            '<span class="audit-value mod-audit-info">' + escapeHtml(log.uploader_info) + '</span>' +
                            '</div>';
                    }

                    if (log.old_content || log.new_content || log.details_json) {
                        html += '<div class="audit-card-row mod-audit-changes">' +
                            '<span class="audit-label">Changes:</span>' +
                            '<div class="audit-diff">' +
                            (log.old_content ? '<div class="diff-removed">- ' + escapeHtml(log.old_content) + '</div>' : '') +
                            (log.new_content ? '<div class="diff-added">+ ' + escapeHtml(log.new_content) + '</div>' : '') +
                            (log.details_json ? '<div class="audit-details-json mod-audit-details">' + escapeHtml(log.details_json) + '</div>' : '') +
                            '</div>' +
                            '</div>';
                    }

                    html += '</div>';
                    card.innerHTML = html;
                    grid.appendChild(card);
                });
                currentPage = data.page;
                hasMore = data.hasMore;
                if (document.getElementById('current-page-display')) {
                    document.getElementById('current-page-display').innerText = currentPage;
                }
            } else {
                hasMore = false;
            }
        } catch (err) {
            console.error('Audit infinite scroll error:', err);
            hasMore = false;
        } finally {
            loading = false;
            if (loader) loader.style.display = 'none';
        }
    }
})();
