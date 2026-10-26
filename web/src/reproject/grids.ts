// Grids from cdn.proj.org, and the dialog that shows them coming.

import type { Session } from '../wasm/xeibe_wasm_reproject.js';
import { escape } from './format';
import type { GridRequest, Info } from './types';
import { opLabel } from './views';

interface FileState {
  bytes: number;
  pending: number;
  failed: string | null;
}

/** The dialog: each file, the operations that use it and how much of it
 * has come. It opens as soon as a grid is needed and stays open until the
 * user continues: Stop while the grids come, Continue once the points are
 * converted. */
class GridDialog {
  private readonly dialog = document.getElementById('grids') as HTMLDialogElement;
  private readonly title = this.dialog.querySelector('h2') as HTMLElement;
  private readonly list = this.dialog.querySelector('.files') as HTMLElement;
  private readonly bar = this.dialog.querySelector('progress') as HTMLProgressElement;
  private readonly state = this.dialog.querySelector('.state') as HTMLElement;
  private readonly stop = this.dialog.querySelector('[data-action=stop]') as HTMLButtonElement;
  private readonly next = this.dialog.querySelector('[data-action=continue]') as HTMLButtonElement;
  private readonly hint = this.dialog.querySelector('.stop-hint') as HTMLElement;
  /** The load the dialog is showing: an older one finishing late leaves
   * it alone. */
  private load = 0;
  private fetching = false;
  onStop: () => void = () => {};

  constructor() {
    this.stop.addEventListener('click', () => this.onStop());
    this.next.addEventListener('click', () => this.dialog.close());
  }

  /** A new load, with grids to fetch: the dialog opens. */
  start(): number {
    this.fetching = true;
    this.title.textContent = 'Fetching grids';
    this.stop.hidden = this.hint.hidden = false;
    this.next.disabled = true;
    if (!this.dialog.open) this.dialog.showModal();
    this.stop.focus();
    return ++this.load;
  }

  /** A new load with nothing to fetch: a dialog still showing an older
   * load's fetch has nothing more to say. */
  idle() {
    this.load++;
    if (this.fetching && this.dialog.open) this.dialog.close();
    this.fetching = false;
  }

  update(files: Map<string, FileState>, done: number, total: number, info: Info | null) {
    this.list.innerHTML = [...files]
      .map(([file, f]) => {
        const users = (info?.operations ?? [])
          .map((op, i) => (op.grids.some((g) => g.file === file) ? `<span class="legend">${opLabel(info!, i)}</span>` : ''))
          .join(' ');
        const state = f.failed
          ? `<span class="error">${escape(f.failed)}</span>`
          : f.pending
            ? `<span class="muted">${f.bytes ? `${size(f.bytes)}…` : 'fetching…'}</span>`
            : size(f.bytes);
        return `<li><code>${escape(file)}</code><span class="size">${state}</span><span class="users">${users}</span></li>`;
      })
      .join('');
    this.bar.max = Math.max(total, 1);
    this.bar.value = done;
    this.state.textContent = `${done} of ${total} parts`;
  }

  /** The fetch is over: say how it went, and let the user continue. */
  finish(load: number, files: Map<string, FileState>, stopped: boolean) {
    if (load !== this.load) return;
    this.fetching = false;
    const failed = [...files.values()].filter((f) => f.failed).length;
    this.title.textContent = stopped
      ? 'Grids stopped'
      : failed
        ? `${failed === files.size ? 'The grids' : 'Some grids'} couldn't be fetched`
        : 'Grids fetched';
    this.state.innerHTML = failed
      ? `Your points are converted. <span class="warn">Those that need a missing grid failed, and the output says why.</span>`
      : 'Your points are converted.';
    this.bar.value = this.bar.max;
    this.stop.hidden = this.hint.hidden = true;
    this.next.disabled = false;
    if (this.dialog.open) this.next.focus();
  }
}

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

let dialog: GridDialog | null = null;

/** The grid ranges the points need, from cdn.proj.org, in rounds until
 * nothing is missing: a file's directories first, then the blocks around
 * the points. A file that can't be fetched, or that the user stops, is
 * given up: the points that need it fail, and say why. Returns false when
 * `current()` turned false on the way (a newer conversion took over). */
export async function loadGrids(
  session: Session,
  points: string,
  info: Info | null,
  current: () => boolean,
  status: (text: string) => void,
): Promise<boolean> {
  dialog ??= new GridDialog();
  const files = new Map<string, FileState>();
  const abort = new AbortController();
  let stopped = false;
  let done = 0;
  let total = 0;
  let load = -1;
  dialog.onStop = () => {
    stopped = true;
    abort.abort();
  };
  const show = () => {
    dialog!.update(files, done, total, info);
    const names = [...files.keys()].join(', ');
    status(`Fetching ${names}: ${done} of ${total} parts…`);
  };
  try {
    for (let round = 0; round < 20; round++) {
      const requests = JSON.parse(session.gridRequests(points)) as GridRequest[];
      if (requests.length === 0) {
        if (round === 0) dialog.idle();
        return true;
      }
      if (round === 0) load = dialog.start();
      total += requests.length;
      for (const r of requests) {
        const f = files.get(r.file) ?? { bytes: 0, pending: 0, failed: null };
        f.pending++;
        files.set(r.file, f);
      }
      show();
      const results = await Promise.allSettled(
        requests.map(async (r) => {
          try {
            const bytes = await fetchRange(r, abort.signal);
            files.get(r.file)!.bytes += bytes.length;
            return bytes;
          } finally {
            files.get(r.file)!.pending--;
            done++;
            if (current()) show();
          }
        }),
      );
      if (!current()) return false;
      results.forEach((r, i) => {
        const { file, offset } = requests[i];
        let failed: string | null = null;
        if (r.status === 'fulfilled') {
          try {
            session.addGridData(file, offset, r.value);
          } catch (error) {
            failed = message(error);
          }
        } else {
          failed = stopped ? 'stopped' : message(r.reason);
        }
        if (failed) {
          session.gridFailed(file, failed);
          files.get(file)!.failed = failed;
        }
      });
      show();
    }
    return true;
  } finally {
    abort.abort();
    if (load >= 0) dialog.finish(load, files, stopped);
  }
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function fetchRange(request: GridRequest, signal: AbortSignal): Promise<Uint8Array> {
  const end = request.length === null ? '' : String(request.offset + request.length - 1);
  const response = await fetch(request.url, { headers: { Range: `bytes=${request.offset}-${end}` }, signal });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  let bytes = new Uint8Array(await response.arrayBuffer());
  // A server that ignores Range sends the whole file.
  if (response.status === 200 && request.offset > 0) {
    bytes = bytes.slice(request.offset, request.length === null ? undefined : request.offset + request.length);
  }
  return bytes;
}
