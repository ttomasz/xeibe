// Messages between the page and its worker (worker.ts).

/** One batch's map buffers in one coordinate space; see
 * crates/xeibe-wasm/src/display.rs. Starts are Arrow-style offsets (one
 * more than the items), rows are feature rows of the whole read. */
export interface Space {
  points: Float64Array;
  pointRows: Uint32Array;
  lines: Float64Array;
  lineStarts: Uint32Array;
  lineRows: Uint32Array;
  polygons: Float64Array;
  polygonStarts: Uint32Array;
  polygonRows: Uint32Array;
  triangles: Uint32Array;
  ringStarts: Uint32Array;
  ringRows: Uint32Array;
  /** min x, min y, max x, max y per feature of the batch; NaN if none. */
  bounds: Float64Array;
}

export interface GridRequest {
  url: string;
  file: string;
  offset: number;
  length: number | null;
}

export type ToWorker =
  | { type: 'open'; files: { name: string; buffer: ArrayBuffer }[] }
  | { type: 'schema'; layer: string }
  | { type: 'read'; id: number; layer: string; limit: number | null }
  | { type: 'stop'; id: number }
  | { type: 'rows'; id: number; request: number; start: number; count: number };

export type FromWorker =
  | { type: 'scanned'; scan: Scan; ms: number }
  | { type: 'schema'; layer: string; schema: Schema }
  | { type: 'info'; id: number; info: ReadInfo }
  | { type: 'grids'; id: number; files: string[] }
  | {
      type: 'batch';
      id: number;
      firstRow: number;
      rows: number;
      origin: [number, number] | null;
      source: Space;
      map: Space | null;
    }
  | { type: 'done'; id: number; report: Report; info: ReadInfo; ms: number }
  | { type: 'stopped'; id: number; report: Report }
  | { type: 'rows'; id: number; request: number; rows: string[][] }
  | { type: 'error'; message: string };

// JSON from crates/xeibe-wasm/src/session.rs.

export interface CrsInfo {
  srs_name: string;
  crs: string | null;
  name: string | null;
  kind: string | null;
  deprecated: boolean | null;
  area_wgs84: [number, number, number, number] | null;
}

export interface AxisDecision {
  layer?: string;
  source: string | null;
  srs_name: string | null;
  dialect: string;
  swap: boolean;
  reason: string;
  conflicts: string[];
}

export interface Scan {
  files: string[];
  complete: boolean;
  skipped_sources: string[];
  extent: [number, number, number, number] | null;
  layers: {
    name: string;
    qname: string;
    feature_count: number;
    geometry_columns: string[];
    crs: CrsInfo[];
    extent: [number, number, number, number] | null;
  }[];
  axis_decisions: AxisDecision[];
}

export interface Column {
  name: string;
  type: string;
  nullable: boolean;
  path: string | null;
  srs_name: string | null;
  axis_decision: string | null;
}

export interface Schema {
  columns: Column[];
  explain: string;
}

export interface ReadInfo {
  columns: Column[];
  geometry_column: string | null;
  map: {
    srs_name: string | null;
    crs: CrsInfo | null;
    target: string | null;
    notes: string[];
    area_of_use: { map: [number, number][]; source: [number, number][] } | null;
  };
}

export interface Location {
  source: string | null;
  byte: number;
  feature: number | null;
  gml_id: string | null;
}

export interface Report {
  rows: number;
  features_per_layer: Record<string, number>;
  skipped: { location: Location; message: string }[];
  warnings: { kind: string; location: Location | null; message: string }[];
  axis_decisions: AxisDecision[];
  not_on_map: number;
  map_error: string | null;
  geometry_errors: { row: number; message: string }[];
}
