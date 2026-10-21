// The Web Worker: xeibe in WebAssembly, on this thread only (threads = 1).
// It holds the dropped files and the current read; the UI thread gets JSON,
// table rows on demand, and the map's buffers (transferred, not copied).

import init, { Dataset, Export, Reader, onProgress } from './wasm/xeibe_wasm.js';
import wasmUrl from './wasm/xeibe_wasm_bg.wasm?url';
import type { AxisOverride, FromWorker, GridRequest, Space, ToWorker } from './protocol';

const ready = init({ module_or_path: wasmUrl });

let dataset: Dataset | null = null;
/** The last read, kept after it ends or stops: the table asks it for rows. */
let reader: Reader | null = null;
let readerId = 0;
/** The read that should go on; 0 stops the one running. */
let wanted = 0;
/** The export that should go on, likewise. */
let exportWanted = 0;

function post(message: FromWorker, transfer: Transferable[] = []) {
  (self as unknown as Worker).postMessage(message, transfer);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

self.onmessage = async (event: MessageEvent<ToWorker>) => {
  const message = event.data;
  await ready;
  try {
    switch (message.type) {
      case 'open': {
        wanted = 0;
        exportWanted = 0;
        reader?.free();
        reader = null;
        dataset?.free();
        dataset = new Dataset();
        // Files are read in slices as they are needed, never copied whole;
        // a zip archive's directory is read now.
        for (const file of message.files) dataset.addBlob(file.name, file);
        post({ type: 'files', files: JSON.parse(dataset.files()) });
        break;
      }
      case 'scan': {
        if (!dataset) return;
        wanted = 0;
        exportWanted = 0;
        reader?.free();
        reader = null;
        dataset.choose(JSON.stringify(message.choices));
        const total = dataset.size();
        const started = performance.now();
        onProgress((bytes: number) => post({ type: 'progress', bytes, total }));
        try {
          const scan = JSON.parse(dataset.scan(undefined, message.sample ?? undefined));
          post({ type: 'scanned', scan, ms: performance.now() - started });
        } finally {
          onProgress(undefined);
        }
        break;
      }
      case 'schema': {
        if (!dataset) return;
        post({ type: 'schema', layer: message.layer, schema: JSON.parse(dataset.schema(message.layer)) });
        break;
      }
      case 'read':
        await read(message.id, message.layer, message.limit, message.axis);
        break;
      case 'stop':
        if (message.id === wanted) wanted = 0;
        break;
      case 'export':
        await exportLayer(message.exportId, message.layer, message.axis, message.target, message.name);
        break;
      case 'stop-export':
        if (message.exportId === exportWanted) exportWanted = 0;
        break;
      case 'xml': {
        // Within the module's limits for a string, and the page's for a <pre>.
        const max = 2 << 20;
        if (!reader || message.id !== readerId) {
          post({ type: 'xml', id: message.id, row: message.row, xml: null, error: 'the read has ended' });
          break;
        }
        try {
          const xml = JSON.parse(reader.featureXml(message.row, max));
          post({ type: 'xml', id: message.id, row: message.row, xml, error: null });
        } catch (error) {
          post({ type: 'xml', id: message.id, row: message.row, xml: null, error: errorMessage(error) });
        }
        break;
      }
      case 'rows': {
        const rows =
          reader && message.id === readerId ? JSON.parse(reader.rows(message.start, message.count)) : [];
        post({ type: 'rows', id: message.id, request: message.request, rows });
        break;
      }
    }
  } catch (error) {
    post({ type: 'error', message: errorMessage(error) });
  }
};

async function read(id: number, layer: string, limit: number | null, axis: Record<string, AxisOverride>) {
  if (!dataset) return;
  wanted = id;
  reader?.free();
  reader = null;
  const total = dataset.size();
  onProgress((bytes: number) => post({ type: 'read-progress', id, bytes, total }));
  try {
    await readBatches(id, layer, limit, readOptions(axis));
  } finally {
    // Unless a newer read has set its own.
    if (wanted === id || wanted === 0) onProgress(undefined);
  }
}

/** `ReadOptions` as JSON, or undefined for the defaults. */
function readOptions(axis: Record<string, AxisOverride>): string | undefined {
  if (Object.keys(axis).length === 0) return undefined;
  return JSON.stringify({ geometry: { axis: { mode: 'Auto', overrides: axis } } });
}

async function readBatches(id: number, layer: string, limit: number | null, options: string | undefined) {
  const started = performance.now();
  const current = dataset!.read(layer, options, limit ?? undefined);
  reader = current;
  readerId = id;
  // Stopped: by the user (the rows so far stay), or by a newer read or
  // file, which freed `current`.
  const stopped = () => {
    if (wanted === id) return false;
    if (reader === current) post({ type: 'stopped', id, report: JSON.parse(current.report()) });
    return true;
  };
  post({ type: 'info', id, info: JSON.parse(current.info()) });
  let firstRow = 0;
  for (;;) {
    // Let a newer message (another layer, stop) in between batches.
    await new Promise((resolve) => setTimeout(resolve, 0));
    if (stopped()) return;
    if (!current.next()) break;
    await loadGrids(current, id);
    if (stopped()) return;
    const drawing = current.take();
    const transfer: Transferable[] = [];
    const space = (map: boolean): Space => {
      const floats = (name: string) => {
        const array = drawing.floats(map, name);
        transfer.push(array.buffer);
        return array;
      };
      const ints = (name: string) => {
        const array = drawing.ints(map, name);
        transfer.push(array.buffer);
        return array;
      };
      return {
        points: floats('points'),
        pointRows: ints('point_rows'),
        lines: floats('lines'),
        lineStarts: ints('line_starts'),
        lineRows: ints('line_rows'),
        polygons: floats('polygons'),
        polygonStarts: ints('polygon_starts'),
        polygonRows: ints('polygon_rows'),
        triangles: ints('triangles'),
        ringStarts: ints('ring_starts'),
        ringRows: ints('ring_rows'),
        bounds: floats('bounds'),
      };
    };
    const source = space(false);
    const map = drawing.hasMap() ? space(true) : null;
    const rows = source.bounds.length / 4;
    drawing.free();
    const origin = current.origin();
    post(
      {
        type: 'batch',
        id,
        firstRow,
        rows,
        origin: origin ? [origin[0], origin[1]] : null,
        source,
        map,
      },
      transfer,
    );
    firstRow += rows;
  }
  wanted = 0;
  post({
    type: 'done',
    id,
    report: JSON.parse(current.report()),
    info: JSON.parse(current.info()),
    ms: performance.now() - started,
  });
}

/** Fetch the grid ranges the map needs for the batch just read, in rounds,
 * from cdn.proj.org. A failure leaves the map without grids. */
async function loadGrids(current: Reader, id: number) {
  for (let round = 0; round < 20; round++) {
    const requests: GridRequest[] = JSON.parse(current.gridRequests());
    if (requests.length === 0) return;
    post({ type: 'grids', id, files: [...new Set(requests.map((r) => r.file))] });
    try {
      const data = await Promise.all(requests.map(fetchRange));
      if (wanted !== id) return;
      requests.forEach((r, i) => current.addGridData(r.file, r.offset, data[i]));
    } catch (error) {
      if (wanted === id) current.withoutGrids(errorMessage(error));
      return;
    }
  }
  current.withoutGrids('grid requests kept coming');
}

/** One range of a grid file. What the wasm module was given stays loaded
 * for the page's life, so a new read asks only for what it hasn't got. */
async function fetchRange(request: GridRequest): Promise<Uint8Array> {
  const end = request.length === null ? '' : String(request.offset + request.length - 1);
  let response: Response;
  try {
    response = await fetch(request.url, { headers: { Range: `bytes=${request.offset}-${end}` } });
  } catch (error) {
    throw new Error(`${request.file}: ${errorMessage(error)}`);
  }
  if (!response.ok) throw new Error(`${request.file}: HTTP ${response.status}`);
  let bytes = new Uint8Array(await response.arrayBuffer());
  // A server that ignores Range sends the whole file.
  if (response.status === 200 && request.offset > 0) {
    bytes = bytes.slice(request.offset, request.length === null ? undefined : request.offset + request.length);
  }
  return bytes;
}

/** Where an export's bytes go. */
interface Sink {
  write(bytes: Uint8Array): Promise<void>;
  /** The file written, if the page still has to download it. */
  close(): Promise<File | null>;
  /** Stopped or failed: nothing is left behind. */
  abort(): Promise<void>;
}

/** The file the user picked. The browser writes to a temporary file and
 * puts it in place on `close()`. */
async function pickedSink(handle: FileSystemFileHandle): Promise<Sink> {
  const writable = await handle.createWritable();
  return {
    write: async (bytes) => {
      if (bytes.length) await writable.write(bytes as Uint8Array<ArrayBuffer>);
    },
    close: async () => {
      await writable.close();
      return null;
    },
    abort: () => writable.abort(),
  };
}

/** The page's directory in the browser's private storage (OPFS). */
const EXPORTS = 'exports';

/** A file in the browser's private storage, on disk rather than in memory;
 * the page downloads it when it is complete. Only the last export is kept:
 * its download may still be reading it. */
async function privateSink(name: string): Promise<Sink> {
  const root = await navigator.storage.getDirectory();
  await root.removeEntry(EXPORTS, { recursive: true }).catch(() => {});
  const directory = await root.getDirectoryHandle(EXPORTS, { create: true });
  const handle = await directory.getFileHandle(name, { create: true });
  const access = await handle.createSyncAccessHandle();
  let at = 0;
  return {
    write: async (bytes) => {
      if (access.write(bytes as Uint8Array<ArrayBuffer>, { at }) !== bytes.length) {
        throw new Error('the browser stored only part of the file (out of space?)');
      }
      at += bytes.length;
    },
    close: async () => {
      access.flush();
      access.close();
      return handle.getFile();
    },
    abort: async () => {
      access.close();
      await directory.removeEntry(name).catch(() => {});
    },
  };
}

/** The file in memory, where the browser has no private storage for the
 * page (Firefox in a private window, or with the site's data blocked). The
 * browser may keep a large file on disk; else it must fit in memory. */
function memorySink(name: string): Sink {
  let parts: Uint8Array<ArrayBuffer>[] = [];
  return {
    write: async (bytes) => {
      if (bytes.length) parts.push(bytes as Uint8Array<ArrayBuffer>);
    },
    close: async () => new File(parts, name, { type: 'application/vnd.apache.parquet' }),
    abort: async () => {
      parts = [];
    },
  };
}

async function exportLayer(
  id: number,
  layer: string,
  axis: Record<string, AxisOverride>,
  target: FileSystemFileHandle | null,
  name: string,
) {
  if (!dataset) return;
  exportWanted = id;
  let current: Export | null = null;
  let sink: Sink | null = null;
  let bytes = 0;
  try {
    current = dataset.export(layer, readOptions(axis));
    sink = target ? await pickedSink(target) : await privateSink(name).catch(() => memorySink(name));
    for (;;) {
      // Let a stop in between batches.
      await new Promise((resolve) => setTimeout(resolve, 0));
      if (exportWanted !== id) {
        await sink.abort();
        post({ type: 'export-stopped', exportId: id });
        return;
      }
      const more = current.next();
      const chunk = current.takeBytes();
      await sink.write(chunk);
      bytes += chunk.length;
      post({ type: 'export-progress', exportId: id, rows: current.rows(), bytes });
      if (!more) break;
    }
    const report = JSON.parse(current.finish());
    const footer = current.takeBytes();
    await sink.write(footer);
    bytes += footer.length;
    const file = await sink.close();
    post({ type: 'exported', exportId: id, report, bytes, file });
  } catch (error) {
    await sink?.abort().catch(() => {});
    post({ type: 'export-failed', exportId: id, message: errorMessage(error) });
  } finally {
    current?.free();
    if (exportWanted === id) exportWanted = 0;
  }
}
