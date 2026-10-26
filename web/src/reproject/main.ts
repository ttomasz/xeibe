// The reproject page (docs/web-reproject.md): coordinates pasted in,
// converted between two EPSG CRSs by xeibe-reproject, with what is needed to
// judge the result. Everything runs on the main thread: setting up a
// conversion takes tens of milliseconds, and thousands of points convert in
// a few.

import './style.css';
import init, { crsCard, epsgVersion, lonlatBounds, Session } from '../wasm/xeibe_wasm_reproject.js';
import wasmUrl from '../wasm/xeibe_wasm_reproject_bg.wasm?url';
import { fetchWasm, loading } from '../common/loading';
import { accuracy, axisLabel, defaultDecimals, escape } from './format';
import { loadGrids } from './grids';
import { parse } from './parse';
import type { Axis, Card, Details, Info, Point, Result } from './types';
import {
  colour,
  commands,
  crsCard as cardHtml,
  details as detailsHtml,
  opTag,
  opNumber,
  operations,
  shortName,
  striped,
  flagged,
  warningText,
} from './views';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const ui = {
  status: $('status'),
  src: $<HTMLInputElement>('src'),
  dst: $<HTMLInputElement>('dst'),
  srcName: $('srcName'),
  dstName: $('dstName'),
  swap: $<HTMLButtonElement>('swap'),
  examples: $<HTMLSelectElement>('examples'),
  options: $<HTMLDetailsElement>('options'),
  optionsSummary: $('optionsSummary'),
  aoi: ['aoiW', 'aoiS', 'aoiE', 'aoiN'].map((id) => $<HTMLInputElement>(id)),
  aoiPoints: $<HTMLButtonElement>('aoiPoints'),
  aoiClear: $<HTMLButtonElement>('aoiClear'),
  accuracy: $<HTMLInputElement>('accuracy'),
  force: $<HTMLSelectElement>('force'),
  input: $<HTMLTextAreaElement>('input'),
  clear: $<HTMLButtonElement>('clear'),
  lineNumbers: $('lineNumbers'),
  invalid: $('invalid'),
  inAxes: $('inAxes'),
  outAxes: $('outAxes'),
  output: $('output'),
  summary: $('summary'),
  key: $('key'),
  precision: $<HTMLSelectElement>('precision'),
  separator: $<HTMLSelectElement>('separator'),
  copy: $<HTMLButtonElement>('copy'),
  details: $('details'),
  operations: $('operations'),
  srcCard: $('srcCard'),
  dstCard: $('dstCard'),
  commands: $('commands'),
  epsgVersion: $('epsgVersion'),
};

interface Example {
  label: string;
  src: string;
  dst: string;
  points: string;
}

const EXAMPLES: Example[] = [
  {
    label: 'WGS 84 → Web Mercator: one datum, no grids',
    src: 'EPSG:4326',
    dst: 'EPSG:3857',
    points: '52.2297 21.0122\n51.5074 -0.1278\n40.7128 -74.0060\n-33.8688 151.2093\n',
  },
  {
    label: 'ED50 → ETRS89: 12 candidates, by area',
    src: 'EPSG:4230',
    dst: 'EPSG:4258',
    points: '40.4168\t-3.7038\n48.8566\t2.3522\n52.5200\t13.4050\n59.9139\t10.7522\n41.9028\t12.4964\n',
  },
  {
    label: 'WGS 84 → PL-1992: one operation, through ETRS89',
    src: 'EPSG:4326',
    dst: 'EPSG:2180',
    points: '52.2297 21.0122\n50.0647 19.9450\n54.3520 18.6466\n',
  },
  {
    label: 'DHDN / Gauss-Krüger 3 → ETRS89 / UTM 32N: BETA2007 grid',
    src: 'EPSG:31467',
    dst: 'EPSG:25832',
    points: '5427000\t3513000\n5800000\t3565000\n5925000\t3567000\n',
  },
  {
    label: 'British National Grid → WGS 84: OSTN15 grid',
    src: 'EPSG:27700',
    dst: 'EPSG:4326',
    points: '530000 180000\n325000 673000\n',
  },
  {
    label: 'NAD27 → NAD83: NADCON grids',
    src: 'EPSG:4267',
    dst: 'EPSG:4269',
    points: '40.7128 -74.0060\n34.0522 -118.2437\n61.2181 -149.9003\n',
  },
  {
    label: 'CH1903+ / LV95 → CH1903 / LV03: CHENyx06 grid',
    src: 'EPSG:2056',
    dst: 'EPSG:21781',
    points: '2600000 1200000\n2683000 1248000\n',
  },
];

interface Options {
  east_first: boolean;
  area_of_interest: number[] | null;
  accuracy: number | null;
  operation: number | null;
}

const state = {
  srcCard: null as Card | null,
  dstCard: null as Card | null,
  session: null as Session | null,
  info: null as Info | null,
  setupError: null as string | null,
  points: [] as (Point | null)[],
  /** The input lines that aren't points. */
  invalid: new Set<number>(),
  /** Per input line: whether its values are separated by tabs. */
  tabbed: [] as boolean[],
  results: [] as (Result | null)[],
  selected: null as number | null,
  /** How long setting up the conversion took. */
  setupMs: null as number | null,
  /** Bumped by every conversion: an older one still fetching grids stops. */
  generation: 0,
};

function options(): Options {
  const aoi = ui.aoi.map((i) => i.valueAsNumber);
  const accuracy = ui.accuracy.valueAsNumber;
  return {
    east_first: (document.querySelector('input[name=order]:checked') as HTMLInputElement).value === 'east',
    area_of_interest: aoi.every(Number.isFinite) ? aoi : null,
    accuracy: Number.isFinite(accuracy) ? accuracy : null,
    operation: ui.force.value === '' ? null : Number(ui.force.value),
  };
}

function status(text: string, error = false) {
  ui.status.textContent = text;
  ui.status.classList.toggle('error', error);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const nextFrame = () => new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));

function debounce(f: () => void, ms: number): () => void {
  let timer = 0;
  return () => {
    clearTimeout(timer);
    timer = window.setTimeout(f, ms);
  };
}

// --- CRSs ---------------------------------------------------------------

function readCard(input: HTMLInputElement): Card | null {
  const text = input.value.trim();
  return text ? (JSON.parse(crsCard(text)) as Card) : null;
}

function nameLine(card: Card | null): string {
  if (!card) return '<span class="muted">an EPSG code, e.g. 2180 or EPSG:2180</span>';
  if (card.error && !card.name) return `<span class="error">${escape(card.error)}</span>`;
  const deprecated = card.deprecated ? ' <span class="warn">deprecated</span>' : '';
  const error = card.error ? ` <span class="error">${escape(card.error)}</span>` : '';
  return `${escape(card.name ?? '')} <span class="muted">${escape(card.kind ?? '')}</span>${deprecated}${error}`;
}

/** Both CRSs read again; a new conversion set up if either changed. */
async function updateCrss(force = false) {
  const before = [state.srcCard?.code, state.dstCard?.code];
  state.srcCard = readCard(ui.src);
  state.dstCard = readCard(ui.dst);
  ui.srcName.innerHTML = nameLine(state.srcCard);
  ui.dstName.innerHTML = nameLine(state.dstCard);
  ui.srcCard.innerHTML = cardHtml('From', state.srcCard);
  ui.dstCard.innerHTML = cardHtml('To', state.dstCard);
  renderAxes();
  const changed = before[0] !== state.srcCard?.code || before[1] !== state.dstCard?.code;
  if (changed) ui.force.value = '';
  if (changed || force) await setup();
}

/** The axes in the page's order: the CRS's own, or east first. */
function pageAxes(card: Card | null): Axis[] {
  const axes = [...(card?.axes ?? [])];
  if (options().east_first && card?.first_axis_north && axes.length >= 2) [axes[0], axes[1]] = [axes[1], axes[0]];
  return axes;
}

function renderAxes() {
  const label = (card: Card | null) => {
    const axes = pageAxes(card);
    if (!axes.length) return '';
    const parts = axes.map((a, i) => `<span class="axis">${i + 1}. ${escape(axisLabel(a))}</span>`);
    if (axes.length === 2) parts.push('<span class="axis muted">3. height, optional: carried through</span>');
    else if (axes.length === 3) parts[2] = parts[2].replace('</span>', ' <span class="muted">(optional)</span></span>');
    return parts.join('');
  };
  ui.inAxes.innerHTML = label(state.srcCard);
  ui.outAxes.innerHTML = label(state.dstCard);
}

// --- The conversion -----------------------------------------------------

async function setup() {
  state.generation++;
  state.session?.free();
  state.session = null;
  state.info = null;
  state.setupError = null;
  const src = state.srcCard;
  const dst = state.dstCard;
  if (src?.code && dst?.code && !src.error && !dst.error) {
    status('Looking for operations…');
    await nextFrame();
    const started = performance.now();
    try {
      state.session = new Session(String(src.code), String(dst.code), JSON.stringify(options()));
      state.info = JSON.parse(state.session.info()) as Info;
      state.setupMs = performance.now() - started;
    } catch (error) {
      state.setupError = errorMessage(error);
      status('');
    }
  } else {
    status('');
  }
  renderForce();
  renderOptionsSummary();
  await run();
}

function renderForce() {
  const keep = ui.force.value;
  const ops = state.info?.operations ?? [];
  ui.force.innerHTML =
    '<option value="">automatic, chosen per point</option>' +
    ops
      .map((op, i) => `<option value="${i}">${opNumber(i)}: ${escape(shortName(op))} · ${accuracy(op.accuracy)}</option>`)
      .join('');
  ui.force.value = keep !== '' && Number(keep) < ops.length ? keep : '';
  ui.force.disabled = ops.length === 0;
}

function renderOptionsSummary() {
  const o = options();
  const parts: string[] = [];
  if (o.area_of_interest) parts.push('area of interest');
  if (o.accuracy !== null) parts.push(`accuracy ≤ ${o.accuracy} m`);
  const forced = o.operation !== null ? state.info?.operations[o.operation] : undefined;
  if (forced) parts.push(`forcing ${opNumber(o.operation!)}`);
  ui.optionsSummary.textContent = parts.length ? `(${parts.join(', ')})` : '';
}

/** Parse the input, fetch the grids its points need, convert. */
async function run() {
  const generation = ++state.generation;
  const parsed = parse(ui.input.value);
  state.points = parsed.points;
  state.invalid = new Set(parsed.invalid);
  state.tabbed = parsed.tabbed;
  renderNumbers();
  ui.invalid.innerHTML = parsed.invalid.length
    ? `<span class="error">Not a point: line${parsed.invalid.length > 1 ? 's' : ''} ${parsed.invalid
        .slice(0, 30)
        .map((i) => i + 1)
        .join(', ')}${parsed.invalid.length > 30 ? '…' : ''}</span>`
    : '';
  const session = state.session;
  if (!session) {
    state.results = [];
    render(null);
    return;
  }
  const json = JSON.stringify(state.points);
  const current = () => generation === state.generation;
  // While grids come, the page shows this conversion, not the last one's
  // results. Without grids to fetch nothing is drawn in between.
  if (state.results.length !== state.points.length || state.info !== shown) {
    state.results = [];
    render(null);
  }
  if (!(await loadGrids(session, json, state.info, current, status))) return;
  const started = performance.now();
  state.results = JSON.parse(session.convert(json)) as (Result | null)[];
  const ms = performance.now() - started;
  status('');
  // Grid failures show in the operations' list.
  state.info = JSON.parse(session.info()) as Info;
  render(ms);
}

/** The operations' list the output was last drawn with. */
let shown: Info | null = null;

// --- Output -------------------------------------------------------------

function decimals(axes: Axis[]): number[] {
  const chosen = ui.precision.value;
  return [0, 1, 2].map((i) => (chosen === 'auto' ? defaultDecimals(axes[i]) : Number(chosen)));
}

/** Coordinates as text, tab-separated. Auto rounds to about 0.1 mm and
 * drops the zeros that rounding leaves at the end (52.2297, not
 * 52.229700000); a chosen number of decimals keeps them, for columns of
 * one width. */
function formatter(card: Card | null, separator = '\t'): (v: number[]) => string {
  const d = decimals(pageAxes(card));
  const trim = ui.precision.value === 'auto';
  return (v) =>
    v
      .map((x, i) => {
        const text = x.toFixed(d[Math.min(i, 2)]);
        return trim && text.includes('.') ? text.replace(/\.?0+$/, '') : text;
      })
      .join(separator);
}

/** The output's formatter for input line `i`: values separated as the
 * Separator option says, by default as on that line of the input. */
function outputFormatter(): (v: number[], i: number) => string {
  const tab = formatter(state.dstCard, '\t');
  const space = formatter(state.dstCard, ' ');
  const choice = ui.separator.value;
  return (v, i) => ((choice === 'input' ? state.tabbed[i] : choice === 'tab') ? tab(v) : space(v));
}

function render(ms: number | null) {
  shown = state.info;
  const format = outputFormatter();
  const lines: string[] = [];
  const counts: number[] = [];
  let errors = 0;
  let warnings = 0;
  let thirds = false;
  state.results.forEach((r, i) => {
    if (state.points[i]?.length === 3) thirds = true;
    const selected = i === state.selected ? ' selected' : '';
    if (!r) {
      lines.push(`<div class="line" data-line="${i}"> </div>`);
      return;
    }
    if (r.error !== undefined) {
      errors++;
      lines.push(`<div class="line err${selected}" data-line="${i}" title="${escape(r.error)}"><span class="e">✕ ${escape(r.error)}</span></div>`);
      return;
    }
    if (r.op !== null) counts[r.op] = (counts[r.op] ?? 0) + 1;
    const op = r.op !== null ? state.info?.operations[r.op] : undefined;
    // The gutter: the operation's number, and its colour as a bar.
    const gutter =
      r.op !== null && op
        ? ` style="--op:${colour(r.op)}" data-op="${opNumber(r.op)}" title="${escape(`through ${opNumber(r.op)}: ${shortName(op)}`)}"`
        : '';
    const stripes = r.op !== null && striped(r.op) ? ' striped' : '';
    // Only the warnings that matter get a ⚠: see `flagged`.
    const shown = r.warnings.filter((w) => flagged(w, op));
    if (shown.length) warnings++;
    // The gutter's number (data-op above) and ⚠ are drawn by CSS from
    // attributes (`content: attr()`), so a selection over the output
    // copies the coordinates only.
    const tag = shown.length
      ? `<span class="tag" data-warn="⚠" title="${escape(shown.map(warningText).join('; '))}"></span>`
      : '';
    lines.push(
      `<div class="line${stripes}${selected}" data-line="${i}"${gutter}><span class="v">${format(r.out, i)}</span>${tag}</div>`,
    );
  });
  ui.output.innerHTML = lines.join('');
  ui.output.scrollTop = ui.input.scrollTop;

  ui.summary.innerHTML = summary(ms, errors, warnings);
  ui.key.innerHTML = key(counts, thirds);
  ui.operations.innerHTML = operations(state.info, state.setupError, counts);
  renderCommands();
  renderDetails();
}

/** Under the output, beside its controls: how many points, and how many
 * need a look. */
function summary(ms: number | null, errors: number, warnings: number): string {
  if (state.setupError) return `<span class="error">${escape(state.setupError)}</span>`;
  const n = state.points.filter(Boolean).length;
  if (!state.session || !n) return '';
  const parts = [`${n} point${n > 1 ? 's' : ''}`];
  if (errors) parts.push(`<span class="error">${errors} failed</span>`);
  if (warnings) parts.push(`<span class="warn">${warnings} with warnings</span>`);
  if (ms !== null) {
    const time = `${ms < 0.1 ? '< 0.1' : ms.toFixed(1)} ms`;
    const setup = state.setupMs !== null ? `; setting up took ${Math.round(state.setupMs)} ms` : '';
    parts.push(`<span class="timing" title="Converted in ${time}${setup}">${time}</span>`);
  }
  return parts.join(' · ');
}

/** Below that, the gutter's key: one operation a row. */
function key(counts: number[], thirds: boolean): string {
  if (state.setupError || !state.session) return '';
  let html = '';
  const info = state.info;
  const rows = info
    ? counts
        .map((c, i) =>
          c
            ? `<span>${opTag(i)}</span><span class="key-name">${escape(shortName(info.operations[i]))}</span><span class="key-count">${c} point${c > 1 ? 's' : ''}</span>`
            : '',
        )
        .join('')
    : '';
  if (rows) html += `<div class="key">${rows}</div>`;
  if (thirds && info && info.source_dimension < 3 && info.target_dimension < 3)
    html += '<p>Third values are carried through unchanged: neither CRS has heights.</p>';
  return html;
}

function renderCommands() {
  const src = state.srcCard?.code;
  const dst = state.dstCard?.code;
  if (!src || !dst) {
    ui.commands.innerHTML = '<p class="muted">Type two EPSG codes.</p>';
    return;
  }
  const lines = state.points.filter((p): p is Point => p !== null).map((p) => p.join(' '));
  const o = options();
  ui.commands.innerHTML = commands(
    src,
    dst,
    lines,
    {
      eastFirst: o.east_first,
      aoi: o.area_of_interest,
      accuracy: o.accuracy,
      forced:
        o.operation !== null && state.info?.operations[o.operation]
          ? `${opNumber(o.operation)} (${state.info.operations[o.operation].name})`
          : null,
    },
    !!state.srcCard?.first_axis_north,
  );
}

function renderDetails() {
  const i = state.selected;
  const p = i === null ? null : state.points[i];
  if (i === null || !p || !state.session) {
    ui.details.hidden = true;
    return;
  }
  const d = JSON.parse(state.session.details(p[0], p[1], p[2])) as Details;
  ui.details.innerHTML = detailsHtml(
    i,
    p,
    d,
    state.info,
    { source: formatter(state.srcCard), target: formatter(state.dstCard) },
    { src: state.srcCard, dst: state.dstCard },
  );
  ui.details.hidden = false;
}

/** The input's gutter: a number per line of the textarea, the lines that
 * aren't points in red. */
function renderNumbers() {
  const count = ui.input.value.split('\n').length;
  const numbers: string[] = [];
  for (let i = 0; i < count; i++) {
    const cls = i === state.selected ? ' class="selected"' : state.invalid.has(i) ? ' class="bad"' : '';
    numbers.push(`<span${cls}>${i + 1}</span>`);
  }
  const inner = ui.lineNumbers.firstElementChild as HTMLElement;
  inner.innerHTML = numbers.join('');
  ui.lineNumbers.parentElement!.style.setProperty('--numbers', `${Math.max(2, String(count).length) * 7 + 18}px`);
  placeNumbers();
}

/** Keep the gutter on the textarea's lines: scrolled with it, and clear of
 * its horizontal scrollbar. */
function placeNumbers() {
  ui.lineNumbers.style.height = `${ui.input.clientHeight}px`;
  (ui.lineNumbers.firstElementChild as HTMLElement).style.transform = `translateY(${-ui.input.scrollTop}px)`;
}

function select(line: number | null) {
  state.selected = line !== null && state.points[line] ? line : null;
  renderNumbers();
  for (const el of ui.output.querySelectorAll('.line')) {
    el.classList.toggle('selected', Number((el as HTMLElement).dataset.line) === state.selected);
  }
  renderDetails();
}

/** The line the textarea's caret is on. */
function caretLine(): number {
  return ui.input.value.slice(0, ui.input.selectionStart).split('\n').length - 1;
}

// --- Events -------------------------------------------------------------

const crsChanged = debounce(() => void updateCrss(), 300);
for (const field of [ui.src, ui.dst]) {
  field.addEventListener('input', () => {
    markOwn();
    crsChanged();
  });
}

ui.swap.addEventListener('click', () => {
  const text = outputText();
  [ui.src.value, ui.dst.value] = [ui.dst.value, ui.src.value];
  if (state.results.some((r) => r && r.error === undefined)) ui.input.value = `${text}\n`;
  state.selected = null;
  markOwn();
  void updateCrss();
});

// The dropdown names what is loaded: an example, or "Your own points"
// once anything that makes it up (a CRS, the points, the axis order) is
// edited. Picking an example then always loads something visibly.
ui.examples.innerHTML =
  '<option value="" disabled>Your own points</option>' +
  EXAMPLES.map((e, i) => `<option value="${i}">${escape(e.label)}</option>`).join('');
ui.examples.addEventListener('change', () => void loadExample(Number(ui.examples.value)));

async function loadExample(index: number) {
  const e = EXAMPLES[index];
  if (!e) return;
  ui.src.value = e.src;
  ui.dst.value = e.dst;
  ui.input.value = e.points;
  // The examples' points are in the CRSs' own axis order.
  (document.querySelector('input[name=order][value=crs]') as HTMLInputElement).checked = true;
  ui.examples.value = String(index);
  state.selected = null;
  await updateCrss(true);
}

function markOwn() {
  ui.examples.value = '';
}

for (const radio of document.querySelectorAll('input[name=order]')) {
  radio.addEventListener('change', () => {
    markOwn();
    renderAxes();
    void setup();
  });
}

const optionsChanged = debounce(() => void setup(), 400);
for (const input of [...ui.aoi, ui.accuracy]) input.addEventListener('input', optionsChanged);
ui.force.addEventListener('change', () => void setup());
ui.aoiClear.addEventListener('click', () => {
  for (const input of ui.aoi) input.value = '';
  void setup();
});
ui.aoiPoints.addEventListener('click', () => {
  const src = state.srcCard?.code;
  if (!src) return;
  const bounds = JSON.parse(lonlatBounds(String(src), options().east_first, JSON.stringify(state.points))) as number[] | null;
  if (!bounds) return;
  bounds.forEach((v, i) => (ui.aoi[i].value = String(Number(v.toFixed(6)))));
  void setup();
});

const inputChanged = debounce(() => void run(), 150);
ui.input.addEventListener('input', () => {
  markOwn();
  renderNumbers();
  inputChanged();
});
ui.clear.addEventListener('click', () => {
  ui.input.value = '';
  state.selected = null;
  markOwn();
  void run();
  ui.input.focus();
});
ui.input.addEventListener('scroll', () => {
  ui.output.scrollTop = ui.input.scrollTop;
  placeNumbers();
});
// The textarea can be made taller: the output and the gutter follow.
new ResizeObserver(() => {
  ui.output.style.height = `${ui.input.offsetHeight}px`;
  placeNumbers();
}).observe(ui.input);
ui.output.addEventListener('scroll', () => (ui.input.scrollTop = ui.output.scrollTop));
ui.input.addEventListener('click', () => select(caretLine()));
ui.input.addEventListener('keyup', (e) => {
  if (e.key === 'ArrowUp' || e.key === 'ArrowDown') select(caretLine());
});
ui.output.addEventListener('click', (e) => {
  const line = (e.target as HTMLElement).closest<HTMLElement>('.line');
  if (line) select(Number(line.dataset.line));
});

ui.precision.innerHTML =
  '<option value="auto">auto</option>' + Array.from({ length: 13 }, (_, i) => `<option>${i}</option>`).join('');
ui.precision.addEventListener('change', () => render(null));
ui.separator.addEventListener('change', () => render(null));

/** The output as text, line for line with the input: the values
 * separated as shown, a blank line where there is no result. */
function outputText(from = 0, to = state.results.length - 1): string {
  const format = outputFormatter();
  return state.results
    .slice(from, to + 1)
    .map((r, i) => (r && r.error === undefined ? format(r.out, from + i) : ''))
    .join('\n');
}

ui.copy.addEventListener('click', async () => {
  await navigator.clipboard.writeText(outputText());
  flash(ui.copy);
});

// A selection over several lines of the output copies those lines as Copy
// does: the browser would drop the empty ones and take the errors' text.
// Within one line it copies what is selected.
ui.output.addEventListener('copy', (e) => {
  const selection = getSelection();
  const lineOf = (node: Node | null) =>
    (node instanceof Element ? node : node?.parentElement)?.closest<HTMLElement>('#output .line');
  const a = lineOf(selection?.anchorNode ?? null);
  const b = lineOf(selection?.focusNode ?? null);
  if (!a || !b || a === b || !e.clipboardData) return;
  const [from, to] = [Number(a.dataset.line), Number(b.dataset.line)].sort((x, y) => x - y);
  e.clipboardData.setData('text/plain', outputText(from, to));
  e.preventDefault();
});

function flash(button: HTMLElement) {
  const before = button.textContent;
  button.textContent = 'Copied';
  setTimeout(() => (button.textContent = before), 1200);
}

document.addEventListener('click', async (e) => {
  const target = e.target as HTMLElement;
  if (target.matches('button.copy')) {
    e.preventDefault();
    const pre = target.closest('.command')?.querySelector('pre');
    if (pre) {
      await navigator.clipboard.writeText(pre.textContent ?? '');
      flash(target);
    }
  } else if (target.matches('button[data-code]')) {
    // A deprecated CRS's replacement: use it in that field.
    const field = target.closest('#srcCard') ? ui.src : ui.dst;
    field.value = `EPSG:${target.dataset.code}`;
    markOwn();
    void updateCrss();
  } else if (target.matches('[data-action=close]')) {
    select(null);
  }
});

// --- Start --------------------------------------------------------------

async function start() {
  try {
    const module = await fetchWasm(wasmUrl, __XEIBE_WASM_REPROJECT_SIZE__, loading.progress);
    await init({ module_or_path: module });
  } catch (error) {
    loading.failed(errorMessage(error));
    return;
  }
  loading.done();
  ui.epsgVersion.textContent = `EPSG ${epsgVersion()}`;
  await loadExample(0);
}

void start();
