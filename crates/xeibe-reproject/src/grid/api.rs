//! Loading a transform's grids: what to fetch, and handing the bytes over.

use std::f64::consts::{FRAC_PI_2, PI};
use std::path::Path;

use super::GridRequest;
use super::store::Region;
use crate::error::Error;
use crate::transform::{Transform, to_geographic, to_internal};

/// Added around the area converted coordinates come from, for the datum
/// shifts between the source CRS and the CRS a grid is read in (a few
/// hundred metres at most): 0.05°, about 5.5 km.
const MARGIN: f64 = 0.05 * PI / 180.0;

/// Points per edge when finding the area of a box.
const EDGE_POINTS: usize = 21;

impl Transform {
    /// PROJ-data's names of the grid files this transform may read, e.g.
    /// `fr_ign_ntf_r93.tif`; empty when no candidate uses a grid.
    pub fn grids(&self) -> Vec<&str> {
        self.candidates()
            .map(|c| c.grid_files().iter().map(|f| f.name.as_str()).collect())
            .unwrap_or_default()
    }

    /// The grid data still to load before converting coordinates within
    /// `bounds` (`[min_x, min_y, max_x, max_y]` in the source CRS, in this
    /// transform's axis order), or anywhere for `None` (and for a
    /// geocentric source CRS, whose box can't place the coordinates without
    /// Z). Empty when nothing is missing. Only the grids of operations whose area of use meets
    /// the box count.
    ///
    /// Loading takes rounds: a file's directories come first, then the
    /// blocks of the grid covering the area. Fetch every request, hand each
    /// to [`Transform::add_grid_data`], and ask again until the list is
    /// empty ([`Transform::load_grids`] does this loop). Grid files are
    /// shared by every transform in the process, so each is loaded once.
    pub fn grid_requests(&self, bounds: Option<[f64; 4]>) -> Vec<GridRequest> {
        let Some(c) = self.candidates() else {
            return Vec::new();
        };
        // A geocentric box has no Z: it can't place the coordinates, so
        // the whole grids are needed.
        let geocentric = matches!(self.source().kind, crate::crs::Kind::Geocentric { .. });
        let region = bounds
            .filter(|_| !geocentric)
            .and_then(|b| self.grid_region(b));
        let files = match (region, bounds) {
            (Some(r), Some(b)) => c.grid_files_within(
                [
                    r.west.to_degrees(),
                    r.south.to_degrees(),
                    r.east.to_degrees(),
                    r.north.to_degrees(),
                ],
                self.internal_box(b),
            ),
            _ => c.grid_files().to_vec(),
        };
        let mut out = Vec::new();
        for file in &files {
            for (offset, length) in file.requests(region) {
                out.push(GridRequest {
                    file: file.name.clone(),
                    offset,
                    length,
                });
            }
        }
        out
    }

    /// Hand over bytes of grid `file` read from `offset` (a whole
    /// [`GridRequest`], or any part of the file).
    pub fn add_grid_data(&self, file: &str, offset: u64, data: Vec<u8>) -> Result<(), Error> {
        let Some(f) = self
            .candidates()
            .and_then(|c| c.grid_files().iter().find(|f| f.name == file))
        else {
            return Err(Error::Grid {
                file: file.to_string(),
                message: "not a grid this transform uses".into(),
            });
        };
        f.insert(offset, data).map_err(|message| Error::Grid {
            file: file.to_string(),
            message,
        })
    }

    /// Load the grids needed within `bounds` (see
    /// [`Transform::grid_requests`]) with `fetch`, which returns the bytes
    /// of one request.
    pub fn load_grids<E: From<Error>>(
        &self,
        bounds: Option<[f64; 4]>,
        mut fetch: impl FnMut(&GridRequest) -> Result<Vec<u8>, E>,
    ) -> Result<(), E> {
        let mut seen = std::collections::HashSet::new();
        loop {
            let requests = self.grid_requests(bounds);
            if requests.is_empty() {
                return Ok(());
            }
            for r in requests {
                // A request asked for twice got nothing usable the first
                // time (a file shorter than its directories say).
                if !seen.insert(r.clone()) {
                    return Err(Error::Grid {
                        file: r.file,
                        message: format!("bytes from {} are missing", r.offset),
                    }
                    .into());
                }
                let data = fetch(&r)?;
                self.add_grid_data(&r.file, r.offset, data)?;
            }
        }
    }

    /// Load the grids needed within `bounds` from a directory holding
    /// PROJ-data's files (a PROJ-data release, or `projsync`'s download).
    pub fn load_grids_from_dir(
        &self,
        dir: impl AsRef<Path>,
        bounds: Option<[f64; 4]>,
    ) -> Result<(), Error> {
        use std::io::{Read, Seek, SeekFrom};
        let dir = dir.as_ref();
        self.load_grids(bounds, |r: &GridRequest| {
            let io = |e: std::io::Error| Error::Grid {
                file: r.file.clone(),
                message: format!("{}: {e}", dir.join(&r.file).display()),
            };
            let mut f = std::fs::File::open(dir.join(&r.file)).map_err(io)?;
            f.seek(SeekFrom::Start(r.offset)).map_err(io)?;
            let mut data = Vec::new();
            f.take(r.length.unwrap_or(u64::MAX))
                .read_to_end(&mut data)
                .map_err(io)?;
            Ok::<_, Error>(data)
        })
    }

    /// The box in the source CRS's internal coordinates (what the
    /// per-point choice compares), or degrees for a geocentric source.
    fn internal_box(&self, bounds: [f64; 4]) -> Option<[f64; 4]> {
        let src = self.source();
        let [x0, y0, x1, y1] = bounds;
        let mut out = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (x, y) in [(x0, y0), (x0, y1), (x1, y0), (x1, y1)] {
            let mut p = to_internal(&src.axes, self.order(), [x, y, 0.0]).ok()?;
            if matches!(src.kind, crate::crs::Kind::Geocentric { .. }) {
                let g = to_geographic(src, p).ok()?;
                p = [g[0].to_degrees(), g[1].to_degrees(), 0.0];
            }
            out = [
                out[0].min(p[0]),
                out[1].min(p[1]),
                out[2].max(p[0]),
                out[3].max(p[1]),
            ];
        }
        Some(out)
    }

    /// Longitude and latitude (radians from Greenwich) around the source
    /// coordinates in `bounds`, with a margin; `None` when no point of the
    /// box converts.
    fn grid_region(&self, bounds: [f64; 4]) -> Option<Region> {
        let src = self.source();
        let pm = src.datum().map_or(0.0, |d| d.prime_meridian);
        let [x0, y0, x1, y1] = bounds;
        let mut lons = Vec::new();
        let (mut south, mut north) = (f64::MAX, f64::MIN);
        // A geographic box given across the antimeridian.
        let x1 = if x1 < x0 && src.longitude_turn().is_some() {
            x1 + src.longitude_turn().unwrap_or(360.0)
        } else {
            x1
        };
        let n = EDGE_POINTS - 1;
        for i in 0..=n {
            let fx = x0 + (x1 - x0) * i as f64 / n as f64;
            let fy = y0 + (y1 - y0) * i as f64 / n as f64;
            for (x, y) in [(fx, y0), (fx, y1), (x0, fy), (x1, fy)] {
                let Ok(internal) = to_internal(&src.axes, self.order(), [x, y, 0.0]) else {
                    continue;
                };
                let Ok([lon, lat, _]) = to_geographic(src, internal) else {
                    continue;
                };
                lons.push(normalize(lon + pm));
                south = south.min(lat);
                north = north.max(lat);
            }
        }
        if lons.is_empty() {
            return None;
        }
        let span = |v: &[f64]| {
            let (lo, hi) = v
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), &l| (lo.min(l), hi.max(l)));
            (lo, hi)
        };
        let (mut west, mut east) = span(&lons);
        if east - west > PI {
            // Maybe across the antimeridian: try longitudes in [0, 2π).
            let wrapped: Vec<f64> = lons
                .iter()
                .map(|&l| if l < 0.0 { l + 2.0 * PI } else { l })
                .collect();
            let (w, e) = span(&wrapped);
            if e - w < east - west {
                (west, east) = (w, e);
            }
        }
        Some(Region {
            west: west - MARGIN,
            east: east + MARGIN,
            south: (south - MARGIN).max(-FRAC_PI_2),
            north: (north + MARGIN).min(FRAC_PI_2),
        })
    }
}

fn normalize(lon: f64) -> f64 {
    let mut l = lon % (2.0 * PI);
    if l > PI {
        l -= 2.0 * PI;
    } else if l < -PI {
        l += 2.0 * PI;
    }
    l
}

#[cfg(test)]
mod tests {
    use crate::{AxisOrder, Crs, Error, Transform};

    /// A grid in 16×16 tiles: a box fetches the directories and its tiles,
    /// not the whole file, and converts as PROJ does with the whole grid. (In this test binary only, so that no
    /// other test loads the untiled file under the same name.)
    #[test]
    fn only_the_tiles_a_box_needs_are_loaded() {
        let data = xeibe_testkit::data_dir().join("reprojection");
        let tiled = std::fs::read(data.join("grids/tiled/us_noaa_stgeorge.tif")).unwrap();
        // PROJ's result for NADCON St. George Island to NAD83 (EPSG:1457).
        let text = std::fs::read_to_string(data.join("operations-grids.jsonl")).unwrap();
        let record: serde_json::Value = text
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|r| r["op"] == 1457)
            .unwrap();
        let (input, want) = record["in"]
            .as_array()
            .unwrap()
            .iter()
            .zip(record["out"].as_array().unwrap())
            .find(|(_, out)| !out.is_null())
            .map(|(i, o)| {
                let v = |x: &serde_json::Value| -> Vec<f64> {
                    serde_json::from_value(x.clone()).unwrap()
                };
                (v(i), v(o))
            })
            .unwrap();

        let (a, b) = (Crs::from_epsg(4138).unwrap(), Crs::from_epsg(4269).unwrap());
        let t = Transform::via(&a, &b, 1457, AxisOrder::Authority).unwrap();
        let bounds = [
            input[0] - 0.01,
            input[1] - 0.01,
            input[0] + 0.01,
            input[1] + 0.01,
        ];
        let mut fetched = vec![false; tiled.len()];
        t.load_grids(Some(bounds), |r| {
            let start = r.offset as usize;
            let end = r
                .length
                .map_or(tiled.len(), |l| (start + l as usize).min(tiled.len()));
            fetched[start..end].iter_mut().for_each(|f| *f = true);
            Ok::<_, Error>(tiled[start..end].to_vec())
        })
        .unwrap();
        let bytes = fetched.iter().filter(|&&f| f).count();
        assert!(
            bytes < tiled.len() * 3 / 4,
            "{bytes} of {} bytes",
            tiled.len()
        );

        let (y, x, _) = t.convert(input[0], input[1], None).unwrap();
        assert!((y - want[0]).abs() < 1e-11 && (x - want[1]).abs() < 1e-11);
    }
}
