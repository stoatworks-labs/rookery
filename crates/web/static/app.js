// rookery control page.
//
// No build step and no framework, matching flock and WebLinked's own control
// page. That has one consequence worth stating: a syntax error anywhere in
// this file kills every line after it and the page just sits on "connecting".
// If it looks dead, run this file's text through `new Function()` in a
// browser console — it names the line at once.

const state = {
  data: null,
  // { kind: 'instance' | 'group' | 'all', id?, tag? }
  selection: { kind: 'all' },
};

// ------------------------------------------------------------------ helpers

const $ = (sel) => document.querySelector(sel);

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function healthClass(health) {
  return 'h-' + (health || 'unknown');
}

async function api(path, options) {
  const res = await fetch(path, options);
  let body = null;
  try {
    body = await res.json();
  } catch (e) {
    body = null;
  }
  if (!res.ok) {
    const message = (body && body.error) || `HTTP ${res.status}`;
    throw new Error(message);
  }
  return body;
}

function selectionLabel() {
  const s = state.selection;
  if (s.kind === 'all') return 'All instances';
  if (s.kind === 'group') return `Group: ${s.tag}`;
  const inst = (state.data ? state.data.instances : []).find((i) => i.id === s.id);
  return inst ? inst.name : 'Instance';
}

function sendPath() {
  const s = state.selection;
  if (s.kind === 'all') return '/api/all/send';
  if (s.kind === 'group') return `/api/groups/${encodeURIComponent(s.tag)}/send`;
  return `/api/instances/${encodeURIComponent(s.id)}/send`;
}

// -------------------------------------------------------------- the targets

function renderTargets() {
  const list = $('#target-list');
  list.replaceChildren();
  if (!state.data) return;

  const allItem = el('li', 'target' + (state.selection.kind === 'all' ? ' selected' : ''));
  allItem.append(el('span', 'target-name', 'All instances'));
  allItem.append(el('span', 'count', String(state.data.instances.length)));
  allItem.onclick = () => { state.selection = { kind: 'all' }; render(); };
  list.append(allItem);

  for (const group of state.data.groups) {
    const item = el('li', 'target' +
      (state.selection.kind === 'group' && state.selection.tag === group.tag ? ' selected' : ''));
    item.append(el('span', 'dot ' + healthClass(group.health)));
    item.append(el('span', 'target-name', group.tag));
    item.append(el('span', 'count', String(group.members)));
    item.onclick = () => { state.selection = { kind: 'group', tag: group.tag }; render(); };
    list.append(item);
  }

  const divider = el('li', 'divider', 'Instances');
  list.append(divider);

  for (const inst of state.data.instances) {
    const item = el('li', 'target' +
      (state.selection.kind === 'instance' && state.selection.id === inst.id ? ' selected' : ''));
    item.append(el('span', 'dot ' + healthClass(inst.health)));
    item.append(el('span', 'target-name', inst.name));
    item.onclick = () => { state.selection = { kind: 'instance', id: inst.id }; render(); };
    list.append(item);
  }
}

// --------------------------------------------------------------- the detail

function selectedInstances() {
  if (!state.data) return [];
  const s = state.selection;
  if (s.kind === 'all') return state.data.instances;
  if (s.kind === 'group') return state.data.instances.filter((i) => i.tags.includes(s.tag));
  return state.data.instances.filter((i) => i.id === s.id);
}

function renderSourceRow(source) {
  const row = el('div', 'source');
  const head = el('div', 'source-head');
  head.append(el('span', 'dot ' + healthClass(source.__health)));
  head.append(el('span', 'source-id', source.id || 'primary'));
  if (source.format) head.append(el('span', 'chip', source.format));
  row.append(head);

  const url = (source.source && (source.source.loaded_url || source.source.url)) || '';
  if (url) row.append(el('div', 'url', url));

  const stats = el('div', 'stats');
  const pacing = source.pacing || {};
  if (pacing.dropped_ticks !== null && pacing.dropped_ticks !== undefined) {
    // Red for *currently* dropping, never for the cumulative count. A
    // long-running instance carries a large historic number forever, and
    // colouring that red makes every mature instance look broken — see
    // SourceState::dropping in rookery-core.
    const stat = el('span', source.dropping ? 'stat bad' : 'stat',
      `dropped ticks ${pacing.dropped_ticks}${source.dropping ? ' — dropping now' : ''}`);
    stat.title = source.dropping
      ? 'The dropped-tick count rose since the last poll: this source is behind its clock right now.'
      : 'Cumulative since this instance started. Steady means it is keeping up.';
    stats.append(stat);
  }
  if (source.source && source.source.console_errors) {
    stats.append(el('span', 'stat bad', `console errors ${source.source.console_errors}`));
  }
  if (source.source && source.source.audio_muted) {
    stats.append(el('span', 'stat', 'muted'));
  }
  row.append(stats);

  const outputs = el('div', 'outputs');
  for (const out of source.outputs || []) {
    const chip = el('span', 'output' + (out.error ? ' bad' : (out.running ? ' on' : ' off')));
    let label = `${out.kind}:${out.name}`;
    if (out.receivers !== null && out.receivers !== undefined) label += ` (${out.receivers})`;
    chip.textContent = label;
    if (out.error) chip.title = out.error;
    outputs.append(chip);
  }
  row.append(outputs);
  return row;
}

function renderInstances() {
  const container = $('#instances');
  container.replaceChildren();

  for (const inst of selectedInstances()) {
    const card = el('div', 'instance');

    const head = el('div', 'instance-head');
    head.append(el('span', 'dot ' + healthClass(inst.health)));
    head.append(el('span', 'instance-name', inst.name));
    head.append(el('span', 'muted small', `${inst.host}  osc ${inst.osc_port} · http ${inst.http_port}`));
    for (const tag of inst.tags) head.append(el('span', 'chip', tag));

    const remove = el('button', 'link danger', 'remove');
    remove.onclick = async () => {
      if (!confirm(`Remove ${inst.name} from rookery? The instance itself keeps running.`)) return;
      try {
        await api(`/api/instances/${inst.id}`, { method: 'DELETE' });
        logActivity(`removed ${inst.name}`, true);
      } catch (e) {
        logActivity(`remove ${inst.name}: ${e.message}`, false);
      }
    };
    head.append(remove);
    card.append(head);

    const st = inst.state || {};
    if (!st.polled) {
      card.append(el('p', 'muted small', 'Polling is off for this instance — commands are sent blind.'));
    } else if (st.error) {
      const msg = st.sources
        ? `Unreachable: ${st.error} (showing the last known state)`
        : `Unreachable: ${st.error}`;
      card.append(el('p', 'error small', msg));
    }

    if (st.sources && st.sources.sources) {
      for (const source of st.sources.sources) {
        // The server sends per-instance health only; recompute a per-source
        // hint here so a single bad pipeline is visible inside a healthy box.
        source.__health = sourceHealth(source);
        card.append(renderSourceRow(source));
      }
    } else if (st.polled && !st.error) {
      card.append(el('p', 'muted small', 'Waiting for the first poll…'));
    }

    container.append(card);
  }

  if (!container.children.length) {
    container.append(el('p', 'muted', 'No instances match this target.'));
  }
}

// Mirrors rookery-core's SourceState::health. Kept in step by hand; the
// server's own value is what colours the instance and group dots, so a drift
// here is cosmetic rather than misleading.
function sourceHealth(source) {
  if (source.running === false) return 'stopped';
  if ((source.outputs || []).some((o) => o.error)) return 'fault';
  // `dropping` is the server's delta between polls, not the cumulative
  // count — same rule as SourceState::health.
  if (source.dropping) return 'degraded';
  if ((source.outputs || []).some((o) => o.enabled === true && o.running === false)) return 'degraded';
  return source.running === true ? 'ok' : 'unknown';
}

function renderDetail() {
  $('#detail-title').textContent = selectionLabel();
  const count = selectedInstances().length;
  $('#detail-sub').textContent =
    count === 1 ? '1 instance' : `${count} instances`;
  $('#controls').classList.toggle('hidden', !state.data);
  renderInstances();
}

function render() {
  renderTargets();
  renderDetail();
}

// -------------------------------------------------------------- the sending

function logActivity(text, ok) {
  const list = $('#activity');
  const item = el('li', ok ? 'ok' : 'bad');
  item.append(el('span', 'time', new Date().toLocaleTimeString()));
  item.append(el('span', 'text', text));
  list.prepend(item);
  while (list.children.length > 40) list.lastChild.remove();
}

const DISRUPTIVE = new Set(['url', 'format', 'output']);

async function sendCommand(command) {
  const source = $('#source').value.trim();
  const body = Object.assign({}, command);
  if (source) body.source = source;

  // Confirm a disruptive command aimed at more than one machine, and name
  // them. "Change the URL on gfx-1, gfx-2 and gfx-5?" is answerable;
  // "change 3 instances?" is not.
  if (DISRUPTIVE.has(command.verb) && state.selection.kind !== 'instance') {
    const names = selectedInstances().map((i) => i.name);
    if (names.length > 1) {
      const list = names.join(', ');
      if (!confirm(`${command.verb} on ${names.length} instances — ${list}.\n\nThis changes what is on air. Continue?`)) {
        return;
      }
    }
  }

  try {
    const result = await api(sendPath(), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    const failed = result.entries.filter((e) => !e.sent);
    if (failed.length) {
      logActivity(
        `${result.command} → ${result.target}: sent to ${result.entries.length - failed.length} of ${result.entries.length}; ` +
        failed.map((f) => `${f.instance_name} (${f.error})`).join(', '),
        false,
      );
    } else {
      logActivity(`${result.command} → ${result.target}: sent to ${result.entries.length}`, true);
    }
  } catch (e) {
    logActivity(`${command.verb}: ${e.message}`, false);
  }
}

function wireControls() {
  $('#controls').addEventListener('click', (event) => {
    const button = event.target.closest('button[data-verb]');
    if (!button) return;
    const verb = button.dataset.verb;

    if (verb === 'url') {
      const url = $('#url').value.trim();
      if (!url) return;
      sendCommand({ verb: 'url', url });
    } else if (verb === 'reload') {
      sendCommand({ verb: 'reload', ignore_cache: false });
    } else if (verb === 'reload-nocache') {
      sendCommand({ verb: 'reload', ignore_cache: true });
    } else if (verb === 'mute-on') {
      sendCommand({ verb: 'mute', muted: true });
    } else if (verb === 'mute-off') {
      sendCommand({ verb: 'mute', muted: false });
    } else if (verb === 'script') {
      const script = $('#script').value.trim();
      if (!script) return;
      sendCommand({ verb: 'script', script });
    } else if (verb === 'format') {
      const format = $('#format').value.trim();
      if (!format) return;
      sendCommand({ verb: 'format', format });
    } else if (verb === 'output-on' || verb === 'output-off') {
      const name = $('#output-name').value.trim();
      if (!name) return;
      sendCommand({ verb: 'output', name, enabled: verb === 'output-on' });
    }
  });
}

// ----------------------------------------------------------------- add/scan

function wireAddForm() {
  $('#add-form').addEventListener('submit', async (event) => {
    event.preventDefault();
    const form = event.target;
    const data = new FormData(form);
    const body = {
      name: data.get('name'),
      host: data.get('host'),
      osc_port: Number(data.get('osc_port')) || undefined,
      http_port: Number(data.get('http_port')) || undefined,
      tags: String(data.get('tags') || '').split(',').map((t) => t.trim()).filter(Boolean),
      token: data.get('token') || undefined,
    };
    try {
      const created = await api('/api/instances', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      logActivity(`added ${created.name}`, true);
      form.reset();
      form.querySelector('[name=osc_port]').value = 7655;
      form.querySelector('[name=http_port]').value = 7654;
    } catch (e) {
      logActivity(`add: ${e.message}`, false);
    }
  });
}

function wireScan() {
  $('#scan').addEventListener('click', async () => {
    const button = $('#scan');
    button.disabled = true;
    button.textContent = 'Scanning…';
    const results = $('#scan-results');
    results.replaceChildren();
    try {
      const body = await api('/api/discovery/scan');
      if (!body.found.length) {
        results.append(el('li', 'muted small', 'Nothing found. Instances on a non-default port, or on another subnet, have to be added by hand.'));
      }
      for (const found of body.found) {
        const item = el('li', 'scan-result');
        let label = found.host;
        if (found.version) label += ` · ${found.version}`;
        if (found.needs_token) label += ' · needs a token';
        item.append(el('span', '', label));
        const add = el('button', 'link', 'add');
        add.onclick = () => {
          const form = $('#add-form');
          form.querySelector('[name=name]').value = found.host;
          form.querySelector('[name=host]').value = found.host;
          form.querySelector('[name=http_port]').value = found.http_port;
          form.querySelector('[name=name]').focus();
        };
        item.append(add);
        results.append(item);
      }
    } catch (e) {
      results.append(el('li', 'error small', e.message));
    } finally {
      button.disabled = false;
      button.textContent = 'Scan the LAN';
    }
  });
}

// ------------------------------------------------------------------ the wire

// Three states, not two. The page is useful from the moment the initial REST
// fetch lands, before the websocket is up — saying "connecting" over a screen
// full of live data is a small lie, and this page's whole job is telling an
// operator what is and is not true right now.
function setConnection(status) {
  const pill = $('#conn');
  const labels = { live: 'live', loaded: 'loaded', lost: 'disconnected' };
  const classes = { live: 'pill-ok', loaded: 'pill-unknown', lost: 'pill-bad' };
  pill.textContent = labels[status];
  pill.className = 'pill ' + classes[status];
  pill.title = status === 'live'
    ? 'Receiving live updates.'
    : status === 'loaded'
      ? 'Showing a snapshot; the live update stream has not connected yet.'
      : 'The live update stream dropped. Retrying — what is on screen may be stale.';
}

function renderNorthbound() {
  const pill = $('#northbound');
  if (!state.data) return;
  if (state.data.northbound) {
    pill.textContent = `OSC in ${state.data.northbound}`;
    pill.className = 'pill pill-ok';
    pill.title = `A desk can drive this fleet at ${state.data.northbound_prefix}/… — with no authentication.`;
  } else {
    pill.textContent = 'OSC in off';
    pill.className = 'pill pill-off';
    pill.title = 'The northbound OSC listener is disabled.';
  }
}

function connect() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const socket = new WebSocket(`${proto}://${location.host}/ws`);
  socket.onmessage = (event) => {
    state.data = JSON.parse(event.data);
    setConnection('live');
    renderNorthbound();
    render();
  };
  socket.onclose = () => {
    setConnection('lost');
    setTimeout(connect, 1000);
  };
  socket.onerror = () => socket.close();
}

async function init() {
  wireControls();
  wireAddForm();
  wireScan();
  try {
    state.data = await api('/api/state');
    setConnection('loaded');
    renderNorthbound();
    render();
  } catch (e) {
    logActivity(`could not read state: ${e.message}`, false);
  }
  connect();
}

init();
