// thinframe app SDK. The server injects it into every app page (internal/api/apps.go) as
// window.thinframe; docs/apps.md and the MCP tool app_guide describe it.
//
// An app runs sandboxed (an opaque origin). Shown full screen it talks to the server itself:
// one event stream, the liveness beacon, reload on update. Inside an "app" widget it asks the
// dashboard around it over postMessage instead, so a page with several apps still has one
// stream (WebKit allows six connections per host).
(function () {
  'use strict';
  if (window.thinframe) return;
  const q = new URLSearchParams(location.search);
  const app = (location.pathname.match(/^\/apps\/([a-z0-9_-]+)\//) || [])[1] || '';
  const embedded = window.parent !== window;
  const enc = encodeURIComponent;
  const subs = {};
  let readyResolve;
  const tf = {
    app, embedded, frame: q.get('frame') || '', widget: '', props: {},
    ready: new Promise(r => { readyResolve = r; }),
  };

  function add(kind, key, cb) {
    const k = kind + ':' + key;
    (subs[k] = subs[k] || []).push(cb);
    return () => { subs[k] = (subs[k] || []).filter(f => f !== cb); };
  }
  function fire(kind, key, val) {
    for (const cb of (subs[kind + ':' + key] || []).concat(subs[kind + ':*'] || [])) {
      try { cb(val, key); } catch (e) { console.error(e); }
    }
  }
  function onServer(m) {
    if (!m) return;
    if (m.source) fire('source', m.source, m.data);
    else if (m.app_state && m.app_state.app === app) fire('state', m.app_state.key, m.app_state.value);
    else if (m.app_event && m.app_event.app === app) fire('event', m.app_event.name, m.app_event.data);
    else if (!embedded && (m.reload || m.app_reload === app)) location.reload();
  }

  // req(method, path, body) -> Promise<{status, body}> (body is text).
  let req;
  if (embedded) {
    let seq = 0;
    const pending = {};
    addEventListener('message', e => {
      const m = e.data;
      if (e.source !== window.parent || !m || m.tf !== 1) return;
      if (m.op === 'init') {
        tf.frame = m.frame; tf.widget = m.widget; tf.props = m.props || {};
        readyResolve(tf);
      } else if (m.op === 'reply' && pending[m.id]) {
        pending[m.id]({ status: m.status, body: m.body }); delete pending[m.id];
      } else if (m.op === 'server') onServer(m.msg);
    });
    req = (method, path, body) => tf.ready.then(() => new Promise(res => {
      const id = ++seq;
      pending[id] = res;
      parent.postMessage({ tf: 1, op: 'fetch', id, method, path: path(), body }, '*');
    }));
    parent.postMessage({ tf: 1, op: 'hello' }, '*');
    // Taps inside the app keep the dashboard's tab from timing out.
    addEventListener('pointerdown', () => parent.postMessage({ tf: 1, op: 'activity' }, '*'), { passive: true });
  } else {
    req = (method, path, body) => fetch(path(), body === undefined ? { method } : { method, body, headers: { 'Content-Type': 'text/plain' } })
      .then(r => r.text().then(t => ({ status: r.status, body: t })))
      .catch(e => ({ status: 0, body: String(e) }));
    readyResolve(tf);
    const stream = () => {
      // Any id works for app messages; a preview without ?frame= uses "preview".
      const es = new EventSource('/api/stream/' + enc(tf.frame || 'preview') + '?apps=1');
      let lost = false;
      es.onmessage = ev => { try { onServer(JSON.parse(ev.data)); } catch (e) { /* not ours */ } };
      es.onopen = () => { if (lost) refetch(); };
      es.onerror = () => { lost = true; es.close(); setTimeout(stream, 5000); };
    };
    stream();
    if (tf.frame) {
      // Liveness for the server's hung-page check: a hung app gets the browser restarted.
      const alive = () => navigator.sendBeacon('/api/frames/' + enc(tf.frame) + '/alive', '');
      alive();
      setInterval(alive, 30000);
    }
  }
  const json = r => { try { return JSON.parse(r.body); } catch (e) { return null; } };

  tf.data = key => req('GET', () => '/api/data/source/' + enc(key)).then(r => r.status === 200 ? json(r) : null);
  tf.on = (key, cb) => {
    const off = add('source', key, cb);
    tf.data(key).then(d => { if (d !== null) cb(d, key); });
    return off;
  };
  tf.emit = (name, data) => req('POST', () => '/api/frames/' + enc(tf.frame) + '/emit', JSON.stringify({ app, name, data }))
    .then(r => json(r) || { ok: false, status: r.status, error: r.body });
  tf.listen = (name, cb) => add('event', name, cb);
  tf.state = {
    all: () => req('GET', () => '/api/apps/' + app + '/state').then(r => (r.status === 200 && json(r)) || {}),
    get: key => tf.state.all().then(s => (key in s ? s[key] : null)),
    set: (key, value) => req('POST', () => '/api/apps/' + app + '/state/' + enc(key), JSON.stringify(value === undefined ? null : value))
      .then(r => r.status === 204),
    watch: (key, cb) => {
      const off = add('state', key, cb);
      tf.state.get(key).then(v => cb(v, key));
      return off;
    },
  };
  // After a lost stream, or after the frame paused the browser (screen off), messages may have
  // been missed: fetch the watched sources and state again.
  function refetch() {
    const keys = kind => Object.keys(subs).filter(k => k.startsWith(kind + ':') && k !== kind + ':*' && subs[k].length).map(k => k.slice(kind.length + 1));
    for (const key of keys('source')) tf.data(key).then(d => { if (d !== null) fire('source', key, d); });
    const st = keys('state');
    if (st.length) tf.state.all().then(all => st.forEach(key => fire('state', key, key in all ? all[key] : null)));
  }
  let lastTick = Date.now();
  setInterval(() => { const now = Date.now(); if (now - lastTick > 15000) refetch(); lastTick = now; }, 5000);
  // Errors go to the server (get_app_events shows them as app-error), at most 5 per load.
  let errors = 0;
  const report = (msg, where) => {
    if (errors++ < 5) tf.ready.then(() => tf.emit('app-error', { message: String(msg).slice(0, 500), where: String(where || '').slice(0, 200) }));
  };
  addEventListener('error', e => report(e.message, (e.filename || '') + ':' + (e.lineno || 0)));
  addEventListener('unhandledrejection', e => report(e.reason && (e.reason.stack || e.reason.message) || e.reason, 'promise'));
  window.thinframe = tf;
})();
