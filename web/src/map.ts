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
}

type Rgba = [number, number, number, number];
const FILL: Rgba = [37, 99, 235, 60];
const OUTLINE: Rgba = [29, 78, 216, 230];
const LINE: Rgba = [29, 78, 216, 230];
const POINT: Rgba = [234, 88, 12, 230];
const SELECTED: Rgba = [250, 204, 21, 255];
const AREA: Rgba = [100, 116, 139, 200];

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

export class MapPane {
  private view: View = 'map';
  private readonly map: MapLibre;
  private readonly overlay: MapboxOverlay;
  private readonly deck: Deck<OrthographicView>;
  private batches: Batch[] = [];
  /** Feature rows of each layer's items, for picking. */
  private rowsByLayer = new Map<string, Uint32Array>();
  /** Binary data per layer, made once: the same object on each render
   * lets deck.gl keep its GPU buffers. */
  private dataCache = new Map<string, object>();
  private info: ReadInfo | null = null;
  private origin: [number, number] | null = null;
  private selected: number | null = null;
  /** The user moved a view: don't fit it to the data any more. */
  private moved = { map: false, source: false };

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
    this.overlay = new MapboxOverlay({
      interleaved: false,
      layers: [],
      onClick: (info) => this.click(info),
      onHover: (info) => this.hover(info, 'map'),
    });
    this.map.addControl(this.overlay);

    this.deck = new Deck({
      parent: sourceContainer,
      views: new OrthographicView({ id: 'source', flipY: false }),
      controller: true,
      initialViewState: { target: [0, 0, 0], zoom: 0 },
      layers: [],
      onClick: (info) => this.click(info),
      onHover: (info) => this.hover(info, 'source'),
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
    this.rowsByLayer.clear();
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
    const zoom = Math.min(Math.log2(scale), 12);
    this.deck.setProps({
      initialViewState: { target: [(x0 + x1) / 2, (y0 + y1) / 2, 0], zoom, transitionDuration: 0 },
    });
  }

  private click(info: PickingInfo) {
    const rows = info.layer ? this.rowsByLayer.get(info.layer.id) : undefined;
    if (rows && info.index >= 0) this.onSelect(rows[info.index]);
  }

  private hover(info: PickingInfo, view: View) {
    const c = info.coordinate;
    if (!c) return this.onPointer('');
    if (view === 'map') {
      this.onPointer(`lon ${c[0].toFixed(6)}, lat ${c[1].toFixed(6)}`);
    } else {
      const [ox, oy] = this.origin ?? [0, 0];
      this.onPointer(`x ${(c[0] + ox).toFixed(3)}, y ${(c[1] + oy).toFixed(3)}`);
    }
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
      if (space) layers.push(...this.batchLayers(`${view}-${index}`, space, coordinateSystem));
    });
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

  private batchLayers(key: string, s: Space, coordinateSystem: CoordinateSystem): Layer[] {
    const layers: Layer[] = [];
    const common = {
      coordinateSystem,
      pickable: true,
      autoHighlight: true,
      highlightColor: [250, 204, 21, 160] as Rgba,
    };
    if (s.polygonRows.length > 0) {
      const id = `${key}-fill`;
      this.rowsByLayer.set(id, s.polygonRows);
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
    }
    if (s.lineRows.length > 0) {
      const id = `${key}-lines`;
      this.rowsByLayer.set(id, s.lineRows);
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
    }
    if (s.pointRows.length > 0) {
      const id = `${key}-points`;
      this.rowsByLayer.set(id, s.pointRows);
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

  /** The selected feature drawn over the rest: its rings, lines and points. */
  private selectedLayers(view: View, coordinateSystem: CoordinateSystem): Layer[] {
    const row = this.selected;
    if (row === null) return [];
    const batch = this.batchOf(row);
    const s = batch && (view === 'map' ? batch.map : batch.source);
    if (!s) return [];
    const paths: [number, number][][] = [];
    const slices = (positions: Float64Array, starts: Uint32Array, rows: Uint32Array) => {
      rows.forEach((r, i) => {
        if (r !== row) return;
        const path: [number, number][] = [];
        for (let k = starts[i]; k < starts[i + 1]; k++) path.push([positions[2 * k], positions[2 * k + 1]]);
        paths.push(path);
      });
    };
    slices(s.polygons, s.ringStarts, s.ringRows);
    slices(s.lines, s.lineStarts, s.lineRows);
    const points: [number, number][] = [];
    s.pointRows.forEach((r, i) => {
      if (r === row) points.push([s.points[2 * i], s.points[2 * i + 1]]);
    });
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
