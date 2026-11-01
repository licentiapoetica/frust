(() => {
  const searchInput = document.getElementById('chan-catalog-search');
  const searchClear = document.getElementById('chan-search-clear');
  const sortSelect = document.getElementById('chan-sort-select');
  const grid = document.getElementById('chan-catalogue-grid');
  const noResults = document.getElementById('chan-no-results');
  if (!grid) return;

  const originalCards = Array.from(grid.querySelectorAll('.chan-thread-card'));

  function filterAndSort() {
    const q = (searchInput?.value || '').trim().toLowerCase();
    const sort = sortSelect?.value || 'bump';

    if (searchClear) {
      searchClear.style.display = q ? 'block' : 'none';
    }

    let visibleCards = originalCards.filter(card => {
      if (!q) return true;
      const no = card.dataset.no || '';
      const text = card.textContent.toLowerCase();
      return no.includes(q) || text.includes(q);
    });

    // Sort
    visibleCards.sort((a, b) => {
      const stickyA = parseInt(a.dataset.sticky || 0, 10);
      const stickyB = parseInt(b.dataset.sticky || 0, 10);
      if (stickyA !== stickyB) return stickyB - stickyA; // Stickies always first

      if (sort === 'replies') {
        return parseInt(b.dataset.replies || 0, 10) - parseInt(a.dataset.replies || 0, 10);
      } else if (sort === 'images') {
        return parseInt(b.dataset.images || 0, 10) - parseInt(a.dataset.images || 0, 10);
      } else if (sort === 'date-new') {
        return parseInt(b.dataset.time || 0, 10) - parseInt(a.dataset.time || 0, 10);
      } else if (sort === 'date-old') {
        return parseInt(a.dataset.time || 0, 10) - parseInt(b.dataset.time || 0, 10);
      }
      // Bump order: preserve original order
      return originalCards.indexOf(a) - originalCards.indexOf(b);
    });

    originalCards.forEach(c => c.style.display = 'none');
    visibleCards.forEach(c => {
      c.style.display = 'flex';
      grid.appendChild(c);
    });

    if (noResults) {
      noResults.style.display = visibleCards.length === 0 ? 'block' : 'none';
    }
  }

  if (searchInput) {
    searchInput.addEventListener('input', filterAndSort);
  }
  if (searchClear) {
    searchClear.addEventListener('click', () => {
      searchInput.value = '';
      filterAndSort();
      searchInput.focus();
    });
  }
  if (sortSelect) {
    sortSelect.addEventListener('change', filterAndSort);
  }
})();
