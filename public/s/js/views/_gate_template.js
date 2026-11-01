function openLoginGate() {
  document.getElementById('gate-container').style.display = 'block';
  const tryOpen = () => {
    const m = document.getElementById('login-modal');
    if (m) { m.style.display = 'flex'; }
    else { setTimeout(tryOpen, 50); }
  };
  tryOpen();
}
F0ckActions.define({ openLoginGate: function() { openLoginGate(); } });
