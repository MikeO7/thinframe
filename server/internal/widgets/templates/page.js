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

  function tickClock() {
    const d = new Date();
    document.querySelectorAll('.clock-time').forEach(e => e.textContent = hm(d, e.dataset.h12 === 'true'));
    document.querySelectorAll('.clock-date').forEach(e => e.textContent =
      d.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' }));
    rerenderTimed();
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
    frunk: '<path d="M3 17.5V14M9 14l3-4h4.5l3 3.5 1.5.5v3.5"/><path d="M9 14 3.8 10.2"/><path d="M3 17.5h2M9 17.5h6M19 17.5h2"/><circle cx="7" cy="17.5" r="2"/><circle cx="17" cy="17.5" r="2"/>',
    trunk: '<path d="M21 17.5V14M15 14l-3-4H7.5l-3 3.5L3 14v3.5"/><path d="M15 14l5.2-3.8"/><path d="M3 17.5h2M9 17.5h6M19 17.5h2"/><circle cx="7" cy="17.5" r="2"/><circle cx="17" cy="17.5" r="2"/>',
    plug: '<path d="M9 3v5M15 3v5M6.5 8h11v3.5a5.5 5.5 0 0 1-11 0z"/><path d="M12 17v4"/>',
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
  function src(v) {
    if (typeof v !== 'string') return '';
    if (/^https?:\/\//.test(v)) return v;
    if (/^asset:[A-Za-z0-9._-]+$/.test(v)) return '/api/data/asset/' + encodeURIComponent(v.slice(6));
    return '';
  }
  function img(v, cls) {
    const u = src(v);
    if (!u) return null;
    const i = h('img', cls); i.alt = ''; i.src = u; i.onerror = () => { i.style.visibility = 'hidden'; };
    return i;
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
  function empty(b, text) { b.closest('.w').classList.add('nodata'); b.append(h('div', 'empty', text || 'No data yet')); }
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
  function inText(m) { return m < 60 ? m + ' min' : Math.floor(m / 60) + ' h ' + (m % 60 ? (m % 60) + ' min' : ''); }

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
      const list = h('div', 'ag');
      for (const e of (p.all ? evs : day).slice(0, num(p.max) || 5)) {
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
      const t = h('div', 'dim', [z[0].name, z[0].target ? 'set ' + z[0].target : '', z[0].action].filter(Boolean).join(' · '));
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
        for (const x of a) { const c = h('div', 'ad'), i = img(x.image); if (i) c.append(i); c.append(h('div', 'ellip', x.title), h('small', '', x.when || '')); g.append(c); }
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
        for (const x of up.filter(x => String(x.date).slice(0, 10) === key)) {
          const e = h('div', 'ep'), i = img(x.image); if (i) e.append(i);
          const t = h('div'); t.style.minWidth = 0; t.append(h('div', 't ellip', x.title), h('small', 'ellip', x.sub || '')); e.append(t); c.append(e);
        }
        w.append(c);
      }
      b.append(w);
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
      sec.querySelector('.live').hidden = !cams.length;
      if (d.setup) { empty(b, 'Cameras need setting up: PROTECT_URL and PROTECT_API_KEY on the server'); return; }
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
      for (const gr of groups) {
        const pn = h('div', 'panel'), top = h('div', 'between');
        const ok = gr.items.filter(c => c.state === 'healthy' || c.state === 'running').length;
        top.append(h('div', 'label', gr.name), h('span', 'faint note', ok + ' / ' + gr.items.length)); pn.append(top);
        const l = h('div', 'sv');
        for (const c of gr.items) {
          const t = h('div', 'tile'), s = { healthy: 'ok', running: 'ok', starting: 'warn', unhealthy: 'bad', exited: 'bad' }[c.state] || 'off';
          t.append(h('span', 'dot ' + (s === 'ok' ? '' : s)), h('span', 'ellip', c.name));
          if (c.note) t.append(h('small', 'faint ellip', c.note));
          l.append(t);
        }
        pn.append(l); gs.append(pn);
      }
      g.append(hp, gs); b.append(g);
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
  function show(id) {
    document.querySelectorAll('.page').forEach(p => p.classList.toggle('on', p.dataset.page === id));
    let parent = null;
    document.querySelectorAll('.seg').forEach(s => {
      const mine = !!s.querySelector('[data-tab="' + id + '"]');
      s.classList.toggle('show', mine);
      s.querySelectorAll('span').forEach(x => x.classList.toggle('on', x.dataset.tab === id));
      if (mine) parent = s.dataset.for;
    });
    document.querySelectorAll('.tab').forEach(t => t.classList.toggle('on', t.dataset.tab === (parent || id)));
    // Themes colour the page by the tab's icon (a fixed set, unlike tab ids).
    const on = document.querySelector('.tab.on');
    document.documentElement.dataset.mood = (on && on.dataset.icon) || 'home';
    dirty.forEach(sec => { if (visible(sec)) draw(sec); });
  }
  if (grid && grid.classList.contains('tabbed')) {
    const first = document.querySelector('.page') && document.querySelector('.page').dataset.page;
    show(first);
    document.querySelector('.tabs').addEventListener('click', e => {
      const t = e.target.closest('[data-tab]');
      if (t) show(t.dataset.tab);
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

  // App widgets: sandboxed iframes running the app SDK (server/internal/api/appsdk.js). They
  // reach the server through this page, so the page keeps the only event stream.
  const appFrames = Array.from(document.querySelectorAll('iframe.app'));
  const toApps = m => appFrames.forEach(f => f.contentWindow && f.contentWindow.postMessage({ tf: 1, op: 'server', msg: m }, '*'));
  if (appFrames.length) {
    addEventListener('message', e => {
      const f = appFrames.find(x => x.contentWindow === e.source), m = e.data;
      if (!f || !m || m.tf !== 1) return;
      const sec = f.closest('.w'), name = f.dataset.app;
      if (m.op === 'hello') {
        let p = {};
        try { p = JSON.parse(sec.querySelector('script.wp').textContent); } catch { /* no props */ }
        e.source.postMessage({ tf: 1, op: 'init', frame: TF.frame, widget: sec.id.slice(2), props: p.props || {} }, '*');
      } else if (m.op === 'activity') {
        dispatchEvent(new Event('tf-activity'));
      } else if (m.op === 'fetch') {
        const reply = (status, body) => e.source.postMessage({ tf: 1, op: 'reply', id: m.id, status, body }, '*');
        const path = String(m.path || ''), fr = encodeURIComponent(TF.frame);
        const ok = /^\/api\/data\/source\/[a-z0-9_-]+$/.test(path) || path === '/api/frames/' + fr + '/emit' ||
          path === '/api/apps/' + name + '/state' || path.startsWith('/api/apps/' + name + '/state/');
        if (!ok || (m.method !== 'GET' && m.method !== 'POST')) { reply(403, 'not allowed: ' + m.method + ' ' + path); return; }
        fetch(path, m.method === 'POST' ? { method: 'POST', body: String(m.body ?? '') } : {})
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
    sec.querySelectorAll('.cam').forEach(t => loadCam(t, t.dataset.cam));
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
    const tabs = Array.from(document.querySelectorAll('.tab'));
    if (!tabs.length) return;
    const i = tabs.findIndex(t => t.classList.contains('on'));
    tabs[(i + 1) % tabs.length].click();
    dispatchEvent(new Event('tf-activity'));
  }
  TF.gesture = gesture;

  function connect() {
    const es = new EventSource('/api/stream/' + encodeURIComponent(TF.frame) + (appFrames.length ? '?apps=1' : ''));
    es.onmessage = ev => {
      TF.perf.msgs = (TF.perf.msgs || 0) + 1;
      let m; try { m = JSON.parse(ev.data); } catch { return; }
      if (m.reload || (m.page_version && m.page_version !== TF.pageVersion)) { location.reload(); return; }
      if (m.source || m.app_state || m.app_event) { toApps(m); return; }
      if (m.app_reload) { appFrames.forEach(f => { if (f.dataset.app === m.app_reload) f.src = f.src; }); return; }
      if (m.doorbell) { ring(m.doorbell); return; }
      if (m.gesture) { gesture(m.gesture); return; }
      if (!m.widget) return;
      const sec = document.getElementById('w-' + m.widget);
      if (!sec) return;
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
    es.onerror = () => { TF.perf.reconnects = (TF.perf.reconnects || 0) + 1; es.close(); setTimeout(connect, 5000); };
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

  let toastT;
  window.toast = t => { const el = document.getElementById('toast'); el.textContent = t; el.classList.add('show'); clearTimeout(toastT); toastT = setTimeout(() => el.classList.remove('show'), 2500); };
})();
