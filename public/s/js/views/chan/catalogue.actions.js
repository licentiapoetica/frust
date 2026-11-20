// Defined before the grid so thumbnail error handlers are registered before any image can fail
F0ckActions.define({
  chanThumbDim: (el) => { el.classList.add('is-broken'); },
});
