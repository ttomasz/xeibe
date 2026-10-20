// The dialog shown when files are opened: which files and zip members to
// read, and how much of them the scan reads for the schema.

import type { Choice, InputFile } from './protocol';
import { escape } from './views';

/** What the dialog asks the worker to scan. `sample`: the first features of
 * each file; `null`: the whole files. */
export interface OpenRequest {
  choices: Choice[];
  sample: number | null;
  /** What was chosen, in words: "3 files, 3.2 GB (263 MB to read)". */
  summary: string;
}

/** Roughly what the page scans per second, of uncompressed GML. */
const SCAN_BYTES_PER_SECOND = 85e6;
/** Above this (uncompressed), the dialog suggests a sample. */
const FULL_SCAN_UP_TO = 150e6;
export const DEFAULT_SAMPLE = 10000;

const number = new Intl.NumberFormat('en');

export function bytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e8 ? 0 : 1)} MB`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} kB`;
  return `${n} B`;
}

function duration(seconds: number): string {
  if (seconds < 1) return 'under a second';
  if (seconds < 90) return `about ${Math.round(seconds)} s`;
  return `about ${Math.round(seconds / 60)} min`;
}

/** A row of the list: a file, or a member of a zip archive. */
interface Item {
  file: number;
  member: string | null;
  /** Uncompressed. */
  size: number;
  /** Read from the file: compressed for a zip member. */
  stored: number;
  box: HTMLInputElement;
}

export class OpenDialog {
  private dialog: HTMLDialogElement;
  private list: HTMLElement;
  private summary: HTMLElement;
  private fullTime: HTMLElement;
  private sampleInput: HTMLInputElement;
  private openButton: HTMLButtonElement;
  private items: Item[] = [];
  /** Changed by the user: then the extent no longer follows the size. */
  private extentTouched = false;

  constructor(private onOpen: (request: OpenRequest) => void) {
    this.dialog = document.getElementById('open') as HTMLDialogElement;
    this.list = this.dialog.querySelector('.files')!;
    this.summary = this.dialog.querySelector('.selected')!;
    this.fullTime = this.dialog.querySelector('.full-time')!;
    this.sampleInput = this.dialog.querySelector('input[name=sample]')!;
    this.openButton = this.dialog.querySelector('button[value=open]')!;
    this.sampleInput.value = String(DEFAULT_SAMPLE);
    this.dialog.querySelectorAll<HTMLInputElement>('input[name=extent]').forEach((radio) =>
      radio.addEventListener('change', () => (this.extentTouched = true)),
    );
    this.sampleInput.addEventListener('focus', () => {
      this.extent('sample');
      this.extentTouched = true;
    });
    this.dialog.querySelector('form')!.addEventListener('submit', (event) => {
      const submitter = (event as SubmitEvent).submitter as HTMLButtonElement | null;
      if (submitter?.value !== 'open') return;
      const request = this.request();
      if (!request) {
        event.preventDefault();
        return;
      }
      this.onOpen(request);
    });
  }

  /** Show `files`; `previous` checks what was chosen last time instead of
   * every GML file, and `sample` sets the extent. */
  show(files: InputFile[], previous: Choice[] | null, sample?: number | null) {
    this.items = [];
    this.list.replaceChildren(...files.map((file, i) => this.fileRow(file, i, previous)));
    if (sample !== undefined) {
      this.extent(sample === null ? 'full' : 'sample');
      if (sample !== null) this.sampleInput.value = String(sample);
      this.extentTouched = true;
    } else {
      this.extentTouched = false;
    }
    this.update();
    this.dialog.showModal();
    this.openButton.focus();
  }

  private extent(value: 'full' | 'sample') {
    this.dialog.querySelector<HTMLInputElement>(`input[name=extent][value=${value}]`)!.checked = true;
  }

  private fileRow(file: InputFile, index: number, previous: Choice[] | null): HTMLElement {
    const chosen = (member: string | null) =>
      previous === null || previous.some((c) => c.file === index && (c.member ?? null) === member);
    const row = document.createElement('div');
    row.className = 'file';
    if (file.members === null) {
      row.append(this.itemLabel({ file: index, member: null, size: file.size, stored: file.size }, file.name, '', chosen(null)));
      return row;
    }
    if (file.error) {
      row.innerHTML = `<div class="item disabled"><input type="checkbox" disabled><span class="name">${escape(file.name)}</span>
        <span class="size">${bytes(file.size)}</span></div><div class="reason error">${escape(file.error)}</div>`;
      return row;
    }
    const gml = file.members.filter((m) => m.gml);
    const others = file.members.length - gml.length;
    const all = document.createElement('input');
    all.type = 'checkbox';
    const head = document.createElement('label');
    head.className = 'item archive';
    head.append(all);
    head.insertAdjacentHTML(
      'beforeend',
      `<span class="name">${escape(file.name)}</span><span class="size">${bytes(file.size)}</span>`,
    );
    const members = document.createElement('div');
    members.className = 'members';
    const boxes: HTMLInputElement[] = [];
    for (const member of gml) {
      if (member.unreadable) {
        members.insertAdjacentHTML(
          'beforeend',
          `<div class="item disabled" title="${escape(member.unreadable)}"><input type="checkbox" disabled>
            <span class="name">${escape(member.name)}</span><span class="size">${bytes(member.size)}</span></div>
            <div class="reason warn">${escape(member.unreadable)}</div>`,
        );
        continue;
      }
      const label = this.itemLabel(
        { file: index, member: member.name, size: member.size, stored: member.compressed_size },
        member.name,
        member.compressed_size < member.size ? `${bytes(member.compressed_size)} compressed` : '',
        chosen(member.name),
      );
      boxes.push(label.querySelector('input')!);
      members.append(label);
    }
    if (gml.length === 0) members.insertAdjacentHTML('beforeend', '<div class="reason warn">No GML files in this archive</div>');
    if (others > 0) {
      members.insertAdjacentHTML(
        'beforeend',
        `<div class="reason muted">and ${number.format(others)} other file${others === 1 ? '' : 's'}, not GML</div>`,
      );
    }
    const sync = () => {
      const checked = boxes.filter((b) => b.checked).length;
      all.checked = boxes.length > 0 && checked === boxes.length;
      all.indeterminate = checked > 0 && checked < boxes.length;
    };
    all.disabled = boxes.length === 0;
    all.addEventListener('change', () => {
      boxes.forEach((b) => (b.checked = all.checked));
      this.update();
    });
    boxes.forEach((b) => b.addEventListener('change', sync));
    sync();
    row.append(head, members);
    return row;
  }

  private itemLabel(item: Omit<Item, 'box'>, name: string, note: string, checked: boolean): HTMLLabelElement {
    const label = document.createElement('label');
    label.className = 'item';
    const box = document.createElement('input');
    box.type = 'checkbox';
    box.checked = checked;
    box.addEventListener('change', () => this.update());
    label.append(box);
    label.insertAdjacentHTML(
      'beforeend',
      `<span class="name">${escape(name)}</span><span class="size">${bytes(item.size)}${note ? `<span class="muted"> · ${escape(note)}</span>` : ''}</span>`,
    );
    this.items.push({ ...item, box });
    return label;
  }

  private update() {
    const chosen = this.items.filter((i) => i.box.checked);
    const size = chosen.reduce((sum, i) => sum + i.size, 0);
    const stored = chosen.reduce((sum, i) => sum + i.stored, 0);
    this.summary.textContent =
      chosen.length === 0
        ? 'Nothing chosen'
        : `${number.format(chosen.length)} file${chosen.length === 1 ? '' : 's'}, ${bytes(size)}${stored < size ? ` (${bytes(stored)} to read)` : ''}`;
    this.fullTime.textContent = chosen.length ? `: ${duration(size / SCAN_BYTES_PER_SECOND)}` : '';
    if (!this.extentTouched) this.extent(size > FULL_SCAN_UP_TO ? 'sample' : 'full');
    this.openButton.disabled = chosen.length === 0;
  }

  private request(): OpenRequest | null {
    const choices: Choice[] = this.items
      .filter((i) => i.box.checked)
      .map((i) => (i.member === null ? { file: i.file } : { file: i.file, member: i.member }));
    if (choices.length === 0) return null;
    const full = this.dialog.querySelector<HTMLInputElement>('input[name=extent][value=full]')!.checked;
    const summary = this.summary.textContent ?? '';
    if (full) return { choices, sample: null, summary };
    const sample = Math.floor(Number(this.sampleInput.value));
    if (!(sample >= 1)) {
      this.sampleInput.setCustomValidity('At least 1 feature');
      this.sampleInput.reportValidity();
      return null;
    }
    this.sampleInput.setCustomValidity('');
    return { choices, sample, summary };
  }
}

/** A scan's progress, over the page until it ends: the status bar alone is
 * easy to miss. Closing it leaves the scan running; a failure shows it
 * again. */
export class ScanProgress {
  private dialog: HTMLDialogElement;
  private title: HTMLElement;
  private what: HTMLElement;
  private bar: HTMLProgressElement;
  private state: HTMLElement;

  constructor(onStop: () => void) {
    this.dialog = document.getElementById('progress') as HTMLDialogElement;
    this.title = this.dialog.querySelector('h2')!;
    this.what = this.dialog.querySelector('.what')!;
    this.bar = this.dialog.querySelector('progress')!;
    this.state = this.dialog.querySelector('.state')!;
    this.dialog.querySelector('[data-action=stop]')!.addEventListener('click', () => {
      this.close();
      onStop();
    });
  }

  /** A scan of `what` starts. */
  show(what: string) {
    this.dialog.classList.remove('failed');
    this.title.textContent = 'Scanning';
    this.what.textContent = what;
    this.update('Starting…', null);
    if (!this.dialog.open) this.dialog.showModal();
  }

  /** How far it is: `share` of the bytes, or `null` if unknown (a sample
   * ends when it has its features, wherever that is). */
  update(text: string, share: number | null) {
    this.state.textContent = text;
    if (share === null) this.bar.removeAttribute('value');
    else this.bar.value = share;
  }

  fail(title: string, message: string) {
    this.dialog.classList.add('failed');
    this.title.textContent = title;
    this.state.textContent = message;
    if (!this.dialog.open) this.dialog.showModal();
  }

  close() {
    if (this.dialog.open) this.dialog.close();
  }
}
