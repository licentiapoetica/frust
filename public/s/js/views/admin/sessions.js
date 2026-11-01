F0ckActions.define({ deleteSession: (el, e, id) => deleteSession(id, el) });

async function deleteSession(id, el) {
  if (!confirm('Are you sure you want to delete this session?')) return;

  try {
    const res = await fetch('/api/v2/admin/sessions/delete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id })
    });
    const data = await res.json();

    if (data.success) {
      // Remove the card
      const card = el.closest('.session-card');
      card.style.opacity = '0';
      setTimeout(() => card.remove(), 200);
    } else {
      alert(data.msg || 'Failed to delete session');
    }
  } catch (err) {
    console.error(err);
    alert('An error occurred');
  }
}
