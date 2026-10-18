// The Data tab: a virtual table. Only the rows in view are in the DOM; their
// text comes from the worker in blocks, formatted by Arrow in Rust.

const ROW_HEIGHT = 26;
const BLOCK = 100;

export class Table {
  private columns: { name: string; geometry: boolean }[] = [];
  private rowCount = 0;
  private blocks = new Map<number, string[][]>();
  private pending = new Set<number>();
  private selected: number | null = null;
  /** Bumped by each reset: answers for an older read are dropped. */
  private generation = 0;
  private readonly sizer: HTMLDivElement;
  private readonly head: HTMLDivElement;
  private readonly body: HTMLDivElement;
  private template = '';

  constructor(
    private readonly container: HTMLElement,
    private readonly fetchRows: (start: number, count: number) => Promise<string[][]>,
    private readonly onSelect: (row: number) => void,
  ) {
    container.classList.add('vtable');
    this.head = document.createElement('div');
    this.head.className = 'vt-head';
    this.sizer = document.createElement('div');
    this.sizer.className = 'vt-sizer';
    this.body = document.createElement('div');
    this.body.className = 'vt-body';
    this.sizer.append(this.body);
    container.append(this.head, this.sizer);
    container.addEventListener('scroll', () => this.render(), { passive: true });
    new ResizeObserver(() => this.render()).observe(container);
    this.body.addEventListener('click', (event) => {
      const row = (event.target as HTMLElement).closest<HTMLElement>('.vt-row');
      if (row?.dataset.row) this.onSelect(Number(row.dataset.row));
    });
  }

  /** A new read: its columns, no rows yet. */
  reset(columns: { name: string; type: string }[]) {
    this.columns = columns.map((c) => ({ name: c.name, geometry: c.type.startsWith('geoarrow.') }));
    this.generation++;
    this.rowCount = 0;
    this.blocks.clear();
    this.pending.clear();
    this.selected = null;
    this.template = ['64px', ...this.columns.map((c) => (c.geometry ? '320px' : '180px'))].join(' ');
    this.head.style.gridTemplateColumns = this.template;
    this.head.replaceChildren(
      cell('#', 'vt-num'),
      ...this.columns.map((c) => cell(c.name, c.geometry ? 'vt-geom' : '')),
    );
    this.container.scrollTop = 0;
    this.render();
  }

  setRowCount(count: number) {
    if (count === this.rowCount) return;
    // The last block may have been fetched before it was full.
    this.blocks.delete(Math.floor(this.rowCount / BLOCK));
    this.rowCount = count;
    this.render();
  }

  select(row: number | null, scroll: boolean) {
    this.selected = row;
    if (row !== null && scroll) {
      const top = row * ROW_HEIGHT;
      const view = this.container.clientHeight - ROW_HEIGHT * 2;
      if (top < this.container.scrollTop || top > this.container.scrollTop + view) {
        this.container.scrollTop = Math.max(0, top - view / 2);
      }
    }
    this.render();
  }

  private render() {
    this.sizer.style.height = `${this.rowCount * ROW_HEIGHT}px`;
    this.sizer.style.width = this.head.scrollWidth ? `${this.head.scrollWidth}px` : '';
    const top = this.container.scrollTop;
    const first = Math.max(0, Math.floor(top / ROW_HEIGHT) - 10);
    const last = Math.min(this.rowCount, Math.ceil((top + this.container.clientHeight) / ROW_HEIGHT) + 10);
    this.body.style.transform = `translateY(${first * ROW_HEIGHT}px)`;
    const rows: HTMLElement[] = [];
    for (let row = first; row < last; row++) {
      const block = this.blocks.get(Math.floor(row / BLOCK));
      if (!block) this.request(Math.floor(row / BLOCK));
      const cells = block?.[row % BLOCK];
      const element = document.createElement('div');
      element.className = row === this.selected ? 'vt-row selected' : 'vt-row';
      element.dataset.row = String(row);
      element.style.gridTemplateColumns = this.template;
      element.append(cell(String(row + 1), 'vt-num'));
      this.columns.forEach((c, i) => element.append(cell(cells ? cells[i] : '…', c.geometry ? 'vt-geom' : '')));
      rows.push(element);
    }
    this.body.replaceChildren(...rows);
  }

  private request(block: number) {
    if (this.pending.has(block)) return;
    this.pending.add(block);
    const start = block * BLOCK;
    const count = Math.min(BLOCK, this.rowCount - start);
    const generation = this.generation;
    this.fetchRows(start, count).then((rows) => {
      if (generation !== this.generation) return;
      this.pending.delete(block);
      this.blocks.set(block, rows);
      this.render();
    });
  }
}

function cell(text: string, className: string): HTMLElement {
  const element = document.createElement('div');
  element.className = `vt-cell ${className}`;
  element.textContent = text;
  element.title = text;
  return element;
}
