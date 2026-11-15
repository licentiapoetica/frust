// Rules and terms before registering (snippets/tos-consent.html). The "I have read the rules and
// terms" checkbox of a register form (input[name="tos"]) can only be ticked through the dialog:
// first the rules, then the terms (always both), each scrolled to the end. A register form sent with the box
// unticked opens the dialog instead. The texts are the admin-edited Markdown (GET /api/v2/tos).
(function () {
  var modal = document.getElementById('tos-consent-modal');
  if (!modal) return;
  var markedSrc = document.currentScript && document.currentScript.dataset.marked;
  var body = document.getElementById('tos-consent-body');
  var title = document.getElementById('tos-consent-title');
  var hint = document.getElementById('tos-consent-hint');
  var accept = document.getElementById('tos-consent-accept');
  var acceptLabel = document.getElementById('tos-consent-accept-label');
  var closeBtn = document.getElementById('tos-consent-close');

  var TOS = 'input[type="checkbox"][name="tos"]';
  var accepted = false; // read to the end on this page: the box ticks freely from then on
  var target = null;    // the checkbox being ticked
  var steps = null;     // [{ title, text }], loaded once
  var step = 0;

  function escapeHtml(str) {
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }
  // As on the /rules and /terms pages (dynamic-page.js)
  function sanitizeHtml(html) {
    var tmp = document.createElement('div');
    tmp.innerHTML = html;
    tmp.querySelectorAll('script,iframe,object,embed,form,input,button,select,meta,link,base,style').forEach(function (el) { el.remove(); });
    tmp.querySelectorAll('*').forEach(function (node) {
      Array.from(node.attributes).forEach(function (attr) {
        if (/^on/i.test(attr.name) || /^data-(on|args?)$/i.test(attr.name) || (attr.name === 'href' && /^javascript:/i.test(attr.value.trim()))) {
          node.removeAttribute(attr.name);
        }
      });
    });
    return tmp.innerHTML;
  }
  function loadMarked() {
    if (typeof marked !== 'undefined' || !markedSrc) return Promise.resolve();
    return new Promise(function (resolve) {
      var s = document.createElement('script');
      s.src = markedSrc;
      s.onload = s.onerror = function () { resolve(); };
      document.head.appendChild(s);
    });
  }
  function renderText(text) {
    var box = document.createElement('div');
    box.className = 'pc-md dynamic-page-content';
    if (!text) {
      box.classList.add('pc-md-empty');
      box.textContent = 'Nothing here yet …';
      return box;
    }
    if (typeof marked === 'undefined') {
      // No Markdown renderer: the text as written
      box.classList.add('pc-md-plain');
      box.textContent = text;
      return box;
    }
    var renderer = new marked.Renderer();
    renderer.code = function (code, lang) {
      var escaped = escapeHtml(typeof code === 'object' ? (code.text || '') : code);
      var langAttr = (typeof code === 'object' ? code.lang : lang) || '';
      return '<pre><code' + (langAttr ? ' class="language-' + escapeHtml(langAttr) + '"' : '') + '>' + escaped + '</code></pre>';
    };
    renderer.codespan = function (code) {
      return '<code>' + escapeHtml(typeof code === 'object' ? (code.text || '') : code) + '</code>';
    };
    box.innerHTML = sanitizeHtml(marked.parse(text, { gfm: true, breaks: true, renderer: renderer }));
    box.querySelectorAll('a[href]').forEach(function (a) { a.target = '_blank'; a.rel = 'noopener'; });
    return box;
  }

  async function loadSteps() {
    if (steps) return steps;
    var r = await fetch('/api/v2/tos', { credentials: 'same-origin', headers: { Accept: 'application/json' } });
    var data = await r.json();
    if (!r.ok || !data || data.success === false) throw new Error('bad response');
    await loadMarked();
    // Always both, one after the other. A text the admins haven't written yet (/admin/rules,
    // /admin/terms) shows a note in its place, as its page does.
    steps = [
      { title: 'Rules', text: (data.rules || '').trim() },
      { title: 'Terms', text: (data.terms || '').trim() },
    ];
    return steps;
  }

  function atEnd() {
    return body.scrollTop + body.clientHeight >= body.scrollHeight - 8;
  }
  function checkEnd() {
    if (modal.hidden || !steps || !steps.length || !accept.disabled) return;
    if (atEnd()) {
      accept.disabled = false;
      hint.textContent = 'You reached the end.';
    }
  }
  function showStep() {
    var s = steps[step];
    var last = step === steps.length - 1;
    title.textContent = s.title + (steps.length > 1 ? ' (' + (step + 1) + '/' + steps.length + ')' : '');
    acceptLabel.textContent = last ? 'Accept' : 'Next: ' + steps[step + 1].title;
    hint.textContent = last ? 'Scroll to the end to accept.' : 'Scroll to the end to continue.';
    accept.disabled = true;
    body.textContent = '';
    body.appendChild(renderText(s.text));
    body.scrollTop = 0;
    // A text short enough not to scroll counts as read
    requestAnimationFrame(checkEnd);
  }

  function finish() {
    accepted = true;
    modal.hidden = true;
    if (target) {
      target.checked = true;
      target.dispatchEvent(new Event('change', { bubbles: true }));
    }
    target = null;
  }
  function close() {
    modal.hidden = true;
    target = null;
  }

  async function open(checkbox) {
    target = checkbox;
    step = 0;
    title.textContent = 'Rules';
    hint.textContent = '';
    accept.disabled = true;
    body.innerHTML = '<div class="pc-loading">Loading…</div>';
    modal.hidden = false;
    try {
      await loadSteps();
    } catch (e) {
      // Not loaded: the pages themselves, and the box stays unticked
      body.textContent = '';
      var p = document.createElement('div');
      p.className = 'pc-md';
      p.innerHTML = 'Could not load the rules and terms. Read them here and try again: <a href="/rules" target="_blank" rel="noopener">Rules</a>, <a href="/terms" target="_blank" rel="noopener">Terms</a>.';
      body.appendChild(p);
      return;
    }
    if (!modal.hidden) showStep();
  }

  body.addEventListener('scroll', checkEnd, { passive: true });
  window.addEventListener('resize', checkEnd);
  accept.addEventListener('click', function () {
    if (accept.disabled || !steps) return;
    if (step < steps.length - 1) {
      step++;
      showStep();
    } else {
      finish();
    }
  });
  closeBtn.addEventListener('click', close);
  modal.addEventListener('click', function (e) { if (e.target === modal) close(); });
  document.addEventListener('keydown', function (e) { if (e.key === 'Escape' && !modal.hidden) close(); });

  // Ticking goes through the dialog; unticking is always allowed. Capture phase, so it also
  // covers forms added later and runs before a frontend's own handlers.
  document.addEventListener('click', function (e) {
    var el = e.target instanceof Element ? e.target : null;
    if (!el) return;
    var box = el.closest(TOS);
    if (box) {
      if (!box.checked || accepted) return;
      e.preventDefault();
      open(box);
      return;
    }
    // The Rules / Terms links next to the checkbox open the dialog too
    var link = el.closest('a[href="/rules"], a[href="/terms"]');
    var form = link && link.closest('form');
    var formBox = form && form.querySelector(TOS);
    if (formBox && !accepted) {
      e.preventDefault();
      e.stopPropagation();
      open(formBox);
    }
  }, true);

  // A register form sent with the box unticked: read first
  document.addEventListener('submit', function (e) {
    var form = e.target instanceof HTMLFormElement ? e.target : null;
    var box = form && form.querySelector(TOS);
    if (!box || box.checked) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    open(box);
  }, true);
})();
