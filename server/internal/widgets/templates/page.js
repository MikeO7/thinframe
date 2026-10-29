(function () {
  'use strict';
  const pad = n => String(n).padStart(2, '0');
  // Errors are kept for tests and debug reports (TF.errors), not only the console.
  TF.errors = [];
  function fail(m) { if (TF.errors.length < 20) TF.errors.push(String(m)); console.error(m); }
  addEventListener('error', e => fail((e.message || 'error') + ' @' + (e.lineno || 0)));
  const widgets = document.querySelectorAll('.w');

  function hm(d, h12) {
    if (h12) return ((d.getHours() + 11) % 12 + 1) + ':' + pad(d.getMinutes());
    return pad(d.getHours()) + ':' + pad(d.getMinutes());
  }

  function paintClock() {
    const d = new Date();
    // Before 2024 the frame's clock is not set yet (a power cut at boot: no RTC battery): show no
    // time rather than a wrong one. tickClock paints it within a minute of the clock being set.
    const set = d.getFullYear() >= 2024;
    document.querySelectorAll('.clock-time').forEach(e => e.textContent = set ? hm(d, e.dataset.h12 === 'true') : '--:--');
    document.querySelectorAll('.clock-date').forEach(e => e.textContent = !set ? '' :
      d.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' }));
    rerenderTimed();
    widgets.forEach(markAge);
    return d;
  }
  function tickClock() {
    const d = paintClock();
    setTimeout(tickClock, 60000 - (d.getSeconds() * 1000 + d.getMilliseconds()) + 50);
  }

  document.querySelectorAll('.w-photo').forEach(sec => {
    let img = sec.querySelector('img');
    img.onerror = () => { img.style.opacity = 0; };
    img.onload = () => { img.style.opacity = 1; };
    img.src = '/api/data/photo/next?frame=' + encodeURIComponent(TF.frame) + '&t=' + Date.now();
    const every = Math.max(10, Number(img.dataset.interval || 600)) * 1000;
    // Show the preloaded element itself: each request advances the rotation (no-store), so
    // reassigning img.src would fetch and decode a second photo and skip the first.
    function next() {
      const n = new Image();
      n.alt = ''; n.style.opacity = 0;
      n.onload = () => {
        img.style.opacity = 0;
        setTimeout(() => { img.replaceWith(n); img = n; void n.offsetWidth; n.style.opacity = 1; }, 300);
      };
      n.src = '/api/data/photo/next?frame=' + encodeURIComponent(TF.frame) + '&t=' + Date.now();
    }
    setInterval(next, every);
  });

  // ---- data-fed widgets ---------------------------------------------------------------------
  // Each widget keeps the last data it was sent (merged across messages) and is redrawn from
  // props + data. Data never reaches innerHTML: text goes through textContent, images only take
  // http(s) URLs or asset:<name> (files the server owner put in DATA_DIR/assets).

  const ICONS = {
    shield: '<path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z"/>',
    lock: '<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
    unlock: '<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V8a4 4 0 0 1 7.5-2"/>',
    bulb: '<path d="M12 3a6 6 0 0 1 4 10.5V16H8v-2.5A6 6 0 0 1 12 3z"/><path d="M9.5 19h5M10.5 21h3"/>',
    therm: '<path d="M10 14V5a2 2 0 0 1 4 0v9a4 4 0 1 1-4 0z"/>',
    box: '<path d="M3 7l9-4 9 4v10l-9 4-9-4z"/><path d="M3 7l9 4 9-4M12 11v10"/>',
    car: '<path d="M3 16v-3l2-5h14l2 5v3"/><path d="M3 16h18"/><circle cx="7.5" cy="17" r="1.8"/><circle cx="16.5" cy="17" r="1.8"/>',
    garage: '<path d="M3 21V9l9-6 9 6v12"/><path d="M7 21v-8h10v8M7 16h10"/>',
    window: '<rect x="4" y="3" width="16" height="18" rx="1.5"/><path d="M4 12h16M12 3v18"/>',
    door: '<path d="M6 21V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v17M4 21h16"/><circle cx="15" cy="12" r=".8"/>',
    wash: '<rect x="4" y="3" width="16" height="18" rx="2"/><circle cx="12" cy="13" r="4.5"/><path d="M7 6.5h2"/>',
    drop: '<path d="M12 3s6 6.5 6 11a6 6 0 0 1-12 0c0-4.5 6-11 6-11z"/>',
    wind: '<path d="M3 8h11a3 3 0 1 0-3-3M3 12h15a3 3 0 1 1-3 3M3 16h8"/>',
    fan: '<circle cx="12" cy="12" r="1.6"/><path d="M12 10.4C11 6 13 3 15.5 4.5S15 10 12 10.4zM13.6 12c4.4-1 7.4 1 5.9 3.5S14 15 13.6 12zM12 13.6c1 4.4-1 7.4-3.5 5.9S9 14 12 13.6zM10.4 12C6 13 3 11 4.5 8.5S10 9 10.4 12z"/>',
    msg: '<path d="M4 5h16v11H9l-5 4z"/>',
    bolt: '<path d="M13 2 4 14h7l-1 8 9-12h-7z"/>',
    person: '<circle cx="12" cy="8" r="4"/><path d="M4 21c1-4 4.5-6 8-6s7 2 8 6"/>',
    bell: '<path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15z"/><path d="M10 20.5a2 2 0 0 0 4 0"/>',
    cam: '<rect x="3" y="6" width="13" height="12" rx="2"/><path d="M16 10l5-3v10l-5-3"/>',
    leaf: '<path d="M5 19C5 9 11 4 20 4c0 9-5 15-15 15zM5 19l7-7"/>',
    dot: '<circle cx="12" cy="12" r="3"/>',
    frunk: '<path d="M2.5 16.5v-2.3c0-.5.4-.9.9-.9h5L11 9.6c.4-.5 1-.8 1.6-.8h3.3c.7 0 1.4.3 1.8.9l2.2 2.8c.9.3 1.6 1.1 1.6 2.1v1.9"/><path d="M8.4 13.3 3.6 9.9"/><path d="M8.9 16.5h6.2"/><circle cx="7" cy="16.5" r="1.9"/><circle cx="17" cy="16.5" r="1.9"/>',
    trunk: '<path d="M2.5 16.5v-1.9c0-1 .7-1.8 1.6-2.1l4.3-1L11 8.9c.4-.4 1-.6 1.6-.6h4.3"/><path d="M16.9 8.3 21.4 6"/><path d="M19.5 12.4h.9c.6 0 1.1.5 1.1 1.1v3"/><path d="M8.9 16.5h6.2"/><circle cx="7" cy="16.5" r="1.9"/><circle cx="17" cy="16.5" r="1.9"/>',
    plug: '<path d="M8 3v4M13 3v4"/><path d="M5.5 7h10v3.5a5 5 0 0 1-10 0z"/><path d="M10.5 15.5v2a3 3 0 0 0 3 3h2a3 3 0 0 0 3-3V8.5"/>',
    sun: '<circle cx="12" cy="12" r="4.2"/><path d="M12 2.5v2.2M12 19.3v2.2M2.5 12h2.2M19.3 12h2.2M5.3 5.3l1.6 1.6M17.1 17.1l1.6 1.6M5.3 18.7l1.6-1.6M17.1 6.9l1.6-1.6"/>',
    partsun: '<path d="M9 3v1.6M3.6 9H2M4.8 4.8l1.1 1.1M13.2 4.8l-1.1 1.1"/><path d="M5.6 11.5A3.6 3.6 0 0 1 12.3 7.6"/><path d="M8 20h10a3.5 3.5 0 0 0 .4-7 5 5 0 0 0-9.6-.8A3.9 3.9 0 0 0 8 20z"/>',
    cloud: '<path d="M7 19h10.5a4 4 0 0 0 .5-8 6 6 0 0 0-11.4-1A4.5 4.5 0 0 0 7 19z"/>',
    fog: '<path d="M7 13h10.5a3.5 3.5 0 0 0 .4-7A5.5 5.5 0 0 0 7.4 5.6 3.8 3.8 0 0 0 7 13z"/><path d="M4 17h16M7 21h10"/>',
    rain: '<path d="M7 14h10.5a3.5 3.5 0 0 0 .4-7A5.5 5.5 0 0 0 7.4 6.6 3.8 3.8 0 0 0 7 14z"/><path d="M8 17l-1 3M12.5 17l-1 3M17 17l-1 3"/>',
    snow: '<path d="M7 14h10.5a3.5 3.5 0 0 0 .4-7A5.5 5.5 0 0 0 7.4 6.6 3.8 3.8 0 0 0 7 14z"/><path d="M8 18h.01M12 20h.01M16 18h.01M10 22h.01M14 22h.01"/>',
    moon: '<path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"/>',
    storm: '<path d="M7 14h10.5a3.5 3.5 0 0 0 .4-7A5.5 5.5 0 0 0 7.4 6.6 3.8 3.8 0 0 0 7 14z"/><path d="M12.5 15l-2 3.5h3l-2 3.5"/>'
  };
  // WMO weather code -> icon (open-meteo).
  function wxIcon(c, night) {
    c = Number(c);
    if (c <= 1) return night ? 'moon' : 'sun';
    if (c === 2) return night ? 'cloud' : 'partsun';
    if (c === 3) return 'cloud';
    if (c === 45 || c === 48) return 'fog';
    if ((c >= 71 && c <= 77) || c === 85 || c === 86) return 'snow';
    if (c >= 95) return 'storm';
    return 'rain';
  }
  const SVGNS = 'http://www.w3.org/2000/svg';

  function h(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined && text !== null) e.textContent = String(text);
    return e;
  }
  // Each icon is parsed once and cloned after: redraws do not re-run the SVG parser.
  const iconCache = {};
  function icon(name, cls) {
    const k = ICONS[name] ? name : 'dot';
    if (!iconCache[k]) {
      const s = document.createElementNS(SVGNS, 'svg');
      s.setAttribute('viewBox', '0 0 24 24');
      s.innerHTML = ICONS[k]; // constant table, never data
      iconCache[k] = s;
    }
    const c = iconCache[k].cloneNode(true);
    c.setAttribute('class', 'ic ' + (cls || '') + ' i-' + k);
    return c;
  }
  // Remote pictures come through the server, shrunk to about the width shown (api/images.go):
  // a full-size poster costs the A33 seconds and tens of MB to decode.
  function src(v, w) {
    if (typeof v !== 'string') return '';
    if (/^https?:\/\//.test(v)) return '/api/data/img?w=' + (w || 640) + '&u=' + encodeURIComponent(v);
    if (/^asset:[A-Za-z0-9._-]+$/.test(v)) return '/api/data/asset/' + encodeURIComponent(v.slice(6));
    return '';
  }
  function img(v, cls, w) {
    const u = src(v, w);
    if (!u) return null;
    const i = h('img', cls); i.alt = ''; i.decoding = 'async'; i.src = u; i.onerror = () => { i.remove(); };
    return i;
  }
  // Drop items from the end of box until it fits, and say how many: a list cut in half mid-row
  // looks broken. Needs the box laid out, so call it at the end of a draw (draws are visible).
  // Hide items until `box` fits, with a "+N more" line: items marked data-past first (what
  // already aired today), then from the end.
  function fit(box, sel) {
    const items = () => box.querySelectorAll(':scope > ' + sel);
    const over = () => box.scrollHeight > box.clientHeight + 1;
    let hid = 0, more = null;
    while (over()) {
      const l = items();
      if (l.length <= 1) break;
      const past = box.querySelector(':scope > ' + sel + '[data-past]');
      (past || l[l.length - 1]).remove(); hid++;
      if (!more) { more = h('div', 'more'); box.append(more); }
      more.textContent = '+' + hid + ' more';
    }
  }
  // "6:00 PM" on `day` is more than an hour before `now`.
  function aired(day, t, now) {
    const m = /^(\d{1,2}):(\d{2})\s*(AM|PM)?/i.exec(t || '');
    if (!m) return false;
    let hr = +m[1] % 12;
    if (m[3] && m[3].toUpperCase() === 'PM') hr += 12;
    if (!m[3]) hr = +m[1];
    const at = new Date(day.getFullYear(), day.getMonth(), day.getDate(), hr, +m[2]);
    return now - at > 3600e3;
  }
  function st(s) { return ['ok', 'warn', 'bad', 'off'].includes(s) ? s : ''; }
  function badge(sec, b) {
    const e = sec.querySelector(':scope > .between .badge, :scope > .badge');
    if (!e) return;
    if (!b || !b.text) { e.hidden = true; return; }
    e.hidden = false; e.textContent = b.text; e.className = 'badge ' + st(b.state);
  }
  function body(sec) { const b = sec.querySelector('.body'); b.textContent = ''; sec.classList.remove('nodata'); return b; }
  function bar(pct, color) {
    const b = h('div', 'bar'), i = h('i');
    i.style.width = Math.max(0, Math.min(100, Number(pct) || 0)) + '%';
    if (color) i.style.background = color;
    b.append(i); return b;
  }
  function arr(v) { return Array.isArray(v) ? v : []; }
  // Home Assistant's default area id for a name: "Living Room" -> "living_room".
  function slug(v) { return String(v || '').toLowerCase().normalize('NFKD').replace(/[^a-z0-9]+/g, '_').replace(/^_|_$/g, ''); }
  function num(v) { return typeof v === 'number' && isFinite(v) ? v : null; }
  // merge static entries from props with live ones from data, matched by id or name.
  function merge(stat, live) {
    const out = arr(stat).map(x => Object.assign({}, x));
    const key = x => x.id || x.name || x.label;
    for (const l of arr(live)) {
      const k = key(l);
      const m = k && out.find(x => key(x) === k);
      if (m) Object.assign(m, l); else out.push(Object.assign({}, l));
    }
    return out;
  }
  function tile(label, value, note, ic, state) {
    const t = h('div', 'tile');
    if (ic) t.append(icon(ic));
    const c = h('div', 't ellip', label);
    if (note) c.append(h('small', 'ellip', note));
    t.append(c);
    if (value !== undefined && value !== null && value !== '') t.append(h('span', 'v' + (state ? ' s-' + st(state) : ''), value));
    return t;
  }
  // An empty card says what it waits for: plain words first, the owner's hint small below.
  function empty(b, text, hint) {
    const sec = b.closest('.w'), src = props(sec).source;
    sec.classList.add('nodata');
    const e = h('div', 'empty'); e.append(h('div', '', text || 'Nothing to show yet'));
    hint = hint || (src && !text ? 'Waiting for the \u201c' + src + '\u201d source' : '');
    if (hint) e.append(h('small', '', hint));
    b.append(e);
  }
  // Hero badge from a services feed when nothing else sets one: "22 / 22 containers healthy".
  function health(d, p) {
    if (!p.health || !Array.isArray(d.containers)) return null;
    const ok = d.containers.filter(c => c.state === 'healthy' || c.state === 'running').length, n = d.containers.length;
    return { text: ok + ' / ' + n + ' containers ' + (ok === n ? 'healthy' : 'up'), state: ok === n ? 'ok' : 'warn' };
  }

  // Events: [{start, end, title, where}], ISO times.
  function events(d) {
    return arr(d.events).map(e => Object.assign({}, e, { s: new Date(e.start), e: new Date(e.end || e.start) }))
      .filter(e => !isNaN(e.s)).sort((a, b) => a.s - b.s);
  }
  function mins(a, b) { return Math.round((b - a) / 60000); }
  function inText(m) { return m < 60 ? m + ' min' : Math.floor(m / 60) + ' h' + (m % 60 ? ' ' + (m % 60) + ' min' : ''); }

  const R = {
    hero(sec, d, p) {
      badge(sec, d.badge || health(d, p));
      const wx = sec.querySelector('.hero-wx'), has = d.temp !== undefined && d.temp !== '';
      wx.hidden = !has;
      if (has) {
        const ic = sec.querySelector('.wx-ic'); ic.textContent = ''; ic.append(icon(wxIcon(d.code, d.night), 'wx'));
        const sub = [d.desc ? d.desc[0].toUpperCase() + d.desc.slice(1) : '', num(d.hi) !== null ? 'H ' + d.hi + '°  L ' + d.lo + '°' : ''];
        sec.querySelector('.hero-sub').textContent = sub.filter(Boolean).join(' · ');
      }
      const c = sec.querySelector('.chips'); c.textContent = '';
      for (const x of arr(d.chips)) { const e = h('span', 'chip'); e.append(icon(x.icon), h('span', '', x.text)); c.append(e); }
    },
    forecast(sec, d, p) {
      const b = body(sec), days = arr(d.days).slice(0, num(p.days) || 7);
      if (!days.length) { empty(b, 'No forecast yet'); return; }
      const lo = Math.min(...days.map(x => x.lo)), hi = Math.max(...days.map(x => x.hi)), span = Math.max(hi - lo, 1);
      const wide = sec.clientWidth > sec.clientHeight * 1.9;
      const l = h('div', 'fc' + (wide ? ' wide' : ''));
      days.forEach((x, i) => {
        const r = h('div', 'fd' + (i === 0 ? ' today' : ''));
        const dt = new Date(String(x.date) + 'T12:00:00');
        r.append(h('span', 'dn', i === 0 ? 'Today' : dt.toLocaleDateString(undefined, { weekday: 'short' })), icon(wxIcon(x.code), 'wx'));
        const rg = h('span', 'rg'), f = h('i');
        f.style.left = ((x.lo - lo) / span * 100) + '%'; f.style.right = ((hi - x.hi) / span * 100) + '%';
        rg.append(f);
        r.append(h('span', 'lo', x.lo + '°'), rg, h('span', 'hi', x.hi + '°'));
        l.append(r);
      });
      b.append(l);
    },
    agenda(sec, d, p) {
      const b = body(sec), now = new Date(), evs = events(d);
      const day = evs.filter(e => e.s.toDateString() === now.toDateString());
      const next = evs.find(e => e.s > now);
      const note = sec.querySelector('.note');
      if (note) note.textContent = day.length ? day.length + (day.length === 1 ? ' event' : ' events') + (next ? ' · next in ' + inText(mins(now, next.s)) : '') : '';
      const view = p.view || 'list';
      if (!d.events) { empty(b); return; }
      if (view === 'next') {
        if (!next) { empty(b, 'Nothing else today'); return; }
        b.append(h('div', 'nx-title ellip', next.title), h('div', 'nx-when ellip', hm(next.s, true) + ' – ' + hm(next.e, true) + (next.where ? ' · ' + next.where : '')));
        const m = mins(now, next.s), n = h('div', 'big nx-in', m < 60 ? m : Math.floor(m / 60));
        n.append(h('span', 'unit', m < 60 ? ' min' : ' h' + (m % 60 ? ' ' + (m % 60) + ' min' : ''))); b.append(n);
        return;
      }
      if (view === 'day') {
        const from = num(p.from) ?? 8, to = num(p.to) ?? 18, span = to - from;
        const tl = h('div', 'tl');
        for (let k = from; k <= to; k++) { const r = h('div', 'hr'); r.style.top = ((k - from) / span * 100) + '%'; r.append(h('span', '', ((k + 11) % 12 + 1) + (k < 12 ? ' AM' : ' PM'))); tl.append(r); }
        for (const e of day) {
          const a = e.s.getHours() + e.s.getMinutes() / 60, z = e.e.getHours() + e.e.getMinutes() / 60;
          if (z <= from || a >= to) continue;
          const ev = h('div', 'ev' + (e === next ? ' next' : ''));
          ev.style.top = (Math.max(a, from) - from) / span * 100 + '%';
          ev.style.height = (Math.min(z, to) - Math.max(a, from)) / span * 100 + '%';
          ev.append(h('b', 'ellip', e.title)); if (e.where) ev.append(h('small', 'ellip', e.where));
          tl.append(ev);
        }
        const t = now.getHours() + now.getMinutes() / 60;
        if (t >= from && t <= to) { const n = h('div', 'now'); n.style.top = (t - from) / span * 100 + '%'; tl.append(n); }
        b.append(tl); return;
      }
      const list = h('div', 'ag'), src = p.all ? evs : day, max = num(p.max) || 5;
      // What is still ahead first: past events fill the list only when fewer remain than fit,
      // so at 6 PM the card shows the evening, not the morning (QA 2026-10-28).
      const ahead = src.findIndex(e => e.e >= now), from = ahead < 0 ? Math.max(0, src.length - max) : Math.min(ahead, Math.max(0, src.length - max));
      for (const e of src.slice(from, from + max)) {
        const r = h('div', 'e' + (e.e < now ? ' past' : ''));
        r.append(h('span', 'faint', hm(e.s, true)));
        const t = h('div', 'ellip', e.title); if (e.where) t.append(h('small', 'ellip', e.where));
        r.append(t); list.append(r);
      }
      if (!list.children.length) empty(b, 'Nothing today'); else b.append(list);
    },
    cars(sec, d, p) {
      const b = body(sec), cars = merge(p.cars, d.cars), compact = p.view === 'compact';
      sec.classList.toggle('full', !compact);
      if (!cars.length) { empty(b); return; }
      const pct = c => { const v = num(c.battery); return v === null ? null : Math.round(Math.max(0, Math.min(100, v))); };
      if (compact) {
        const l = h('div', 'cars-compact');
        for (const c of cars) {
          const r = h('div', 'carrow'); r.append(img(c.image) || icon('car'));
          const t = h('div', 'cr');
          const top = h('div', 'between'); top.append(h('span', 'ellip cn', c.name));
          if (pct(c) !== null) top.append(h('span', 'cp' + (c.charging ? ' s-ok' : ''), pct(c) + '%'));
          t.append(top);
          if (pct(c) !== null) t.append(bar(pct(c), c.charging ? 'var(--ok)' : ''));
          t.append(h('div', 'faint ellip cs', c.status || (pct(c) === null ? c.model || '' : '')));
          r.append(t); l.append(r);
        }
        b.append(l); return;
      }
      const g = h('div', 'cars-full');
      for (const c of cars) {
        const k = h('div', 'car'), top = h('div', 'between'), live = pct(c) !== null;
        const nm = h('div'); nm.append(h('div', 'model', c.model || ''), h('div', 'name ellip', c.name));
        top.append(nm);
        if (live) {
          const bt = h('div', 'cbat'), pr = h('div', 'row');
          const ba = h('span', 'batt' + (c.charging ? ' chg' : '')), bi = h('i'); bi.style.width = pct(c) + '%'; ba.append(bi);
          pr.append(ba); if (c.charging) pr.append(icon('bolt', 'chgi'));
          pr.append(h('span', 'pct' + (c.charging ? ' chg' : ''), pct(c) + '%'));
          bt.append(pr, h('div', 'dim sub', [c.range, c.status].filter(Boolean).join(' · ')));
          top.append(bt);
        }
        const sg = h('div', 'stage'); const ci = img(c.image); if (ci) sg.append(ci);
        k.append(top, sg);
        if (arr(c.stats).length) {
          const ss = h('div', 'stats');
          for (const x of c.stats) { const e = h('div', 'stat', x.label); e.append(h('b', 'ellip', x.value)); ss.append(e); }
          k.append(ss);
        }
        // Climate: one big button. Display-only until car control is wired (docs/decisions.md).
        const cl = c.climate || null, on = !!(cl && cl.on);
        const cb = h('div', 'climb' + (on ? ' on' : '') + (cl ? '' : ' nodata')); cb.dataset.act = 'climate';
        const ring = h('span', 'fanr'); ring.append(icon('fan')); cb.append(ring);
        const tx = h('div', 'ct', on ? 'Climate on' : 'Climate');
        tx.append(h('small', 'ellip', cl ? (cl.note || (on ? 'Running' : 'Off')) : 'No live data'));
        cb.append(tx);
        if (cl && cl.set) cb.append(h('span', 'cset', cl.set));
        k.append(cb);
        const acts = h('div', 'acts');
        const act = (name, ic, label, hot) => { const a = h('div', 'act' + (hot ? ' hot' : '')); a.dataset.act = name; a.append(icon(ic), h('span', '', label)); acts.append(a); };
        const locked = c.locked;
        act('lock', locked === false ? 'unlock' : 'lock', locked === undefined ? 'Lock' : locked ? 'Locked' : 'Unlocked', locked === false);
        act('frunk', 'frunk', c.frunk === 'open' ? 'Frunk open' : 'Frunk', c.frunk === 'open');
        act('trunk', 'trunk', c.trunk === 'open' ? 'Trunk open' : 'Trunk', c.trunk === 'open');
        act('port', 'plug', c.charging ? 'Charging' : c.port === 'open' ? 'Port open' : 'Charge port', !!c.charging || c.port === 'open');
        k.append(acts);
        g.append(k);
      }
      b.append(g);
    },
    rooms(sec, d, p) {
      const b = body(sec), rooms = merge(p.rooms, d.rooms);
      if (!rooms.length) { empty(b); return; }
      const g = h('div', 'rgrid');
      for (const r of rooms.slice(0, 9)) {
        const t = h('div', 'room' + (r.on ? ' lit' : ''));
        t.dataset.room = r.name || ''; t.dataset.area = r.area || slug(r.name); t.dataset.on = r.on ? '1' : '';
        const top = h('div', 'between'); top.append(h('span', 'rn ellip', r.name), h('span', 'sw' + (r.on ? ' on' : ''))); t.append(top);
        const ln = h('div', 'ln'); ln.append(icon('bulb'), h('span', '', r.on ? (r.lights || 'On') : 'Off')); t.append(ln);
        if (r.on && num(r.brightness) !== null) t.append(bar(r.brightness));
        const ft = h('div', 'ft'), th = h('span', 'row');
        th.style.gap = '.6rem'; th.append(icon('therm'), h('span', '', [r.temp, r.humidity].filter(Boolean).join(' · ')));
        ft.append(th); if (r.occupied) { const o = h('span', 'row faint'); o.style.gap = '.6rem'; o.append(h('span', 'dot'), h('span', '', 'Occupied')); ft.append(o); }
        t.append(ft); g.append(t);
      }
      b.append(g);
      const sc = arr(d.scenes).length ? d.scenes : arr(p.scenes);
      if (sc.length) { const s = h('div', 'scenes'); for (const x of sc) s.append(h('div', 'scene' + (x.active ? ' on' : ''), x.name || x)); b.append(s); }
    },
    climate(sec, d) {
      badge(sec, d.badge);
      const b = body(sec), z = arr(d.zones);
      if (!z.length) { empty(b); return; }
      const m = h('div', 'zone-main'); m.append(h('div', 'big', z[0].temp || '—'));
      // Two short lines instead of one that wraps at a random word.
      const t = h('div', 'dim zone-sub');
      t.append(h('div', '', [z[0].name, z[0].target ? 'set ' + z[0].target : ''].filter(Boolean).join(' · ')));
      if (z[0].action) t.append(h('div', '', String(z[0].action).replace(/^./, c => c.toUpperCase())));
      t.style.fontSize = '1.5rem'; m.append(t); b.append(m);
      const zs = h('div', 'zones');
      for (const x of z.slice(1)) zs.append(tile(x.name, [x.temp, x.target ? 'set ' + x.target : '', x.action].filter(Boolean).join(' · ')));
      b.append(zs);
    },
    tiles(sec, d, p) {
      badge(sec, d.badge);
      const b = body(sec), items = merge(p.items, d.items);
      if (!items.length) { empty(b); return; }
      const g = h('div', 'tgrid');
      for (const x of items) { const t = h('div', 'tile'); t.append(h('span', 'lb', x.label), h('b', 'ellip', x.value ?? '—')); if (x.note) t.append(h('span', 's-' + (st(x.state) || 'ok'), x.note)); t.lastChild.style.fontSize = '1.2rem'; g.append(t); }
      b.append(g);
    },
    list(sec, d, p) {
      badge(sec, d.badge);
      const b = body(sec), items = merge(p.items, d.items);
      if (!items.length) { empty(b); return; }
      const l = h('div', 'lst');
      for (const x of items) l.append(tile(x.label, x.value, x.note, x.icon, x.state));
      b.append(l);
      if (d.people) {
        const pe = h('div', 'people');
        for (const n of arr(d.people.home)) pe.append(h('span', '', String(n).slice(0, 2)));
        if (d.people.text) pe.append(h('span', 'faint', d.people.text));
        b.append(pe);
      }
    },
    energy(sec, d) {
      const b = body(sec);
      if (num(d.now_kw) === null) { empty(b); return; }
      const g = h('div', 'en'), l = h('div');
      l.append(h('div', 'label', 'Energy now'));
      const big = h('div', 'big', d.now_kw); big.append(h('span', 'unit', ' kW')); l.append(big);
      const ps = h('div', 'parts'), mx = Math.max(...arr(d.parts).map(x => num(x.kw) || 0), d.now_kw, 0.1);
      for (const x of arr(d.parts)) { const r = h('div'), t = h('div', 'between'); t.append(h('span', 'ellip', x.label), h('span', 'dim', x.kw + ' kW')); r.append(t, bar((num(x.kw) || 0) / mx * 100)); ps.append(r); }
      l.append(ps);
      const r = h('div'); r.style.cssText = 'display:flex;flex-direction:column;min-height:0';
      const top = h('div', 'between'); top.append(h('div', 'label', 'Today'), h('span', 'dim note', d.today || '')); r.append(top);
      const c = h('div', 'chart'), hr = arr(d.hourly), hmx = Math.max(...hr.map(v => num(v) || 0), 0.1);
      hr.forEach((v, i) => { const x = h('i', i === d.hour ? 'now' : ''); x.style.height = ((num(v) || 0) / hmx * 100) + '%'; c.append(x); });
      const ax = h('div', 'ax'); for (const t of ['12 AM', '6 AM', '12 PM', '6 PM', '12 AM']) ax.append(h('span', '', t));
      r.append(c, ax); g.append(l, r); b.append(g);
    },
    shows(sec, d, p) {
      const b = body(sec), part = p.part || 'upcoming';
      const top = h('div', 'between');
      top.append(h('div', 'label', p.title || (part === 'added' ? 'Just added' : 'Coming up')));
      b.append(top);
      if (part === 'added') {
        const a = arr(d.added).slice(0, num(p.max) || 6);
        if (!a.length) { empty(b); return; }
        const g = h('div', 'adds'); g.style.gridTemplateColumns = 'repeat(' + a.length + ',1fr)';
        for (const x of a) { const c = h('div', 'ad'), i = img(x.image, '', 320); if (i) c.append(i); c.append(h('div', 'ellip', x.title), h('small', '', x.when || '')); g.append(c); }
        b.append(g); return;
      }
      const up = arr(d.upcoming);
      if (!d.upcoming) { empty(b); return; }
      top.append(h('span', 'dim note', up.length + ' this week'));
      const w = h('div', 'week'), now = new Date();
      for (let k = 0; k < 7; k++) {
        const day = new Date(now.getFullYear(), now.getMonth(), now.getDate() + k);
        const key = day.getFullYear() + '-' + pad(day.getMonth() + 1) + '-' + pad(day.getDate());
        const c = h('div', 'day' + (k === 0 ? ' today' : '')), dh = h('div', 'dh');
        dh.append(h('span', '', k === 0 ? 'Today' : day.toLocaleDateString(undefined, { weekday: 'short' })), h('b', '', day.getDate()));
        c.append(dh);
        // Seven narrow columns: no poster, the title on up to two lines, the time first.
        for (const x of up.filter(x => String(x.date).slice(0, 10) === key)) {
          const parts = String(x.sub || '').split(' · '), ti = parts.findIndex(q => /^\d{1,2}:\d{2}/.test(q));
          if (ti > 0) parts.unshift(parts.splice(ti, 1)[0]);
          const got = parts.indexOf('\u2713');
          if (got >= 0) parts.splice(got, 1);
          const e = h('div', 'ep'), sm = h('small', '', parts.join(' · '));
          if (k === 0 && aired(day, parts[0], now)) e.dataset.past = '1';
          if (got >= 0) sm.append(h('span', 'got', '\u2713'));
          e.append(h('div', 't', x.title), sm); c.append(e);
        }
        if (c.children.length === 1) c.append(h('div', 'none', 'Nothing new'));
        w.append(c);
      }
      b.append(w);
      w.querySelectorAll('.day').forEach(c => fit(c, '.ep'));
    },
    worldclock(sec, d, p) {
      const b = body(sec), l = h('div', 'wc'), now = new Date();
      for (const z of arr(p.zones)) {
        let t = '', off = '';
        try {
          t = now.toLocaleTimeString('en-GB', { timeZone: z.tz, hour: '2-digit', minute: '2-digit' });
          const there = new Date(now.toLocaleString('en-US', { timeZone: z.tz })), here = new Date(now.toLocaleString('en-US'));
          const m = Math.round((there - here) / 60000), s = m < 0 ? '−' : '+', a = Math.abs(m);
          off = s + Math.floor(a / 60) + (a % 60 ? ':' + pad(a % 60) : '') + ' h';
        } catch (e) { t = '—'; }
        const r = tile(z.label, null, off); r.style.justifyContent = 'space-between'; r.append(h('b', '', t)); l.append(r);
      }
      b.append(l);
    },
    cameras(sec, d, p) {
      const b = body(sec), cams = arr(d.cameras);
      sec.querySelector('.live').hidden = !cams.length || !!d.error || !!d.setup;
      if (d.setup) { empty(b, 'Cameras are not connected yet', 'Set PROTECT_URL and PROTECT_API_KEY on the server'); return; }
      if (d.error) { empty(b, 'Cameras unavailable'); return; }
      if (!cams.length) { empty(b, d.cameras ? 'No cameras match' : 'No camera data yet'); return; }
      const g = h('div', 'cams n' + Math.min(cams.length, 9));
      for (const c of cams) {
        const t = h('div', 'cam' + (c.online ? '' : ' offline')), cap = h('div', 'cap');
        t.dataset.cam = c.id; t.dataset.name = c.name;
        cap.append(h('span', 'dot' + (c.online ? '' : ' off')), h('span', 'ellip', c.name + (c.online ? '' : ' · offline')));
        t.append(cap); g.append(t);
      }
      b.append(g);
      // More than 9: the column count that shows the most of each 16:9 picture (14 in 3
      // columns were 2.6:1 strips, QA 2026-10-28).
      if (cams.length > 9 && g.clientWidth && g.clientHeight) {
        let best = 3, area = 0;
        for (let c = 3; c <= 6; c++) {
          const w = g.clientWidth / c, ht = g.clientHeight / Math.ceil(cams.length / c);
          const a = Math.min(w, ht * 16 / 9) * Math.min(ht, w * 9 / 16);
          if (a > area) { area = a; best = c; }
        }
        g.style.gridTemplateColumns = 'repeat(' + best + ',1fr)';
      }
      camTick(sec, true);
    },
    services(sec, d, p) {
      const b = body(sec), cs = arr(d.containers), host = d.host || {};
      if (!d.containers) { empty(b); return; }
      const g = h('div', 'svc'), hp = h('div', 'panel');
      hp.append(h('div', 'label', p.title || host.name || 'Server'));
      const run = cs.filter(c => c.state === 'healthy' || c.state === 'running').length;
      const ring = h('div', 'ring'), sv = document.createElementNS(SVGNS, 'svg');
      sv.setAttribute('viewBox', '0 0 100 100');
      const c1 = document.createElementNS(SVGNS, 'circle'), c2 = document.createElementNS(SVGNS, 'circle');
      for (const c of [c1, c2]) { c.setAttribute('cx', 50); c.setAttribute('cy', 50); c.setAttribute('r', 44); c.setAttribute('fill', 'none'); c.setAttribute('stroke-width', 7); }
      c1.setAttribute('stroke', 'var(--chip2)'); c2.setAttribute('stroke', run === cs.length ? 'var(--accent)' : 'var(--warn)');
      c2.setAttribute('stroke-linecap', 'round'); c2.setAttribute('stroke-dasharray', 276.5); c2.setAttribute('stroke-dashoffset', 276.5 * (1 - (cs.length ? run / cs.length : 0)));
      sv.append(c1, c2); const rb = h('b', '', run); rb.append(h('small', '', 'of ' + cs.length + ' running')); ring.append(sv, rb); hp.append(ring);
      const bad = cs.filter(c => c.state === 'unhealthy' || c.state === 'exited').length;
      const pill = h('div', 'badge' + (bad ? ' bad' : ''), bad ? bad + ' need attention' : 'All health checks passing'); pill.style.alignSelf = 'center'; hp.append(pill);
      for (const m of arr(host.meters)) { const x = h('div', 'meter'), t = h('div', 'between'); t.append(h('span', 'dim', m.label), h('span', '', m.text || '')); x.append(t, bar(m.pct)); hp.append(x); }
      if (host.note) { const n = h('div', 'faint', host.note); n.style.cssText = 'font-size:1.3rem;margin-top:auto'; hp.append(n); }
      const gs = h('div', 'groups'), used = new Set();
      const groups = arr(p.groups).map(gr => ({ name: gr.name, items: cs.filter(c => arr(gr.members).includes(c.name)) }));
      groups.forEach(gr => gr.items.forEach(c => used.add(c.name)));
      for (let i = groups.length - 1; i >= 0; i--) if (!groups[i].items.length) groups.splice(i, 1);
      const rest = cs.filter(c => !used.has(c.name)); if (rest.length) groups.push({ name: groups.length ? 'Other' : 'Containers', items: rest });
      if (groups.length === 1) gs.style.gridTemplateColumns = '1fr';
      // An odd count leaves one panel a whole row: give it to the biggest group, first.
      if (groups.length > 1 && groups.length % 2) {
        const big = groups.reduce((m, g, i) => g.items.length > groups[m].items.length ? i : m, 0);
        groups.unshift(groups.splice(big, 1)[0]);
      }
      for (const gr of groups) {
        const pn = h('div', 'panel'), top = h('div', 'between');
        const ok = gr.items.filter(c => c.state === 'healthy' || c.state === 'running').length;
        top.append(h('div', 'label', gr.name), h('span', 'faint note', ok + ' / ' + gr.items.length)); pn.append(top);
        const l = h('div', 'sv'), rank = c => ({ unhealthy: 0, exited: 0, starting: 1 })[c.state] ?? 2;
        // Problems first, so a "+N more" never hides one.
        for (const c of gr.items.slice().sort((a, b) => rank(a) - rank(b))) {
          const t = h('div', 'tile'), s = { healthy: 'ok', running: 'ok', starting: 'warn', unhealthy: 'bad', exited: 'bad' }[c.state] || 'off';
          t.append(h('span', 'dot ' + (s === 'ok' ? '' : s)), h('span', 'ellip', c.name));
          if (c.note) t.append(h('small', 'faint ellip', c.note));
          l.append(t);
        }
        pn.append(l); gs.append(pn);
      }
      if (groups.length > 1 && groups.length % 2) gs.firstChild.style.gridColumn = 'span 2';
      g.append(hp, gs); b.append(g);
      gs.querySelectorAll('.sv').forEach(l => fit(l, '.tile'));
    }
  };

  const state = {};
  function props(sec) {
    if (!sec._props) { try { sec._props = JSON.parse((sec.querySelector('script.wp') || {}).textContent || '{}'); } catch (e) { sec._props = {}; } }
    return sec._props;
  }
  // A widget on a hidden tab is only marked dirty; it is drawn when its tab is shown. The A33
  // then spends no time building DOM nobody sees (a services update every minute, the clock tick).
  const dirty = new Set();
  TF.perf = { draws: 0, ms: 0, skipped: 0 };
  function visible(sec) { const pg = sec.closest('.page'); return !pg || pg.classList.contains('on'); }
  function draw(sec) {
    const r = R[sec.dataset.type];
    if (!r) return;
    if (!visible(sec)) { dirty.add(sec); return; }
    dirty.delete(sec);
    const t = performance.now();
    try { r(sec, state[sec.id] || {}, props(sec)); } catch (e) { fail(sec.id + ': ' + e); }
    TF.perf.draws++; TF.perf.ms += performance.now() - t;
  }
  // ---- how old a card's data is ------------------------------------------------------------
  // Source messages carry the source's age (age_s, measured on the server, so no clock needs to
  // be in step) and when to call it old (stale_s, server/internal/api/sources.go staleAfter).
  // An old source dims the card and says "updated 40 min ago" in its bottom margin, as the
  // cars app does. Ages run on performance.now(), which a clock being set does not move.
  function fresh(sec, m) {
    if (m.key !== props(sec).source || typeof m.age_s !== 'number') return;
    sec._seenAt = performance.now() - m.age_s * 1000;
    sec._staleMs = (Number(m.stale_s) || 0) * 1000;
    markAge(sec);
  }
  function ago(ms) {
    const m = Math.round(ms / 60000), hr = Math.floor(m / 60);
    return m < 60 ? m + ' min ago' : hr < 24 ? hr + ' h ago' : Math.floor(hr / 24) + ' d ago';
  }
  function markAge(sec) {
    if (sec._seenAt === undefined) return;
    const age = performance.now() - sec._seenAt, old = sec._staleMs > 0 && age > sec._staleMs;
    const text = old ? 'updated ' + ago(age) : '';
    if (sec._ageText === text) return; // most minutes: nothing changes, nothing is redrawn
    sec._ageText = text;
    sec.classList.toggle('stale', old);
    if (!sec._ageEl) { if (!old) return; sec._ageEl = h('div', 'age'); sec.append(sec._ageEl); }
    sec._ageEl.textContent = text;
    sec._ageEl.hidden = !old;
  }

  function rerenderTimed() {
    document.querySelectorAll('.w-agenda,.w-worldclock,.w-shows').forEach(draw);
  }
  // Drawn after the tab bar has picked the first page, so hidden tabs start dirty.
  function drawAll() { widgets.forEach(draw); }

  // Part of the day, for themes that light the page by it (aurora's Home). Checked every 5 min.
  const root = document.documentElement;
  root.dataset.mood = root.dataset.mood || 'home';
  const daypart = () => {
    const h = new Date().getHours();
    root.dataset.daypart = h >= 5 && h < 8 ? 'dawn' : h >= 8 && h < 17 ? 'day' : h >= 17 && h < 20 ? 'dusk' : 'night';
  };
  daypart();
  setInterval(daypart, 300000);

  // ---- tabs -------------------------------------------------------------------------------
  const grid = document.getElementById('grid');
  let idleT;
  // The rail first: it is small and cheap to redraw, so the touch shows in the next frame. A
  // tab's second pages are always listed (.seg); "show" marks the list of the open tab.
  function highlight(id) {
    let parent = null;
    document.querySelectorAll('.seg').forEach(s => {
      const mine = !!s.querySelector('[data-tab="' + id + '"]');
      s.classList.toggle('show', mine);
      s.querySelectorAll('span').forEach(x => x.classList.toggle('on', x.dataset.tab === id));
      if (mine) parent = s.dataset.for;
    });
    document.querySelectorAll('.tab').forEach(t => t.classList.toggle('on', t.dataset.tab === (parent || id)));
  }
  function showPage(id) {
    document.querySelectorAll('.page').forEach(p => p.classList.toggle('on', p.dataset.page === id));
    dirty.forEach(sec => { if (visible(sec)) draw(sec); });
  }
  function show(id) { highlight(id); showPage(id); }
  if (grid && grid.classList.contains('tabbed')) {
    const first = document.querySelector('.page') && document.querySelector('.page').dataset.page;
    // Themes colour a page and its tab by the tab's icon (a fixed set, unlike tab ids). The mood
    // sits on the page and the tab, never on <html>: changing an inherited colour on a tab switch
    // made WebKit redraw the whole screen, light layer included (~300 ms on the frame).
    document.querySelectorAll('.tab[data-tab]').forEach(t => {
      const mood = t.dataset.icon || 'home';
      t.dataset.mood = mood;
      const seg = document.querySelector('.seg[data-for="' + t.dataset.tab + '"]');
      const ids = seg ? Array.from(seg.querySelectorAll('[data-tab]'), x => x.dataset.tab) : [t.dataset.tab];
      ids.forEach(id => { const p = document.querySelector('.page[data-page="' + id + '"]'); if (p) p.dataset.mood = mood; });
    });
    show(first);
    // Switch on touch-down, not on release: the tab lights at once, and the page follows once
    // that frame is out (two frames), so drawing the page cannot hold the highlight back.
    document.querySelector('.tabs').addEventListener('pointerdown', e => {
      const t = e.target.closest('[data-tab]');
      if (!t) return;
      const id = t.dataset.tab;
      highlight(id);
      requestAnimationFrame(() => requestAnimationFrame(() => showPage(id)));
    });
    const idle = Math.max(10, Number(grid.dataset.idle || 90)) * 1000;
    const arm = () => { clearTimeout(idleT); idleT = setTimeout(() => show(first), idle); };
    addEventListener('pointerdown', arm, { passive: true });
    addEventListener('tf-activity', arm);
  }
  drawAll();
  tickClock();
  // Car and home controls are display-only for now: say so instead of doing nothing.
  addEventListener('click', e => { if (e.target.closest('[data-act]')) window.toast('Controls are not connected yet'); });
  // Emits carry the frame's emit key (server/internal/api/apps.go EmitKey): only pages the
  // server serves have it, so another site cannot fire a hook. The page's apps never see it.
  const emitPath = '/api/frames/' + encodeURIComponent(TF.frame) + '/emit';
  const emitURL = emitPath + (TF.k ? '?k=' + encodeURIComponent(TF.k) : '');
  // A room's switch: the hook room_on or room_off gets {"area_id"} (Home Assistant's light
  // service takes it as it is). Without such a hook on the server, say so; nothing flips.
  addEventListener('click', e => {
    const t = e.target.closest('.room[data-room]');
    if (!t || t._busy || !t.dataset.area) return;
    const on = !t.dataset.on, sec = t.closest('.w'), name = t.dataset.room;
    t._busy = true;
    fetch(emitURL, { method: 'POST',
      body: JSON.stringify({ name: on ? 'room_on' : 'room_off', data: { area_id: t.dataset.area } }) })
      .then(r => r.json()).then(res => {
        if (!res.status && res.error) { window.toast(name + ': ' + res.error); return; }
        if (!res.status) { window.toast('Room lights are not connected yet'); return; }
        if (!res.ok) { window.toast(name + ': ' + (res.error || 'failed')); return; }
        // Show it now; the next rooms update brings the real state.
        const st = state[sec.id] = state[sec.id] || {}, list = merge(props(sec).rooms, st.rooms);
        const r = list.find(x => x.name === name);
        if (r) { r.on = on; r.lights = on ? 'On' : 'Off'; if (!on) r.brightness = null; st.rooms = list; sec._last = {}; draw(sec); }
      }, () => window.toast('No answer from the server')).finally(() => { t._busy = false; });
  });

  // App widgets: sandboxed iframes running the app SDK (server/internal/api/appsdk.js). They
  // reach the server through this page, so the page keeps the only event stream.
  const appFrames = Array.from(document.querySelectorAll('iframe.app'));
  const toApps = m => appFrames.forEach(f => f.contentWindow && f.contentWindow.postMessage({ tf: 1, op: 'server', msg: m }, '*'));
  // Every app says hello once its script runs (the SDK is in every app document). One that has
  // not after 15 s (the server unreachable, an error page, a broken app) gets a note in its place
  // instead of a blank tab, and is loaded again every 30 s until it answers.
  function appWait(f) {
    clearTimeout(f._t);
    f._hello = false;
    f._t = setTimeout(function check(retry) {
      if (f._hello) return;
      if (!f._fail) {
        const n = f.dataset.app || 'app', el = f._fail = h('div', 'appfail');
        el.append(h('div', '', n[0].toUpperCase() + n.slice(1) + ' did not load'), h('small', '', 'Trying again every 30 seconds'));
        f.after(el);
      }
      f._fail.hidden = false;
      if (retry) f.src = f.src;
      f._t = setTimeout(check, 30000, true);
    }, 15000, false);
  }
  appFrames.forEach(appWait);
  if (appFrames.length) {
    addEventListener('message', e => {
      const f = appFrames.find(x => x.contentWindow === e.source), m = e.data;
      if (!f || !m || m.tf !== 1) return;
      const sec = f.closest('.w'), name = f.dataset.app;
      if (m.op === 'hello') {
        f._hello = true; clearTimeout(f._t); if (f._fail) f._fail.hidden = true;
        let p = {};
        try { p = JSON.parse(sec.querySelector('script.wp').textContent); } catch { /* no props */ }
        e.source.postMessage({ tf: 1, op: 'init', frame: TF.frame, widget: sec.id.slice(2), props: p.props || {} }, '*');
      } else if (m.op === 'activity') {
        dispatchEvent(new Event('tf-activity'));
      } else if (m.op === 'fetch') {
        const reply = (status, body) => e.source.postMessage({ tf: 1, op: 'reply', id: m.id, status, body }, '*');
        const path = String(m.path || ''), emit = path === emitPath;
        const ok = /^\/api\/data\/source\/[a-z0-9_-]+$/.test(path) || emit ||
          path === '/api/apps/' + name + '/state' || path.startsWith('/api/apps/' + name + '/state/');
        if (!ok || (m.method !== 'GET' && m.method !== 'POST')) { reply(403, 'not allowed: ' + m.method + ' ' + path); return; }
        // The key is added here, after the check on the path the app asked for.
        fetch(emit ? emitURL : path, m.method === 'POST' ? { method: 'POST', body: String(m.body ?? '') } : {})
          .then(r => r.text().then(t => reply(r.status, t)), err => reply(0, String(err)));
      }
    });
  }

  // ---- cameras and the doorbell (UniFi Protect, server/internal/api/cameras.go) ------------
  // Snapshots, not video. A new picture is decoded off screen and swapped in when ready, one
  // request per picture in flight, so a slow camera never queues requests.
  function loadCam(holder, id) {
    if (holder._busy) return;
    holder._busy = true;
    const n = new Image();
    n.alt = '';
    n.onload = () => { holder._busy = false; holder.classList.remove('stale'); const o = holder.querySelector('img'); if (o) o.replaceWith(n); else holder.prepend(n); };
    n.onerror = () => { holder._busy = false; holder.classList.add('stale'); };
    n.src = '/api/data/camera/' + encodeURIComponent(id) + '?t=' + Date.now();
  }
  function camTick(sec, force) {
    const every = Math.max(2, num(props(sec).interval_s) || 4) * 1000, now = Date.now();
    if (!force && now - (sec._camAt || 0) < every) return;
    sec._camAt = now;
    // An offline camera only answers 502: no request (it comes back with the camera list).
    sec.querySelectorAll('.cam:not(.offline)').forEach(t => loadCam(t, t.dataset.cam));
  }
  const camSecs = Array.from(document.querySelectorAll('.w-cameras'));
  const view = { el: null };
  if (camSecs.length) setInterval(() => { if (!view.el) camSecs.forEach(s => { if (visible(s)) camTick(s); }); }, 1000);

  // One camera over the whole page: a tapped camera, or the doorbell when it rings.
  function openView(o) {
    closeView();
    const el = h('div', 'camview' + (o.ring ? ' cv-ring' : '')), pic = h('div', 'cv-pic'), top = h('div', 'cv-top'), pill = h('div', 'cv-pill');
    pill.append(icon(o.ring ? 'bell' : 'cam'), h('span', '', o.name));
    top.append(pill, h('span', 'cv-live', 'Live'));
    const foot = h('div', 'cv-foot'), txt = h('div', 'cv-txt');
    txt.append(h('div', 'cv-title', o.ring ? 'Someone\u2019s at the door' : o.name), h('div', 'cv-sub', o.sub || 'Tap anywhere to close'));
    foot.append(txt, h('div', 'cv-btn', o.ring ? 'Dismiss' : 'Close'));
    el.append(pic, top, foot);
    el.addEventListener('click', () => { if (o.ring) dismissed(o.ringId, true); closeView(); });
    document.body.append(el);
    const tick = () => loadCam(pic, o.id);
    tick();
    Object.assign(view, { el, ringId: o.ringId || '', timer: setInterval(tick, o.ring ? 1000 : 1500), end: setTimeout(closeView, (o.secs || 60) * 1000) });
    dispatchEvent(new Event('tf-activity'));
  }
  function closeView() {
    if (!view.el) return;
    clearInterval(view.timer); clearTimeout(view.end);
    view.el.remove(); view.el = null;
  }
  addEventListener('click', e => { const t = e.target.closest('.cam'); if (t) openView({ id: t.dataset.cam, name: t.dataset.name, secs: 120 }); });
  // A dismissed ring stays dismissed across the reload a waking screen does.
  function dismissed(id, set) {
    try {
      const l = JSON.parse(localStorage.getItem('tf-rings') || '[]');
      if (!set) return l.includes(id);
      localStorage.setItem('tf-rings', JSON.stringify(l.concat(id).slice(-20)));
    } catch (e) { /* no storage: may show again after a reload */ }
    return false;
  }
  function ring(d) {
    const age = Number(d.age_s) || 0;
    if (!d.camera || age > 90 || dismissed(d.id) || (view.el && view.ringId === d.id)) return;
    openView({ id: d.camera, name: d.name || 'Doorbell', ring: true, ringId: d.id, secs: Math.max(20, 90 - age),
      sub: (d.name || 'Doorbell') + ' \u00b7 ' + (age < 60 ? 'just now' : Math.round(age / 60) + ' min ago') });
  }
  TF.ring = ring; // tests and the MCP preview

  // A hand held over the light sensor (tfctl sense.rs): close a camera view, else the next tab.
  function gesture(g) {
    if (g !== 'wave') return;
    if (view.el) { if (view.ringId) dismissed(view.ringId, true); closeView(); return; }
    const tabs = Array.from(document.querySelectorAll('.tab[data-tab]'));
    if (!tabs.length) return;
    const i = tabs.findIndex(t => t.classList.contains('on'));
    show(tabs[(i + 1) % tabs.length].dataset.tab);
    dispatchEvent(new Event('tf-activity'));
  }
  TF.gesture = gesture;

  // The stream down for more than 30 s (server gone, Wi-Fi out): a small "Offline" note, so old
  // numbers are not taken for current ones. A reconnect within 30 s (a server restart, a resume)
  // shows nothing. Counted from page load until the stream first opens.
  let stream, downSince = performance.now();
  const offEl = document.getElementById('offline');
  function offlineTick(now) {
    const off = !!downSince && now - downSince > 30000;
    if (offEl && offEl.hidden === off) offEl.hidden = !off;
  }
  function connect() {
    const es = stream = new EventSource('/api/stream/' + encodeURIComponent(TF.frame) + (appFrames.length ? '?apps=1' : ''));
    es.onopen = () => { if (stream === es) { downSince = 0; offlineTick(0); } };
    es.onmessage = ev => {
      TF.perf.msgs = (TF.perf.msgs || 0) + 1;
      let m; try { m = JSON.parse(ev.data); } catch { return; }
      if (m.reload || (m.page_version && m.page_version !== TF.pageVersion)) { location.reload(); return; }
      if (m.source || m.app_state || m.app_event) { toApps(m); return; }
      if (m.app_reload) { appFrames.forEach(f => { if (f.dataset.app === m.app_reload) { f.src = f.src; appWait(f); } }); return; }
      if (m.doorbell) { ring(m.doorbell); return; }
      if (m.gesture) { gesture(m.gesture); return; }
      if (!m.widget) return;
      const sec = document.getElementById('w-' + m.widget);
      if (!sec) return;
      if (m.key) { fresh(sec, m); if (!m.data) return; } // no data: the source is still fresh
      // The same message again (a reconnect replays every source) changes nothing: skip it.
      const raw = JSON.stringify(m.data || {}), key = Object.keys(m.data || {}).sort().join();
      sec._last = sec._last || {};
      if (sec._last[key] === raw) { TF.perf.skipped++; return; }
      sec._last[key] = raw;
      for (const [k, v] of Object.entries(m.data || {})) {
        if (typeof v === 'object' && v !== null) continue;
        const el = sec.querySelector('[data-field="' + k + '"]');
        if (el) el.textContent = v;
      }
      if (R[sec.dataset.type]) { state[sec.id] = Object.assign(state[sec.id] || {}, m.data || {}); draw(sec); }
    };
    es.onerror = () => {
      TF.perf.reconnects = (TF.perf.reconnects || 0) + 1; es.close();
      if (stream === es && !downSince) downSince = performance.now();
      // Only if no newer stream was opened meanwhile (the resume detector): two at once
      // delivered every message twice.
      if (stream === es) setTimeout(() => { if (stream === es) connect(); }, 5000);
    };
  }
  connect();

  // Touch to paint: from when the input reached WebKit (e.timeStamp, or now if that is on another
  // clock) until the next frame was drawn. Cards do not dim on touch: any change to a card
  // (opacity, border) made WebKit redraw it, 0.3 to 0.9 s of stalled frames per tap on the
  // Mali-400 under Aurora [V 2026-10-26]. painted_at_ms is the frame's wall clock
  // then, so `tfctl bench` can measure from the moment it injected the tap.
  addEventListener('pointerdown', e => {
    const now = performance.now();
    const t0 = e.timeStamp > 0 && e.timeStamp <= now && now - e.timeStamp < 10000 ? e.timeStamp : now;
    const sec = e.target.closest('.w');
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const end = performance.now();
      navigator.sendBeacon('/api/frames/' + encodeURIComponent(TF.frame) + '/beacon', JSON.stringify({
        latency_ms: Math.max(1, Math.round(end - t0)), widget: sec ? sec.id : '',
        painted_at_ms: Math.round(performance.timeOrigin + end) }));
    }));
  }, { passive: true });

  // How long this load took, once, for `tfctl bench`-style checks of boot and reload speed.
  // Load can come before the first paint: wait for it (up to 3 s), or paint_ms read 0.
  addEventListener('load', () => setTimeout(function send(tries) {
    const n = performance.getEntriesByType('navigation')[0];
    const p = performance.getEntriesByName('first-contentful-paint')[0];
    if (!n) return;
    if (!p && (tries || 0) < 15) { setTimeout(send, 200, (tries || 0) + 1); return; }
    navigator.sendBeacon('/api/frames/' + encodeURIComponent(TF.frame) + '/beacon', JSON.stringify({ load: {
      response_ms: Math.round(n.responseEnd), dom_ms: Math.round(n.domContentLoadedEventEnd),
      paint_ms: p ? Math.round(p.startTime) : 0, load_ms: Math.round(n.loadEventEnd || performance.now()) } }));
  }, 0));

  // Liveness for the server's hung-page check (api/watchdog.go): stops if this WebProcess hangs.
  const alive = () => navigator.sendBeacon('/api/frames/' + encodeURIComponent(TF.frame) + '/alive', '');
  alive();
  setInterval(alive, 30000);

  // A dark room or the schedule turns the screen off by pausing this whole page (SIGSTOP, tfctl
  // kiosk), and a wake resumes it as it was: the picture is there at once. A 1 s timer that fires
  // far too late is the sign. The stream is stale or dead by then: open a new one now (it replays
  // every source) rather than wait for its error and 5 s more.
  let beat = Date.now();
  setInterval(() => {
    const now = Date.now();
    if (now - beat > 10000) {
      TF.perf.resumes = (TF.perf.resumes || 0) + 1;
      paintClock(); daypart(); alive();
      const old = stream; stream = null; if (old) old.close();
      if (downSince) downSince = performance.now(); // the pause is not the stream's fault
      connect();
    }
    beat = now;
    offlineTick(performance.now());
  }, 1000);

  let toastT;
  window.toast = t => { const el = document.getElementById('toast'); el.textContent = t; el.classList.add('show'); clearTimeout(toastT); toastT = setTimeout(() => el.classList.remove('show'), 2500); };
})();
