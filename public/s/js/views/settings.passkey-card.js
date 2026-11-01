      F0ckActions.define({
        chooseAvatarFile: () => { document.getElementById('avatar-file-input').click(); },
      });
      (function() {
        if (window.f0ckPasskeyCard) return;
        function esc(s) { return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }
        var rtf = window.Intl && Intl.RelativeTimeFormat ? new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' }) : null;
        function rel(d) {
          var t = new Date(d).getTime();
          if (!t) return '—';
          var diff = (t - Date.now()) / 1000, a = Math.abs(diff);
          if (!rtf) return new Date(t).toLocaleDateString();
          if (a < 60) return rtf.format(Math.round(diff), 'second');
          if (a < 3600) return rtf.format(Math.round(diff / 60), 'minute');
          if (a < 86400) return rtf.format(Math.round(diff / 3600), 'hour');
          if (a < 86400 * 30) return rtf.format(Math.round(diff / 86400), 'day');
          if (a < 86400 * 365) return rtf.format(Math.round(diff / (86400 * 30)), 'month');
          return rtf.format(Math.round(diff / (86400 * 365)), 'year');
        }
        function full(d) { var t = new Date(d); return isNaN(t) ? '' : t.toLocaleString(); }
        function shortId(id) { id = String(id || ''); return id.length > 14 ? id.slice(0, 6) + '…' + id.slice(-6) : id; }

        // opts.actions: extra HTML at the end of the card (e.g. a delete button)
        window.f0ckPasskeyCard = function(pk, opts) {
          opts = opts || {};
          var provider = pk.provider && pk.provider !== 'Undisclosed'
            ? '<span class="pk-chip" title="Stored in"><i class="fa-solid fa-vault"></i> ' + esc(pk.provider) + '</span>'
            : '<span class="pk-chip" title="The authenticator did not disclose which app or device stores this passkey">Provider unknown</span>';
          var primary = pk.primary ? '<span class="pk-chip is-primary" title="Your anonymous identity (fingerprint) comes from this passkey">Identity</span>' : '';
          if (pk.current) primary += '<span class="pk-chip is-current" title="You are signed in with this passkey right now; it cannot be removed from this session"><i class="fa-solid fa-circle"></i> This session</span>';
          return '<div class="pk-card">' +
            '<i class="fa-solid fa-key pk-icon"></i>' +
            '<div class="pk-main">' +
              '<div class="pk-title">' + esc(pk.name || 'Passkey') + provider + primary + '</div>' +
              '<div class="pk-meta">' +
                '<span title="Credential ID: ' + esc(pk.credential_id || '') + '"><i class="fa-solid fa-hashtag"></i><code>' + esc(shortId(pk.credential_id)) + '</code></span>' +
                '<span title="' + esc(full(pk.created_at)) + '"><i class="fa-solid fa-calendar-plus"></i> Added ' + esc(rel(pk.created_at)) + '</span>' +
                '<span title="' + esc(full(pk.last_used)) + '"><i class="fa-solid fa-clock-rotate-left"></i> Last used ' + esc(rel(pk.last_used)) + '</span>' +
              '</div>' +
            '</div>' +
            // The passkey this session runs on can't be removed (the server refuses it too)
            (pk.current
              ? '<span class="pk-locked" title="In use by this session"><i class="fa-solid fa-lock"></i></span>'
              : (opts.actions || '')) +
          '</div>';
        };
      })();
