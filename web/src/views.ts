// The Overview and Schema tabs, as HTML from the worker's JSON.

import type { AxisDecision, CrsInfo, ReadInfo, Report, Scan, Schema } from './protocol';

export function escape(text: unknown): string {
  return String(text ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

const number = new Intl.NumberFormat('en');

function extent(e: [number, number, number, number] | null): string {
  if (!e) return '—';
  const f = (v: number) => number.format(Math.round(v * 1000) / 1000);
  return `(${f(e[0])}, ${f(e[1])}) – (${f(e[2])}, ${f(e[3])})`;
}

function crs(c: CrsInfo): string {
  const name = c.name ? `${escape(c.name)} ` : '';
  const code = c.crs ? `<code>${escape(c.crs)}</code>` : '<em>unknown CRS</em>';
  const deprecated = c.deprecated ? ' <span class="tag warn">deprecated code</span>' : '';
  const written = c.crs && c.srs_name !== c.crs ? ` <span class="muted">as <code>${escape(c.srs_name)}</code></span>` : '';
  return `${name}${code}${deprecated}${written}`;
}

/** A reason clause from xeibe as a sentence. */
function sentence(text: string): string {
  return escape(text.charAt(0).toUpperCase() + text.slice(1)) + '.';
}

function axisTable(decisions: AxisDecision[], withFile: boolean): string {
  if (decisions.length === 0) return '<p class="muted">No geometry, no axis decisions.</p>';
  const rows = decisions
    .map(
      (d) => `<tr>
        <td><code>${escape(d.srs_name ?? '(none)')}</code>${withFile ? `<div class="muted">${escape(d.source)}</div>` : ''}</td>
        <td class="order">${d.swap ? 'y/x <span class="tag warn">swapped</span>' : 'x/y'}</td>
        <td>${sentence(d.reason)}${
          d.conflicts.length
            ? `<div class="warn">Against it:<ul>${d.conflicts.map((c) => `<li>${sentence(c)}</li>`).join('')}</ul></div>`
            : ''
        }</td>
      </tr>`,
    )
    .join('');
  return `<p class="muted">The output is x/y: easting or longitude first. A file can have its coordinates the
    other way round, y/x, as some CRSs define their axes; xeibe then swaps them.</p>
    <table class="grid"><thead><tr><th>srsName</th><th>In the file</th><th>Why</th></tr></thead><tbody>${rows}</tbody></table>`;
}

export interface OverviewState {
  scan: Scan;
  scanMs: number;
  layer: string | null;
  info: ReadInfo | null;
  report: Report | null;
  reading: boolean;
  limit: number | null;
  grids: string[];
}

export function overview(s: OverviewState): string {
  const { scan } = s;
  const layer = scan.layers.find((l) => l.name === s.layer);
  const parts: string[] = [];
  const scanned =
    scan.sample === null
      ? 'Scanned'
      : `Scanned the first ${number.format(scan.sample)} feature${scan.sample === 1 ? '' : 's'} of each file`;
  parts.push(`<section><h2>Input</h2>
    <p>${inputs(scan.files)} ${scanned} in ${number.format(Math.round(s.scanMs))} ms:
    ${scan.layers.length} layer${scan.layers.length === 1 ? '' : 's'}.</p>
    ${
      scan.complete
        ? ''
        : `<p class="sampled">The schema comes from a sample: layers that start later in a file are missing, and the
        feature counts are what the sample saw. <button class="button" data-action="full-scan">Scan the whole files</button></p>`
    }
    ${scan.skipped_sources.length ? `<p class="warn">Skipped (no GML features): ${scan.skipped_sources.map(escape).join(', ')}</p>` : ''}
  </section>`);
  if (layer) {
    parts.push(`<section><h2>Layer <code>${escape(layer.name)}</code></h2>
      <dl>
        <dt>Name</dt><dd><code>${escape(layer.qname)}</code></dd>
        <dt>Features</dt><dd>${scan.complete || layer.counted ? '' : 'at least '}${number.format(layer.feature_count)}</dd>
        <dt>Geometry</dt><dd>${layer.geometry_columns.length ? layer.geometry_columns.map((c) => `<code>${escape(c)}</code>`).join(', ') : '<em>none</em>'}</dd>
        <dt>CRS</dt><dd>${layer.crs.length ? layer.crs.map(crs).join('<br>') : '<em>no srsName</em>'}</dd>
        <dt>Extent</dt><dd>${extent(layer.extent)} <span class="muted">as the collection's boundedBy says</span></dd>
      </dl></section>`);
  }
  if (s.info) parts.push(mapSection(s.info, s.report, s.grids));
  if (s.report) parts.push(reportSection(s.report, s.reading, s.limit));
  else if (s.reading) parts.push('<section><h2>Read</h2><p class="muted">Reading…</p></section>');
  const severalFiles = scan.files.length > 1;
  const decisions = s.report?.axis_decisions.length
    ? axisTable(s.report.axis_decisions, severalFiles)
    : axisTable(scan.axis_decisions.filter((d) => !layer || d.layer === layer.name), severalFiles);
  parts.push(`<section><h2>Axis order</h2>${decisions}</section>`);
  return parts.join('');
}

/** The sources, with a zip's members under the archive's name. */
function inputs(files: string[]): string {
  const groups = new Map<string, string[]>();
  for (const file of files) {
    const [archive, member] = file.includes('!/') ? file.split('!/', 2) : [file, ''];
    if (!groups.has(archive)) groups.set(archive, []);
    if (member) groups.get(archive)!.push(member);
  }
  return [...groups]
    .map(([name, members]) => {
      if (members.length === 0) return `<code>${escape(name)}</code>.`;
      const list = members.map((m) => `<li><code>${escape(m)}</code></li>`).join('');
      return `<details><summary><code>${escape(name)}</code>: ${number.format(members.length)} GML member${members.length === 1 ? '' : 's'}</summary><ul class="members">${list}</ul></details>`;
    })
    .join('');
}

function mapSection(info: ReadInfo, report: Report | null, grids: string[]): string {
  const m = info.map;
  const lines: string[] = [];
  if (info.geometry_column) lines.push(`Drawn: <code>${escape(info.geometry_column)}</code>`);
  if (m.crs) lines.push(`CRS: ${crs(m.crs)}`);
  if (m.target) lines.push(`Converted to longitude/latitude in ${escape(m.target)} by xeibe-reproject, then drawn in Web Mercator`);
  else lines.push('Source CRS view only');
  if (grids.length) lines.push(`Grids from cdn.proj.org: ${grids.map((g) => `<code>${escape(g)}</code>`).join(', ')}`);
  for (const note of m.notes) lines.push(`<span class="warn">${escape(note)}</span>`);
  if (report?.not_on_map) {
    lines.push(
      `<span class="warn">${number.format(report.not_on_map)} feature${report.not_on_map === 1 ? '' : 's'} not on the map: ${escape(report.map_error)}</span>`,
    );
  }
  return `<section><h2>Map</h2><ul class="plain">${lines.map((l) => `<li>${l}</li>`).join('')}</ul></section>`;
}

function reportSection(r: Report, reading: boolean, limit: number | null): string {
  const limited = limit !== null && r.rows >= limit;
  const head = reading
    ? `Reading… ${number.format(r.rows)} rows`
    : `${number.format(r.rows)} rows read${limited ? ` <span class="tag">stopped at the limit</span>` : ''}`;
  const parts = [`<p>${head}</p>`];
  if (r.skipped.length) {
    parts.push(`<h3>Skipped features (${number.format(r.skipped.length)})</h3><ul>${r.skipped
      .slice(0, 50)
      .map((s) => `<li>${location(s.location)}: ${escape(s.message)}</li>`)
      .join('')}</ul>`);
  }
  if (r.warnings.length) {
    parts.push(`<h3>Warnings (${number.format(r.warnings.length)})</h3><ul>${r.warnings
      .slice(0, 50)
      .map((w) => `<li><span class="tag">${escape(w.kind)}</span> ${w.location ? `${location(w.location)}: ` : ''}${escape(w.message)}</li>`)
      .join('')}</ul>`);
  }
  if (r.geometry_errors.length) {
    parts.push(`<h3>Geometry not drawn</h3><ul>${r.geometry_errors
      .map((e) => `<li>row ${e.row + 1}: ${escape(e.message)}</li>`)
      .join('')}</ul>`);
  }
  return `<section><h2>Read report</h2>${parts.join('')}</section>`;
}

function location(l: { source: string | null; byte: number; feature: number | null; gml_id: string | null }): string {
  const bits = [l.feature !== null ? `feature ${number.format(l.feature)}` : `byte ${number.format(l.byte)}`];
  if (l.gml_id) bits.push(`<code>${escape(l.gml_id)}</code>`);
  return bits.join(' ');
}

export function schema(layer: string, s: Schema): string {
  const rows = s.columns
    .map(
      (c) => `<tr>
        <td><code>${escape(c.name)}</code></td>
        <td><code>${escape(c.type)}</code>${c.nullable ? '' : ' <span class="tag">not null</span>'}</td>
        <td><code class="path">${escape(c.path)}</code></td>
      </tr>`,
    )
    .join('');
  return `<section><h2>Columns of <code>${escape(layer)}</code></h2>
    <p class="muted">As a scan infers them: what <code>xeibe gml scan</code> writes to a settings file.</p>
    <table class="grid"><thead><tr><th>Column</th><th>Arrow type</th><th>Path</th></tr></thead><tbody>${rows}</tbody></table>
  </section>
  <section><h2>Why</h2><pre class="explain">${escape(s.explain)}</pre></section>`;
}
