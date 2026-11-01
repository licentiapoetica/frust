(() => {
    var csrf = document.getElementById('add-koepfe').dataset.csrf;

    function esc(s) {
        return (s || '').toString()
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#039;');
    }

    function loadAll() {
        fetch('/api/v2/admin/koepfe', { headers: { 'X-Requested-With': 'XMLHttpRequest' } })
            .then(function(r) { return r.json(); })
            .then(function(data) {
                renderKoepfe(data.koepfe || []);
            })
            .catch(function(err) {
                console.error('[KOEPFE] Load Error:', err);
            });
    }

    function renderKoepfe(koepfe) {
        var grid = document.getElementById('koepfe-list');
        if (!grid) return;
        if (koepfe.length === 0) {
            grid.innerHTML = '<p class="adm-koepfe-empty">Keine Köpfe vorhanden.</p>';
            return;
        }
        grid.innerHTML = koepfe.map(function(k) {
            return '<div class="emoji-card">' +
                '<button class="emoji-delete" data-on="click:deleteKoepfe" data-arg="' + esc(k.filename) + '" title="Delete">x</button>' +
                '<img class="emoji-preview" src="' + esc(k.url) + '" alt="">' +
                '<span class="emoji-label adm-koepfe-filename">' + esc(k.filename) + '</span>' +
                '</div>';
        }).join('');
    }

    function addKoepfe(e) {
        if (e) e.preventDefault();
        var fileInput = document.getElementById('koepfe-file');
        if (!fileInput.files[0]) return alert('Bitte eine Datei auswählen.');

        var btn = document.getElementById('add-koepfe');
        var oldText = btn.textContent;
        btn.disabled = true;
        btn.textContent = 'Lädt...';

        var formData = new FormData();
        formData.append('file', fileInput.files[0]);
        formData.append('csrf_token', csrf);

        var headers = { 'X-Requested-With': 'XMLHttpRequest', 'X-CSRF-Token': csrf };

        fetch('/api/v2/admin/koepfe/upload', { method: 'POST', headers: headers, body: formData })
            .then(function(r) { return r.json(); })
            .then(function(data) {
                if (data.success) {
                    document.getElementById('koepfe-file').value = '';
                    loadAll();
                } else {
                    alert('Fehler: ' + (data.message || data.msg || 'Unbekannter Fehler'));
                }
            })
            .catch(function(err) { alert('Error: ' + err.message); })
            .finally(function() { btn.disabled = false; btn.textContent = oldText; });
    }

    window.deleteKoepfe = function(filename) {
        if (!confirm('Diesen Kopf wirklich löschen?')) return;
        fetch('/api/v2/admin/koepfe/delete', { 
            method: 'POST', 
            headers: { 'X-CSRF-Token': csrf, 'Content-Type': 'application/json' },
            body: JSON.stringify({ filename: filename })
        })
        .then(function(r) { return r.json(); })
        .then(function(data) { if (data.success) loadAll(); else alert('Löschen fehlgeschlagen: ' + (data.msg || '')); })
        .catch(function(err) { console.error(err); });
    };

    F0ckActions.define({ deleteKoepfe: function(el, e, filename) { window.deleteKoepfe(filename); } });

    document.getElementById('add-koepfe').addEventListener('click', addKoepfe);

    loadAll();
})();
