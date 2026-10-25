(function () {
  'use strict';
  const pad = n => String(n).padStart(2, '0');
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
    cam: '<rect x="3" y="6" width="13" height="12" rx="2"/><path d="M16 10l5-3v10l-5-3"/>',
    leaf: '<path d="M5 19C5 9 11 4 20 4c0 9-5 15-15 15zM5 19l7-7"/>',
    dot: '<circle cx="12" cy="12" r="3"/>'
  };
  const SVGNS = 'http://www.w3.org/2000/svg';

  function h(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined && text !== null) e.textContent = String(text);
    return e;
  }
  function icon(name) {
    const s = document.createElementNS(SVGNS, 'svg');
    s.setAttribute('viewBox', '0 0 24 24');
    s.setAttribute('class', 'ic');
    s.innerHTML = ICONS[name] || ICONS.dot; // constant table, never data
    return s;
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
  function body(sec) { const b = sec.querySelector('.body'); b.textContent = ''; return b; }
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
  function empty(b, text) { b.append(h('div', 'empty', text || 'Waiting for data')); }

  // Events: [{start, end, title, where}], ISO times.
  function events(d) {
    return arr(d.events).map(e => Object.assign({}, e, { s: new Date(e.start), e: new Date(e.end || e.start) }))
      .filter(e => !isNaN(e.s)).sort((a, b) => a.s - b.s);
  }
  function mins(a, b) { return Math.round((b - a) / 60000); }
  function inText(m) { return m < 60 ? m + ' min' : Math.floor(m / 60) + ' h ' + (m % 60 ? (m % 60) + ' min' : ''); }

  const R = {
    hero(sec, d) {
      badge(sec, d.badge);
      const t = sec.querySelector('[data-field="temp"]');
      sec.querySelector('.hero-wx').hidden = !t.textContent;
      const c = sec.querySelector('.chips'); c.textContent = '';
      for (const x of arr(d.chips)) { const e = h('span', 'chip'); e.append(icon(x.icon), h('span', '', x.text)); c.append(e); }
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
      if (compact) {
        sec.querySelector('.body').before(sec.querySelector('.label') || h('div', 'label', p.title || 'Garage'));
        const l = h('div', 'cars-compact');
        for (const c of cars) {
          const r = h('div', 'carrow'); r.append(img(c.image) || icon('car'));
          const t = h('div'); t.style.flex = 1; t.style.minWidth = 0;
          const top = h('div', 'between'); top.style.fontSize = '1.7rem';
          top.append(h('span', 'ellip', c.name), h('span', c.charging ? 's-ok' : 'dim', num(c.battery) !== null ? c.battery + '%' : '—'));
          t.append(top, bar(c.battery));
          t.lastChild.style.marginTop = '.8rem';
          t.append(h('div', 'faint ellip', c.status || ''));
          t.lastChild.style.cssText = 'font-size:1.3rem;margin-top:.6rem';
          r.append(t); l.append(r);
        }
        b.append(l); return;
      }
      const g = h('div', 'cars-full');
      for (const c of cars) {
        const k = h('div', 'car'), top = h('div', 'between');
        const nm = h('div'); nm.append(h('div', 'model', c.model || ''), h('div', 'name ellip', c.name));
        const bt = h('div'); bt.style.textAlign = 'right';
        const pr = h('div', 'row'); pr.style.justifyContent = 'flex-end';
        const ba = h('span', 'batt' + (c.charging ? ' chg' : '')), bi = h('i'); bi.style.width = (num(c.battery) || 0) + '%'; ba.append(bi);
        pr.append(ba); if (c.charging) { const z = icon('bolt'); z.style.color = 'var(--ok)'; pr.append(z); }
        pr.append(h('span', 'pct' + (c.charging ? ' chg' : ''), num(c.battery) !== null ? c.battery + '%' : '—'));
        bt.append(pr, h('div', 'dim', [c.range ? c.range : '', c.status || ''].filter(Boolean).join(' · ')));
        bt.lastChild.style.cssText = 'font-size:1.5rem;margin-top:.4rem';
        top.append(nm, bt);
        const sg = h('div', 'stage'); const ci = img(c.image); if (ci) sg.append(ci);
        const ss = h('div', 'stats');
        for (const s of arr(c.stats)) { const x = h('div', 'stat', s.label); x.append(h('b', 'ellip', s.value)); ss.append(x); }
        k.append(top, sg, ss);
        if (c.climate) {
          const cl = h('div', 'clim' + (c.climate.on ? ' on' : '')); cl.append(icon('fan'));
          const t = h('div', '', 'Climate ' + (c.climate.on ? 'on' : 'off')); t.style.flex = 1;
          t.append(h('small', '', c.climate.note || '')); cl.append(t);
          if (c.climate.set) cl.append(h('span', 'dim', c.climate.set));
          k.append(cl);
        }
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
      const rest = cs.filter(c => !used.has(c.name)); if (rest.length) groups.push({ name: groups.length ? 'Other' : 'Containers', items: rest });
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
  function draw(sec) {
    const r = R[sec.dataset.type];
    if (!r) return;
    try { r(sec, state[sec.id] || {}, props(sec)); } catch (e) { console.error(sec.id, e); }
  }
  function rerenderTimed() {
    document.querySelectorAll('.w-agenda,.w-worldclock,.w-shows').forEach(draw);
  }
  widgets.forEach(draw);
  tickClock();

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
  }

  function connect() {
    const es = new EventSource('/api/stream/' + encodeURIComponent(TF.frame));
    es.onmessage = ev => {
      let m; try { m = JSON.parse(ev.data); } catch { return; }
      if (m.reload || (m.page_version && m.page_version !== TF.pageVersion)) { location.reload(); return; }
      if (!m.widget) return;
      const sec = document.getElementById('w-' + m.widget);
      if (!sec) return;
      for (const [k, v] of Object.entries(m.data || {})) {
        if (typeof v === 'object' && v !== null) continue;
        const el = sec.querySelector('[data-field="' + k + '"]');
        if (el) el.textContent = v;
      }
      if (R[sec.dataset.type]) { state[sec.id] = Object.assign(state[sec.id] || {}, m.data || {}); draw(sec); }
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

  let toastT;
  window.toast = t => { const el = document.getElementById('toast'); el.textContent = t; el.classList.add('show'); clearTimeout(toastT); toastT = setTimeout(() => el.classList.remove('show'), 2500); };
})();
