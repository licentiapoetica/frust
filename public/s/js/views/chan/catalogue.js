// 4chan catalogue (views/chan/catalogue.html). Without JavaScript the filter and sort are a GET form
// the server applies, and the board field goes through /4/catalogue?board=. Here the filter and sort
// apply in place as you type (the URL follows with replaceState), and a board opens without a page
// load. Runs again after every in-place navigation to a catalogue: the page's own elements are new,
// so the listeners go on them, not on the document.
(() => {
  'use strict';

  const main = document.querySelector('#main.chan-catalogue-page');
  const grid = document.getElementById('chan-catalogue-grid');
  if (!main || !grid || grid.dataset.bound) return;
  grid.dataset.bound = '1';

  const controls = document.getElementById('chan-catalog-controls');
  const searchInput = document.getElementById('chan-catalog-search');
  const searchClear = document.getElementById('chan-search-clear');
  const sortSelect = document.getElementById('chan-sort-select');
  const noResults = document.getElementById('chan-no-results');
  const boardForm = main.querySelector('.chan-board-custom-form');
  const cards = Array.from(grid.querySelectorAll('.chan-thread-card'));
  const num = (card, key) => parseInt(card.dataset[key] || '0', 10) || 0;

  const navigate = (url) => {
    if (typeof window.loadPageAjax === 'function') window.loadPageAjax(url, true);
    else window.location.href = url;
  };

  function apply() {
    const q = (searchInput ? searchInput.value : '').trim().toLowerCase();
    const sort = sortSelect ? sortSelect.value : 'bump';

    if (searchClear) searchClear.classList.toggle('is-shown', !!q);

    let shown = 0;
    cards.forEach((card) => {
      const hit = !q || (card.dataset.no || '').includes(q) || card.textContent.toLowerCase().includes(q);
      card.classList.toggle('is-hidden', !hit);
      if (hit) shown++;
    });

    const sorted = cards.slice().sort((a, b) => {
      const sticky = num(b, 'sticky') - num(a, 'sticky');
      if (sticky) return sticky; // stickies always first
      switch (sort) {
        case 'replies': return num(b, 'replies') - num(a, 'replies');
        case 'images': return num(b, 'images') - num(a, 'images');
        case 'date-new': return num(b, 'time') - num(a, 'time');
        case 'date-old': return num(a, 'time') - num(b, 'time');
        default: return num(a, 'bump') - num(b, 'bump');
      }
    });
    sorted.forEach((card) => grid.appendChild(card));

    if (noResults) noResults.classList.toggle('is-shown', shown === 0);

    // The address keeps the filter, so reloading or sharing it shows the same list
    const params = new URLSearchParams();
    if (q) params.set('q', searchInput.value.trim());
    if (sort !== 'bump') params.set('sort', sort);
    const qs = params.toString();
    history.replaceState(history.state, '', location.pathname + (qs ? '?' + qs : ''));
  }

  if (searchInput) searchInput.addEventListener('input', apply);
  if (sortSelect) sortSelect.addEventListener('change', apply);
  if (controls) {
    controls.addEventListener('submit', (e) => {
      e.preventDefault();
      apply();
    });
  }
  if (searchClear) {
    searchClear.addEventListener('click', (e) => {
      e.preventDefault();
      searchInput.value = '';
      apply();
      searchInput.focus();
    });
  }

  if (boardForm) {
    boardForm.addEventListener('submit', (e) => {
      e.preventDefault();
      const input = boardForm.querySelector('input[name="board"]');
      const board = (input ? input.value : '').trim().replace(/^\/+|\/+$/g, '').toLowerCase();
      if (!/^[a-z0-9]{1,12}$/.test(board)) {
        if (typeof window.showFlash === 'function') window.showFlash('Board names are letters and numbers', 'error');
        return;
      }
      navigate('/4/' + board + '/catalogue');
    });
  }
})();
