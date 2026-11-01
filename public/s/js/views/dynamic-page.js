// Renders the admin-edited Markdown of /terms, /rules and /about. The page name comes from the
// script tag's data-page (ids: <page>-raw-data, <page>-dynamic-content).
(function() {
  var page = document.currentScript.dataset.page;
  var raw = document.getElementById(page + '-raw-data');
  var el  = document.getElementById(page + '-dynamic-content');
  function escapeHtml(str) {
    return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;');
  }
  function sanitizeHtml(html) {
    var tmp = document.createElement('div');
    tmp.innerHTML = html;
    tmp.querySelectorAll('script,iframe,object,embed,form,input,button,select,meta,link,base,style').forEach(function(el) { el.remove(); });
    tmp.querySelectorAll('*').forEach(function(node) {
      Array.from(node.attributes).forEach(function(attr) {
        if (/^on/i.test(attr.name) || /^data-(on|args?)$/i.test(attr.name) || (attr.name === 'href' && /^javascript:/i.test(attr.value.trim()))) {
          node.removeAttribute(attr.name);
        }
      });
    });
    return tmp.innerHTML;
  }
  function render() {
    if (raw && el && typeof marked !== 'undefined') {
      var bytes = Uint8Array.from(atob(raw.textContent.trim()), function(c) { return c.charCodeAt(0); });
      var text = new TextDecoder('utf-8').decode(bytes);
      var renderer = new marked.Renderer();
      renderer.code = function(code, lang) {
        var escaped = escapeHtml(typeof code === 'object' ? (code.text || '') : code);
        var langAttr = (typeof code === 'object' ? code.lang : lang) || '';
        return '<pre><code' + (langAttr ? ' class="language-' + escapeHtml(langAttr) + '"' : '') + '>' + escaped + '</code></pre>';
      };
      renderer.codespan = function(code) {
        var escaped = escapeHtml(typeof code === 'object' ? (code.text || '') : code);
        return '<code>' + escaped + '</code>';
      };
      el.innerHTML = sanitizeHtml(marked.parse(text, { gfm: true, breaks: true, renderer: renderer }));
    }
  }
  if (typeof marked !== 'undefined') {
    render();
  } else {
    document.addEventListener('DOMContentLoaded', render);
  }
})();
