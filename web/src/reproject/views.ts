// The general area and a point's details, as HTML.

import {
  accuracy,
  axisLabel,
  box,
  crsUrl,
  distance,
  escape,
  methodUrl,
  operationUrl,
  parameter,
  plain,
  projectionValue,
} from './format';
import type {
  Axis,
  Card,
  Conversion,
  Details,
  EpsgOperation,
  Info,
  Operation,
  Outcome,
  Step,
  Trace,
  TracedStep,
  Warning,
} from './types';

/** The operations' colours, Op. 1 first. None of them reads as a grade
 * or a warning: no green, red, orange or yellow, and no light-to-dark run.
 * Past seven they come round again, striped. */
const COLOURS = ['#1f77b4', '#9467bd', '#17becf', '#e377c2', '#5b6b7f', '#393b79', '#a55194'];

export function colour(index: number): string {
  return COLOURS[index % COLOURS.length];
}

export function striped(index: number): boolean {
  return index >= COLOURS.length;
}

/** An operation's colour, as a small square. */
export function swatch(index: number, title?: string): string {
  const t = title ? ` title="${escape(title)}"` : '';
  return `<span class="swatch${striped(index) ? ' striped' : ''}" style="--c:${colour(index)}"${t}></span>`;
}

/** An operation by the EPSG transformations it runs, without the map
 * projections around them: "DHDN to ETRS89 (9)" for "Inverse of 3-degree
 * Gauss-Kruger zone 3 + DHDN to ETRS89 (9) + UTM zone 32N". */
export function shortName(op: Operation): string {
  const names = op.chain.filter((s) => s.kind === 'epsg').map((s) => s.name);
  return names.length ? names.join(' + ') : op.name;
}

/** An operation's number: its rank, Op. 1 the best. */
export function opNumber(index: number): string {
  return `Op. ${index + 1}`;
}

/** An operation's swatch and number. */
export function opTag(index: number): string {
  return `${swatch(index)} <span class="opn">${opNumber(index)}</span>`;
}

/** An operation's swatch, number and short name. */
export function opLabel(info: Info, index: number): string {
  const op = info.operations[index];
  return op ? `${opTag(index)} ${escape(shortName(op))}` : '';
}

/** The two CRSs' cards, for what the warnings say about them. */
export interface Cards {
  src: Card | null;
  dst: Card | null;
}

/** Each warning, short: the ⚠'s tooltip. */
const WARNINGS: Record<Warning, string> = {
  source_area: "outside the source CRS's area of use: check the CRS and the axis order",
  target_area: "the result is outside the target CRS's area of use: its accuracy is unknown",
  fallback_none: 'no operation covers this point: a stand-in was used',
  fallback_failed: "the operations covering this point couldn't convert it: a stand-in was used",
  only_area: 'outside the area of use of the only operation',
  forced_area: 'outside the area of use of the operation forced',
};

export function warningText(w: Warning): string {
  return WARNINGS[w];
}

/** Whether a warning earns a ⚠: not for an operation you forced, nor for
 * the only operation used outside its area when it shifts nothing. */
export function flagged(w: Warning, op: Operation | undefined): boolean {
  if (w === 'forced_area') return false;
  if (w === 'only_area') return !op?.null;
  return true;
}

/** How near two epochs must be to be one, as the engine has it: the same
 * thousandth of a year. */
export function sameEpoch(a: number, b: number): boolean {
  return Math.abs(a - b) <= 5e-4;
}

/** A link that sets the coordinate epoch to `value`, or with none opens
 * the option for one. */
function epochButton(value: number | null, text: string): string {
  return `<button class="link" data-epoch="${value ?? ''}">${text}</button>`;
}

const REASONS: Record<NonNullable<Outcome['reason']>, string> = {
  less_accurate: 'less accurate',
  unknown_accuracy: 'its accuracy is unknown',
  larger_area: 'as accurate, larger area',
  unknown_area_name: 'as accurate, its area has no name',
  offshore: 'its area is offshore',
  priority: 'the other is one PROJ prefers among equals',
  earlier: 'the other ranks first',
  at_epoch: "as accurate, and the other holds at the coordinate's epoch",
};

/** What the choice made of a candidate, in words. */
function outcomeText(o: Outcome | null, index: number, forced: number | null): string {
  if (!o) return index === forced ? 'forced' : '<span class="muted">—</span>';
  switch (o.kind) {
    case 'chosen':
      return 'chosen';
    case 'fallback':
      return 'stand-in: the first without grids, used whatever its area';
    case 'only':
      return 'the only one: used whatever its area';
    case 'passed':
      return `passed over for ${opNumber(o.by!)}: ${REASONS[o.reason!]}`;
    case 'outside':
      return '<span class="muted">outside its area</span>';
    case 'failed':
      return `<span class="error">failed: ${escape(o.error ?? '')}</span>`;
    case 'not_reached':
      return '<span class="muted">not reached: three were tried</span>';
    case 'not_at_epoch':
      return '<span class="muted">not looked at: only for a point at its epoch</span>';
  }
}

/** A CRS as the warnings name it: "EPSG:2180 ETRS89 / PL-1992 (Poland -
 * onshore and offshore.)". */
function crsPhrase(card: Card | null): string {
  if (!card?.code) return 'the CRS';
  const area = card.usages?.[0]?.area;
  return `EPSG:${card.code} ${escape(card.name ?? '')}${area ? ` (${escape(area)})` : ''}`;
}

/** A warning in full, for the details: what it means for this point. */
function warningDetail(w: Warning, d: Details, info: Info | null, cards: Cards): string {
  const used = d.op !== null && d.op !== undefined ? info?.operations[d.op] : undefined;
  const tag = used ? `${opTag(d.op!)} ${escape(shortName(used))}` : 'the operation';
  const area = used?.area_name ? ` (${escape(used.area_name)})` : '';
  const spread = Math.max(0, ...d.candidates.map((c) => c.distance ?? 0));
  const caveat = `Its parameters were determined for another region, so its stated accuracy (${accuracy(used?.accuracy)}) doesn't apply here${
    spread > 0 ? `: the candidates' results lie up to ${distance(spread)} from it` : ''
  }.`;
  switch (w) {
    case 'source_area':
      return `The input lies outside the area of use of ${crsPhrase(cards.src)}. Check that the coordinates are in this CRS,
        in the order shown above the input: a wrong CRS and swapped axes are the usual causes. If they are right, nothing is
        promised for a CRS outside its area: the accuracy of the result is unknown.`;
    case 'target_area': {
      const projected = cards.dst?.kind === 'projected' || !!cards.dst?.conversion;
      return `The result lies outside the area of use of ${crsPhrase(cards.dst)}. It is computed all the same, but nothing is
        promised for it: EPSG defines the CRS for that area only${
          projected
            ? `, and its projection${cards.dst?.conversion ? ` (${escape(cards.dst.conversion.method.name)})` : ''} distorts
               distances and areas more and more away from it. The numbers can look plausible and still be of little use`
            : ', where its datum is realised'
        }. If the point belongs there, a CRS whose area covers it is the right one.`;
    }
    case 'fallback_none':
      return `No operation is defined for this point. ${tag}${area} was used, as PROJ does: the first without grids, whatever its area. ${caveat}`;
    case 'fallback_failed': {
      const failed = d.candidates
        .filter((c) => c.inside && (c.outcome?.kind === 'failed' || c.outcome?.kind === 'not_reached'))
        .map((c) => `${info ? opLabel(info, c.index) : opNumber(c.index)} (${escape(c.outcome?.error ?? 'not reached')})`)
        .join(', ');
      return `The operations whose area covers this point couldn't convert it: ${failed}. So ${tag}${area} was used instead, as PROJ does. ${caveat}`;
    }
    case 'only_area':
      return used?.null
        ? `Outside the area of use of the only operation there is, which was used anyway, as PROJ does. It shifts nothing:
           it takes the two datums to be the same, which EPSG accepts to ${accuracy(used.accuracy)} within that area only;
           outside it, how far apart they are isn't known.`
        : used && used.only_at_epoch !== null
          ? `Outside the area of use of the only operation there is at this epoch. It was used anyway, as any only operation
             is; outside its area its accuracy isn't known.`
          : `Outside the area of use of the only operation there is. It was used anyway, as PROJ does; outside its area its
             accuracy isn't known.`;
    case 'forced_area':
      return `Outside the area of use of the operation you forced, ${tag}.`;
  }
}

function link(href: string, text: string): string {
  return `<a href="${escape(href)}" target="_blank" rel="noopener">${text}</a>`;
}

function row(label: string, value: string): string {
  return `<tr><th>${label}</th><td>${value}</td></tr>`;
}

/** A projection's method, linked, with its code. */
function conversionMethod(conversion: Conversion): string {
  const method = conversion.method;
  return method.code
    ? `${link(methodUrl(method.code, method.name), escape(method.name))} <span class="muted">(EPSG method ${method.code})</span>`
    : escape(method.name);
}

/** A projection's parameters as a table, or nothing when it has none. */
function conversionParams(conversion: Conversion): string {
  const params = (conversion.parameters ?? [])
    .map((p) => `<tr><td>${escape(p.name ?? '')}</td><td class="num">${projectionValue(p.value, p.unit)}</td></tr>`)
    .join('');
  return params ? `<table class="params">${params}</table>` : '';
}

function conversionRows(conversion: Conversion): string {
  const params = conversionParams(conversion);
  return (
    row('Projection', `${escape(conversion.name)}: ${conversionMethod(conversion)}`) + (params ? row('Parameters', params) : '')
  );
}

/** A CRS's card: what EPSG says about it. */
export function crsCard(title: string, card: Card | null): string {
  if (!card) return `<h3>${title}</h3><p class="muted">Type an EPSG code.</p>`;
  if (card.error && !card.name) return `<h3>${title}</h3><p class="error">${escape(card.error)}</p>`;
  const code = card.code!;
  const rows: string[] = [];
  rows.push(row('Kind', escape(card.kind ?? '')));
  const d = card.datum;
  if (d) {
    const ens = d.ensemble
      ? ` <span class="muted">(datum ensemble${d.members ? ` of ${d.members} members` : ''}${d.accuracy ? `, accuracy ${escape(d.accuracy)} m` : ''})</span>`
      : '';
    rows.push(row(d.ensemble ? 'Datum ensemble' : 'Datum', `${escape(d.name)}${d.code ? ` <span class="muted">EPSG:${d.code}</span>` : ''}${ens}`));
    const e = d.ellipsoid;
    if (e.name) {
      const shape =
        e.inverse_flattening !== null
          ? `a = ${plain(e.a ?? NaN)} m, 1/f = ${plain(e.inverse_flattening)}`
          : e.b !== null
            ? `a = ${plain(e.a ?? NaN)} m, b = ${plain(e.b)} m`
            : `sphere, R = ${plain(e.a ?? NaN)} m`;
      rows.push(row('Ellipsoid', `${escape(e.name)}: ${shape}`));
    }
    if (d.prime_meridian && d.prime_meridian.name !== 'Greenwich') {
      rows.push(row('Prime meridian', `${escape(d.prime_meridian.name)}, ${plain(d.prime_meridian.longitude ?? NaN)}° from Greenwich`));
    }
  }
  if (card.vertical_datum) {
    rows.push(row('Vertical datum', escape(card.vertical_datum.name)));
  }
  if (card.axes?.length) {
    const axes = card.axes
      .map((a, i) => `${i + 1}. ${escape(axisLabel(a))} <span class="muted">${escape(a.abbreviation)}, ${escape(a.direction)}</span>`)
      .join('<br>');
    rows.push(row('Axes', axes));
  }
  if (card.conversion) rows.push(conversionRows(card.conversion));
  for (const u of card.usages ?? []) {
    const bbox = u.bbox ? ` <span class="muted">${box([u.bbox[1], u.bbox[0], u.bbox[3], u.bbox[2]])}</span>` : '';
    rows.push(row('Area of use', `${escape(u.area ?? '')}${bbox}${u.scope ? `<br><span class="muted">${escape(u.scope)}</span>` : ''}`));
  }
  if (card.deprecated) {
    const repl = (card.replacements ?? [])
      .map((r) => `<button class="link" data-code="${r.code}">EPSG:${r.code}</button> ${escape(r.name ?? '')}`)
      .join(', ');
    rows.push(row('<span class="warn">Deprecated</span>', repl ? `replaced by ${repl}` : 'EPSG names no replacement'));
  }
  if (card.remarks) rows.push(row('Remarks', `<span class="remarks">${escape(card.remarks)}</span>`));
  const error = card.error ? `<p class="error">${escape(card.error)}</p>` : '';
  return `<h3>${title}: ${link(crsUrl(code, card.name), `EPSG:${code}`)} ${escape(card.name ?? '')}</h3>
    ${error}<table class="facts">${rows.join('')}</table>`;
}

/** An EPSG operation's method, linked, with its code. */
function epsgMethod(op: EpsgOperation): string {
  return op.method
    ? `${link(methodUrl(op.method.code, op.method.name), escape(op.method.name ?? ''))} <span class="muted">(EPSG method ${op.method.code})</span>`
    : '';
}

/** What an EPSG operation's step has under its head: the parameters,
 * the notes and EPSG's remarks. The head names the operation. */
function epsgOperation(op: EpsgOperation, inverse: boolean): string {
  const pvcf = /Position Vector|Coordinate Frame/.test(op.method?.name ?? '')
    ? `<p class="note">Position Vector and Coordinate Frame rotations differ only in the rotations' sign: check which convention your parameters follow.</p>`
    : '';
  const inv = inverse
    ? `<p class="note">Run inverted: from EPSG:${op.dst} to EPSG:${op.src}. The parameters are as EPSG publishes them for the forward direction.</p>`
    : '';
  const params = (op.params ?? [])
    .map((p) => `<tr><td>${escape(p.name ?? `parameter ${p.code}`)}</td><td class="num">${parameter(p)}</td></tr>`)
    .join('');
  const steps = (op.steps ?? []).length
    ? `<ol class="substeps">${op.steps!.map((s) => `<li>${link(operationUrl(s.code, s.name, s.table), `EPSG:${s.code}`)} ${escape(s.name ?? '')}${s.method ? `: ${escape(s.method.name ?? '')}` : ''}</li>`).join('')}</ol>`
    : '';
  return `<div class="op-detail">
    ${params ? `<table class="params">${params}</table>` : ''}${steps}${pvcf}${inv}
    ${op.remarks ? `<details class="remarks"><summary>EPSG's remarks</summary><p>${escape(op.remarks)}</p></details>` : ''}
  </div>`;
}

const STEP_KINDS: Record<Step['kind'], string> = {
  epsg: 'Transformation',
  ensemble_identity: 'Null operation',
  projection: 'Projection',
  same_datum: 'On one datum',
  ensemble_member: 'Ensemble member',
  vertical_conversion: 'Vertical conversion',
  ballpark_vertical: 'Ballpark vertical',
};

function stepHtml(step: Step, info: Info): string {
  const kind = STEP_KINDS[step.kind];
  const flag =
    step.kind === 'ballpark_vertical'
      ? '<p class="note warn">Heights are passed unchanged between two vertical datums: not a transformation.</p>'
      : step.kind === 'ensemble_identity' || step.kind === 'ensemble_member'
        ? '<p class="note">Between a datum ensemble and one of its members: no shift, as in PROJ.</p>'
        : '';
  let body = '';
  const d = step.detail;
  if (step.kind === 'epsg' && d?.operation) body = epsgOperation(d.operation, !!d.inverse);
  // A projection is named once, with its method. Its parameters are in
  // the card of the CRS it belongs to, when that is the source or the
  // target; the projection of a CRS without a card keeps them here.
  let projection = '';
  let pointer = '';
  if (step.kind === 'projection' && d?.conversion) {
    const card = d.crs === info.source ? ['#srcCard', 'From'] : d.crs === info.target ? ['#dstCard', 'To'] : null;
    projection = `: ${conversionMethod(d.conversion)}`;
    if (card) pointer = ` · parameters in the <a href="${card[0]}">${card[1]} card</a>`;
    else body = `<div class="op-detail">${conversionParams(d.conversion)}</div>`;
  }
  if (step.kind === 'ensemble_identity' && d?.method) {
    body = `<div class="op-detail">${escape(d.method.name ?? '')} <span class="muted">(EPSG method ${d.method.code})</span>, with zero parameters</div>`;
  }
  const crs = (c: Step['source']) => `<span title="${escape(c.name ?? '')}">${escape(c.label)}</span>`;
  // An EPSG operation's step is named once: its code, linked, the step's
  // name (which says when it runs inverted) and its method.
  const epsg = step.kind === 'epsg' ? d?.operation : undefined;
  const method = epsg ? epsgMethod(epsg) : '';
  const name = epsg
    ? `${link(operationUrl(epsg.code, epsg.name, epsg.table), `EPSG:${epsg.code}`)} ${escape(step.name)}${method ? `: ${method}` : ''}`
    : escape(step.name) + projection;
  // A projection or a conversion on one datum is exact: no accuracy to state.
  const exact = step.kind === 'projection' || step.kind === 'same_datum';
  return `<li class="step">
    <div class="step-head"><span class="kind">${kind}</span> ${name}
      <span class="muted">${crs(step.source)} → ${crs(step.target)}${step.accuracy !== null && !exact ? `, ${accuracy(step.accuracy)}` : ''}${pointer}</span></div>
    ${flag}${body}
  </li>`;
}

/** The candidate operations, best first. `counts`: how many points went
 * through each. */
export function operations(info: Info | null, error: string | null, counts: number[], perPoint = false): string {
  if (error) return `<p class="error">${escape(error)}</p>`;
  if (!info) return '<p class="muted">Type two EPSG codes.</p>';
  if (info.conversion) {
    return `<p>Both CRSs share one datum, so this is a <b>conversion</b>, with no transformation to choose.</p>`;
  }
  const items = info.operations.map((op, i) => operationHtml(op, i, info, counts[i] ?? 0, perPoint)).join('');
  // The routes through a time-specific transformation come last, and
  // aren't among the candidates for a point without an epoch.
  const epoch = info.epoch;
  const routeOps = info.operations.filter((op) => op.only_at_epoch !== null);
  const atEpoch = routeOps.length;
  const ordinary = info.operations.length - atEpoch;
  const holding = epoch === null ? 0 : routeOps.filter((op) => sameEpoch(op.only_at_epoch!, epoch)).length;
  const routes = `${atEpoch} route${atEpoch === 1 ? '' : 's'} through a time-specific transformation, which PROJ can't run`;
  const set = epochButton(null, 'Set an epoch');
  // What the epoch set, or the lack of one, means for those routes.
  const atTheEpoch = perPoint
    ? ordinary === 0
      ? 'Each point is at the epoch of its fourth value, and one without it fails.'
      : 'Each point is at the epoch of its fourth value; one without it has none.'
    : epoch === null
      ? ordinary === 0
        ? `No coordinate epoch is set, so the points fail unless you force a route. ${set}.`
        : `No coordinate epoch is set, so no point does unless you force it. ${set}.`
      : holding === 0
        ? `None holds at the epoch set, ${epoch}${ordinary === 0 ? ', so the points fail' : ''}.`
        : `${holding === atEpoch ? (atEpoch === 1 ? 'It holds' : 'They hold') : `${holding} of them hold${holding === 1 ? 's' : ''}`} at the epoch set, ${epoch}${
            ordinary > 0
              ? ': a point in its area goes through it when nothing there is more accurate'
              : atEpoch === 1
                ? ': the points go through it, whatever its area, as through any only operation'
                : ''
          }.`;
  const timeSpecific =
    atEpoch === 0
      ? ''
      : ordinary === 0
        ? `No operation for a point without an epoch: only ${routes}. Each holds at one epoch, and only a point at it
           goes that way. ${atTheEpoch}`
        : ` After ${ordinary === 1 ? 'it' : 'them'}, ${routes}: only a point at its epoch goes that way. ${atTheEpoch}`;
  // The other operations whose result depends on the epoch.
  const dependent = info.operations.some((op) => op.only_at_epoch === null && (op.time_dependent || op.needs_epoch));
  const timeDependent = !dependent
    ? ''
    : perPoint
      ? " Those that depend on time run at each point's epoch, its fourth value."
      : epoch === null
        ? ` Some depend on when the points were measured, and no coordinate epoch is set. ${set}.`
        : ` Those that depend on time run at the epoch set, ${epoch}.`;
  const intro =
    info.forced !== null
      ? `<p>${opLabel(info, info.forced)} is forced for every point, whatever its area.</p>`
      : ordinary > 1
        ? `<p>${ordinary} candidate operations, best first, as PROJ ranks them. Each point goes through the first whose area of use contains it
            (with its grids loaded); a point outside them all goes through the first without grids.${timeDependent}${timeSpecific}</p>`
        : ordinary === 1
          ? `<p>One operation: it is used for every point, whatever its area.${timeDependent}${timeSpecific}</p>`
          : `<p>${timeSpecific}</p>`;
  return `${intro}<ol class="operations">${items}</ol>`;
}

function operationHtml(op: Operation, index: number, info: Info, count: number, perPoint: boolean): string {
  const code = op.code
    ? `${link(operationUrl(op.code, op.name), `EPSG:${op.code}`)}${op.inverse ? ' <span class="muted">inverted</span>' : ''}`
    : '<span class="muted">put together from EPSG operations</span>';
  const grids = op.grids.length
    ? op.grids
        .map((g) => `<code>${escape(g.file)}</code>${g.failed ? ` <span class="error">couldn't be fetched: ${escape(g.failed)}</span>` : ''}`)
        .join(', ')
    : '';
  const flags: string[] = [];
  if (op.accuracy === null) flags.push('<span class="warn">accuracy unknown</span>');
  // What doesn't call for a look: how the epoch set is used.
  const notes: string[] = [];
  const epoch = info.epoch;
  const setEpoch = epochButton(null, 'Set an epoch');
  const muted = (text: string) => `<span class="muted">${text}</span>`;
  const warn = (text: string, button: string) => `<span class="warn">${text}</span>. ${button}`;
  // The only operation there is goes for every point, whatever its area.
  const whenChosen = info.operations.length === 1 ? '' : ', in its area, when nothing there is more accurate';
  if (op.only_at_epoch !== null) {
    const at = op.only_at_epoch;
    const use = epochButton(at, `Use ${at}`);
    if (perPoint) notes.push(muted(`time-specific: it holds at epoch ${at} only. A point with that epoch goes through it${whenChosen}`));
    else if (epoch === null)
      flags.push(
        warn(`time-specific: it holds at epoch ${at} only, and no coordinate epoch is set, so no point goes through it unless you force it`, use),
      );
    else if (sameEpoch(epoch, at)) notes.push(muted(`time-specific: it holds at epoch ${at}, the one set. A point goes through it${whenChosen}`));
    else flags.push(warn(`time-specific: it holds at epoch ${at} only, and the one set is ${epoch}, so no point goes through it`, use));
  } else if (op.needs_epoch) {
    if (perPoint) notes.push(muted("a deformation model: run at each point's epoch, its fourth value. A point without one fails"));
    else if (epoch === null)
      flags.push(warn('a deformation model: it needs the epoch of the points, and none is set, so its points fail', setEpoch));
    else notes.push(muted(`a deformation model: run at the epoch set, ${epoch}`));
  } else if (op.time_dependent) {
    if (perPoint)
      notes.push(muted("time-dependent: run at each point's epoch, its fourth value, and at its reference epoch for a point without one"));
    else if (epoch === null) flags.push(warn('time-dependent: run at its reference epoch, as no coordinate epoch is set', setEpoch));
    else notes.push(muted(`time-dependent: run at the epoch set, ${epoch}`));
  }
  if (op.chain.some((s) => s.kind === 'ballpark_vertical')) flags.push('<span class="warn">heights passed unchanged</span>');
  if (op.chain.some((s) => s.kind === 'ensemble_identity' || s.kind === 'ensemble_member'))
    flags.push('<span class="muted">datum ensemble to a member: no shift</span>');
  const used = info.forced !== null ? info.forced === index : count > 0;
  const where = op.area_name ? escape(op.area_name.length > 70 ? `${op.area_name.slice(0, 68)}…` : op.area_name) : '';
  // The summary line is the only place for the accuracy and the points
  // that went through the operation, and says nothing of points for none.
  const stated =
    op.accuracy === null ? 'accuracy unknown' : `<span title="Accuracy, as EPSG states it">${accuracy(op.accuracy)}</span>`;
  // An operation that is one EPSG transformation has that step's remarks:
  // they are shown once, in the step, beside the parameters they are about.
  const remarks = op.remarks && !op.chain.some((s) => s.detail?.operation?.remarks === op.remarks) ? op.remarks : null;
  return `<li class="operation${used ? ' used' : ''}" style="--c:${colour(index)}"><details>
    <summary class="op-head">${opTag(index)} <b>${escape(op.name)}</b>
      <span class="op-sum">${stated}${op.grids.length ? ' · grid' : ''}${op.only_at_epoch !== null ? ` · at epoch ${op.only_at_epoch} only` : ''}${flags.length ? ' · <span class="warn">⚠</span>' : ''}${
        count ? ` · ${count} point${count === 1 ? '' : 's'}` : ''
      }</span>
      <span class="op-area muted">${where}</span></summary>
    <table class="facts">
      ${row('EPSG', code)}
      ${op.area_name || op.area ? row('Area of use', `${escape(op.area_name ?? '')}${op.area ? ` <span class="muted">${box(op.area)}</span>` : ''}`) : ''}
      ${op.scope ? row('Scope', escape(op.scope)) : ''}
      ${grids ? row('Grids', grids) : ''}
      ${flags.length + notes.length ? row('Note', [...flags, ...notes].join('; ')) : ''}
    </table>
    ${remarks ? `<details class="remarks"><summary>EPSG's remarks</summary><p>${escape(remarks)}</p></details>` : ''}
    <details class="steps" open><summary>${op.chain.length} step${op.chain.length === 1 ? '' : 's'}: methods and parameters</summary>
      <ol>${op.chain.map((s) => stepHtml(s, info)).join('')}</ol></details>
  </details></li>`;
}

/** One point's details. `values` formats coordinates in the source's and
 * the target's decimals. */
export function details(
  line: number,
  input: number[],
  d: Details,
  info: Info | null,
  values: { source: (v: number[]) => string; target: (v: number[]) => string },
  cards: Cards,
  perPoint = false,
): string {
  const head = `<h2>Line ${line + 1} <button class="link close" data-action="close" aria-label="Close">×</button></h2>`;
  const lonlat = (ll: [number, number] | null | undefined) =>
    ll ? `${plain(Number(ll[1].toFixed(10)))}°, ${plain(Number(ll[0].toFixed(10)))}° <span class="muted">(lat, lon)</span>` : '—';
  const rows: string[] = [];
  rows.push(row('Input', `<code>${values.source(input)}</code>`));
  if (d.epoch !== null && d.epoch !== undefined)
    rows.push(
      row('Epoch', `${d.epoch} <span class="muted">(${perPoint ? "the line's fourth value" : 'the coordinate epoch set for every point'})</span>`),
    );
  else if (perPoint) rows.push(row('Epoch', '<span class="muted">none: the line has no fourth value</span>'));
  rows.push(row('On the source datum', lonlat(d.input_lonlat)));
  if (d.error) {
    rows.push(
      row(
        'Result',
        `<span class="error">${escape(d.error)}</span>${
          !d.missing_epoch ? '' : perPoint ? '. Give the line a fourth value, its epoch' : `. ${epochButton(null, 'Set an epoch')}`
        }`,
      ),
    );
  } else {
    rows.push(row('Result', `<code>${values.target(d.out!)}</code>`));
    rows.push(row('On the target datum', lonlat(d.output_lonlat)));
    if (info && !info.conversion && d.op !== null && d.op !== undefined) {
      const op = info.operations[d.op];
      rows.push(row('Through', `${opTag(d.op)} ${escape(op?.name ?? '')}`));
    } else if (info?.conversion) {
      rows.push(row('Through', 'a conversion on one datum'));
    }
  }
  for (const w of d.warnings ?? []) {
    const used = d.op !== null && d.op !== undefined ? info?.operations[d.op] : undefined;
    rows.push(
      flagged(w, used)
        ? row('<span class="warn">⚠</span>', `<span class="warn">${warningDetail(w, d, info, cards)}</span>`)
        : row('Note', `<span class="muted">${warningDetail(w, d, info, cards)}</span>`),
    );
  }
  const rt = d.round_trip;
  if (rt) {
    const text = rt.error
      ? `<span class="error">${escape(rt.error)}</span>`
      : `<code>${values.source(rt.back!)}</code><br>${rt.distance !== null && rt.distance !== undefined ? `${distance(rt.distance)} from the input` : ''}
         <span class="muted">(differences ${rt.delta!.filter((v) => v !== null).map((v) => plain(v!)).join(', ')})${rt.operation ? `, back through ${escape(rt.operation)}` : ''}</span>`;
    rows.push(row('Round trip', text));
  }
  let candidates = '';
  if (info && !info.conversion && d.candidates.length > 1) {
    const chosen = d.op ?? null;
    const items = d.candidates
      .map((c) => {
        const op = info.operations[c.index];
        const inside = c.inside === null ? '—' : c.inside ? '✓' : '<span class="muted">✗</span>';
        const result = c.error
          ? `<span class="error">${escape(c.error)}</span>`
          : `<code>${values.target(c.out!)}</code>`;
        const dist =
          c.index === chosen
            ? '<span class="muted">chosen</span>'
            : c.distance !== null && c.distance !== undefined
              ? distance(c.distance) + (c.dz ? `, Δh ${plain(c.dz)}` : '')
              : '—';
        return `<tr${c.index === chosen ? ' class="chosen"' : ''}><td>${opLabel(info, c.index)}</td><td class="center">${inside}</td>
          <td>${accuracy(op?.accuracy)}</td><td>${outcomeText(c.outcome, c.index, info.forced)}</td><td>${result}</td><td class="num">${dist}</td></tr>`;
      })
      .join('');
    candidates = `<h3>Through every candidate</h3>
      <p class="muted">What the choice made of each operation for this point, and how far each one's result lands from the chosen one, on the target's ellipsoid:
        whether the choice matters at all. "In area": the operation's area of use contains the point, tested as the engine tests it
        (the area converted into the source CRS).</p>
      <table class="grid"><thead><tr><th>Operation</th><th>In area</th><th>Accuracy</th><th>Choice</th><th>Result</th><th>From the chosen</th></tr></thead>
      <tbody>${items}</tbody></table>`;
  }
  return `${head}<table class="facts">${rows.join('')}</table>${traceHtml(d.trace)}${candidates}`;
}

/** A value with its axis's unit. */
function axisValue(v: number, axis: Axis | undefined): string {
  const unit = axis?.unit ?? '';
  if (unit === 'degree') return `${plain(v)}°`;
  if (unit === 'metre') return `${plain(v)} m`;
  return unit ? `${plain(v)} ${escape(unit)}` : plain(v);
}

const ARC_SECONDS_PER_DEGREE = 3600;
/** The mean Earth radius, for a shift's rough size in metres. */
const EARTH_RADIUS = 6371008.8;

/** The change across a step between CRSs with the same axes: each value's
 * difference (arc-seconds for degrees), and for latitude and longitude
 * the rough horizontal distance. */
function change(before: TracedStep, after: TracedStep): string {
  const same =
    before.axes.length >= 2 &&
    before.axes.slice(0, 2).every((a, i) => a.direction === after.axes[i]?.direction && a.unit === after.axes[i]?.unit);
  if (!same) return '';
  const parts: string[] = [];
  const n = Math.min(before.coords.length, after.coords.length);
  for (let i = 0; i < n; i++) {
    const d = after.coords[i] - before.coords[i];
    const axis = after.axes[i];
    if (d === 0) continue;
    const sign = d > 0 ? '+' : '−';
    const abs = Math.abs(d);
    // To 0.00001″ (0.3 mm on the ground) and 0.1 mm.
    const value =
      axis?.unit === 'degree'
        ? `${plain(Number((abs * ARC_SECONDS_PER_DEGREE).toFixed(5)))}″`
        : axisValue(Number(abs.toFixed(4)), axis);
    parts.push(`Δ${escape(axis?.abbreviation ?? String(i + 1))} ${sign}${value}`);
  }
  const lat = after.axes.findIndex((a) => a.direction === 'north' && a.unit === 'degree');
  const lon = after.axes.findIndex((a) => a.direction === 'east' && a.unit === 'degree');
  if (lat >= 0 && lon >= 0) {
    const rad = Math.PI / 180;
    const dy = (after.coords[lat] - before.coords[lat]) * rad * EARTH_RADIUS;
    const dx = (after.coords[lon] - before.coords[lon]) * rad * EARTH_RADIUS * Math.cos(after.coords[lat] * rad);
    const metres = Math.hypot(dx, dy);
    if (metres > 0) parts.push(`≈ ${distance(metres)} horizontally`);
  }
  return parts.length ? `<div class="muted change">${parts.join(', ')}</div>` : '';
}

/** What a traced step did, in words. */
function tracedStepText(s: TracedStep, previous: TracedStep | undefined): string {
  const d = s.detail ?? {};
  switch (s.kind) {
    case 'start':
      return 'As given';
    case 'convert': {
      const from = previous?.crs.kind;
      const to = s.crs.kind;
      if (from === 'projected' && to !== 'projected') return 'Inverse projection';
      if (to === 'projected' && from !== 'projected') return 'Projection';
      if (to === 'geocentric' && from !== 'geocentric') return 'To geocentric X, Y, Z';
      if (from === 'geocentric' && to !== 'geocentric') return 'From geocentric X, Y, Z';
      return 'Conversion on one datum';
    }
    case 'operation': {
      const op =
        d.code !== null && d.code !== undefined
          ? `${link(operationUrl(d.code, d.name), `EPSG:${d.code}`)} ${escape(d.name ?? '')}`
          : escape(d.name ?? '');
      const method = d.method?.name
        ? `<div class="muted">${link(methodUrl(d.method.code, d.method.name), escape(d.method.name))}${d.inverse ? ', run inverted' : ''}</div>`
        : '';
      const kept =
        d.height_kept && s.height !== 'none'
          ? '<div class="muted">The height comes back as it went in.</div>'
          : '';
      return `${op}${method}${kept}`;
    }
    case 'save':
      return 'Horizontal position kept, for an operation on heights';
    case 'restore':
      return 'Horizontal position put back, with the new height';
    case 'bound':
      return `${escape(d.name ?? '')}${d.inverse ? ', inverted' : ''} <div class="muted">The CRS's own transformation to its hub (TOWGS84)</div>`;
  }
}

/** Under geocentric coordinates: Z isn't a height. */
const GEOCENTRIC = "geocentric: from the Earth's centre, Z towards the North Pole";

/** A change of X, Y and Z as it is at the point: north, east and up.
 * `flat`: neither CRS has heights, so the result has none of the last. */
function onTheGround(shift: [number, number, number] | null | undefined, flat: boolean): string {
  if (!shift) return '';
  // Under a micrometre is the arithmetic, not the operation.
  const part = (v: number, plus: string, minus: string) =>
    Math.abs(v) < 1e-6 ? '' : `${distance(Math.abs(v))} ${v > 0 ? plus : minus}`;
  const [east, north, up] = shift;
  const vertical = part(up, 'up', 'down');
  const parts = [part(north, 'north', 'south'), part(east, 'east', 'west'), vertical].filter(Boolean);
  if (parts.length === 0) return '';
  const dropped = flat && vertical ? `. The ${vertical} is dropped: neither CRS has heights` : '';
  return `<div class="muted change">on the ground: ${parts.join(', ')}${dropped}</div>`;
}

const HEIGHTS: Record<TracedStep['height'], string> = {
  none: '',
  ellipsoidal: 'ellipsoidal height',
  gravity_related: 'gravity-related height',
};

/** Each step the point went through, with the coordinates after it. */
function traceHtml(trace: Trace | null | undefined): string {
  if (!trace || trace.steps.length === 0) return '';
  // From a CRS without heights to another: the third value is carried.
  const flat = trace.steps[0].height === 'none' && trace.steps[trace.steps.length - 1].height === 'none';
  const rows = trace.steps
    .map((s, i) => {
      const previous = trace.steps[i - 1];
      const coords = s.coords
        .map((v, j) => {
          const axis = s.axes[j];
          return `<div><span class="muted">${escape(axis?.abbreviation ?? '')}</span> ${axisValue(v, axis)}</div>`;
        })
        .join('');
      const shifts =
        previous && (s.kind === 'operation' || s.kind === 'bound')
          ? change(previous, s) + onTheGround(s.local_shift, flat)
          : '';
      const kind = s.crs.kind === 'geocentric' ? GEOCENTRIC : s.coords.length > 2 ? HEIGHTS[s.height] : '';
      const height = kind ? `<div class="muted change">${kind}</div>` : '';
      const crs = s.crs.code
        ? link(crsUrl(s.crs.code, s.crs.name), escape(s.crs.label))
        : escape(s.crs.label);
      return `<tr><td class="num">${i}</td><td>${tracedStepText(s, previous)}
          <div class="crs">${i === 0 ? 'in' : 'into'} ${crs} <span class="muted">${escape(s.crs.name)}</span></div></td>
        <td class="num">${coords}${height}${shifts}</td></tr>`;
    })
    .join('');
  const error = trace.error
    ? `<tr><td></td><td colspan="2"><span class="error">Stopped: ${escape(trace.error)}</span></td></tr>`
    : '';
  return `<h3>Step by step</h3>
    <p class="muted">The steps this point went through, with the coordinates after each, as the engine ran them: the last row is the result.
      An operation's change is the difference across it; for a grid, its shift here. Between geocentric CRSs the change of X, Y and Z
      is also given as it is on the ground. Conversions that change nothing are left out.</p>
    <div class="scroll"><table class="grid trace"><thead><tr><th>#</th><th>Step</th><th>Coordinates after it</th></tr></thead>
    <tbody>${rows}${error}</tbody></table></div>`;
}

/** Commands that do the same with PROJ, which xeibe is tested against. */
export function commands(
  src: number,
  dst: number,
  lines: string[],
  options: {
    eastFirst: boolean;
    aoi: number[] | null;
    accuracy: number | null;
    forced: string | null;
    /** The points' epoch: `lines` then have it as a fourth value. */
    epoch: number | null;
    /** Each point has its own, where its line has a fourth value. */
    perPoint: boolean;
    /** A route through a time-specific transformation holds at it. */
    timeSpecific: boolean;
  },
  srcFirstNorth: boolean,
): string {
  const extra = [
    options.aoi ? `--bbox ${options.aoi.map(plain).join(',')}` : '',
    options.accuracy !== null ? `--accuracy ${plain(options.accuracy)}` : '',
  ]
    .filter(Boolean)
    .join(' ');
  const sample = lines.length ? lines.slice(0, 20).join('\n') : options.epoch !== null ? `52.2 21.0 0 ${options.epoch}` : '52.2 21.0';
  const cs2cs = `PROJ_NETWORK=ON cs2cs -d 10 ${extra ? `${extra} ` : ''}EPSG:${src} EPSG:${dst} <<'EOF'\n${sample}\nEOF`;
  const projinfo = `PROJ_NETWORK=ON projinfo -s EPSG:${src} -t EPSG:${dst} --spatial-test intersects${extra ? ` ${extra}` : ''} -o PROJ`;
  const notes: string[] = [];
  if (options.eastFirst && srcFirstNorth)
    notes.push("cs2cs takes the CRSs' own axis order: with east first on, swap the first two columns of the input (and of its output).");
  if (options.epoch !== null || options.perPoint)
    notes.push(
      `${
        options.perPoint
          ? "cs2cs reads a fourth column as the point's epoch too."
          : "The coordinate epoch is cs2cs's fourth column, after a height: 0 where the input has none."
      }${
        options.timeSpecific
          ? " PROJ can't run a time-specific transformation, so a point that goes through one here comes out differently there."
          : ''
      }`,
    );
  if (options.forced !== null)
    notes.push(`cs2cs can't force one operation: take the pipeline of ${escape(options.forced)} from projinfo's list and run it with <code>cct</code>.`);
  return `<p class="muted">The same conversion with PROJ, ready to copy. PROJ_NETWORK=ON fetches grids from cdn.proj.org, as this page does.</p>
    <div class="command"><pre>${escape(cs2cs)}</pre><button class="button copy" data-copy="cs2cs">Copy</button></div>
    <div class="command"><pre>${escape(projinfo)}</pre><button class="button copy" data-copy="projinfo">Copy</button></div>
    ${notes.map((n) => `<p class="note">${n}</p>`).join('')}`;
}
