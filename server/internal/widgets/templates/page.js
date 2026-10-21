(function () {
  'use strict';
  const pad = n => String(n).padStart(2, '0');
  const widgets = document.querySelectorAll('.w');

  function tickClock() {
    const d = new Date();
    document.querySelectorAll('.clock-time').forEach(e => e.textContent = pad(d.getHours()) + ':' + pad(d.getMinutes()));
    document.querySelectorAll('.clock-date').forEach(e => e.textContent =
      d.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' }));
    setTimeout(tickClock, 60000 - (d.getSeconds() * 1000 + d.getMilliseconds()) + 50);
  }
  tickClock();

  document.querySelectorAll('.w-photo').forEach(sec => {
    const img = sec.querySelector('img');
    img.onerror = () => { img.style.opacity = 0; };
    img.onload = () => { img.style.opacity = 1; };
    img.src = '/api/data/photo/next?frame=' + encodeURIComponent(TF.frame) + '&t=' + Date.now();
    const every = Math.max(10, Number(img.dataset.interval || 600)) * 1000;
    function next() {
      const n = new Image();
      n.onload = () => { img.style.opacity = 0; setTimeout(() => { img.src = n.src; img.style.opacity = 1; }, 300); };
      n.src = '/api/data/photo/next?frame=' + encodeURIComponent(TF.frame) + '&t=' + Date.now();
    }
    setInterval(next, every);
  });

  function connect() {
    const es = new EventSource('/api/stream/' + encodeURIComponent(TF.frame));
    es.onmessage = ev => {
      let m; try { m = JSON.parse(ev.data); } catch { return; }
      if (m.reload || (m.page_version && m.page_version !== TF.pageVersion)) { location.reload(); return; }
      if (!m.widget) return;
      const sec = document.getElementById('w-' + m.widget);
      if (!sec) return;
      for (const [k, v] of Object.entries(m.data || {})) {
        const el = sec.querySelector('[data-field="' + k + '"]');
        if (el) el.textContent = v;
      }
    };
    es.onerror = () => { es.close(); setTimeout(connect, 5000); };
  }
  connect();

  // Touch to paint: from when the input reached WebKit (e.timeStamp, or now if that is on another
  // clock) until the frame after the highlight was drawn. painted_at_ms is the frame's wall clock
  // then, so `tfctl bench` can measure from the moment it injected the tap.
  addEventListener('pointerdown', e => {
    const now = performance.now();
    const t0 = e.timeStamp > 0 && e.timeStamp <= now && now - e.timeStamp < 10000 ? e.timeStamp : now;
    const sec = e.target.closest('.w');
    if (sec) sec.classList.add('touched');
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const end = performance.now();
      navigator.sendBeacon('/api/frames/' + encodeURIComponent(TF.frame) + '/beacon', JSON.stringify({
        latency_ms: Math.max(1, Math.round(end - t0)), widget: sec ? sec.id : '',
        painted_at_ms: Math.round(performance.timeOrigin + end) }));
    }));
  }, { passive: true });
  const clear = () => widgets.forEach(w => w.classList.remove('touched'));
  addEventListener('pointerup', clear, { passive: true });
  addEventListener('pointercancel', clear, { passive: true });

  // Liveness for the server's hung-page check (api/watchdog.go): stops if this WebProcess hangs.
  const alive = () => navigator.sendBeacon('/api/frames/' + encodeURIComponent(TF.frame) + '/alive', '');
  alive();
  setInterval(alive, 30000);

  window.toast = t => { const el = document.getElementById('toast'); el.textContent = t; el.classList.add('show'); setTimeout(() => el.classList.remove('show'), 2500); };
})();
