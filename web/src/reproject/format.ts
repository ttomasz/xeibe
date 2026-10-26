// Numbers, units and parameters as the page shows them.

import type { Axis, OpParameter } from './types';

export function escape(text: string): string {
  return text.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

/** A number without float noise: 12 significant digits at most. */
export function plain(value: number): string {
  if (!Number.isFinite(value)) return String(value);
  return String(Number(value.toPrecision(12)));
}

/** A distance in metres, in the unit that suits it. */
export function distance(metres: number): string {
  const m = Math.abs(metres);
  if (m === 0) return '0';
  if (m < 1e-6) return `${(metres * 1e9).toPrecision(2)} nm`;
  if (m < 1e-3) return `${(metres * 1e6).toPrecision(2)} µm`;
  if (m < 0.01) return `${(metres * 1e3).toPrecision(2)} mm`;
  if (m < 1) return `${(metres * 100).toPrecision(3)} cm`;
  if (m < 1000) return `${metres.toPrecision(4)} m`;
  return `${(metres / 1000).toPrecision(4)} km`;
}

export function accuracy(metres: number | null | undefined): string {
  return metres === null || metres === undefined ? 'unknown' : `${plain(metres)} m`;
}

const ARC_SECONDS = (180 / Math.PI) * 3600;
/** EPSG's year (unit 1029), in seconds. */
const YEAR = 31556925.445;

/** An EPSG operation's parameter, from SI units back to the units EPSG
 * publishes such parameters in: arc-seconds for rotations and offsets,
 * parts per million for a scale difference, years for epochs. */
export function parameter(p: OpParameter): string {
  if (typeof p.value === 'string') return escape(p.value);
  const v = p.value;
  switch (p.unit) {
    case 'length':
      return `${plain(v)} m`;
    case 'angle': {
      const s = v * ARC_SECONDS;
      return Math.abs(s) >= 3600 ? `${plain((v * 180) / Math.PI)}°` : `${plain(s)}″`;
    }
    case 'scale':
      // Scale differences (8611) are published in ppm; scale factors as is.
      return p.code === 8611 ? `${plain(v * 1e6)} ppm` : plain(v);
    case 'time':
      return `${plain(v / YEAR)} (year)`;
    case 'length_per_time':
      return `${plain(v * YEAR)} m/yr`;
    case 'angle_per_time':
      return `${plain(v * YEAR * ARC_SECONDS)}″/yr`;
    case 'scale_per_time':
      return `${plain(v * YEAR * 1e6)} ppm/yr`;
    default:
      return plain(v);
  }
}

/** A projection's parameter, in the unit its PROJJSON gives. */
export function projectionValue(value: number | string, unit: string | null): string {
  if (typeof value === 'string') return escape(value);
  switch (unit) {
    case 'degree':
      return `${plain(value)}°`;
    case 'metre':
      return `${plain(value)} m`;
    case 'unity':
    case null:
      return plain(value);
    default:
      return `${plain(value)} ${escape(unit)}`;
  }
}

/** Default decimals for an axis: 9 for degrees (about 0.1 mm), 4 for
 * metres and feet. */
export function defaultDecimals(axis: Axis | undefined): number {
  const unit = axis?.unit ?? 'metre';
  if (/degree|grad|radian|arc/i.test(unit)) return 9;
  return 4;
}

/** An axis's short label: "Lat (°)", "x (m)". */
export function axisLabel(axis: Axis): string {
  const unit = axis.unit === 'degree' ? '°' : axis.unit === 'metre' ? 'm' : (axis.unit ?? '');
  return `${axis.name}${unit ? ` (${unit})` : ''}`;
}

/** A slug for epsg.org's page URLs, which want one but ignore what it is. */
function slug(name: string | null | undefined): string {
  return (name ?? 'x').replace(/[^A-Za-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'x';
}

export function crsUrl(code: number, name?: string | null): string {
  return `https://epsg.org/crs_${code}/${slug(name)}.html`;
}

export function operationUrl(code: number, name?: string | null, table?: string): string {
  const kind = table === 'concatenated_operation' ? 'concatenated-operation' : 'transformation';
  return `https://epsg.org/${kind}_${code}/${slug(name)}.html`;
}

export function methodUrl(code: number, name?: string | null): string {
  return `https://epsg.org/coord-operation-method_${code}/${slug(name)}.html`;
}

/** A box as west, south, east, north. */
export function box(area: readonly number[]): string {
  const [w, s, e, n] = area.map((v) => plain(v));
  return `W ${w}°, S ${s}°, E ${e}°, N ${n}°`;
}
