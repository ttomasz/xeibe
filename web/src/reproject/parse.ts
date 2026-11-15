// The input: one point per line, two or three decimal numbers separated by
// spaces or tabs, and a fourth, the point's epoch, when the page reads one.

import type { Point } from './types';

const NUMBER = String.raw`[+-]?\d+(\.\d+)?`;
/** The whole syntax of a line, checked before anything is parsed. */
export const LINE = new RegExp(String.raw`^\s*${NUMBER}[ \t]+${NUMBER}([ \t]+${NUMBER})?\s*$`);
/** A line with a fourth value: x y z t. */
export const LINE_WITH_EPOCH = new RegExp(String.raw`^\s*${NUMBER}([ \t]+${NUMBER}){3}\s*$`);

export interface Parsed {
  /** One entry per line: null for a blank or invalid line, so output line
   * n is input line n. */
  points: (Point | null)[];
  /** Lines that don't match, counted from 0. */
  invalid: number[];
  /** Of those, the lines that would be points with a fourth value read as
   * the epoch. */
  withEpoch: number[];
  /** One entry per line: whether its values are separated by tabs. */
  tabbed: boolean[];
}

/** `epochs`: a fourth value is the point's epoch, a year above zero. */
export function parse(text: string, epochs = false): Parsed {
  const points: (Point | null)[] = [];
  const invalid: number[] = [];
  const withEpoch: number[] = [];
  const tabbed: boolean[] = [];
  text.split('\n').forEach((raw, i) => {
    const line = raw.replace(/\r$/, '');
    tabbed.push(line.trim().includes('\t'));
    const values = () => line.trim().split(/[ \t]+/).map(Number);
    if (line.trim() === '') {
      points.push(null);
    } else if (LINE.test(line) || (epochs && LINE_WITH_EPOCH.test(line) && values()[3] > 0)) {
      points.push(values() as Point);
    } else {
      points.push(null);
      invalid.push(i);
      if (!epochs && LINE_WITH_EPOCH.test(line) && values()[3] > 0) withEpoch.push(i);
    }
  });
  // A trailing newline leaves no line behind it.
  while (points.length && points[points.length - 1] === null && !invalid.includes(points.length - 1)) points.pop();
  return { points, invalid, withEpoch, tabbed };
}
