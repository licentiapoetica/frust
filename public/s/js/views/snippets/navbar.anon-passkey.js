(function(){
  // Keep legacy modal ID working for any nav onclick handlers that reference anon-ssh-modal
  var legacyAlias = document.getElementById('anon-ssh-modal');
  if (!legacyAlias) {
    // Create a minimal alias element that delegates to the passkey modal
    var alias = document.createElement('div');
    alias.id = 'anon-ssh-modal';
    alias.style.display = 'none';
    Object.defineProperty(alias.style, 'display', {
      set: function(v) { if (v === 'flex' || v === 'block') { var m = document.getElementById('anon-passkey-modal'); if (m) m.style.display = 'flex'; } }
    });
    document.body.appendChild(alias);
  }

  var modalClose = document.getElementById('anon-passkey-modal-close');
  if (modalClose) {
    modalClose.addEventListener('click', function() {
      var m = document.getElementById('anon-passkey-modal');
      if (m) m.style.display = 'none';
    });
  }
  var overlay = document.getElementById('anon-passkey-modal');
  if (overlay) {
    overlay.addEventListener('click', function(e) { if (e.target === overlay) overlay.style.display = 'none'; });
  }
})();
