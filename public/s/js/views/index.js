function openLoginGate() {
  const container = document.getElementById('gate-container');
  container.style.display = 'block';
  if (!document.getElementById('injected-gate-styles')) {
    const link = document.createElement('link');
    link.id = 'injected-gate-styles';
    link.rel = 'stylesheet';
    link.href = container.dataset.cssHref;
    document.head.appendChild(link);
  }
  const modal = document.getElementById('login-modal');
  if (modal) {
    modal.style.display = 'flex';
  } else {
    const checkModal = setInterval(() => {
      const m = document.getElementById('login-modal');
      if (m) { m.style.display = 'flex'; clearInterval(checkModal); }
    }, 50);
  }
}
document.getElementById('secret-letter').onclick = openLoginGate;
document.getElementById('hot-corner').onclick = openLoginGate;
let secretBuffer = '';
document.addEventListener('keydown', (e) => {
  secretBuffer += e.key.toLowerCase();
  if (secretBuffer.endsWith('premiumhumor')) { openLoginGate(); secretBuffer = ''; }
  if (secretBuffer.length > 10) secretBuffer = secretBuffer.substring(1);
});
