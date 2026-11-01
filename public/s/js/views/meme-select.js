document.querySelectorAll('.category-chip').forEach(chip => {
    chip.addEventListener('click', () => {
        const category = chip.getAttribute('data-category');

        // Update active chip
        document.querySelectorAll('.category-chip').forEach(c => c.classList.remove('active'));
        chip.classList.add('active');

        // Filter grid
        document.querySelectorAll('.template-item').forEach(item => {
            if (category === 'All' || item.getAttribute('data-category') === category) {
                item.style.display = 'flex';
            } else {
                item.style.display = 'none';
            }
        });
    });
});
