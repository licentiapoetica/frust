// Defined before the grid so thumbnail error handlers are registered before any image can fail
F0ckActions.define({
  chanThumbDim: (el) => { el.style.opacity = '0.2'; },
  chanGoToBoard: () => {
    const b = document.getElementById('custom-board-input').value.trim().toLowerCase();
    if (b) window.location.href = '/4/' + b + '/catalogue';
  },
});
