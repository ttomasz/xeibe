// Export: the whole layer to GeoParquet, into a file the user picks where the
// browser can (Chromium), else into the browser's storage and then a download.

import { bytes } from './open';
import type { CrsInfo, ExportReport } from './protocol';
import { escape } from './views';

type SaveFilePicker = (options: {
  suggestedName: string;
  types: { description: string; accept: Record<string, string[]> }[];
}) => Promise<FileSystemFileHandle>;

export interface ExportRequest {
  layer: string;
  /** The CRS to convert to; null keeps the coordinates as read. */
  targetCrs: string | null;
  /** The file picked; null: the browser's storage, then a download. */
  target: FileSystemFileHandle | null;
  name: string;
}

/** What the exported layer holds, for the dialog. */
export interface ExportLayer {
  name: string;
  /** The features, if the scan counted them all. */
  features: number | null;
  /** The srsNames whose axis order the user chose. */
  axis: string[];
  /** The layer's CRSs (`EPSG:2180`), as the scan found them. */
  crs: string[];
}

/** The CRSs offered besides the layer's own. */
const OFFERED: [string, string][] = [
  ['EPSG:4326', 'WGS 84'],
  ['EPSG:3857', 'Web Mercator'],
];

/** `2180` → `EPSG:2180`; anything else as typed. */
function srsName(text: string): string {
  const trimmed = text.trim();
  return /^\d+$/.test(trimmed) ? `EPSG:${trimmed}` : trimmed;
}

const number = new Intl.NumberFormat('en');

function plural(n: number, word: string): string {
  return `${number.format(n)} ${word}${n === 1 ? '' : 's'}`;
}

/** `layer.parquet`, without characters file systems refuse. */
function fileName(layer: string): string {
  return `${layer.replace(/[\\/:*?"<>|]/g, '_')}.parquet`;
}

/** Whether the browser lets the page store files (OPFS). Firefox refuses in
 * a private window, or with the site's data blocked, and the export then
 * keeps the file in memory. */
async function storageAllowed(): Promise<boolean> {
  try {
    await navigator.storage.getDirectory();
    return true;
  } catch {
    return false;
  }
}

export class ExportDialog {
  private dialog: HTMLDialogElement;
  private body: HTMLElement;
  private bar: HTMLProgressElement;
  private state: HTMLElement;
  private save: HTMLButtonElement;
  private stop: HTMLButtonElement;
  private close: HTMLButtonElement;
  private layer: ExportLayer | null = null;
  /** The file's name. */
  private name = '';
  private started = 0;
  /** The last download's object URL. */
  private url: string | null = null;
  private storage: Promise<boolean> | null = null;
  /** The CRS chosen (`''` as read, `other` typed), kept between exports. */
  private choice = '';
  private typed = '';
  /** The CRS to convert to, once known; `undefined` while it is looked up
   * or isn't one. */
  private targetCrs: string | null | undefined = null;
  /** For the bar at the top: what the export is doing. */
  label = 'Export…';
  running = false;

  constructor(
    private onStart: (request: ExportRequest) => void,
    private onStop: () => void,
    private onChange: () => void,
    private lookup: (srsName: string) => Promise<CrsInfo>,
  ) {
    this.dialog = document.getElementById('export') as HTMLDialogElement;
    this.body = this.dialog.querySelector('.body')!;
    this.bar = this.dialog.querySelector('progress')!;
    this.state = this.dialog.querySelector('.state')!;
    this.save = this.dialog.querySelector('[data-action=save]')!;
    this.stop = this.dialog.querySelector('[data-action=stop]')!;
    this.close = this.dialog.querySelector('[value=close]')!;
    this.save.addEventListener('click', () => void this.start());
    this.stop.addEventListener('click', () => this.onStop());
  }

  /** Offer to export `layer`; while an export runs, show it instead. */
  show(layer: ExportLayer) {
    if (!this.running) {
      this.layer = layer;
      this.phase('ready');
      const count = layer.features === null ? 'every feature' : `all ${plural(layer.features, 'feature')}`;
      const axis = layer.axis.length
        ? `<p class="muted">Uses the axis order you chose for ${layer.axis.map((s) => `<code>${escape(s)}</code>`).join(', ')}.</p>`
        : '';
      const storage = this.picker() ? '' : '<p class="muted storage">The file downloads when it is ready.</p>';
      const read = layer.crs.length ? ` (${layer.crs.map(escape).join(', ')})` : '';
      const offered = OFFERED.map(([code, name]) => `<option value="${code}">${code}, ${name}</option>`).join('');
      this.body.innerHTML = `<p>Saves ${count} of <code>${escape(layer.name)}</code> as a GeoParquet file.</p>
        <div class="crs">
          <label class="field">Coordinates
            <select name="crs"><option value="">as read${read}</option>${offered}<option value="other">another CRS…</option></select>
          </label>
          <input name="srs" placeholder="EPSG code, e.g. 2180" aria-label="CRS to convert to" spellcheck="false" hidden />
        </div>
        <p class="crs-note muted" role="status"></p>
        ${axis}
        <p class="muted">Curves are linearized: each arc becomes many short straight segments.</p>
        ${storage}`;
      this.state.textContent = '';
      const select = this.body.querySelector<HTMLSelectElement>('select[name=crs]')!;
      const input = this.body.querySelector<HTMLInputElement>('input[name=srs]')!;
      select.value = this.choice;
      input.value = this.typed;
      select.addEventListener('change', () => {
        this.choice = select.value;
        void this.chooseCrs(select, input);
        if (select.value === 'other') input.focus();
      });
      input.addEventListener('input', () => {
        this.typed = input.value;
        void this.chooseCrs(select, input);
      });
      void this.chooseCrs(select, input);
    }
    if (!this.running && !this.picker()) void this.memoryNote();
    if (!this.dialog.open) this.dialog.showModal();
  }

  /** Look up the CRS chosen, say what it is, and allow saving once it is
   * one xeibe knows. */
  private async chooseCrs(select: HTMLSelectElement, input: HTMLInputElement) {
    const note = this.body.querySelector<HTMLElement>('.crs-note')!;
    input.hidden = select.value !== 'other';
    const wanted = select.value === 'other' ? srsName(input.value) : select.value;
    if (!wanted) {
      this.targetCrs = null;
      note.textContent = select.value === 'other' ? '' : 'Keeps the coordinates as read.';
      this.save.disabled = select.value === 'other';
      return;
    }
    this.targetCrs = undefined;
    this.save.disabled = true;
    const info = await this.lookup(wanted);
    // Typed on since, or the dialog moved on.
    const current = select.value === 'other' ? srsName(input.value) : select.value;
    if (current !== wanted || !this.body.contains(note)) return;
    if (info.crs === null || info.error) {
      note.innerHTML = `<span class="error"><code>${escape(wanted)}</code> isn't a CRS xeibe knows.</span>`;
      return;
    }
    const name = info.name ? `${escape(info.name)} (<code>${escape(info.crs)}</code>)` : `<code>${escape(info.crs)}</code>`;
    const order = info.kind?.startsWith('Geographic') ? ', longitude first' : '';
    note.innerHTML = `Converts the coordinates to ${name}${order}.`;
    this.targetCrs = info.crs;
    this.save.disabled = false;
  }

  /** Where the browser refuses the page its storage, say that the file is
   * kept in memory. */
  private async memoryNote() {
    const note = this.body.querySelector('.storage');
    this.storage ??= storageAllowed();
    if (note && !(await this.storage)) {
      note.textContent =
        "This browser doesn't let the page store files, so the file stays in memory until it downloads. " +
        'A very large layer may not fit.';
    }
  }

  private picker(): SaveFilePicker | undefined {
    return (window as unknown as { showSaveFilePicker?: SaveFilePicker }).showSaveFilePicker;
  }

  private async start() {
    const layer = this.layer;
    const targetCrs = this.targetCrs;
    if (!layer || this.running || targetCrs === undefined) return;
    let name = fileName(layer.name);
    let target: FileSystemFileHandle | null = null;
    const picker = this.picker();
    if (picker) {
      try {
        target = await picker.call(window, {
          suggestedName: name,
          types: [{ description: 'GeoParquet', accept: { 'application/vnd.apache.parquet': ['.parquet'] } }],
        });
        name = target.name;
      } catch (error) {
        // Cancelled; any other failure falls back to a download.
        if (error instanceof DOMException && error.name === 'AbortError') return;
      }
    }
    this.running = true;
    this.name = name;
    this.started = performance.now();
    this.phase('running');
    this.body.innerHTML = `<p>Writing <code>${escape(name)}</code>…</p>`;
    this.progress(0, 0);
    this.onStart({ layer: layer.name, targetCrs, target, name });
  }

  progress(rows: number, written: number) {
    const expected = this.layer?.features ?? null;
    if (expected) this.bar.value = Math.min(1, rows / expected);
    else this.bar.removeAttribute('value');
    const of = expected ? ` of ${number.format(expected)}` : '';
    this.state.textContent = `${number.format(rows)}${of} rows, ${bytes(written)} written`;
    this.label = expected ? `Exporting… ${Math.floor((100 * rows) / expected)}%` : `Exporting… ${plural(rows, 'row')}`;
    this.onChange();
  }

  done(report: ExportReport, written: number, file: File | null) {
    const name = this.name;
    this.finished();
    this.phase('done');
    const seconds = ((performance.now() - this.started) / 1000).toFixed(1);
    const what = `${plural(report.rows, 'row')}, ${bytes(written)}, in ${seconds} s`;
    let saved = `<p>Saved <code>${escape(name)}</code>: ${what}.</p>`;
    if (file) {
      if (this.url) URL.revokeObjectURL(this.url);
      this.url = URL.createObjectURL(file);
      const link = document.createElement('a');
      link.href = this.url;
      link.download = name;
      link.click();
      saved = `<p><code>${escape(name)}</code> is ready: ${what}. Your browser saves it to your Downloads folder.
        If it didn't, <a href="${this.url}" download="${escape(name)}">click here to save it</a>.</p>`;
    }
    for (const r of report.reprojections.filter((r) => !r.error)) {
      const grids = r.grids.length ? `, with the grids ${r.grids.map((g) => `<code>${escape(g)}</code>`).join(', ')}` : '';
      saved += `<p class="muted">Converted from <code>${escape(r.srs_name ?? 'no srsName')}</code> to <code>${escape(r.target)}</code>${grids}.</p>`;
    }
    const notes: string[] = [];
    for (const r of report.reprojections.filter((r) => r.error)) notes.push(escape(r.error!));
    if (report.skipped.length) {
      notes.push(`${plural(report.skipped.length, 'feature')} skipped: ${escape(report.skipped[0].message)}`);
    }
    // The Axis order section shows the conflicts.
    for (const w of report.warnings.filter((w) => w.kind !== 'AxisConflict').slice(0, 5)) {
      notes.push(escape(w.message));
    }
    for (const w of report.export_warnings) notes.push(escape(w));
    this.body.innerHTML = saved + (notes.length ? `<ul class="warn">${notes.map((n) => `<li>${n}</li>`).join('')}</ul>` : '');
    this.state.textContent = '';
  }

  failed(message: string) {
    this.finished();
    this.phase('failed');
    this.body.innerHTML = `<p class="error">The export failed: ${escape(message)}</p>`;
    this.state.textContent = '';
  }

  stopped() {
    this.finished();
    this.phase('ready');
    this.body.innerHTML = '<p class="muted">Export stopped.</p>';
    this.state.textContent = '';
  }

  /** The worker was replaced (other files): the export is gone with it. */
  reset() {
    if (!this.running) return;
    this.finished();
    if (this.dialog.open) this.dialog.close();
  }

  private finished() {
    this.running = false;
    this.label = 'Export…';
    this.onChange();
  }

  private phase(phase: 'ready' | 'running' | 'done' | 'failed') {
    this.dialog.dataset.phase = phase;
    this.save.hidden = phase !== 'ready';
    this.stop.hidden = phase !== 'running';
    this.bar.hidden = phase !== 'running';
    this.close.textContent = phase === 'ready' ? 'Cancel' : 'Close';
  }
}
