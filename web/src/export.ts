// Export: the whole layer to GeoParquet, into a file the user picks where the
// browser can (Chromium), else into the browser's storage and then a download.

import { bytes } from './open';
import type { ExportReport } from './protocol';
import { escape } from './views';

type SaveFilePicker = (options: {
  suggestedName: string;
  types: { description: string; accept: Record<string, string[]> }[];
}) => Promise<FileSystemFileHandle>;

export interface ExportRequest {
  layer: string;
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
}

const number = new Intl.NumberFormat('en');

function plural(n: number, word: string): string {
  return `${number.format(n)} ${word}${n === 1 ? '' : 's'}`;
}

/** `layer.parquet`, without characters file systems refuse. */
function fileName(layer: string): string {
  return `${layer.replace(/[\\/:*?"<>|]/g, '_')}.parquet`;
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
  /** For the bar at the top: what the export is doing. */
  label = 'Export…';
  running = false;

  constructor(
    private onStart: (request: ExportRequest) => void,
    private onStop: () => void,
    private onChange: () => void,
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
        ? `<p class="muted">With the axis order you chose for ${layer.axis.map((s) => `<code>${escape(s)}</code>`).join(', ')}.</p>`
        : '';
      const storage = this.picker()
        ? ''
        : `<p class="muted">This browser can't save to a file you pick, so the file is written to its storage first,
          then downloaded.</p>`;
      this.body.innerHTML = `<p>The layer <code>${escape(layer.name)}</code>, ${count}, as GeoParquet: the file
          <code>xeibe gml convert</code> writes. It is read again from the files, whatever Max features says.</p>
        ${axis}
        <p class="muted">Curves, if the layer has any, become straight segments: Parquet has none.</p>
        ${storage}`;
      this.state.textContent = '';
    }
    if (!this.dialog.open) this.dialog.showModal();
  }

  private picker(): SaveFilePicker | undefined {
    return (window as unknown as { showSaveFilePicker?: SaveFilePicker }).showSaveFilePicker;
  }

  private async start() {
    const layer = this.layer;
    if (!layer || this.running) return;
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
    this.onStart({ layer: layer.name, target, name });
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
      saved = `<p><code>${escape(name)}</code>: ${what}. Your browser downloads it; if it doesn't,
        <a href="${this.url}" download="${escape(name)}">save it from here</a>.</p>`;
    }
    const notes: string[] = [];
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
    this.body.innerHTML = '<p class="muted">Stopped; nothing was saved.</p>';
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
