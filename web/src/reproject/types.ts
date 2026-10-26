// What crosses from crates/xeibe-wasm-reproject as JSON (src/page.rs).

export interface Axis {
  name: string;
  abbreviation: string;
  direction: string;
  unit: string | null;
}

export interface Parameter {
  name: string | null;
  code: number | null;
  value: number | string;
  /** A projection's unit ("degree", "metre", "unity"…). */
  unit: string | null;
}

export interface Conversion {
  name: string;
  method: { name: string; code: number | null };
  parameters: Parameter[] | null;
}

export interface Datum {
  name: string;
  code: number | null;
  ensemble: boolean;
  members: number | null;
  accuracy: string | null;
  ellipsoid: {
    name: string | null;
    code: number | null;
    a: number | null;
    inverse_flattening: number | null;
    b: number | null;
  };
  prime_meridian: { name: string; longitude: number | null } | null;
}

export interface Usage {
  scope: string | null;
  area: string | null;
  /** South, west, north, east. */
  bbox: [number, number, number, number] | null;
}

export interface Card {
  code?: number;
  name?: string;
  kind?: string;
  deprecated?: boolean;
  replacements?: { code: number; name: string | null }[];
  first_axis_north?: boolean;
  geographic?: boolean;
  dimension?: number;
  datum?: Datum | null;
  vertical_datum?: { name: string; code: number | null } | null;
  axes?: Axis[];
  conversion?: Conversion | null;
  area_wgs84?: number[] | null;
  usages?: Usage[];
  remarks?: string | null;
  error?: string | null;
}

/** An EPSG operation's parameter: its value in SI units, and the kind of
 * unit ("length", "angle", "scale", "time", "file", …_per_time). */
export interface OpParameter {
  code: number;
  name: string | null;
  unit: string | null;
  value: number | string;
}

export interface EpsgOperation {
  code: number;
  name?: string;
  table?: string;
  method?: { code: number; name: string | null } | null;
  params?: OpParameter[];
  steps?: EpsgOperation[];
  src?: number;
  dst?: number;
  accuracy?: number | null;
  remarks?: string | null;
}

export interface StepCrs {
  label: string;
  name: string | null;
}

export interface Step {
  name: string;
  kind:
    | 'epsg'
    | 'ensemble_identity'
    | 'projection'
    | 'same_datum'
    | 'ensemble_member'
    | 'vertical_conversion'
    | 'ballpark_vertical';
  detail: {
    inverse?: boolean;
    operation?: EpsgOperation;
    method?: { code: number; name: string | null };
    crs?: number;
    conversion?: Conversion | null;
  } | null;
  source: StepCrs;
  target: StepCrs;
  accuracy: number | null;
}

export interface Operation {
  name: string;
  code: number | null;
  inverse: boolean | null;
  accuracy: number | null;
  /** West, south, east, north. */
  area: [number, number, number, number] | null;
  area_name: string | null;
  grids: { file: string; failed: string | null }[];
  time_dependent: boolean;
  scope: string | null;
  remarks: string | null;
  /** It shifts nothing: its parameters are zero. */
  null: boolean;
  chain: Step[];
}

export interface Info {
  source: number;
  target: number;
  conversion: boolean;
  forced: number | null;
  source_dimension: number;
  target_dimension: number;
  operations: Operation[];
}

/** Outside the source's or the target's area of use; or the operation used
 * outside its own: no operation covers the point (`fallback_none`), those
 * that do couldn't convert it (`fallback_failed`), the only operation there
 * is (`only_area`), the one forced (`forced_area`). */
export type Warning =
  | 'source_area'
  | 'target_area'
  | 'fallback_none'
  | 'fallback_failed'
  | 'only_area'
  | 'forced_area';

/** What the engine's choice made of a candidate, for one point
 * (`Transform::explain_point`). */
export interface Outcome {
  kind: 'chosen' | 'fallback' | 'only' | 'passed' | 'outside' | 'failed' | 'not_reached';
  /** passed: the operation preferred, and why. */
  by?: number;
  reason?:
    | 'less_accurate'
    | 'unknown_accuracy'
    | 'larger_area'
    | 'unknown_area_name'
    | 'offshore'
    | 'priority'
    | 'earlier';
  /** failed: why. */
  error?: string;
}

export type Result =
  | { out: number[]; op: number | null; warnings: Warning[]; error?: undefined }
  | { error: string; out?: undefined };

export interface Candidate {
  index: number;
  /** Whether its area contains the point, as the engine tests it. */
  inside: boolean | null;
  /** null with an operation forced. */
  outcome: Outcome | null;
  out?: number[];
  distance?: number | null;
  dz?: number | null;
  error?: string;
}

export interface Details {
  input_lonlat: [number, number] | null;
  error?: string;
  out?: number[];
  output_lonlat?: [number, number] | null;
  op?: number | null;
  warnings?: Warning[];
  candidates: Candidate[];
  round_trip?: {
    error?: string;
    back?: number[];
    delta?: (number | null)[];
    distance?: number | null;
    operation?: string | null;
  };
}

export interface GridRequest {
  file: string;
  offset: number;
  length: number | null;
  url: string;
}

/** A line of the input: null when blank or invalid. */
export type Point = [number, number] | [number, number, number];
