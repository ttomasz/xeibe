// The XML tab: the selected row's feature as it is written in its file.

import type { FeatureXml } from './protocol';
import { escape } from './views';

const number = new Intl.NumberFormat('en');

/** Whether to indent by default: a feature written on one line. */
export function wantsIndent(xml: string): boolean {
  return !xml.slice(0, 4000).includes('\n');
}

export function xmlView(f: FeatureXml, indent: boolean): string {
  const size = f.end - f.start;
  const text = indent ? indented(f.xml) : dedented(f.xml);
  return `<section class="xml-head">
      <div><b>Row ${number.format(f.row + 1)}</b> · <code>${escape(f.source)}</code>,
        bytes ${number.format(f.start)}–${number.format(f.end)} (${number.format(size)} bytes)</div>
      <div class="xml-tools">
        <label><input type="checkbox" data-action="indent"${indent ? ' checked' : ''}> Indent</label>
        <button class="button" data-action="copy">Copy</button>
      </div>
    </section>
    ${f.truncated ? `<p class="warn">Only the first ${number.format(f.xml.length)} characters are shown.</p>` : ''}
    <pre class="xml">${highlight(text)}</pre>`;
}

/** As written, less the indentation the feature has in its file: the
 * first line starts at its `<`, and the last (its end tag) has it. */
function dedented(xml: string): string {
  const lines = xml.split('\n');
  const margin = /^[ \t]*/.exec(lines[lines.length - 1])![0];
  if (lines.length < 2 || margin === '') return xml;
  return lines.map((line, i) => (i > 0 && line.startsWith(margin) ? line.slice(margin.length) : line)).join('\n');
}

/** Comments, CDATA, processing instructions, tags, and the text between. */
const TOKENS = /<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>|<\?[\s\S]*?\?>|<[^>]*>|[^<]+/g;

/** One element per line, indented by depth; an element holding only text
 * stays on one line. The text itself is kept as written. */
export function indented(xml: string): string {
  const out: string[] = [];
  let depth = 0;
  /** The last token opened an element, or was text in one just opened. */
  let open = false;
  for (const token of xml.match(TOKENS) ?? []) {
    if (!token.startsWith('<')) {
      if (token.trim() === '') continue;
      if (open) out[out.length - 1] += token;
      else out.push('  '.repeat(depth) + token.trim());
      continue;
    }
    if (token.startsWith('</')) {
      depth = Math.max(0, depth - 1);
      if (open) out[out.length - 1] += token;
      else out.push('  '.repeat(depth) + token);
      open = false;
    } else if (token.startsWith('<!') || token.startsWith('<?') || token.endsWith('/>')) {
      out.push('  '.repeat(depth) + token);
      open = false;
    } else {
      out.push('  '.repeat(depth) + token);
      depth++;
      open = true;
    }
  }
  return out.join('\n');
}

/** HTML with tags, attribute names and values marked. */
function highlight(xml: string): string {
  return (xml.match(/<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>|<[^>]*>|[^<]+/g) ?? [])
    .map((token) => {
      if (token.startsWith('<!--')) return `<span class="x-comment">${escape(token)}</span>`;
      if (!token.startsWith('<') || token.startsWith('<![CDATA[')) return escape(token);
      const match = /^(<\/?[^\s/>]*)([\s\S]*?)(\/?>)$/.exec(token);
      if (!match) return escape(token);
      const attributes = match[2].replace(
        /([^\s=]+)(\s*=\s*)("[^"]*"|'[^']*')/g,
        '\u0001$1\u0002$2\u0003$3\u0004',
      );
      const html = escape(attributes)
        .replace(/\u0001([^\u0002]*)\u0002/g, '<span class="x-attr">$1</span>')
        .replace(/\u0003([^\u0004]*)\u0004/g, '<span class="x-value">$1</span>');
      return `<span class="x-tag">${escape(match[1])}</span>${html}<span class="x-tag">${escape(match[3])}</span>`;
    })
    .join('');
}
