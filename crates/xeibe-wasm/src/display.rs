//! The map's geometry: a batch's geometry column as flat buffers for
//! deck.gl's binary layers, in two coordinate spaces (`docs/web.md`, "Map").
//!
//! * **Source**: the coordinates as read, less an origin (the layer's first
//!   coordinate), so that they fit the GPU's 32-bit floats.
//! * **Map**: longitude/latitude, converted by `xeibe-reproject`.
//!
//! Curves are linearized and polygons triangulated here. Only x and y are
//! kept.

use std::sync::Arc;

use arrow_array::{Array, RecordBatch};
use arrow_schema::Field;
use geoarrow_array::GeoArrowArray;
use geoarrow_array::array::from_arrow_array;
use geoarrow_array::cast::to_wkb;
use xeibe_geom::Geometry;
use xeibe_geom::linearize::linearize;
use xeibe_geom::model::{LineString, Polygon};
use xeibe_geom::options::LinearizeOptions;
use xeibe_geom::wkb::read_wkb;
use xeibe_reproject::Transform;

/// One geometry, flattened for drawing: positions as `x y` pairs.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Point([f64; 2]),
    Line(Vec<f64>),
    /// Rings, the exterior first.
    Polygon(Vec<Vec<f64>>),
}

/// The parts of one batch's features, in source coordinates.
#[derive(Debug, Default)]
pub struct Parts {
    /// Per feature row of the batch: its parts.
    pub rows: Vec<Vec<Part>>,
    /// Features whose geometry couldn't be decoded, with the reason.
    pub errors: Vec<(usize, String)>,
}

impl Parts {
    /// The geometries of column `index` of `batch`.
    pub fn decode(batch: &RecordBatch, index: usize) -> Result<Parts, String> {
        let field = batch.schema_ref().field(index).clone();
        let wkb = column_wkb(batch.column(index).as_ref(), &field)?;
        let options = LinearizeOptions::default();
        let mut parts = Parts::default();
        for (row, value) in wkb.iter().enumerate() {
            let mut out = Vec::new();
            if let Some(bytes) = value {
                match read_wkb(bytes) {
                    Ok(geometry) => flatten(linearize(geometry, &options), &mut out),
                    Err(error) => parts.errors.push((row, error)),
                }
            }
            parts.rows.push(out);
        }
        Ok(parts)
    }

    /// `[min_x, min_y, max_x, max_y]` of every position, `None` if none.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let mut bounds = Bounds::default();
        for part in self.rows.iter().flatten() {
            part.each_position(|x, y| bounds.add(x, y));
        }
        bounds.get()
    }

    /// The first position, for the source space's origin.
    pub fn first_position(&self) -> Option<[f64; 2]> {
        let mut first = None;
        for part in self.rows.iter().flatten() {
            part.each_position(|x, y| {
                if first.is_none() && x.is_finite() && y.is_finite() {
                    first = Some([x, y]);
                }
            });
            if first.is_some() {
                break;
            }
        }
        first
    }
}

/// A geometry column as WKB values, whatever its GeoArrow type.
pub(crate) fn column_wkb(array: &dyn Array, field: &Field) -> Result<Vec<Option<Vec<u8>>>, String> {
    let geo: Arc<dyn GeoArrowArray> =
        from_arrow_array(array, field).map_err(|e| format!("{}: {e}", field.name()))?;
    let wkb = to_wkb::<i64>(geo.as_ref()).map_err(|e| format!("{}: {e}", field.name()))?;
    let inner = wkb.inner();
    Ok((0..inner.len())
        .map(|i| (!inner.is_null(i)).then(|| inner.value(i).to_vec()))
        .collect())
}

/// Flatten a linear geometry into parts. Empty parts are left out.
fn flatten(geometry: Geometry, out: &mut Vec<Part>) {
    let line = |line: &LineString| xy(&line.coords.values, line.coords.size());
    let polygon = |polygon: &Polygon| -> Vec<Vec<f64>> {
        polygon
            .rings()
            .map(line)
            .filter(|ring| !ring.is_empty())
            .collect()
    };
    match geometry {
        Geometry::Point(point) => {
            if let Some([x, y, ..]) = point.coord.as_deref()
                && x.is_finite()
                && y.is_finite()
            {
                out.push(Part::Point([*x, *y]));
            }
        }
        Geometry::MultiPoint(points) => {
            for point in points.0 {
                flatten(Geometry::Point(point), out);
            }
        }
        Geometry::LineString(l) => push_line(line(&l), out),
        Geometry::MultiLineString(lines) => lines.0.iter().for_each(|l| push_line(line(l), out)),
        Geometry::Polygon(p) => push_polygon(polygon(&p), out),
        Geometry::MultiPolygon(polygons) => {
            polygons
                .0
                .iter()
                .for_each(|p| push_polygon(polygon(p), out));
        }
        Geometry::GeometryCollection(members) => {
            members.0.into_iter().for_each(|g| flatten(g, out));
        }
        // `linearize` leaves no curves.
        _ => {}
    }
}

fn push_line(line: Vec<f64>, out: &mut Vec<Part>) {
    if line.len() >= 4 {
        out.push(Part::Line(line));
    }
}

fn push_polygon(rings: Vec<Vec<f64>>, out: &mut Vec<Part>) {
    if !rings.is_empty() {
        out.push(Part::Polygon(rings));
    }
}

/// The x and y of interleaved positions of `size` ordinates.
fn xy(values: &[f64], size: usize) -> Vec<f64> {
    values
        .chunks_exact(size)
        .flat_map(|position| [position[0], position[1]])
        .collect()
}

impl Part {
    fn each_position(&self, mut f: impl FnMut(f64, f64)) {
        let mut pairs = |values: &[f64]| {
            values.as_chunks::<2>().0.iter().for_each(|&[x, y]| f(x, y));
        };
        match self {
            Part::Point(p) => pairs(p),
            Part::Line(line) => pairs(line),
            Part::Polygon(rings) => rings.iter().for_each(|r| pairs(r)),
        }
    }

    /// The part with every position mapped by `f`; `None` if `f` fails on
    /// one of them.
    fn map(&self, f: &mut impl FnMut(f64, f64) -> Option<[f64; 2]>) -> Option<Part> {
        let mut all = |values: &[f64]| -> Option<Vec<f64>> {
            let mut out = Vec::with_capacity(values.len());
            for &[x, y] in values.as_chunks::<2>().0 {
                out.extend(f(x, y)?);
            }
            Some(out)
        };
        Some(match self {
            Part::Point(p) => {
                let v = all(p)?;
                Part::Point([v[0], v[1]])
            }
            Part::Line(line) => Part::Line(all(line)?),
            Part::Polygon(rings) => {
                Part::Polygon(rings.iter().map(|r| all(r)).collect::<Option<_>>()?)
            }
        })
    }
}

#[derive(Default)]
struct Bounds([f64; 4], bool);

impl Bounds {
    fn add(&mut self, x: f64, y: f64) {
        if !(x.is_finite() && y.is_finite()) {
            return;
        }
        let b = &mut self.0;
        if !self.1 {
            *b = [x, y, x, y];
            self.1 = true;
        } else {
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }

    fn get(&self) -> Option<[f64; 4]> {
        self.1.then_some(self.0)
    }
}

/// One coordinate space's buffers, in deck.gl's binary layout: positions as
/// `x y` pairs, Arrow-style start offsets (one more than the items, counting
/// positions), and the feature row (in the whole read) of each item.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Buffers {
    pub points: Vec<f64>,
    pub point_rows: Vec<u32>,
    pub lines: Vec<f64>,
    pub line_starts: Vec<u32>,
    pub line_rows: Vec<u32>,
    /// Every polygon's rings, one after the other.
    pub polygons: Vec<f64>,
    pub polygon_starts: Vec<u32>,
    pub polygon_rows: Vec<u32>,
    /// Triangles as position indices into `polygons`.
    pub triangles: Vec<u32>,
    /// The rings as paths over `polygons`, for outlines.
    pub ring_starts: Vec<u32>,
    pub ring_rows: Vec<u32>,
    /// `[min_x, min_y, max_x, max_y]` per feature of the batch; NaN when it
    /// has nothing to draw here.
    pub bounds: Vec<f64>,
}

impl Buffers {
    fn new() -> Self {
        Buffers {
            line_starts: vec![0],
            polygon_starts: vec![0],
            ring_starts: vec![0],
            ..Buffers::default()
        }
    }

    fn push(&mut self, row: u32, part: &Part, triangles: &[u32], bounds: &mut Bounds) {
        part.each_position(|x, y| bounds.add(x, y));
        match part {
            Part::Point(p) => {
                self.points.extend(p);
                self.point_rows.push(row);
            }
            Part::Line(line) => {
                self.lines.extend(line);
                self.line_starts.push(positions(&self.lines));
                self.line_rows.push(row);
            }
            Part::Polygon(rings) => {
                let base = positions(&self.polygons);
                for ring in rings {
                    self.polygons.extend(ring);
                    self.ring_starts.push(positions(&self.polygons));
                    self.ring_rows.push(row);
                }
                self.polygon_starts.push(positions(&self.polygons));
                self.polygon_rows.push(row);
                self.triangles.extend(triangles.iter().map(|i| base + i));
            }
        }
    }
}

fn positions(values: &[f64]) -> u32 {
    u32::try_from(values.len() / 2).expect("fewer than 2^32 positions per batch")
}

/// What one batch draws, in both spaces.
#[derive(Debug, Default)]
pub struct Drawing {
    pub source: Buffers,
    /// `None` when the layer has no map conversion.
    pub map: Option<Buffers>,
    /// Feature rows (in the whole read) left off the map: a coordinate
    /// failed to convert. The first error is kept.
    pub not_on_map: Vec<u32>,
    pub map_error: Option<String>,
}

/// Lay out a batch's parts. `first_row` is the batch's first row in the
/// whole read; `origin` is subtracted in the source space; `to_map`
/// converts source coordinates to longitude/latitude.
pub fn draw(
    parts: &Parts,
    first_row: u32,
    origin: [f64; 2],
    to_map: Option<&Transform>,
) -> Drawing {
    let mut drawing = Drawing {
        source: Buffers::new(),
        map: to_map.map(|_| Buffers::new()),
        ..Drawing::default()
    };
    let mut earcut = earcut::Earcut::new();
    let mut triangles = Vec::new();
    let mut shifted_by_origin = |x: f64, y: f64| Some([x - origin[0], y - origin[1]]);
    for (i, row_parts) in parts.rows.iter().enumerate() {
        let row = first_row + i as u32;
        let mut source_bounds = Bounds::default();
        // Map first: a feature is drawn there whole or not at all.
        let mut map_parts = Vec::new();
        if let Some(transform) = to_map {
            let mut error = None;
            let mut convert = |x: f64, y: f64| match transform.convert(x, y, None) {
                Ok((lon, lat, _)) => Some([lon, lat]),
                Err(e) => {
                    error.get_or_insert_with(|| e.to_string());
                    None
                }
            };
            match row_parts
                .iter()
                .map(|p| p.map(&mut convert))
                .collect::<Option<Vec<_>>>()
            {
                Some(parts) => map_parts = parts,
                None => {
                    drawing.not_on_map.push(row);
                    if drawing.map_error.is_none() {
                        drawing.map_error = error;
                    }
                }
            }
        }
        let mut map_bounds = Bounds::default();
        for (k, part) in row_parts.iter().enumerate() {
            let shifted = part
                .map(&mut shifted_by_origin)
                .expect("subtracting never fails");
            triangles.clear();
            if let Part::Polygon(rings) = &shifted {
                triangulate(&mut earcut, rings, &mut triangles);
            }
            drawing
                .source
                .push(row, &shifted, &triangles, &mut source_bounds);
            if let (Some(map), Some(part)) = (drawing.map.as_mut(), map_parts.get(k)) {
                map.push(row, part, &triangles, &mut map_bounds);
            }
        }
        let nan = [f64::NAN; 4];
        drawing
            .source
            .bounds
            .extend(source_bounds.get().unwrap_or(nan));
        if let Some(map) = drawing.map.as_mut() {
            map.bounds.extend(map_bounds.get().unwrap_or(nan));
        }
    }
    drawing
}

/// Triangles of one polygon, as indices of its positions (rings in order).
fn triangulate(earcut: &mut earcut::Earcut<f64>, rings: &[Vec<f64>], out: &mut Vec<u32>) {
    let mut holes = Vec::with_capacity(rings.len().saturating_sub(1));
    let mut count = 0u32;
    for (i, ring) in rings.iter().enumerate() {
        if i > 0 {
            holes.push(count);
        }
        count += (ring.len() / 2) as u32;
    }
    let positions = rings
        .iter()
        .flat_map(|ring| ring.as_chunks::<2>().0.iter().copied());
    earcut.earcut(positions, &holes, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, size: f64) -> Vec<f64> {
        vec![x, y, x + size, y, x + size, y + size, x, y + size, x, y]
    }

    #[test]
    fn polygons_get_triangles_and_outlines_in_both_spaces() {
        let parts = Parts {
            rows: vec![
                vec![Part::Polygon(vec![
                    square(10.0, 20.0, 4.0),
                    square(11.0, 21.0, 1.0),
                ])],
                vec![],
                vec![
                    Part::Point([12.0, 22.0]),
                    Part::Line(vec![10.0, 20.0, 14.0, 24.0]),
                ],
            ],
            errors: Vec::new(),
        };
        assert_eq!(parts.bounds(), Some([10.0, 20.0, 14.0, 24.0]));
        let drawing = draw(&parts, 100, [10.0, 20.0], None);
        let source = &drawing.source;
        assert_eq!(source.polygon_starts, [0, 10]);
        assert_eq!(source.ring_starts, [0, 5, 10]);
        assert_eq!(source.ring_rows, [100, 100]);
        assert_eq!(source.polygon_rows, [100]);
        // A square with a square hole: 8 triangles.
        assert_eq!(source.triangles.len(), 8 * 3);
        assert!(source.triangles.iter().all(|&i| i < 10));
        assert_eq!(source.points, [2.0, 2.0]);
        assert_eq!(source.point_rows, [102]);
        assert_eq!(source.line_starts, [0, 2]);
        assert_eq!(source.lines, [0.0, 0.0, 4.0, 4.0]);
        assert_eq!(&source.bounds[..4], &[0.0, 0.0, 4.0, 4.0]);
        assert!(source.bounds[4..8].iter().all(|v| v.is_nan()));
        assert!(drawing.map.is_none());
    }

    #[test]
    fn curves_in_a_wkb_column_are_drawn_linearized() {
        use arrow_array::BinaryArray;
        use geoarrow_schema::GeoArrowType;
        use geoarrow_schema::{Metadata, WkbType};
        use xeibe_geom::model::{CircularString, Coords};
        use xeibe_geom::wkb::{Endianness, write_wkb};

        let arc = Geometry::CircularString(CircularString {
            coords: Coords {
                dim: Some(xeibe_geom::Dim::Xy),
                values: vec![0.0, 0.0, 1.0, 1.0, 2.0, 0.0],
            },
            computed: Vec::new(),
        });
        let mut wkb = Vec::new();
        write_wkb(&arc, Endianness::Little, &mut wkb);
        let field =
            GeoArrowType::Wkb(WkbType::new(Arc::new(Metadata::default()))).to_field("geom", true);
        let array = BinaryArray::from(vec![Some(wkb.as_slice()), None]);
        let schema = Arc::new(arrow_schema::Schema::new(vec![field]));
        let batch = RecordBatch::try_new(schema, vec![Arc::new(array)]).unwrap();

        let parts = Parts::decode(&batch, 0).unwrap();
        assert_eq!(parts.rows.len(), 2);
        assert!(parts.rows[1].is_empty());
        let [Part::Line(line)] = parts.rows[0].as_slice() else {
            panic!("{:?}", parts.rows[0]);
        };
        // A half circle in steps of at most 4°, on the unit circle around (1, 0).
        assert!(line.len() / 2 > 45, "{} positions", line.len() / 2);
        for p in line.chunks(2) {
            assert!((((p[0] - 1.0).powi(2) + p[1].powi(2)).sqrt() - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn a_feature_that_fails_to_convert_is_left_off_the_map_only() {
        // ETRS89 / PL-1992 to WGS 84: a point far outside the projection's
        // domain fails, the other converts.
        let transform = Transform::new(
            &xeibe_reproject::Crs::from_epsg(2180).unwrap(),
            &xeibe_reproject::Crs::from_epsg(4326).unwrap(),
            xeibe_reproject::AxisOrder::EastFirst,
        )
        .unwrap();
        let parts = Parts {
            rows: vec![
                vec![Part::Point([636_640.467_013, 483_432.200_828])],
                vec![Part::Point([f64::MAX, 1e300])],
            ],
            errors: Vec::new(),
        };
        let drawing = draw(&parts, 0, [0.0, 0.0], Some(&transform));
        let map = drawing.map.unwrap();
        assert_eq!(map.point_rows, [0]);
        assert!((map.points[0] - 21.0).abs() < 1e-6 && (map.points[1] - 52.2).abs() < 1e-6);
        assert_eq!(drawing.not_on_map, [1]);
        assert!(drawing.map_error.is_some());
        assert_eq!(drawing.source.point_rows, [0, 1]);
    }
}
