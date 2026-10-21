//! Reprojection in reads (`docs/geometry.md`, "Reprojection"): with
//! `target_crs`, each geometry converts from its own CRS right after it is
//! parsed, once its axis order is decided and its arcs are linearized. One
//! [`Transform`] per srsName, chosen as `xeibe-reproject` chooses
//! (`docs/reprojection.md`).
//!
//! Grids are loaded when an srsName's transform is set up, for the source
//! CRS's area of use: before the read for the srsNames the read's sample
//! saw, on first use for the others. A grid that can't be loaded fails the
//! read; a geometry that can't be converted is a feature error.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use xeibe_geom::model::{Coords, Envelope, Point};
use xeibe_geom::{CrsRef, Geometry, GeometryOptions, SrsName};
use xeibe_reproject::{AxisOrder, Crs, DEFAULT_DENSIFY, GridRequest, Grids, Transform};

use crate::geometry_column::visit_coords;

type Fetch = dyn Fn(&GridRequest) -> Result<Vec<u8>, String> + Send + Sync;

/// Fetches grid data for reprojection: the bytes of one [`GridRequest`]
/// (`docs/reprojection.md`, decision 6). The read itself does no network
/// I/O; the CLI and the Python module fetch from `cdn.proj.org`.
#[derive(Clone)]
pub struct GridLoader(Arc<Fetch>);

impl GridLoader {
    pub fn new(
        fetch: impl Fn(&GridRequest) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    ) -> Self {
        GridLoader(Arc::new(fetch))
    }

    /// From a directory that holds PROJ-data's files: a PROJ-data release,
    /// `projsync`'s download, or files fetched from `cdn.proj.org`.
    pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        GridLoader::new(move |request: &GridRequest| {
            let path = dir.join(&request.file);
            let error = |e: std::io::Error| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    format!("{} not found (it is {})", path.display(), request.url())
                }
                _ => format!("{}: {e}", path.display()),
            };
            let mut file = std::fs::File::open(&path).map_err(error)?;
            file.seek(SeekFrom::Start(request.offset)).map_err(error)?;
            let mut data = Vec::new();
            file.take(request.length.unwrap_or(u64::MAX))
                .read_to_end(&mut data)
                .map_err(error)?;
            Ok(data)
        })
    }

    pub fn fetch(&self, request: &GridRequest) -> Result<Vec<u8>, String> {
        (self.0)(request)
    }
}

impl std::fmt::Debug for GridLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GridLoader")
    }
}

/// How the geometries of one srsName convert, as the read's report lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Reprojection {
    /// The srsName as written (or `crs_override`); `None` for geometries
    /// without one.
    pub srs_name: Option<String>,
    pub target: String,
    /// EPSG's names of the operations the transform chooses among, per
    /// coordinate (one for a conversion on one datum).
    pub operations: Vec<String>,
    /// PROJ-data's names of the grids those operations read.
    pub grids: Vec<String>,
    /// Why these geometries can't be converted.
    pub error: Option<String>,
}

#[derive(Clone)]
enum Setup {
    Ready(Arc<Transform>),
    Failed(String),
}

/// The read's transforms to `target_crs`, one per srsName, shared by the
/// workers.
pub(crate) struct Conversions {
    target_srs: String,
    target: Crs,
    grids: Grids,
    loader: Option<GridLoader>,
    crs_override: Option<String>,
    setups: RwLock<HashMap<Option<String>, Setup>>,
}

impl Conversions {
    /// `None` without a target CRS.
    pub(crate) fn new(
        options: &GeometryOptions,
        loader: Option<GridLoader>,
    ) -> crate::Result<Option<Self>> {
        let Some(target_srs) = &options.target_crs else {
            return Ok(None);
        };
        let target = crs(target_srs).map_err(|message| crate::Error::Option {
            key: "target_crs".into(),
            message: format!("target CRS {target_srs:?}: {message}"),
        })?;
        Ok(Some(Conversions {
            target_srs: target_srs.clone(),
            target,
            grids: if options.no_grids {
                Grids::None
            } else {
                Grids::ProjData
            },
            loader,
            crs_override: options.crs_override.clone(),
            setups: RwLock::default(),
        }))
    }

    /// Set up the transform of geometries in `srs_name` now, loading its
    /// grids: an error is the read's. One that can't be set up is left to
    /// its geometries, as feature errors.
    pub(crate) fn prepare(&self, srs_name: Option<&str>) -> crate::Result<()> {
        self.setup(srs_name).map(|_| ())
    }

    /// Convert `geometry`, in `srs_name`, in place. The outer error is the
    /// read's (a grid that can't be loaded), the inner one the geometry's.
    pub(crate) fn convert(
        &self,
        geometry: &mut Geometry,
        srs_name: Option<&str>,
    ) -> crate::Result<Result<(), String>> {
        let transform = match self.setup(srs_name)? {
            Setup::Ready(transform) => transform,
            Setup::Failed(message) => return Ok(Err(message)),
        };
        if !geometry.is_simple() {
            return Ok(Err(format!(
                "a {:?} with arcs can't be converted to {} exactly: linearize it first \
                 (--linearize)",
                geometry.kind(),
                self.target_srs
            )));
        }
        // The first failure; the rest of the geometry is left as it is.
        let failed = std::cell::RefCell::new(None);
        let record = |result: Result<(), String>| {
            if let Err(reason) = result {
                failed.borrow_mut().get_or_insert(reason);
            }
        };
        visit_coords(
            geometry,
            &mut |coords: &mut Coords| {
                if failed.borrow().is_none() {
                    record(convert_coords(&transform, coords));
                }
            },
            &mut |point: &mut Point| {
                if failed.borrow().is_none()
                    && let Some(coord) = point.coord.as_mut()
                {
                    record(convert_position(&transform, coord));
                }
            },
        );
        Ok(match failed.into_inner() {
            Some(reason) => Err(self.failure(&transform, &reason)),
            None => Ok(()),
        })
    }

    /// Convert a box (a `boundedBy` column) by its densified edges, as
    /// [`Transform::convert_bounds`] does. A third ordinate is kept as it is.
    pub(crate) fn convert_envelope(
        &self,
        envelope: &mut Envelope,
    ) -> crate::Result<Result<(), String>> {
        if envelope.lower.len() < 2 || envelope.upper.len() < 2 {
            return Ok(Ok(()));
        }
        let transform = match self.setup(envelope.srs_name.as_deref())? {
            Setup::Ready(transform) => transform,
            Setup::Failed(message) => return Ok(Err(message)),
        };
        let bounds = [
            envelope.lower[0],
            envelope.lower[1],
            envelope.upper[0],
            envelope.upper[1],
        ];
        Ok(match transform.convert_bounds(bounds, DEFAULT_DENSIFY) {
            Ok([min_x, min_y, max_x, max_y]) => {
                envelope.lower[..2].copy_from_slice(&[min_x, min_y]);
                envelope.upper[..2].copy_from_slice(&[max_x, max_y]);
                envelope.srs_name = Some(self.target_srs.clone());
                Ok(())
            }
            Err(error) => Err(self.failure(&transform, &error.to_string())),
        })
    }

    /// One entry per srsName the read set up.
    pub(crate) fn report(&self) -> Vec<Reprojection> {
        let setups = self.setups.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<Reprojection> = setups
            .iter()
            .map(|(srs_name, setup)| {
                let (operations, grids, error) = match setup {
                    Setup::Ready(t) => (
                        t.operations().into_iter().map(str::to_string).collect(),
                        t.grids().into_iter().map(str::to_string).collect(),
                        None,
                    ),
                    Setup::Failed(message) => (Vec::new(), Vec::new(), Some(message.clone())),
                };
                Reprojection {
                    srs_name: srs_name.clone(),
                    target: self.target_srs.clone(),
                    operations,
                    grids,
                    error,
                }
            })
            .collect();
        out.sort_by(|a, b| a.srs_name.cmp(&b.srs_name));
        out
    }

    fn failure(&self, transform: &Transform, reason: &str) -> String {
        format!(
            "can't convert from {} to {}: {reason}",
            transform.source().name(),
            self.target_srs
        )
    }

    /// The transform for geometries in `srs_name`, set up on first use.
    fn setup(&self, srs_name: Option<&str>) -> crate::Result<Setup> {
        let srs_name = self.crs_override.as_deref().or(srs_name).map(str::trim);
        let key = srs_name.map(str::to_string);
        if let Some(setup) = self
            .setups
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(setup.clone());
        }
        // Held while the grids load, so that each is fetched once.
        let mut setups = self.setups.write().unwrap_or_else(|e| e.into_inner());
        if let Some(setup) = setups.get(&key) {
            return Ok(setup.clone());
        }
        let setup = match srs_name {
            None => Setup::Failed(format!(
                "a geometry without srsName can't be converted to {}: name its CRS with \
                 crs_override (--crs)",
                self.target_srs
            )),
            Some(srs) => match crs(srs) {
                Err(message) => Setup::Failed(format!(
                    "srsName {srs:?} can't be converted to {}: {message}",
                    self.target_srs
                )),
                Ok(source) => {
                    match Transform::with_grids(
                        &source,
                        &self.target,
                        AxisOrder::EastFirst,
                        self.grids,
                    ) {
                        Err(error) => Setup::Failed(format!("srsName {srs:?}: {error}")),
                        Ok(transform) => {
                            self.load_grids(&transform, srs)?;
                            Setup::Ready(Arc::new(transform))
                        }
                    }
                }
            },
        };
        setups.insert(key, setup.clone());
        Ok(setup)
    }

    /// Load what `transform` needs for the source CRS's area of use.
    fn load_grids(&self, transform: &Transform, srs: &str) -> crate::Result<()> {
        if transform.grids().is_empty() {
            return Ok(());
        }
        let bounds = SrsName::parse(srs)
            .crs
            .and_then(|crs| area_of_use(transform, &crs));
        let error = |message: String| crate::Error::Reproject {
            message: format!("{srs} to {}: {message}", self.target_srs),
        };
        let Some(loader) = &self.loader else {
            let requests = transform.grid_requests(bounds);
            if requests.is_empty() {
                return Ok(());
            }
            let mut files: Vec<&str> = requests.iter().map(|r| r.file.as_str()).collect();
            files.dedup();
            return Err(error(format!(
                "needs grids ({}) and the read has no way to load them: give it a grid \
                 loader, or read without grids (no_grids)",
                files.join(", ")
            )));
        };
        transform.load_grids(bounds, |request| {
            loader
                .fetch(request)
                .map_err(|message| error(format!("grid {}: {message}", request.file)))
        })
    }
}

/// An srsName as a CRS `xeibe-reproject` can convert: an EPSG code or a
/// compound of them, or OGC's CRS84 and kin. What `target_crs` accepts.
pub fn crs(srs: &str) -> Result<Crs, String> {
    let crs = SrsName::parse(srs)
        .crs
        .ok_or_else(|| "names no known CRS".to_string())?;
    if let CrsRef::Code { authority, code } = &crs
        && authority.eq_ignore_ascii_case("OGC")
    {
        // Longitude first by definition: the EPSG CRS of the same datum, read
        // longitude first, as xeibe writes it anyway.
        let epsg = match code.to_ascii_uppercase().as_str() {
            "CRS84" => 4326,
            "CRS84H" => 4979,
            "CRS83" => 4269,
            "CRS27" => 4267,
            _ => return Err(format!("{authority}:{code} is not supported")),
        };
        return Crs::from_epsg(epsg).map_err(|e| e.to_string());
    }
    let json = crs
        .projjson()
        .ok_or_else(|| format!("{} isn't in the EPSG tables", crs.authority_code()))?;
    Crs::from_projjson(&json.to_string()).map_err(|e| e.to_string())
}

/// EPSG's area of use of the CRS, in the source CRS, longitude or easting
/// first: where grids are loaded for. `None` (the grids whole) when it
/// can't be had.
fn area_of_use(transform: &Transform, crs: &CrsRef) -> Option<[f64; 4]> {
    let CrsRef::Code { authority, code } = crs.horizontal() else {
        return None;
    };
    if !authority.eq_ignore_ascii_case("EPSG") {
        return None;
    }
    let [south, west, north, east] = xeibe_crs::get(code.parse().ok()?)?
        .area_wgs84?
        .map(f64::from);
    let wgs84 = Crs::from_epsg(4326).ok()?;
    let to_source = Transform::with_grids(
        &wgs84,
        transform.source(),
        AxisOrder::EastFirst,
        Grids::None,
    )
    .ok()?;
    let bounds = to_source
        .convert_bounds([west, south, east, north], DEFAULT_DENSIFY)
        .ok()?;
    // Across the antimeridian: the grids whole.
    (bounds[0] <= bounds[2] && bounds[1] <= bounds[3]).then_some(bounds)
}

fn convert_coords(transform: &Transform, coords: &mut Coords) -> Result<(), String> {
    let size = coords.size();
    for position in coords.values.chunks_exact_mut(size) {
        convert_position(transform, position)?;
    }
    Ok(())
}

/// One position: x and y, and a height when there is one (a NaN Z is a 2D
/// position in a 3D column).
fn convert_position(transform: &Transform, position: &mut [f64]) -> Result<(), String> {
    if position.len() < 2 {
        return Ok(());
    }
    let z = position.get(2).copied().filter(|z| !z.is_nan());
    let (x, y, height) = transform
        .convert(position[0], position[1], z)
        .map_err(|reason| format!("({}, {}): {reason}", position[0], position[1]))?;
    position[0] = x;
    position[1] = y;
    if let (Some(height), Some(slot)) = (height, position.get_mut(2)) {
        *slot = height;
    }
    Ok(())
}
