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

  // The expanded pane is preserved across renders rather than rebuilt.
  //
  // State arrives every 500ms and this list is redrawn each time. Rebuilding
  // the pane with it destroyed the <select> mid-interaction and threw away
  // keyboard focus twice a second, which made typing into an armed preview
  // impossible — you cannot hold focus on an element that is replaced between
  // keystrokes. Found by driving the real UI, not by reading it.
  const keptPane = document.querySelector('#expanded');
  if (keptPane) keptPane.remove();

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

    const body = el('div', 'instance-body');
    body.append(renderThumb(inst));

    const detail = el('div', 'instance-detail');
    if (st.sources && st.sources.sources) {
      for (const source of st.sources.sources) {
        // The server sends per-instance health only; recompute a per-source
        // hint here so a single bad pipeline is visible inside a healthy box.
        source.__health = sourceHealth(source);
        detail.append(renderSourceRow(source));
      }
    } else if (st.polled && !st.error) {
      detail.append(el('p', 'muted small', 'Waiting for the first poll…'));
    }
    body.append(detail);
    card.append(body);

    if (preview.expanded === inst.id) {
      // Re-attach the live one if we have it; only build a new pane when the
      // expansion has actually moved to a different instance.
      const pane = keptPane && keptPane.dataset.instance === inst.id
        ? keptPane
        : renderExpanded(inst);
      card.append(pane);
    }

    container.append(card);
  }

  if (!container.children.length) {
    container.append(el('p', 'muted', 'No instances match this target.'));
  }
}

function renderThumb(inst) {
  const wrap = el('div', 'thumb');
  const img = el('img', 'empty');
  img.dataset.preview = inst.id;
  img.alt = `live preview of ${inst.name}`;
  // Held by the CSS at 16:9 so the box does not jump when the first frame
  // lands, and so an instance with no picture is still a box rather than a
  // collapsed line.
  wrap.append(img);

  const note = el('div', 'thumb-note');
  note.dataset.previewNote = inst.id;
  wrap.append(note);

  const tile = preview.tiles.get(inst.id);
  if (tile && tile.url) { img.src = tile.url; img.classList.remove('empty'); }
  if (tile && tile.unavailable) paintUnavailable(inst.id, tile.unavailable);

  wrap.onclick = () => {
    preview.expanded = preview.expanded === inst.id ? null : inst.id;
    // Arming never survives changing what you are looking at.
    setArmed(false);
    render();
  };
  wrap.title = preview.expanded === inst.id ? 'Close the large view' : 'Open the large view';
  return wrap;
}

function renderExpanded(inst) {
  const pane = el('div', 'expanded');
  pane.id = 'expanded';
  pane.dataset.instance = inst.id;
  pane.tabIndex = 0;   // so the pane can receive keydown once armed
  if (preview.armed) pane.classList.add('armed');

  const bar = el('div', 'expanded-bar');
  bar.append(el('span', 'expanded-name', inst.name));

  const arm = el('button', 'arm', 'Take control');
  arm.onclick = (e) => { e.stopPropagation(); setArmed(!preview.armed); };
  bar.append(arm);

  // The factor is the instance's own setting and changing it also changes what
  // that machine's control page shows, so it is offered explicitly rather than
  // applied behind anyone's back.
  const factor = el('select', 'factor');
  for (const [value, label] of [['', 'preview: as configured'], ['4', 'preview: 1/4 (480x270)'],
                                ['8', 'preview: 1/8 (240x135)'], ['2', 'preview: 1/2 (960x540)']]) {
    const option = el('option', null, label);
    option.value = value;
    if (String(inst.preview_factor ?? '') === value) option.selected = true;
    factor.append(option);
  }
  factor.title = 'Changes this instance\u2019s own preview output, so the picture on its ' +
    'local control page changes too. Lower is cheaper on the network.';
  factor.onchange = async () => {
    try {
      await api(`/api/instances/${inst.id}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: inst.name, host: inst.host, osc_port: inst.osc_port,
          http_port: inst.http_port, tags: inst.tags, poll: inst.poll,
          preview_factor: factor.value ? Number(factor.value) : null,
        }),
      });
      // The cached tile is the old raster; drop it so the next frame is not
      // compared against an ETag for a different size.
      preview.tiles.delete(inst.id);
      logActivity(`${inst.name}: preview factor ${factor.value || 'left as configured'}`, true);
    } catch (e) {
      logActivity(`preview factor: ${e.message}`, false);
    }
  };
  bar.append(factor);

  const close = el('button', 'link', 'close');
  close.onclick = () => { preview.expanded = null; setArmed(false); render(); };
  bar.append(close);
  pane.append(bar);

  const img = el('img', 'empty');
  img.dataset.preview = inst.id;
  img.alt = `live preview of ${inst.name}`;
  pane.append(img);
  const tile = preview.tiles.get(inst.id);
  if (tile && tile.url) { img.src = tile.url; img.classList.remove('empty'); }

  pane.append(el('div', 'arm-note', 'View only. Nothing you do here reaches the page.'));
  wireExpandedInput(pane, inst.id);
  return pane;
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


// ---------------------------------------------------------------- previews
//
// Each visible instance gets a small live picture; clicking one expands it
// into a bigger pane that can, once armed, take mouse and keyboard.
//
// Fetched by hand rather than by pointing an <img> at the URL, for one reason:
// this way the If-None-Match / 304 path is explicit and visible. rookery's
// ETag is WebLinked's paint sequence, so a graphic that is not moving costs a
// request with no body rather than a fresh JPEG several times a second. Setting
// img.src to the same URL would leave that to browser cache heuristics.

const preview = {
  // instanceId -> { etag, url, unavailable, width, height, inflight }
  tiles: new Map(),
  expanded: null,     // instance id, or null
  armed: false,       // take-control, always false until deliberately set
  lastPointerSent: 0,
  pending: [],        // batched pointer motion
};

const WALL_INTERVAL_MS = 250;     // ~4 fps for the thumbnails
const FOCUS_INTERVAL_MS = 80;     // ~12 fps for the one being watched
const WALL_QUALITY = 55;
const FOCUS_QUALITY = 75;

function previewUrl(id, quality) {
  const source = $('#source').value.trim();
  const params = new URLSearchParams({ quality: String(quality) });
  if (source) params.set('source', source);
  return `/api/instances/${encodeURIComponent(id)}/preview?${params}`;
}

async function fetchTile(id, quality) {
  const tile = preview.tiles.get(id) || {};
  if (tile.inflight) return;                 // never queue behind a slow instance
  tile.inflight = true;
  preview.tiles.set(id, tile);
  try {
    const headers = tile.etag ? { 'If-None-Match': tile.etag } : {};
    const res = await fetch(previewUrl(id, quality), { headers });

    if (res.status === 304) { tile.unavailable = null; return; }
    if (res.status === 204) {
      // A working instance with no picture: --no-preview, or not painted yet.
      tile.unavailable = res.headers.get('X-Preview-Unavailable') || 'unavailable';
      if (tile.url) { URL.revokeObjectURL(tile.url); tile.url = null; }
      return;
    }
    if (!res.ok) { tile.unavailable = `http ${res.status}`; return; }

    const blob = await res.blob();
    if (tile.url) URL.revokeObjectURL(tile.url);
    tile.url = URL.createObjectURL(blob);
    tile.etag = res.headers.get('ETag');
    tile.unavailable = null;
    paintTile(id, tile);
  } catch (e) {
    tile.unavailable = 'unreachable';
  } finally {
    tile.inflight = false;
  }
}

function paintTile(id, tile) {
  for (const img of document.querySelectorAll(`img[data-preview="${id}"]`)) {
    img.src = tile.url;
    img.classList.remove('empty');
  }
  for (const note of document.querySelectorAll(`[data-preview-note="${id}"]`)) {
    note.textContent = '';
  }
}

function paintUnavailable(id, why) {
  for (const note of document.querySelectorAll(`[data-preview-note="${id}"]`)) {
    note.textContent = why === 'not-configured'
      ? 'started --no-preview'
      : (why === 'no-frame-yet' ? 'waiting for the first frame' : why);
  }
}

async function previewLoop() {
  for (;;) {
    const visible = selectedInstances().map((i) => i.id);
    const jobs = [];
    for (const id of visible) {
      const focused = id === preview.expanded;
      jobs.push(fetchTile(id, focused ? FOCUS_QUALITY : WALL_QUALITY));
    }
    await Promise.allSettled(jobs);
    for (const [id, tile] of preview.tiles) {
      if (tile.unavailable) paintUnavailable(id, tile.unavailable);
    }
    await new Promise((r) => setTimeout(r,
      preview.expanded ? FOCUS_INTERVAL_MS : WALL_INTERVAL_MS));
  }
}

// ------------------------------------------------------------- interaction

function normalised(img, event) {
  const rect = img.getBoundingClientRect();
  return {
    nx: Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width)),
    ny: Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height)),
  };
}

async function sendInput(id, events) {
  if (!events.length) return;
  const source = $('#source').value.trim();
  const params = source ? `?source=${encodeURIComponent(source)}` : '';
  try {
    await api(`/api/instances/${encodeURIComponent(id)}/input${params}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ events }),
    });
  } catch (e) {
    logActivity(`input: ${e.message}`, false);
  }
}

// Pointer motion is batched: a drag is one request rather than sixty.
function queuePointer(id, event) {
  preview.pending.push(event);
  const now = performance.now();
  if (now - preview.lastPointerSent < 50) return;
  preview.lastPointerSent = now;
  const batch = preview.pending;
  preview.pending = [];
  sendInput(id, batch);
}

function keyEvents(e) {
  // WebLinked wants three events per character, and `character` must be the
  // CHARACTER code — 104 is `h`, 72 is `H`. With only a virtual key code the
  // page sees e.key as "Unidentified" and a graphic listening for a key never
  // fires.
  const printable = e.key.length === 1;
  const character = printable ? e.key.charCodeAt(0) : 0;
  const keyCode = printable ? character : e.keyCode;
  const modifiers =
    (e.shiftKey ? 1 << 1 : 0) | (e.ctrlKey ? 1 << 2 : 0) |
    (e.altKey ? 1 << 3 : 0) | (e.metaKey ? 1 << 7 : 0);

  const base = { type: 'key', key_code: keyCode, character, modifiers };
  return printable
    ? [{ ...base, action: 'down' }, { ...base, action: 'char' }, { ...base, action: 'up' }]
    : [{ ...base, action: 'down' }, { ...base, action: 'up' }];
}

function wireExpandedInput(pane, id) {
  const img = pane.querySelector('img');

  img.addEventListener('mousemove', (e) => {
    if (!preview.armed) return;
    queuePointer(id, { type: 'move', ...normalised(img, e) });
  });
  img.addEventListener('mousedown', (e) => {
    if (!preview.armed) return;
    e.preventDefault();
    // Focus first: an offscreen browser that has never had input may not route
    // keystrokes without it, and it costs one event.
    sendInput(id, [
      { type: 'focus', focused: true },
      { type: 'down', ...normalised(img, e), button: e.button, clicks: e.detail || 1 },
    ]);
  });
  img.addEventListener('mouseup', (e) => {
    if (!preview.armed) return;
    sendInput(id, [{ type: 'up', ...normalised(img, e), button: e.button }]);
  });
  img.addEventListener('wheel', (e) => {
    if (!preview.armed) return;
    e.preventDefault();
    sendInput(id, [{ type: 'wheel', ...normalised(img, e), dx: -e.deltaX, dy: -e.deltaY }]);
  }, { passive: false });

  pane.addEventListener('keydown', (e) => {
    if (!preview.armed) return;
    // Let the operator out without the page swallowing it.
    if (e.key === 'Escape') { setArmed(false); return; }
    e.preventDefault();
    sendInput(id, keyEvents(e));
  });
}

function setArmed(on) {
  preview.armed = on;
  const pane = $('#expanded');
  if (!pane) return;
  pane.classList.toggle('armed', on);
  const button = pane.querySelector('.arm');
  if (button) button.textContent = on ? 'Release control (Esc)' : 'Take control';
  const note = pane.querySelector('.arm-note');
  if (note) {
    note.textContent = on
      ? 'Live — clicks and keystrokes go to the page that is on air.'
      : 'View only. Nothing you do here reaches the page.';
  }
  if (on) pane.focus();
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
        let label = found.name ? `${found.name} · ${found.host}` : found.host;
        if (found.version) label += ` · ${found.version}`;
        if (found.needs_token) label += ' · needs a token';
        item.append(el('span', '', label));

        // The distinction that matters when the operator clicks add. An
        // advertised instance told us its OSC port; a swept one did not, and
        // the form is about to be filled with the default. Saying so here is
        // the only warning there will ever be — a wrong OSC port produces an
        // instance that polls green and silently ignores every cue, because
        // OSC has no replies to notice the absence of.
        if (found.found_via === 'mdns') {
          const osc = found.osc_port ? `OSC ${found.osc_port}` : 'OSC off';
          item.append(el('span', 'muted small', ` advertised · ${osc}`));
        } else {
          item.append(el('span', 'muted small', ' found by scanning · OSC port assumed'));
        }

        const add = el('button', 'link', 'add');
        add.onclick = () => {
          const form = $('#add-form');
          form.querySelector('[name=name]').value = found.name || found.host;
          form.querySelector('[name=host]').value = found.host;
          form.querySelector('[name=http_port]').value = found.http_port;
          // Only when the instance actually said so; otherwise leave whatever
          // the form defaults to, which is visibly a default.
          if (found.osc_port) {
            const oscField = form.querySelector('[name=osc_port]');
            if (oscField) oscField.value = found.osc_port;
          }
          if (found.osc_prefix) {
            const prefixField = form.querySelector('[name=osc_prefix]');
            if (prefixField) prefixField.value = found.osc_prefix;
          }
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
  previewLoop();
}

init();
