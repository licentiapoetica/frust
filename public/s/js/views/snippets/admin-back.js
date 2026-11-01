(() => {
  // Breadcrumb = the page's own heading (first h1/h2 in #main). The rest of the page isn't in the
  // DOM yet when this runs (parser / AJAX insert), so wait until it is.
  const fill = () => {
    const crumb = document.querySelector('#main .adm-back-crumb');
    const main = document.getElementById('main');
    if (!crumb || !main) return;
    const heading = Array.from(main.querySelectorAll('h1, h2')).find(h => !h.closest('.adm-back') && h.textContent.trim());
    if (heading) crumb.textContent = heading.innerText.trim().slice(0, 60); // innerText already collapses whitespace (no regex: the template engine strips backslashes)
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', fill, { once: true });
  else setTimeout(fill, 0);
})();
