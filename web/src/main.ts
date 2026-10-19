// The page: files in, the worker reads them, the tabs and the map show it.

import './style.css';
import { MapPane, type View } from './map';
import { bytes, DEFAULT_SAMPLE, OpenDialog, type OpenRequest } from './open';
import type { Choice, FeatureXml, FromWorker, InputFile, ReadInfo, Report, Scan, Schema, ToWorker } from './protocol';
import { Table } from './table';
import { escape, overview, schema as schemaView } from './views';
import { wantsIndent, xmlView } from './xml';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// A scan is one call into the module: stopping it means a new worker.
let worker = startWorker();
const send = (message: ToWorker, transfer: Transferable[] = []) => worker.postMessage(message, transfer);

/** A new worker, holding the same files. */
function restartWorker() {
  worker.terminate();
  worker = startWorker();
  state.scanning = false;
  if (state.files.length) send({ type: 'open', files: state.files });
}

function startWorker(): Worker {
  const w = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
  w.onmessage = (event: MessageEvent<FromWorker>) => receive(event.data);
  w.onerror = (event) => setStatus(`The worker failed: ${event.message}`, true);
  return w;
}

const state = {
  /** The files opened, and what is in them. */
  files: [] as File[],
  listing: null as InputFile[] | null,
  /** Show the dialog when the worker has listed the files. */
  choose: false,
  /** What the dialog asked for last. */
  choices: null as Choice[] | null,
  sample: null as number | null,
  scan: null as Scan | null,
  scanMs: 0,
  layer: null as string | null,
  readId: 0,
  info: null as ReadInfo | null,
  report: null as Report | null,
  reading: false,
  scanning: false,
  rows: 0,
  limit: 100000 as number | null,
  grids: new Set<string>(),
  schemas: new Map<string, Schema>(),
  tab: 'overview',
  started: 0,
  /** The row selected on the map or in the table. */
  selected: null as number | null,
  /** The selected row's XML, once the worker has read it. */
  xml: null as FeatureXml | null,
  /** Indent the XML: `null` until the user chooses, then what they chose. */
  indent: null as boolean | null,
};

const status = $('status');
const layerSelect = $<HTMLSelectElement>('layer');
const limitInput = $<HTMLInputElement>('limit');
const stopButton = $<HTMLButtonElement>('stop');
const changeButton = $<HTMLButtonElement>('change');
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
  state.selected = row;
  // The table shows first: a hidden one can't scroll to the row.
  if (from === 'map' && state.tab !== 'xml') showTab('data');
  table.select(row, from === 'map');
  map.select(row, from === 'table');
  if (state.tab === 'xml') loadXml();
}

const xmlTab = $('xml');

/** The selected row's XML, read again from its file by the worker. */
function loadXml() {
  const row = state.selected;
  if (row === null) {
    xmlTab.innerHTML = '<p class="muted">Select a feature on the map or a row in the Data tab to see its XML.</p>';
    return;
  }
  if (state.xml?.row === row) return renderXml();
  state.xml = null;
  xmlTab.innerHTML = `<p class="muted">Reading the XML of row ${(row + 1).toLocaleString('en')}…</p>`;
  send({ type: 'xml', id: state.readId, row });
}

function renderXml() {
  const xml = state.xml;
  if (!xml) return;
  xmlTab.innerHTML = xmlView(xml, state.indent ?? wantsIndent(xml.xml));
}

xmlTab.addEventListener('change', (event) => {
  const box = event.target as HTMLInputElement;
  if (box.dataset.action !== 'indent') return;
  state.indent = box.checked;
  renderXml();
});
xmlTab.addEventListener('click', (event) => {
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-action=copy]');
  if (!button || !state.xml) return;
  void navigator.clipboard.writeText(state.xml.xml).then(
    () => (button.textContent = 'Copied'),
    () => (button.textContent = "Can't copy"),
  );
});

function setStatus(text: string, error = false) {
  status.textContent = text;
  status.classList.toggle('error', error);
}

// Files

const dialog = new OpenDialog(scan);

/** List the files: the dialog then asks what to read. */
function open(files: File[]) {
  if (files.length === 0) return;
  state.files = files;
  state.listing = null;
  state.choices = null;
  state.choose = true;
  // The worker can't take a message while it scans: start over.
  if (state.scanning) restartWorker();
  else send({ type: 'open', files });
  setStatus(`Opening ${files.length === 1 ? files[0].name : `${files.length} files`}…`);
}

function scan(request: OpenRequest) {
  state.choices = request.choices;
  state.sample = request.sample;
  state.scan = null;
  state.layer = null;
  state.info = null;
  state.report = null;
  // Batches of a read still on their way are for the old files.
  state.readId++;
  state.schemas.clear();
  layerSelect.disabled = true;
  state.scanning = true;
  stopButton.hidden = false;
  setStatus('Scanning…');
  state.started = performance.now();
  send({ type: 'scan', choices: request.choices, sample: request.sample });
}

changeButton.addEventListener('click', () => {
  if (!state.listing) return;
  if (state.scanning) {
    restartWorker();
    stopButton.hidden = true;
    setStatus('Scan stopped');
  }
  dialog.show(state.listing, state.choices, state.sample);
});

$('overview').addEventListener('click', (event) => {
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-action=full-scan]');
  if (button && state.choices) scan({ choices: state.choices, sample: null });
});

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
  state.selected = null;
  state.xml = null;
  state.grids.clear();
  state.started = performance.now();
  mapNote.hidden = true;
  stopButton.hidden = false;
  setStatus(`Reading ${layer}…`);
  send({ type: 'read', id: state.readId, layer, limit: state.limit });
  renderOverview();
  if (state.tab === 'schema') loadSchema();
  if (state.tab === 'xml') loadXml();
}

layerSelect.addEventListener('change', () => startRead(layerSelect.value));
limitInput.addEventListener('change', () => {
  const value = Number(limitInput.value);
  state.limit = limitInput.value.trim() === '' || !(value > 0) ? null : Math.floor(value);
  if (state.layer) startRead(state.layer);
});
stopButton.addEventListener('click', () => {
  if (!state.scanning) {
    send({ type: 'stop', id: state.readId });
    return;
  }
  // Back to the dialog, to choose less or a sample.
  state.choose = true;
  const sample = state.sample ?? DEFAULT_SAMPLE;
  restartWorker();
  stopButton.hidden = true;
  layerSelect.replaceChildren();
  setStatus('Scan stopped');
  state.sample = sample;
});

function receive(message: FromWorker) {
  if ('id' in message && message.type !== 'rows' && message.id !== state.readId) return;
  switch (message.type) {
    case 'files':
      state.listing = message.files;
      changeButton.hidden = false;
      if (state.choose) {
        state.choose = false;
        dialog.show(message.files, state.choices, state.choices ? state.sample : undefined);
        if (!state.choices) setStatus('');
      }
      break;
    case 'progress': {
      const seconds = (performance.now() - state.started) / 1000;
      if (state.sample !== null) {
        setStatus(`Scanning the first ${plural(state.sample, 'feature')} of each file… ${bytes(message.bytes)} read in ${seconds.toFixed(0)} s`);
      } else {
        const share = message.total > 0 ? Math.min(100, (100 * message.bytes) / message.total) : 0;
        setStatus(`Scanning… ${share.toFixed(0)}% of ${bytes(message.total)} in ${seconds.toFixed(0)} s`);
      }
      break;
    }
    case 'scanned': {
      state.scanning = false;
      stopButton.hidden = true;
      state.scan = message.scan;
      state.scanMs = message.ms;
      const layers = message.scan.layers;
      // A sample's counts are only the features it saw.
      const atLeast = message.scan.complete ? '' : '≥ ';
      layerSelect.replaceChildren(
        ...layers.map((l) => new Option(`${l.name} (${atLeast}${l.feature_count.toLocaleString('en')})`, l.name)),
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
      if (message.type === 'done') countLayer(message.report);
      stopButton.hidden = true;
      map.finish();
      map.fit();
      const seconds = ((performance.now() - state.started) / 1000).toFixed(1);
      setStatus(
        `${plural(message.report.rows, 'row')} of ${state.layer} in ${seconds} s${message.type === 'stopped' ? ' (stopped)' : ''}`,
      );
      updateMapNote();
      renderOverview();
      break;
    }
    case 'xml':
      if (message.id !== state.readId || message.row !== state.selected) break;
      if (message.xml) {
        state.xml = message.xml;
        if (state.tab === 'xml') renderXml();
      } else if (state.tab === 'xml') {
        xmlTab.innerHTML = `<p class="error">Couldn't read the XML: ${escape(message.error)}</p>`;
      }
      break;
    case 'rows':
      rowAnswers.get(message.request)?.(message.rows);
      rowAnswers.delete(message.request);
      break;
    case 'schema':
      state.schemas.set(message.layer, message.schema);
      if (message.layer === state.layer) renderSchema();
      break;
    case 'error':
      state.scanning = false;
      state.reading = false;
      stopButton.hidden = true;
      setStatus(message.message, true);
      renderOverview(message.message);
      break;
  }
}

/** After a sampled scan, a read that went through every file counts its
 * layer exactly. */
function countLayer(report: Report) {
  const layer = state.scan?.layers.find((l) => l.name === state.layer);
  const limited = state.limit !== null && report.rows >= state.limit;
  if (!layer || state.scan!.complete || limited) return;
  layer.feature_count = report.rows + report.skipped.length;
  layer.counted = true;
  const option = [...layerSelect.options].find((o) => o.value === layer.name);
  if (option) option.text = `${layer.name} (${layer.feature_count.toLocaleString('en')})`;
}

// Tabs

function showTab(tab: string) {
  state.tab = tab;
  document.querySelectorAll<HTMLElement>('#tabs [data-tab]').forEach((b) => {
    b.classList.toggle('active', b.dataset.tab === tab);
    b.setAttribute('aria-selected', String(b.dataset.tab === tab));
  });
  document.querySelectorAll<HTMLElement>('.tab').forEach((t) => t.classList.toggle('active', t.id === tab));
  if (tab === 'schema') loadSchema();
  if (tab === 'xml') loadXml();
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
