// The input: one point per line, two or three decimal numbers separated by
// spaces or tabs.

import type { Point } from './types';

/** The whole syntax of a line, checked before anything is parsed. */
export const LINE = /^\s*[+-]?\d+(\.\d+)?[ \t]+[+-]?\d+(\.\d+)?([ \t]+[+-]?\d+(\.\d+)?)?\s*$/;

export interface Parsed {
  /** One entry per line: null for a blank or invalid line, so output line
   * n is input line n. */
  points: (Point | null)[];
  /** Lines that don't match, counted from 0. */
  invalid: number[];
  /** One entry per line: whether its values are separated by tabs. */
  tabbed: boolean[];
}

export function parse(text: string): Parsed {
  const points: (Point | null)[] = [];
  const invalid: number[] = [];
  const tabbed: boolean[] = [];
  text.split('\n').forEach((raw, i) => {
    const line = raw.replace(/\r$/, '');
    tabbed.push(line.trim().includes('\t'));
    if (line.trim() === '') {
      points.push(null);
    } else if (LINE.test(line)) {
      points.push(line.trim().split(/[ \t]+/).map(Number) as Point);
    } else {
      points.push(null);
      invalid.push(i);
    }
  });
  // A trailing newline leaves no line behind it.
  while (points.length && points[points.length - 1] === null && !invalid.includes(points.length - 1)) points.pop();
  return { points, invalid, tabbed };
}
