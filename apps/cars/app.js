(function () {
  'use strict';
  var META = {
    ares: { name: 'Ares', model: 'Model S', image: '/api/data/asset/ares.png' },
    pegasus: { name: 'Pegasus', model: 'Model X', image: '/api/data/asset/pegasus.png' }
  };
  var ORDER = ['ares', 'pegasus'];
  // Line icons drawn like the Tesla app's: padlock, car in profile with the hood or the
  // liftgate up, the NACS port face, a bolt, the Sentry "eye", a fan, a headlamp.
  var SVG = {
    lock: '<rect x="5.5" y="10.5" width="13" height="10" rx="2.6"/><path d="M8.5 10.5V7.9a3.5 3.5 0 0 1 7 0v2.6"/><path d="M12 14.6v2"/>',
    unlock: '<rect x="5.5" y="10.5" width="13" height="10" rx="2.6"/><path d="M8.5 10.5V7.9a3.5 3.5 0 0 1 6.9-.9"/><path d="M12 14.6v2"/>',
    frunk: '<path d="M2.5 16.5v-2.3c0-.5.4-.9.9-.9h5L11 9.6c.4-.5 1-.8 1.6-.8h3.3c.7 0 1.4.3 1.8.9l2.2 2.8c.9.3 1.6 1.1 1.6 2.1v1.9"/><path d="M8.4 13.3 3.6 9.9"/><path d="M8.9 16.5h6.2"/><circle cx="7" cy="16.5" r="1.9"/><circle cx="17" cy="16.5" r="1.9"/>',
    trunk: '<path d="M2.5 16.5v-1.9c0-1 .7-1.8 1.6-2.1l4.3-1L11 8.9c.4-.4 1-.6 1.6-.6h4.3"/><path d="M16.9 8.3 21.4 6"/><path d="M19.5 12.4h.9c.6 0 1.1.5 1.1 1.1v3"/><path d="M8.9 16.5h6.2"/><circle cx="7" cy="16.5" r="1.9"/><circle cx="17" cy="16.5" r="1.9"/>',
    port: '<path d="M8 3v4M13 3v4"/><path d="M5.5 7h10v3.5a5 5 0 0 1-10 0z"/><path d="M10.5 15.5v2a3 3 0 0 0 3 3h2a3 3 0 0 0 3-3V8.5"/>',
    charge: '<path d="M13.4 2.8 5.6 13.1h5.7l-1.1 8.1 8.2-10.6h-5.8z"/>',
    sentry: '<circle cx="12" cy="12" r="8.6"/><circle class="eye" cx="12" cy="12" r="3.3" fill="currentColor" stroke="none"/>',
    climate: '<circle cx="12" cy="12" r="1.7"/><path d="M12 10.3c-.9-3.9.8-6.6 3-5.4s.3 5-3 5.4zM13.7 12c3.9-.9 6.6.8 5.4 3s-5 .3-5.4-3zM12 13.7c.9 3.9-.8 6.6-3 5.4s-.3-5 3-5.4zM10.3 12c-3.9.9-6.6-.8-5.4-3s5-.3 5.4 3z"/>',
    flash: '<path d="M11 6.2c-3.8 0-6.5 2.6-6.5 5.8s2.7 5.8 6.5 5.8z"/><path d="M14.5 7.5h5.5M14.5 12h5.5M14.5 16.5h5.5"/>'
  };
  // The cars in profile are wide and low: crop their box so they show as big as the others.
  var BOX = { frunk: '1.5 4.5 21 15', trunk: '1.5 4.5 21 15' };
  function svg(name) {
    return '<svg viewBox="' + (BOX[name] || '0 0 24 24') + '" width="26" height="26" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">' + (SVG[name] || '') + '</svg>';
  }
  var CONFIRM = { unlock: 'Tap again to unlock', frunk: 'Tap again to open frunk', trunk_open: 'Tap again to open trunk', trunk_close: 'Tap again to close trunk', charge_stop: 'Tap again to stop charging' };
  var root = document.getElementById('cars');
  var toastEl = document.getElementById('toast');
  var tpl = document.getElementById('tpl');
  var cards = {};
  var data = {};
  var armed = null;      // {id, act, el, timer}
  var pending = {};      // id -> {temp, limit, timer}
  var toastT = null;

  function toast(msg, err) {
    toastEl.textContent = msg;
    toastEl.className = 'toast show' + (err ? ' err' : '');
    clearTimeout(toastT);
    toastT = setTimeout(function () { toastEl.className = 'toast'; }, err ? 5000 : 2600);
  }

  function ago(iso) {
    var t = Date.parse(iso || '');
    if (isNaN(t)) return '';
    var m = Math.round((Date.now() - t) / 60000);
    if (m < 1) return 'just now';
    if (m < 60) return m + ' min ago';
    var h = Math.floor(m / 60);
    return h < 24 ? h + ' h ago' : Math.floor(h / 24) + ' d ago';
  }

  function build(id) {
    var n = tpl.content.firstElementChild.cloneNode(true);
    n.dataset.car = id;
    n.querySelector('.model').textContent = META[id].model;
    n.querySelector('.name').textContent = META[id].name;
    var img = n.querySelector('.stage img');
    img.src = META[id].image;
    img.onerror = function () { img.style.visibility = 'hidden'; };
    Array.prototype.forEach.call(n.querySelectorAll('button[data-act] > .ic'), function (s) { s.innerHTML = svg(s.parentNode.dataset.act); });
    n.addEventListener('click', function (e) {
      var b = e.target.closest('button[data-act]');
      if (b) onTap(id, b);
    });
    root.appendChild(n);
    return n;
  }

  function q(card, sel) { return card.querySelector(sel); }
  function setBtn(card, act, label, cls, icon) {
    var b = q(card, 'button[data-act="' + act + '"]');
    if (!b || b.classList.contains('busy') || b.classList.contains('arm')) return;
    q(b, '.t').textContent = label;
    if (icon) q(b, '.ic').innerHTML = svg(icon);
    b.classList.toggle('on', cls === 'on');
    b.classList.toggle('alert', cls === 'alert');
  }

  function render() {
    ORDER.forEach(function (id) {
      var c = data[id];
      var card = cards[id] || (cards[id] = build(id));
      if (!c) return;
      var pct = typeof c.battery === 'number' ? Math.round(c.battery) : null;
      q(card, '.pv').textContent = pct === null ? '--' : pct;
      q(card, '.pct').classList.toggle('chg', !!c.charging);
      q(card, '.bar').classList.toggle('chg', !!c.charging);
      q(card, '.bar i').style.width = (pct || 0) + '%';
      q(card, '.sub').textContent = [c.range, c.status, ago(c.updated) ? 'updated ' + ago(c.updated) : ''].filter(Boolean).join(' · ');
      q(card, '.sleep').hidden = c.online !== false;
      var cl = c.climate || {};
      var cb = q(card, '.cbtn');
      if (!cb.classList.contains('busy')) {
        cb.classList.toggle('on', !!cl.on);
        q(cb, '.cl').textContent = cl.on ? 'Climate on' : 'Climate';
        q(cb, '.cn').textContent = cl.on ? (cl.note || 'Running') : (cl.note ? 'Off · ' + cl.note : 'Off');
      }
      var p = pending[id] || {};
      var st = p.temp !== undefined ? p.temp : c.set_temp;
      q(card, '.tv').textContent = typeof st === 'number' ? Math.round(st) + '°' : '--°';
      var lim = p.limit !== undefined ? p.limit : c.limit;
      q(card, '.lv').textContent = typeof lim === 'number' ? lim + '%' : '--%';
      if (c.locked === false) setBtn(card, 'lock', 'Unlocked', 'alert', 'unlock');
      else setBtn(card, 'lock', 'Locked', '', 'lock');
      setBtn(card, 'frunk', c.frunk === 'open' ? 'Frunk open' : 'Frunk', c.frunk === 'open' ? 'alert' : '');
      setBtn(card, 'trunk', c.trunk === 'open' ? 'Close trunk' : 'Trunk', c.trunk === 'open' ? 'alert' : '');
      setBtn(card, 'port', c.port === 'open' ? 'Close port' : 'Charge port', c.port === 'open' ? 'on' : '');
      var chgLabel = c.charging ? 'Stop charging' : (c.cable ? (c.charge_state === 'complete' ? 'Charged' : 'Start charging') : 'Not plugged in');
      setBtn(card, 'charge', chgLabel, c.charging ? 'on' : '');
      setBtn(card, 'sentry', c.sentry ? 'Sentry on' : 'Sentry off', c.sentry ? 'on' : '');
    });
  }

  function disarm() {
    if (!armed) return;
    clearTimeout(armed.timer);
    armed.el.classList.remove('arm');
    armed = null;
    render();
  }

  function actionFor(id, act) {
    var c = data[id] || {};
    switch (act) {
      case 'lock': return c.locked === false ? 'lock' : 'unlock';
      case 'frunk': return c.frunk === 'open' ? null : 'frunk';
      case 'trunk': return c.trunk === 'open' ? 'trunk_close' : 'trunk_open';
      case 'port': return c.port === 'open' ? 'port_close' : 'port_open';
      case 'charge': return c.charging ? 'charge_stop' : (c.cable && c.charge_state !== 'complete' ? 'charge_start' : null);
      case 'sentry': return c.sentry ? 'sentry_off' : 'sentry_on';
      case 'climate': return (c.climate && c.climate.on) ? 'climate_off' : 'climate_on';
      case 'flash': return 'flash';
    }
    return null;
  }

  var LABEL = { lock: 'locked', unlock: 'unlocked', climate_on: 'climate on', climate_off: 'climate off', frunk: 'frunk opened', trunk_open: 'trunk opened', trunk_close: 'trunk closed', port_open: 'charge port opened', port_close: 'charge port closed', charge_start: 'charging started', charge_stop: 'charging stopped', sentry_on: 'Sentry on', sentry_off: 'Sentry off', flash: 'lights flashed', temp: 'temperature set', limit: 'charge limit set' };

  function send(id, action, value, el) {
    var c = data[id] || {};
    if (el) el.classList.add('busy');
    var slow = setTimeout(function () { toast(META[id].name + ': ' + (c.online === false ? 'waking the car…' : 'working…')); }, 2500);
    var body = { car: id, action: action };
    if (value !== undefined) body.value = value;
    return thinframe.emit('car', body).then(function (r) {
      clearTimeout(slow);
      if (el) el.classList.remove('busy');
      var msg = null, ok = false, parsed = null;
      try { parsed = r && r.body ? (typeof r.body === 'string' ? JSON.parse(r.body) : r.body) : null; } catch (e) { parsed = null; }
      if (r && r.ok && parsed && parsed.service_response) {
        ok = !!parsed.service_response.ok;
        msg = parsed.service_response.message;
      } else if (parsed && parsed.message) {
        msg = parsed.message;
      } else if (r && r.error) {
        msg = /timeout|deadline/i.test(r.error) ? 'Still working — the car may be waking up' : r.error;
      }
      if (ok) { assume(id, action); toast(META[id].name + ' ' + (LABEL[action] || 'done')); }
      else toast(META[id].name + ': ' + (msg || 'command failed'), true);
      render();
    }, function () {
      clearTimeout(slow);
      if (el) el.classList.remove('busy');
      toast(META[id].name + ': no answer from the server', true);
    });
  }

  // Show what the car just confirmed at once, like the Tesla app; the next status update from
  // Home Assistant (a minute or so) replaces it with the car's own report.
  var AFTER = {
    lock: { locked: true }, unlock: { locked: false }, frunk: { frunk: 'open' },
    trunk_open: { trunk: 'open' }, trunk_close: { trunk: 'closed' },
    port_open: { port: 'open' }, port_close: { port: 'closed' },
    charge_start: { charging: true }, charge_stop: { charging: false },
    sentry_on: { sentry: true }, sentry_off: { sentry: false }
  };
  function assume(id, action) {
    var c = data[id];
    if (!c) return;
    var a = AFTER[action];
    if (a) Object.keys(a).forEach(function (k) { c[k] = a[k]; });
    if (action === 'climate_on' || action === 'climate_off') {
      c.climate = Object.assign({}, c.climate || {}, { on: action === 'climate_on' });
    }
  }

  function bump(id, kind, delta) {
    var c = data[id] || {};
    var p = pending[id] || (pending[id] = {});
    if (kind === 'temp') {
      var t = p.temp !== undefined ? p.temp : (typeof c.set_temp === 'number' ? Math.round(c.set_temp) : 70);
      p.temp = Math.max(60, Math.min(82, t + delta));
    } else {
      var l = p.limit !== undefined ? p.limit : (typeof c.limit === 'number' ? c.limit : 80);
      p.limit = Math.max(50, Math.min(100, l + delta));
    }
    render();
    clearTimeout(p['t_' + kind]);
    p['t_' + kind] = setTimeout(function () {
      var v = p[kind];
      send(id, kind, v, null).then(function () {
        // Keep the shown value until the car data catches up (next poll).
        setTimeout(function () { if (p[kind] === v) { delete p[kind]; render(); } }, 20000);
      });
    }, 1200);
  }

  function onTap(id, el) {
    var act = el.dataset.act;
    if (el.classList.contains('busy')) return;
    if (act === 'tup') return bump(id, 'temp', 1);
    if (act === 'tdown') return bump(id, 'temp', -1);
    if (act === 'lup') return bump(id, 'limit', 5);
    if (act === 'ldown') return bump(id, 'limit', -5);
    var action = actionFor(id, act);
    if (!action) {
      if (act === 'frunk') toast('The frunk closes by hand');
      else if (act === 'charge') toast(data[id] && data[id].cable ? 'Charge is complete' : 'Plug the car in first');
      return;
    }
    if (CONFIRM[action]) {
      if (armed && armed.el === el && armed.action === action) {
        disarm();
        return send(id, action, undefined, el);
      }
      disarm();
      el.classList.add('arm');
      q(el, '.t') && (q(el, '.t').textContent = CONFIRM[action]);
      armed = { el: el, action: action, timer: setTimeout(disarm, 4000) };
      return;
    }
    disarm();
    send(id, action, undefined, el);
  }

  function apply(d) {
    if (!d || !Array.isArray(d.cars)) return;
    d.cars.forEach(function (c) { if (c && META[c.id]) data[c.id] = c; });
    render();
  }

  ORDER.forEach(function (id) { cards[id] = build(id); });
  thinframe.ready.then(function () { thinframe.on('cars', apply); });
  setInterval(render, 60000);  // refresh "updated x min ago"
})();
