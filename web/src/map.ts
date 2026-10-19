// The map pane (docs/web.md, "Map"): the same deck.gl layers in two views.
// - Map: Web Mercator over OpenStreetMap tiles, coordinates converted to
//   longitude/latitude by xeibe-reproject in the worker.
// - Source CRS: the coordinates as read, as plain x/y, no basemap.

import { COORDINATE_SYSTEM, Deck, OrthographicView } from '@deck.gl/core';
import type { CoordinateSystem, Layer, PickingInfo } from '@deck.gl/core';
import { PathLayer, ScatterplotLayer, SolidPolygonLayer } from '@deck.gl/layers';
import { MapboxOverlay } from '@deck.gl/mapbox';
import { Map as MapLibre, NavigationControl, setWorkerUrl, type StyleSpecification } from 'maplibre-gl';
import 'maplibre-gl/dist/maplibre-gl.css';
// MapLibre looks for its worker next to its own module, which a bundle
// doesn't keep: bundle the worker on its own and say where it is.
import maplibreWorker from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url';
import type { ReadInfo, Space } from './protocol';

setWorkerUrl(maplibreWorker);

export type View = 'map' | 'source';

interface Batch {
  firstRow: number;
  rows: number;
  map: Space | null;
  source: Space;
  /** A typical segment's length in each space, for showing vertices. */
  segment?: { map: number | null; source: number | null };
}

/** What a pickable layer draws, to say what is under the pointer. */
interface Picked {
  batch: Batch;
  kind: 'polygon' | 'line' | 'point' | 'polygon-vertex' | 'line-vertex' | 'selected-vertex';
}

/** A vertex of the selected feature: its index in the polygons' or the
 * lines' positions. */
interface SelectedVertex {
  part: 'polygon' | 'line';
  k: number;
}

type Rgba = [number, number, number, number];
const FILL: Rgba = [37, 99, 235, 60];
const OUTLINE: Rgba = [29, 78, 216, 230];
const LINE: Rgba = [29, 78, 216, 230];
const POINT: Rgba = [234, 88, 12, 230];
const SELECTED: Rgba = [250, 204, 21, 255];
const HOVER: Rgba = [250, 204, 21, 160];
const AREA: Rgba = [100, 116, 139, 200];
const VERTEX: Rgba = [255, 255, 255, 255];
const VERTEX_LINE: Rgba = [30, 41, 59, 255];

/** Pixels around the pointer that still hit a feature. */
const PICKING_RADIUS = 6;
/** Vertices show once a typical segment is this long on screen. */
const VERTEX_SPACING = 10;

// OpenStreetMap's own raster tiles, with the attribution their tile policy
// requires (https://operations.osmfoundation.org/policies/tiles/).
const OSM_STYLE: StyleSpecification = {
  version: 8,
  sources: {
    osm: {
      type: 'raster',
      tiles: ['https://tile.openstreetmap.org/{z}/{x}/{y}.png'],
      tileSize: 256,
      maxzoom: 19,
      attribution:
        'basemap © <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener">OpenStreetMap</a> contributors',
    },
  },
  layers: [{ id: 'osm', type: 'raster', source: 'osm' }],
};

const TOOLTIP_STYLE = {
  font: '12px/1.4 ui-monospace, Menlo, Consolas, monospace',
  background: 'rgba(15, 23, 42, 0.92)',
  color: 'white',
  padding: '6px 8px',
  borderRadius: '4px',
  whiteSpace: 'pre',
};

export class MapPane {
  private view: View = 'map';
  private readonly map: MapLibre;
  private readonly overlay: MapboxOverlay;
  private readonly deck: Deck<OrthographicView>;
  private batches: Batch[] = [];
  /** What each pickable layer draws. */
  private picked = new Map<string, Picked>();
  /** Binary data per layer, made once: the same object on each render
   * lets deck.gl keep its GPU buffers. */
  private dataCache = new Map<string, object>();
  private info: ReadInfo | null = null;
  private origin: [number, number] | null = null;
  private selected: number | null = null;
  /** The user moved a view: don't fit it to the data any more. */
  private moved = { map: false, source: false };
  /** Each view's zoom, for showing vertices. */
  private zoom = { map: 3, source: 0 };
  /** Which batches show their vertices, by view: rendered again when it
   * changes. */
  private vertexKey = '';

  constructor(
    mapContainer: HTMLElement,
    private readonly sourceContainer: HTMLDivElement,
    private readonly onSelect: (row: number) => void,
    private readonly onPointer: (text: string) => void,
  ) {
    this.map = new MapLibre({
      container: mapContainer,
      style: OSM_STYLE,
      center: [15, 50],
      zoom: 3,
      attributionControl: { compact: false },
    });
    this.map.addControl(new NavigationControl({ showCompass: false }), 'top-left');
    this.map.on('movestart', (event) => {
      if ((event as { originalEvent?: unknown }).originalEvent) this.moved.map = true;
    });
    this.map.on('zoom', () => this.zoomed('map', this.map.getZoom()));
    this.overlay = new MapboxOverlay({
      interleaved: false,
      layers: [],
      pickingRadius: PICKING_RADIUS,
      onClick: (info) => this.click(info),
      onHover: (info) => this.hover(info, 'map'),
      getTooltip: (info) => this.tooltip(info, 'map'),
    });
    this.map.addControl(this.overlay);

    this.deck = new Deck({
      parent: sourceContainer,
      views: new OrthographicView({ id: 'source', flipY: false }),
      controller: true,
      initialViewState: { target: [0, 0, 0], zoom: 0 },
      layers: [],
      pickingRadius: PICKING_RADIUS,
      onClick: (info) => this.click(info),
      onHover: (info) => this.hover(info, 'source'),
      getTooltip: (info) => this.tooltip(info, 'source'),
      onViewStateChange: ({ viewState }) => {
        this.zoomed('source', (viewState as { zoom: number }).zoom);
        return viewState;
      },
      onInteractionStateChange: (state) => {
        if (state.isDragging || state.isZooming || state.isPanning) this.moved.source = true;
      },
      getCursor: ({ isHovering, isDragging }) => (isDragging ? 'grabbing' : isHovering ? 'pointer' : 'grab'),
    });
    this.setView('map');
  }

  /** A new read: forget the last one's features. */
  reset(info: ReadInfo) {
    this.info = info;
    this.batches = [];
    this.picked.clear();
    this.dataCache.clear();
    this.origin = null;
    this.selected = null;
    this.moved = { map: false, source: false };
    // Without a conversion there is only the source view.
    this.setView(info.map.target ? this.view : 'source');
    this.render();
  }

  /** The map view is possible: the CRS converts to longitude/latitude. */
  hasMap(): boolean {
    return Boolean(this.info?.map.target);
  }

  addBatch(batch: Batch, origin: [number, number] | null) {
    this.origin ??= origin;
    batch.segment = {
      map: batch.map ? typicalSegment(batch.map) : null,
      source: typicalSegment(batch.source),
    };
    this.batches.push(batch);
    this.render();
    if (this.batches.length === 1) this.fit();
  }

  /** Fit the views to the data, unless the user has moved them. */
  fit() {
    const map = this.bounds('map');
    if (map && !this.moved.map) {
      this.map.fitBounds(
        [
          [map[0], map[1]],
          [map[2], map[3]],
        ],
        { padding: 40, maxZoom: 18, duration: 0 },
      );
    }
    const source = this.bounds('source');
    if (source && !this.moved.source && this.view === 'source') this.fitSource(source);
  }

  setView(view: View) {
    if (view === 'map' && !this.hasMap() && this.info) view = 'source';
    this.view = view;
    this.sourceContainer.style.display = view === 'source' ? 'block' : 'none';
    if (view === 'map') this.map.resize();
    this.render();
    // A hidden view has no size to fit: fit it once it shows.
    const source = view === 'source' && !this.moved.source ? this.bounds('source') : null;
    if (source) this.fitSource(source);
  }

  currentView(): View {
    return this.view;
  }

  resize() {
    this.map.resize();
  }

  /** Highlight a feature row; `zoom` moves the view to it. */
  select(row: number | null, zoom: boolean) {
    this.selected = row;
    this.render();
    if (row === null || !zoom) return;
    const batch = this.batchOf(row);
    if (!batch) return;
    const i = (row - batch.firstRow) * 4;
    const space = this.view === 'map' ? batch.map : batch.source;
    if (!space) return;
    const b = Array.from(space.bounds.subarray(i, i + 4));
    if (b.some(Number.isNaN)) return;
    if (this.view === 'map') {
      this.moved.map = true;
      this.map.fitBounds(
        [
          [b[0], b[1]],
          [b[2], b[3]],
        ],
        { padding: 80, maxZoom: 18, duration: 400 },
      );
    } else {
      this.moved.source = true;
      this.fitSource(b as [number, number, number, number]);
    }
  }

  private batchOf(row: number): Batch | undefined {
    return this.batches.find((b) => row >= b.firstRow && row < b.firstRow + b.rows);
  }

  private bounds(view: View): [number, number, number, number] | null {
    let out: [number, number, number, number] | null = null;
    for (const batch of this.batches) {
      const space = view === 'map' ? batch.map : batch.source;
      if (!space) continue;
      const b = space.bounds;
      for (let i = 0; i < b.length; i += 4) {
        if (Number.isNaN(b[i])) continue;
        if (!out) out = [b[i], b[i + 1], b[i + 2], b[i + 3]];
        else {
          out[0] = Math.min(out[0], b[i]);
          out[1] = Math.min(out[1], b[i + 1]);
          out[2] = Math.max(out[2], b[i + 2]);
          out[3] = Math.max(out[3], b[i + 3]);
        }
      }
    }
    return out;
  }

  private fitSource([x0, y0, x1, y1]: [number, number, number, number]) {
    const { clientWidth: w, clientHeight: h } = this.sourceContainer;
    const width = Math.max(x1 - x0, 1e-9);
    const height = Math.max(y1 - y0, 1e-9);
    const scale = Math.min((w || 400) / width, (h || 400) / height) * 0.85;
    const zoom = Math.min(Math.log2(scale), 24);
    this.deck.setProps({
      initialViewState: { target: [(x0 + x1) / 2, (y0 + y1) / 2, 0], zoom, transitionDuration: 0 },
    });
    this.zoomed('source', zoom);
  }

  /** Screen pixels per unit of a view's coordinates. */
  private scale(view: View): number {
    // MapLibre's world is 512 pixels wide at zoom 0.
    return view === 'map' ? (512 * 2 ** this.zoom.map) / 360 : 2 ** this.zoom.source;
  }

  private showsVertices(batch: Batch, view: View): boolean {
    const segment = batch.segment?.[view];
    return segment != null && segment * this.scale(view) >= VERTEX_SPACING;
  }

  /** A view zoomed: render again if that shows or hides vertices. */
  private zoomed(view: View, zoom: number) {
    this.zoom[view] = zoom;
    if (view !== this.view) return;
    const key = view + this.batches.map((b) => (this.showsVertices(b, view) ? 1 : 0)).join('');
    if (key !== this.vertexKey) this.render();
  }

  private click(info: PickingInfo) {
    const row = this.rowOf(info);
    if (row !== null) this.onSelect(row);
  }

  private hover(info: PickingInfo, view: View) {
    // Over the map, deck.gl's canvas lets the pointer through to MapLibre's,
    // which shows the cursor.
    if (view === 'map') this.map.getCanvas().style.cursor = info.picked ? 'pointer' : '';
    const c = info.coordinate;
    if (!c) return this.onPointer('');
    if (view === 'map') {
      this.onPointer(`lon ${c[0].toFixed(6)}, lat ${c[1].toFixed(6)}`);
    } else {
      const [ox, oy] = this.origin ?? [0, 0];
      this.onPointer(`x ${(c[0] + ox).toFixed(3)}, y ${(c[1] + oy).toFixed(3)}`);
    }
  }

  /** The feature row under the pointer. */
  private rowOf(info: PickingInfo): number | null {
    const picked = info.layer ? this.picked.get(info.layer.id) : undefined;
    if (!picked || info.index < 0) return null;
    const s = this.view === 'map' ? picked.batch.map : picked.batch.source;
    if (!s) return null;
    switch (picked.kind) {
      case 'polygon':
        return s.polygonRows[info.index];
      case 'line':
        return s.lineRows[info.index];
      case 'point':
        return s.pointRows[info.index];
      case 'polygon-vertex':
        return s.ringRows[containing(s.ringStarts, info.index)];
      case 'line-vertex':
        return s.lineRows[containing(s.lineStarts, info.index)];
      case 'selected-vertex':
        return this.selected;
    }
  }

  /** A vertex or a point under the pointer: its coordinates, and where it
   * is in its feature. */
  private tooltip(info: PickingInfo, view: View) {
    const picked = info.layer ? this.picked.get(info.layer.id) : undefined;
    if (!picked || info.index < 0) return null;
    const { batch } = picked;
    const s = view === 'map' ? batch.map : batch.source;
    if (!s) return null;
    let part: 'polygon' | 'line' | 'point';
    let k: number;
    switch (picked.kind) {
      case 'polygon-vertex':
        [part, k] = ['polygon', info.index];
        break;
      case 'line-vertex':
        [part, k] = ['line', info.index];
        break;
      case 'point':
        [part, k] = ['point', info.index];
        break;
      case 'selected-vertex': {
        const vertex = info.object as SelectedVertex;
        [part, k] = [vertex.part, vertex.k];
        break;
      }
      default:
        return null;
    }
    const lines = [...this.coordinates(batch, view, part, k), ...where(s, part, k)];
    return { text: lines.join('\n'), style: TOOLTIP_STYLE };
  }

  /** A position's coordinates as read and, on the map, as shown. */
  private coordinates(batch: Batch, view: View, part: 'polygon' | 'line' | 'point', k: number): string[] {
    const positions = (s: Space) => (part === 'polygon' ? s.polygons : part === 'line' ? s.lines : s.points);
    const out: string[] = [];
    if (view === 'map' && batch.map) {
      const p = positions(batch.map);
      out.push(`lon ${p[2 * k].toFixed(8)}`, `lat ${p[2 * k + 1].toFixed(8)}`);
    }
    // The source space holds the same positions unless some features
    // couldn't be put on the map.
    const source = positions(batch.source);
    if (view === 'source' || (batch.map && positions(batch.map).length === source.length)) {
      const [ox, oy] = this.origin ?? [0, 0];
      out.push(`x ${coordinate(source[2 * k] + ox)}`, `y ${coordinate(source[2 * k + 1] + oy)}`);
    }
    return out;
  }

  private render() {
    const view = this.view;
    const layers: Layer[] = [];
    const coordinateSystem: CoordinateSystem =
      view === 'map' ? COORDINATE_SYSTEM.LNGLAT : COORDINATE_SYSTEM.CARTESIAN;
    const area = this.areaPath(view);
    if (area) {
      layers.push(
        new PathLayer({
          id: `${view}-area`,
          data: [{ path: area }],
          getPath: (d: { path: [number, number][] }) => d.path,
          getColor: AREA,
          getWidth: 1.5,
          widthUnits: 'pixels',
          coordinateSystem,
        }),
      );
    }
    this.batches.forEach((batch, index) => {
      const space = view === 'map' ? batch.map : batch.source;
      if (space) {
        const vertices = this.showsVertices(batch, view);
        layers.push(...this.batchLayers(`${view}-${index}`, batch, space, coordinateSystem, vertices));
      }
    });
    this.vertexKey = view + this.batches.map((b) => (this.showsVertices(b, view) ? 1 : 0)).join('');
    layers.push(...this.selectedLayers(view, coordinateSystem));
    if (view === 'map') {
      this.overlay.setProps({ layers });
      this.deck.setProps({ layers: [] });
    } else {
      this.overlay.setProps({ layers: [] });
      this.deck.setProps({ layers });
    }
  }

  private areaPath(view: View): [number, number][] | null {
    const area = this.info?.map.area_of_use;
    if (!area) return null;
    if (view === 'map') return area.map;
    const [ox, oy] = this.origin ?? [0, 0];
    return area.source.map(([x, y]): [number, number] => [x - ox, y - oy]);
  }

  /** `make()` once per id, then the same object. */
  private data(id: string, make: () => object): never {
    let data = this.dataCache.get(id);
    if (!data) {
      data = make();
      this.dataCache.set(id, data);
    }
    return data as never;
  }

  private batchLayers(
    key: string,
    batch: Batch,
    s: Space,
    coordinateSystem: CoordinateSystem,
    vertices: boolean,
  ): Layer[] {
    const layers: Layer[] = [];
    const common = {
      coordinateSystem,
      pickable: true,
      autoHighlight: true,
      highlightColor: HOVER,
    };
    const vertexLayer = (id: string, positions: Float64Array, kind: Picked['kind']) => {
      this.picked.set(id, { batch, kind });
      return new ScatterplotLayer({
        ...common,
        id,
        data: this.data(id, () => ({
          length: positions.length / 2,
          attributes: { getPosition: { value: positions, size: 2 } },
        })),
        getFillColor: VERTEX,
        getRadius: 2.5,
        radiusUnits: 'pixels',
        stroked: true,
        getLineColor: VERTEX_LINE,
        lineWidthUnits: 'pixels',
        getLineWidth: 1,
      });
    };
    if (s.polygonRows.length > 0) {
      const id = `${key}-fill`;
      this.picked.set(id, { batch, kind: 'polygon' });
      layers.push(
        new SolidPolygonLayer({
          ...common,
          id,
          data: this.data(id, () => ({
            length: s.polygonRows.length,
            startIndices: s.polygonStarts,
            attributes: {
              getPolygon: { value: s.polygons, size: 2 },
              indices: s.triangles,
            },
          })),
          _normalize: false,
          getFillColor: FILL,
        }),
      );
      layers.push(
        new PathLayer({
          id: `${key}-rings`,
          coordinateSystem,
          data: this.data(`${key}-rings`, () => ({
            length: s.ringRows.length,
            startIndices: s.ringStarts,
            attributes: { getPath: { value: s.polygons, size: 2 } },
          })),
          _pathType: 'open',
          getColor: OUTLINE,
          getWidth: 1,
          widthUnits: 'pixels',
        }),
      );
      if (vertices) layers.push(vertexLayer(`${key}-polygon-vertices`, s.polygons, 'polygon-vertex'));
    }
    if (s.lineRows.length > 0) {
      const id = `${key}-lines`;
      this.picked.set(id, { batch, kind: 'line' });
      layers.push(
        new PathLayer({
          ...common,
          id,
          data: this.data(id, () => ({
            length: s.lineRows.length,
            startIndices: s.lineStarts,
            attributes: { getPath: { value: s.lines, size: 2 } },
          })),
          _pathType: 'open',
          getColor: LINE,
          getWidth: 2,
          widthUnits: 'pixels',
        }),
      );
      if (vertices) layers.push(vertexLayer(`${key}-line-vertices`, s.lines, 'line-vertex'));
    }
    if (s.pointRows.length > 0) {
      const id = `${key}-points`;
      this.picked.set(id, { batch, kind: 'point' });
      layers.push(
        new ScatterplotLayer({
          ...common,
          id,
          data: this.data(id, () => ({
            length: s.pointRows.length,
            attributes: { getPosition: { value: s.points, size: 2 } },
          })),
          getFillColor: POINT,
          getRadius: 3,
          radiusUnits: 'pixels',
          stroked: true,
          getLineColor: [255, 255, 255, 200] as Rgba,
          lineWidthUnits: 'pixels',
          getLineWidth: 0.5,
        }),
      );
    }
    return layers;
  }

  /** The selected feature drawn over the rest: its rings, lines and points,
   * and its vertices. */
  private selectedLayers(view: View, coordinateSystem: CoordinateSystem): Layer[] {
    const row = this.selected;
    if (row === null) return [];
    const batch = this.batchOf(row);
    const s = batch && (view === 'map' ? batch.map : batch.source);
    if (!batch || !s) return [];
    const paths: [number, number][][] = [];
    const vertices: SelectedVertex[] = [];
    const slices = (part: 'polygon' | 'line', positions: Float64Array, starts: Uint32Array, rows: Uint32Array) => {
      rows.forEach((r, i) => {
        if (r !== row) return;
        const path: [number, number][] = [];
        for (let k = starts[i]; k < starts[i + 1]; k++) {
          path.push([positions[2 * k], positions[2 * k + 1]]);
          vertices.push({ part, k });
        }
        paths.push(path);
      });
    };
    slices('polygon', s.polygons, s.ringStarts, s.ringRows);
    slices('line', s.lines, s.lineStarts, s.lineRows);
    const points: [number, number][] = [];
    s.pointRows.forEach((r, i) => {
      if (r === row) points.push([s.points[2 * i], s.points[2 * i + 1]]);
    });
    const id = `${view}-selected-vertices`;
    this.picked.set(id, { batch, kind: 'selected-vertex' });
    const position = (v: SelectedVertex): [number, number] => {
      const p = v.part === 'polygon' ? s.polygons : s.lines;
      return [p[2 * v.k], p[2 * v.k + 1]];
    };
    return [
      new PathLayer({
        id: `${view}-selected-paths`,
        coordinateSystem,
        data: paths,
        getPath: (d: [number, number][]) => d,
        getColor: SELECTED,
        getWidth: 4,
        widthUnits: 'pixels',
      }),
      new ScatterplotLayer({
        id,
        coordinateSystem,
        data: vertices,
        getPosition: position,
        pickable: true,
        autoHighlight: true,
        highlightColor: [234, 88, 12, 255],
        getFillColor: VERTEX,
        getRadius: 3.5,
        radiusUnits: 'pixels',
        stroked: true,
        getLineColor: [0, 0, 0, 255] as Rgba,
        lineWidthUnits: 'pixels',
        getLineWidth: 1.5,
      }),
      new ScatterplotLayer({
        id: `${view}-selected-points`,
        coordinateSystem,
        data: points,
        getPosition: (d: [number, number]) => d,
        getFillColor: SELECTED,
        getRadius: 7,
        radiusUnits: 'pixels',
        stroked: true,
        getLineColor: [0, 0, 0, 255] as Rgba,
        lineWidthUnits: 'pixels',
        getLineWidth: 1,
      }),
    ];
  }
}

/** The item of `starts` (Arrow-style offsets) that holds position `k`. */
function containing(starts: Uint32Array, k: number): number {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= k) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** The first item of a run of items of the same feature row. */
function firstOfRow(rows: Uint32Array, i: number): number {
  let j = i;
  while (j > 0 && rows[j - 1] === rows[i]) j--;
  return j;
}

/** Where position `k` is in its feature, as lines of text. */
function where(s: Space, part: 'polygon' | 'line' | 'point', k: number): string[] {
  if (part === 'point') {
    const first = firstOfRow(s.pointRows, k);
    const count = countRow(s.pointRows, k);
    return [`row ${s.pointRows[k] + 1}${count > 1 ? ` · point ${k - first + 1} of ${count}` : ''}`];
  }
  if (part === 'line') {
    const line = containing(s.lineStarts, k);
    const count = countRow(s.lineRows, line);
    const vertex = k - s.lineStarts[line];
    const length = s.lineStarts[line + 1] - s.lineStarts[line];
    return [
      `row ${s.lineRows[line] + 1}${count > 1 ? ` · line ${line - firstOfRow(s.lineRows, line) + 1} of ${count}` : ''}`,
      `vertex ${vertex + 1} of ${length}`,
    ];
  }
  const ring = containing(s.ringStarts, k);
  const polygon = containing(s.polygonStarts, k);
  const firstRing = containing(s.ringStarts, s.polygonStarts[polygon]);
  const inPolygon = ring - firstRing;
  const polygons = countRow(s.polygonRows, polygon);
  const first = s.ringStarts[ring];
  const vertex = k - first;
  // A ring's last position repeats its first: count the vertices without it.
  const length = s.ringStarts[ring + 1] - first;
  const last = first + length - 1;
  const closed =
    length > 1 && s.polygons[2 * first] === s.polygons[2 * last] && s.polygons[2 * first + 1] === s.polygons[2 * last + 1];
  const vertices = closed ? length - 1 : length;
  const which = closed && k === last ? `vertex 1 of ${vertices} (closing the ring)` : `vertex ${vertex + 1} of ${vertices}`;
  return [
    `row ${s.polygonRows[polygon] + 1}${polygons > 1 ? ` · polygon ${polygon - firstOfRow(s.polygonRows, polygon) + 1} of ${polygons}` : ''}`,
    `${inPolygon === 0 ? 'exterior ring' : `interior ring ${inPolygon}`} · ${which}`,
  ];
}

/** Items of the same feature row as item `i`. */
function countRow(rows: Uint32Array, i: number): number {
  const first = firstOfRow(rows, i);
  let end = i + 1;
  while (end < rows.length && rows[end] === rows[i]) end++;
  return end - first;
}

/** A coordinate as read: the origin added back can be off in the last
 * bit, so 15 significant digits, which a coordinate written with no more
 * survives. */
function coordinate(value: number): string {
  return String(Number(value.toPrecision(15)));
}

/** The median length of a sample of a space's segments; `null` without
 * any. */
function typicalSegment(s: Space): number | null {
  const lengths: number[] = [];
  const sample = (positions: Float64Array, starts: Uint32Array) => {
    const items = starts.length - 1;
    const step = Math.max(1, Math.floor(items / 500));
    for (let i = 0; i < items; i += step) {
      const k = starts[i];
      if (starts[i + 1] - k < 2) continue;
      const dx = positions[2 * k + 2] - positions[2 * k];
      const dy = positions[2 * k + 3] - positions[2 * k + 1];
      const length = Math.hypot(dx, dy);
      if (length > 0) lengths.push(length);
    }
  };
  sample(s.polygons, s.ringStarts);
  sample(s.lines, s.lineStarts);
  if (lengths.length === 0) return null;
  lengths.sort((a, b) => a - b);
  return lengths[lengths.length >> 1];
}
