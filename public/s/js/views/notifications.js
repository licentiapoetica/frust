// Tab switching for notification history page
(function () {
    const USER_TYPES = ['comment_reply', 'subscription', 'mention', 'upload_comment'];
    const tabs = document.querySelectorAll('.notif-page-tab');
    const container = document.getElementById('notifications-container');

    tabs.forEach(tab => {
        tab.addEventListener('click', async () => {
            tabs.forEach(t => t.classList.remove('active'));
            tab.classList.add('active');
            const tabName = tab.dataset.tab;
            container.dataset.tab = tabName;
            container.dataset.page = '1';

            // Load first page for this tab
            try {
                const res = await fetch('/ajax/notifications?page=1&tab=' + tabName);
                const data = await res.json();
                if (data.success) {
                    container.innerHTML = data.html || '<div class="notif-empty">' + (window.f0ckI18n?.no_notifications || 'No new notifications') + '</div>';
                    if (window.applyNotifThumbBlur) window.applyNotifThumbBlur(container);
                    const footbar = document.getElementById('footbar');
                    if (footbar) {
                        footbar.style.display = data.hasMore ? '' : 'none';
                    }
                }
            } catch (e) {
                console.error('Failed to load notifications tab', e);
            }
        });
    });

    // Mark all read (current tab only)
    const btn = document.getElementById('mark-all-read-page');
    if (btn) {
        btn.onclick = async () => {
            const activeTabEl = document.querySelector('.notif-page-tab.active');
            const tab = activeTabEl ? activeTabEl.dataset.tab : container.dataset.tab;
            const res = await fetch('/api/notifications/read?tab=' + encodeURIComponent(tab), { method: 'POST' });
            const data = await res.json();
            if (data.success) {
                document.querySelectorAll('.notif-item.unread').forEach(el => el.classList.remove('unread'));
                if (window.NotificationSystemInstance) {
                    window.NotificationSystemInstance.markAllReadUI(tab);
                }
            }
        };
    }
})();
