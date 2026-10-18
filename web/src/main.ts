// The page: files in, the worker reads them, the tabs and the map show it.

import './style.css';
import { MapPane, type View } from './map';
import type { FromWorker, ReadInfo, Report, Scan, Schema, ToWorker } from './protocol';
import { Table } from './table';
import { escape, overview, schema as schemaView } from './views';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
const send = (message: ToWorker, transfer: Transferable[] = []) => worker.postMessage(message, transfer);

const state = {
  scan: null as Scan | null,
  scanMs: 0,
  layer: null as string | null,
  readId: 0,
  info: null as ReadInfo | null,
  report: null as Report | null,
  reading: false,
  rows: 0,
  limit: 100000 as number | null,
  grids: new Set<string>(),
  schemas: new Map<string, Schema>(),
  tab: 'overview',
  started: 0,
};

const status = $('status');
const layerSelect = $<HTMLSelectElement>('layer');
const limitInput = $<HTMLInputElement>('limit');
const stopButton = $<HTMLButtonElement>('stop');
const pointer = $('pointer');
const mapNote = $('mapNote');

/** What the map can't show, over the map. */
function updateMapNote() {
  const info = state.info;
  let note = '';
  if (info && !info.geometry_column) note = 'This layer has no geometry';
  else if (info && !info.map.target) note = info.map.notes[0] ?? '';
  else if (info?.map.notes.length) note = info.map.notes.join('; ');
  if (state.report?.not_on_map) {
    const n = state.report.not_on_map;
    note = [note, `${plural(n, 'feature')} not on the map: ${state.report.map_error}`].filter(Boolean).join('; ');
  }
  mapNote.textContent = note;
  mapNote.hidden = note === '';
}

function plural(n: number, word: string): string {
  return `${n.toLocaleString('en')} ${word}${n === 1 ? '' : 's'}`;
}

const map = new MapPane(
  $('map'),
  $<HTMLDivElement>('source'),
  (row) => select(row, 'map'),
  (text) => (pointer.textContent = text),
);

let rowRequest = 0;
const rowAnswers = new Map<number, (rows: string[][]) => void>();
const table = new Table(
  $('data'),
  (start, count) =>
    new Promise((resolve) => {
      const request = ++rowRequest;
      rowAnswers.set(request, resolve);
      send({ type: 'rows', id: state.readId, request, start, count });
    }),
  (row) => select(row, 'table'),
);

function select(row: number, from: 'map' | 'table') {
  table.select(row, from === 'map');
  map.select(row, from === 'table');
  if (from === 'map') showTab('data');
}

function setStatus(text: string, error = false) {
  status.textContent = text;
  status.classList.toggle('error', error);
}

// Files

async function open(files: File[]) {
  if (files.length === 0) return;
  const zips = files.filter((f) => f.name.toLowerCase().endsWith('.zip'));
  if (zips.length) {
    setStatus(`${zips[0].name}: zip archives aren't supported yet; unzip it and open the GML files`, true);
    return;
  }
  setStatus(`Reading ${files.length === 1 ? files[0].name : `${files.length} files`}…`);
  const loaded = await Promise.all(files.map(async (f) => ({ name: f.name, buffer: await f.arrayBuffer() })));
  state.scan = null;
  state.layer = null;
  state.schemas.clear();
  layerSelect.disabled = true;
  setStatus('Scanning…');
  state.started = performance.now();
  send({ type: 'open', files: loaded }, loaded.map((f) => f.buffer));
}

$<HTMLInputElement>('file').addEventListener('change', (event) => {
  const input = event.target as HTMLInputElement;
  open([...(input.files ?? [])]);
  input.value = '';
});

const drop = $('drop');
let dragDepth = 0;
window.addEventListener('dragenter', (event) => {
  if (!event.dataTransfer?.types.includes('Files')) return;
  dragDepth++;
  drop.hidden = false;
});
window.addEventListener('dragleave', () => {
  if (--dragDepth <= 0) {
    dragDepth = 0;
    drop.hidden = true;
  }
});
window.addEventListener('dragover', (event) => event.preventDefault());
window.addEventListener('drop', (event) => {
  event.preventDefault();
  dragDepth = 0;
  drop.hidden = true;
  open([...(event.dataTransfer?.files ?? [])]);
});

// Reading

function startRead(layer: string) {
  state.layer = layer;
  state.readId++;
  state.info = null;
  state.report = null;
  state.reading = true;
  state.rows = 0;
  state.grids.clear();
  state.started = performance.now();
  mapNote.hidden = true;
  stopButton.hidden = false;
  setStatus(`Reading ${layer}…`);
  send({ type: 'read', id: state.readId, layer, limit: state.limit });
  renderOverview();
  if (state.tab === 'schema') loadSchema();
}

layerSelect.addEventListener('change', () => startRead(layerSelect.value));
limitInput.addEventListener('change', () => {
  const value = Number(limitInput.value);
  state.limit = limitInput.value.trim() === '' || !(value > 0) ? null : Math.floor(value);
  if (state.layer) startRead(state.layer);
});
stopButton.addEventListener('click', () => send({ type: 'stop', id: state.readId }));

worker.onmessage = (event: MessageEvent<FromWorker>) => {
  const message = event.data;
  if ('id' in message && message.type !== 'rows' && message.id !== state.readId) return;
  switch (message.type) {
    case 'scanned': {
      state.scan = message.scan;
      state.scanMs = message.ms;
      const layers = message.scan.layers;
      layerSelect.replaceChildren(
        ...layers.map((l) => new Option(`${l.name} (${l.feature_count.toLocaleString('en')})`, l.name)),
      );
      layerSelect.disabled = layers.length === 0;
      if (layers.length === 0) {
        setStatus('No layers found', true);
        renderOverview();
        return;
      }
      const first = layers.find((l) => l.geometry_columns.length > 0) ?? layers[0];
      layerSelect.value = first.name;
      startRead(first.name);
      break;
    }
    case 'info':
      state.info = message.info;
      map.reset(message.info);
      table.reset(message.info.columns);
      syncViewButtons();
      updateMapNote();
      renderOverview();
      break;
    case 'grids':
      message.files.forEach((f) => state.grids.add(f));
      setStatus(`Fetching grid ${message.files.join(', ')}…`);
      break;
    case 'batch':
      state.rows = message.firstRow + message.rows;
      map.addBatch(
        { firstRow: message.firstRow, rows: message.rows, map: message.map, source: message.source },
        message.origin,
      );
      table.setRowCount(state.rows);
      setStatus(`Reading ${state.layer}… ${plural(state.rows, 'row')}`);
      break;
    case 'done':
    case 'stopped': {
      state.reading = false;
      state.report = message.report;
      if (message.type === 'done') state.info = message.info;
      stopButton.hidden = true;
      map.fit();
      const seconds = ((performance.now() - state.started) / 1000).toFixed(1);
      setStatus(
        `${plural(message.report.rows, 'row')} of ${state.layer} in ${seconds} s${message.type === 'stopped' ? ' (stopped)' : ''}`,
      );
      updateMapNote();
      renderOverview();
      break;
    }
    case 'rows':
      rowAnswers.get(message.request)?.(message.rows);
      rowAnswers.delete(message.request);
      break;
    case 'schema':
      state.schemas.set(message.layer, message.schema);
      if (message.layer === state.layer) renderSchema();
      break;
    case 'error':
      state.reading = false;
      stopButton.hidden = true;
      setStatus(message.message, true);
      renderOverview(message.message);
      break;
  }
};

worker.onerror = (event) => setStatus(`The worker failed: ${event.message}`, true);

// Tabs

function showTab(tab: string) {
  state.tab = tab;
  document.querySelectorAll<HTMLElement>('#tabs [data-tab]').forEach((b) => {
    b.classList.toggle('active', b.dataset.tab === tab);
    b.setAttribute('aria-selected', String(b.dataset.tab === tab));
  });
  document.querySelectorAll<HTMLElement>('.tab').forEach((t) => t.classList.toggle('active', t.id === tab));
  if (tab === 'schema') loadSchema();
}

document.querySelectorAll<HTMLElement>('#tabs [data-tab]').forEach((b) =>
  b.addEventListener('click', () => showTab(b.dataset.tab!)),
);

function renderOverview(error?: string) {
  const element = $('overview');
  if (!state.scan) {
    if (error) element.innerHTML = `<section><h2>Couldn't read the input</h2><p class="error">${escape(error)}</p></section>`;
    return;
  }
  element.innerHTML =
    (error ? `<section><p class="error">${escape(error)}</p></section>` : '') +
    overview({
      scan: state.scan,
      scanMs: state.scanMs,
      layer: state.layer,
      info: state.info,
      report: state.report,
      reading: state.reading,
      limit: state.limit,
      grids: [...state.grids],
    });
}

function loadSchema() {
  if (!state.layer) return;
  if (state.schemas.has(state.layer)) renderSchema();
  else send({ type: 'schema', layer: state.layer });
}

function renderSchema() {
  const s = state.layer ? state.schemas.get(state.layer) : undefined;
  if (s && state.layer) $('schema').innerHTML = schemaView(state.layer, s);
}

// Map views

function syncViewButtons() {
  document.querySelectorAll<HTMLButtonElement>('#views [data-view]').forEach((b) => {
    b.classList.toggle('active', b.dataset.view === map.currentView());
    if (b.dataset.view === 'map') {
      b.disabled = !map.hasMap();
      b.title = map.hasMap() ? '' : (state.info?.map.notes[0] ?? '');
    }
  });
}

document.querySelectorAll<HTMLButtonElement>('#views [data-view]').forEach((b) =>
  b.addEventListener('click', () => {
    map.setView(b.dataset.view as View);
    syncViewButtons();
  }),
);

// The divider between the map and the tabs.
const main = $('main');
$('divider').addEventListener('pointerdown', (event) => {
  const divider = event.target as HTMLElement;
  divider.setPointerCapture(event.pointerId);
  const move = (e: PointerEvent) => {
    const fraction = Math.min(0.8, Math.max(0.2, e.clientX / main.clientWidth));
    main.style.setProperty('--split', `${fraction * 100}%`);
    map.resize();
  };
  divider.addEventListener('pointermove', move);
  divider.addEventListener(
    'pointerup',
    () => {
      divider.removeEventListener('pointermove', move);
      map.resize();
    },
    { once: true },
  );
});
