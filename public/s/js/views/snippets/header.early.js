(function(){var h=document.documentElement;h.classList.remove('no-js','blur-sfw-active','blur-nsfw-active','blur-nsfl-active','blur-untagged-active','hide-item-ratings-active','hide-infobox-active');if(localStorage.getItem('hideItemRatings')==='true')h.classList.add('hide-item-ratings-active');if(localStorage.getItem('hideInfobox')==='true')h.classList.add('hide-infobox-active');if(localStorage.getItem('hideNavbar')==='true')h.classList.add('hide-navbar-active');if(localStorage.getItem('blurNsfw')==='true')h.classList.add('blur-nsfw-active');if(localStorage.getItem('blurNsfl')==='true')h.classList.add('blur-nsfl-active');if(localStorage.getItem('blurSfw')==='true')h.classList.add('blur-sfw-active');if(localStorage.getItem('blurUntagged')==='true')h.classList.add('blur-untagged-active');if(localStorage.getItem('blurDetail')!=='false')h.classList.add('blur-detail-active');if(localStorage.getItem('imageExpandOnClick')!=='false')h.classList.add('image-expand-active');var ts=localStorage.getItem('scroller_thumb_size');if(ts&&['s','m','l','xl'].includes(ts))h.dataset.thumbSize=ts;})();
// Attach the CSRF token to every same-origin state-changing fetch that doesn't already carry one.
(function(){
  var orig = window.fetch;
  if (!orig || orig.__csrf) return;
  var SAFE = { GET: 1, HEAD: 1, OPTIONS: 1 };
  function token() {
    var s = window.f0ckSession && window.f0ckSession.csrf_token;
    if (s) return s;
    var m = document.querySelector('meta[name="csrf-token"]');
    return m ? m.content : '';
  }
  var wrapped = function(input, init) {
    try {
      var isReq = typeof Request !== 'undefined' && input instanceof Request;
      var method = String((init && init.method) || (isReq ? input.method : 'GET')).toUpperCase();
      var url = new URL(isReq ? input.url : String(input), location.href);
      if (!SAFE[method] && url.origin === location.origin) {
        var t = token();
        if (t) {
          var headers = new Headers((init && init.headers) || (isReq ? input.headers : undefined));
          if (!headers.has('X-CSRF-Token')) {
            headers.set('X-CSRF-Token', t);
            init = Object.assign({}, init, { headers: headers });
          }
        }
      }
    } catch (e) {}
    return orig.call(this, input, init);
  };
  wrapped.__csrf = true;
  window.fetch = wrapped;
})();
