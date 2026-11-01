F0ckActions.define({
  copyBgCommand: () => copyBgCommand(),
  applyBgFromAdmin: () => applyBgFromAdmin(),
  clearBgFromAdmin: () => clearBgFromAdmin(),
});

// Chat Background Builder Logic
window.builder_url = "";
window.builder_pos = "";
window.builder_size = "";
window.builder_repeat = "";
window.builder_opts = "";
window.builder_cmd = "";

function updateBgCommand() {
  var builder_uEl = document.getElementById('bg_url');
  var builder_pEl = document.getElementById('bg_pos');
  var builder_sEl = document.getElementById('bg_size');
  var builder_rEl = document.getElementById('bg_repeat');
  var builder_oEl = document.getElementById('bg_command_output');

  if (!builder_uEl || !builder_pEl || !builder_sEl || !builder_rEl || !builder_oEl) return { url: '', opts: '' };

  window.builder_url = builder_uEl.value.trim();
  window.builder_pos = builder_pEl.value;
  window.builder_size = builder_sEl.value;
  window.builder_repeat = builder_rEl.value;

  window.builder_opts = window.builder_pos + " / " + window.builder_size + " " + window.builder_repeat;
  window.builder_cmd = "/setbackground " + window.builder_url + " " + window.builder_opts;
  builder_oEl.textContent = window.builder_cmd;
  return { url: window.builder_url, opts: window.builder_opts };
}

if (document.getElementById('bg_url')) {
    document.getElementById('bg_url').addEventListener('input', updateBgCommand);
    document.getElementById('bg_pos').addEventListener('change', updateBgCommand);
    document.getElementById('bg_size').addEventListener('change', updateBgCommand);
    document.getElementById('bg_repeat').addEventListener('change', updateBgCommand);
    document.getElementById('bg_size').addEventListener('input', updateBgCommand);
    document.getElementById('bg_size_presets').addEventListener('change', function () {
        document.getElementById('bg_size').value = this.value;
        updateBgCommand();
    });
}

function copyBgCommand() {
  const outputElem = document.getElementById('bg_command_output');
  if (!outputElem) return;
  const cmd = outputElem.textContent;
  if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(cmd);
      const status = document.getElementById('chat-status');
      if (status) {
          status.textContent = 'Command copied to clipboard!';
          status.style.color = 'var(--accent)';
          setTimeout(() => { status.textContent = ''; }, 2000);
      }
  } else {
      alert('Clipboard access denied or not supported.');
  }
}

async function applyBgFromAdmin() {
  const { url, opts } = updateBgCommand();
  if (!url) return alert('Please enter an image URL first.');

  const btn = document.getElementById('apply_bg_btn');
  const status = document.getElementById('chat-status');
  if (btn) {
      btn.disabled = true;
      btn.textContent = 'Applying...';
  }

  try {
    const res = await fetch('/api/chat/background', {
      method: 'POST',
      headers: {
        'X-Requested-With': 'XMLHttpRequest',
        'Content-Type': 'application/x-www-form-urlencoded',
        'X-CSRF-Token': document.getElementById('chat-status').dataset.csrf
      },
      body: new URLSearchParams({ url, opts }).toString()
    });
    const data = await res.json();
    if (data.success) {
      if (status) {
          status.textContent = 'Background applied successfully!';
          status.style.color = '#28a745';
      }
      if (btn) btn.textContent = 'Applied!';
      setTimeout(() => {
        if (btn) {
            btn.disabled = false;
            btn.textContent = 'Apply Instantly';
        }
        if (status) status.textContent = '';
      }, 3000);
    } else {
      throw new Error(data.msg || 'Failed to apply background');
    }
  } catch (err) {
    alert(err.message);
    if (btn) {
        btn.disabled = false;
        btn.textContent = 'Apply Instantly';
    }
  }
}

async function clearBgFromAdmin() {
  if (!confirm('Are you sure you want to clear the chat background?')) return;
  const status = document.getElementById('chat-status');
  try {
    const res = await fetch('/api/chat/background', {
      method: 'POST',
      headers: {
        'X-Requested-With': 'XMLHttpRequest',
        'Content-Type': 'application/x-www-form-urlencoded',
        'X-CSRF-Token': document.getElementById('chat-status').dataset.csrf
      },
      body: new URLSearchParams({}).toString()
    });
    const data = await res.json();
    if (data.success) {
      if (status) {
          status.textContent = 'Background cleared!';
          status.style.color = '#28a745';
      }
      const urlInput = document.getElementById('bg_url');
      if (urlInput) urlInput.value = '';
      updateBgCommand();
      setTimeout(() => { if (status) status.textContent = ''; }, 3000);
    }
  } catch (err) { alert(err.message); }
}
