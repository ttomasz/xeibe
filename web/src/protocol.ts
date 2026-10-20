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

/** A file to scan and read, or one member of a zip archive. */
export interface Choice {
  /** The file's index in the files opened. */
  file: number;
  member?: string;
}

/** The axis order the user chose for an srsName, over xeibe's own decision. */
export type AxisOverride = 'XY' | 'YX';

export type ToWorker =
  | { type: 'open'; files: File[] }
  | { type: 'scan'; choices: Choice[]; sample: number | null }
  | { type: 'schema'; layer: string }
  /** `axis`: srsName as written → the order chosen for it. */
  | { type: 'read'; id: number; layer: string; limit: number | null; axis: Record<string, AxisOverride> }
  | { type: 'stop'; id: number }
  | { type: 'rows'; id: number; request: number; start: number; count: number }
  | { type: 'xml'; id: number; row: number };

export type FromWorker =
  | { type: 'files'; files: InputFile[] }
  | { type: 'progress'; bytes: number; total: number }
  /** A read's bytes so far: features of a layer can start late in a file. */
  | { type: 'read-progress'; id: number; bytes: number; total: number }
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
  | { type: 'xml'; id: number; row: number; xml: FeatureXml | null; error: string | null }
  | { type: 'error'; message: string };

// JSON from crates/xeibe-wasm/src/session.rs.

export interface InputFile {
  name: string;
  size: number;
  /** Why the file can't be read (a .zip that isn't one). */
  error: string | null;
  /** A zip archive's files; null for any other file. */
  members:
    | {
        name: string;
        size: number;
        compressed_size: number;
        /** Named like GML. */
        gml: boolean;
        /** Why xeibe can't read it. */
        unreadable: string | null;
      }[]
    | null;
}

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
  /** false: the sample stopped before the end of a file. */
  complete: boolean;
  /** The features scanned of each file; null: the whole files. */
  sample: number | null;
  skipped_sources: string[];
  extent: [number, number, number, number] | null;
  layers: {
    name: string;
    qname: string;
    feature_count: number;
    /** Counted by a read of the whole layer (the page sets it). */
    counted?: boolean;
    geometry_columns: string[];
    crs: CrsInfo[];
    extent: [number, number, number, number] | null;
  }[];
  axis_decisions: AxisDecision[];
}

/** Where a row is written, and its XML. */
export interface FeatureXml {
  row: number;
  source: string;
  /** Byte offsets in the source's decompressed UTF-8 text. */
  start: number;
  end: number;
  xml: string;
  /** Only the first bytes of a long feature. */
  truncated: boolean;
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
