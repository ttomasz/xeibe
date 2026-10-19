// The Data tab: a virtual table. Only the rows in view are in the DOM; their
// text comes from the worker in blocks, formatted by Arrow in Rust.

const ROW_HEIGHT = 26;
const BLOCK = 100;
/** Browsers cap an element's height (Chrome near 33.5 million pixels,
 * Firefox near 17.9 million: 1.29 million and 690,000 rows). Taller than
 * this, the scrollbar stands for the rows rather than measuring them. */
const MAX_HEIGHT = 15_000_000;
/** Blocks of row text kept; beyond it, those farthest from the view go. */
const MAX_BLOCKS = 400;

export class Table {
  private columns: { name: string; geometry: boolean }[] = [];
  private rowCount = 0;
  private blocks = new Map<number, string[][]>();
  private pending = new Set<number>();
  private selected: number | null = null;
  /** A row to scroll to once the table shows: a hidden one can't scroll. */
  private scrollTo: number | null = null;
  /** Bumped by each reset: answers for an older read are dropped. */
  private generation = 0;
  private readonly sizer: HTMLDivElement;
  private readonly head: HTMLDivElement;
  private readonly body: HTMLDivElement;
  private template = '';
  /** Where the view is in the rows, in pixels of rows: the scroll position
   * itself until the rows are taller than `MAX_HEIGHT`. */
  private rowsTop = 0;
  /** The scroll position this table last set or saw. */
  private lastScrollTop = 0;

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
    container.addEventListener(
      'scroll',
      () => {
        // A scroll by the user (the scrollbar, keys): where it is in the rows.
        if (Math.abs(container.scrollTop - this.lastScrollTop) > 1) {
          this.rowsTop = Math.min(this.mostTop(), container.scrollTop * this.ratio());
        }
        this.lastScrollTop = container.scrollTop;
        this.render();
      },
      { passive: true },
    );
    // The wheel moves by its own distance in the rows, not in the scrollbar.
    container.addEventListener(
      'wheel',
      (event) => {
        if (this.ratio() === 1 || event.ctrlKey || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
        event.preventDefault();
        const unit = event.deltaMode === 1 ? ROW_HEIGHT : event.deltaMode === 2 ? container.clientHeight : 1;
        this.scrollRows(this.rowsTop + event.deltaY * unit);
        this.render();
      },
      { passive: false },
    );
    new ResizeObserver(() => {
      if (this.scrollTo !== null && this.container.clientHeight > 0) this.scrollIntoView(this.scrollTo);
      this.render();
    }).observe(container);
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
    this.layout(0);
    this.head.replaceChildren(
      cell('#', 'vt-num'),
      ...this.columns.map((c) => cell(c.name, c.geometry ? 'vt-geom' : '')),
    );
    this.rowsTop = 0;
    this.lastScrollTop = 0;
    this.container.scrollTop = 0;
    this.render();
  }

  setRowCount(count: number) {
    if (count === this.rowCount) return;
    // The row number column fits the largest number.
    if (String(count).length !== String(this.rowCount).length) this.layout(count);
    // The last block may have been fetched before it was full.
    this.blocks.delete(Math.floor(this.rowCount / BLOCK));
    this.rowCount = count;
    // The scrollbar may stand for more rows now: keep the view where it is.
    if (this.ratio() > 1) this.scrollRows(this.rowsTop);
    this.render();
  }

  /** Column widths: the row number's for `rows` rows. */
  private layout(rows: number) {
    const number = `${Math.max(64, String(rows).length * 9 + 20)}px`;
    this.template = [number, ...this.columns.map((c) => (c.geometry ? '320px' : '180px'))].join(' ');
    this.head.style.gridTemplateColumns = this.template;
  }

  /** The height rows show in: below the header. */
  private view(): number {
    return this.container.clientHeight - this.head.offsetHeight;
  }

  /** The farthest the view goes into the rows: the last row at its bottom. */
  private mostTop(): number {
    return Math.max(0, this.rowCount * ROW_HEIGHT - this.view());
  }

  /** Rows' pixels per scrollbar pixel: 1 until the rows are taller than
   * `MAX_HEIGHT`. */
  private ratio(): number {
    const rows = this.rowCount * ROW_HEIGHT;
    const view = this.view();
    if (rows <= MAX_HEIGHT || view <= 0) return 1;
    return (rows - view) / (MAX_HEIGHT - view);
  }

  /** Move the view to `top` pixels into the rows. */
  private scrollRows(top: number) {
    this.rowsTop = Math.min(this.mostTop(), Math.max(0, top));
    this.lastScrollTop = this.rowsTop / this.ratio();
    this.container.scrollTop = this.lastScrollTop;
    // The browser rounds it: what it took is what is seen from now on.
    this.lastScrollTop = this.container.scrollTop;
  }

  select(row: number | null, scroll: boolean) {
    this.selected = row;
    this.scrollTo = null;
    if (row !== null && scroll) {
      if (this.container.clientHeight > 0) this.scrollIntoView(row);
      else this.scrollTo = row;
    }
    this.render();
  }

  private scrollIntoView(row: number) {
    this.scrollTo = null;
    const top = row * ROW_HEIGHT;
    const view = this.container.clientHeight - ROW_HEIGHT * 2;
    if (top < this.rowsTop || top > this.rowsTop + view) this.scrollRows(top - view / 2);
  }

  private render() {
    this.sizer.style.height = `${Math.min(this.rowCount * ROW_HEIGHT, MAX_HEIGHT)}px`;
    this.sizer.style.width = this.head.scrollWidth ? `${this.head.scrollWidth}px` : '';
    if (this.ratio() === 1) this.rowsTop = this.container.scrollTop;
    // The first row in view, and how much of it is above the view.
    const top = this.rowsTop;
    const visible = Math.floor(top / ROW_HEIGHT);
    const above = top - visible * ROW_HEIGHT;
    const first = Math.max(0, visible - 10);
    const last = Math.min(this.rowCount, Math.ceil((top + this.container.clientHeight) / ROW_HEIGHT) + 10);
    // Rows `first..` from where the scrollbar puts the view.
    const at = this.container.scrollTop - above - (visible - first) * ROW_HEIGHT;
    this.body.style.transform = `translateY(${at}px)`;
    this.forget(Math.floor(first / BLOCK));
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

  /** Keep the text of the blocks near the view only. */
  private forget(near: number) {
    if (this.blocks.size <= MAX_BLOCKS) return;
    for (const block of this.blocks.keys()) {
      if (Math.abs(block - near) > MAX_BLOCKS / 2) this.blocks.delete(block);
    }
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
