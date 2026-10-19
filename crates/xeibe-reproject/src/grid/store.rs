//! Grid files in memory, filled by the caller (`docs/reprojection.md`,
//! decision 6: the transform does no I/O).
//!
//! A file starts empty. [`GridFile::requests`] says which byte ranges are
//! needed next: the start of the file until its directories are read, then
//! the blocks covering an area. The caller fetches them (from
//! `cdn.proj.org` with range requests, or from a local copy) and hands them
//! to [`GridFile::insert`], which parses and decodes what it can. Files are
//! shared by name across all transforms in the process, and stay loaded
//! after the transforms that used them are gone, so a range is fetched once
//! per process (decision 6); [`clear_grids`] lets them go.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::set::ExtentAndRes;
use super::tiff::{self, Image, Ranges, TiffError};
use super::tin::Tin;

/// How much of a file's start to ask for first: PROJ's network chunk.
const HEADER_CHUNK: u64 = 16 * 1024;

/// One grid file.
#[derive(Debug)]
pub(crate) struct GridFile {
    /// PROJ-data's name, e.g. `fr_ign_ntf_r93.tif`.
    pub(crate) name: String,
    loaded: Mutex<Ranges>,
    images: OnceLock<Result<Vec<Image>, String>>,
    /// A triangulation (`.json`), read whole.
    tin: OnceLock<Result<Tin, String>>,
}

/// Why a grid can't be used.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GridState {
    /// Not loaded (enough) yet.
    NotLoaded,
    /// The file isn't a grid PROJ could read.
    Invalid(String),
}

/// An area in a grid's coordinates (radians of longitude and latitude from
/// Greenwich for a geographic grid).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Region {
    pub(crate) west: f64,
    pub(crate) south: f64,
    pub(crate) east: f64,
    pub(crate) north: f64,
}

fn registry() -> &'static Mutex<HashMap<String, Arc<GridFile>>> {
    static FILES: OnceLock<Mutex<HashMap<String, Arc<GridFile>>>> = OnceLock::new();
    FILES.get_or_init(Default::default)
}

/// The process's one copy of a grid file.
pub(crate) fn file(name: &str) -> Arc<GridFile> {
    let mut files = registry().lock().unwrap_or_else(|e| e.into_inner());
    files
        .entry(name.to_string())
        .or_insert_with(|| {
            Arc::new(GridFile {
                name: name.to_string(),
                loaded: Mutex::default(),
                images: OnceLock::new(),
                tin: OnceLock::new(),
            })
        })
        .clone()
}

/// Forget the grid data loaded so far, for a long-running process that
/// wants the memory back. Transforms set up before keep what they hold; a
/// transform set up after starts with empty grids, to be loaded again
/// ([`crate::Transform::grid_requests`]).
pub fn clear_grids() {
    registry().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

impl GridFile {
    fn is_tin(&self) -> bool {
        self.name.ends_with(".json")
    }

    /// The triangulation, once the file is loaded.
    pub(crate) fn tin(&self) -> Result<&Tin, GridState> {
        match self.tin.get() {
            Some(Ok(t)) => Ok(t),
            Some(Err(e)) => Err(GridState::Invalid(e.clone())),
            None => Err(GridState::NotLoaded),
        }
    }

    /// The file's images, once its directories are loaded.
    pub(crate) fn images(&self) -> Result<&[Image], GridState> {
        match self.images.get() {
            Some(Ok(images)) => Ok(images),
            Some(Err(e)) => Err(GridState::Invalid(e.clone())),
            None => Err(GridState::NotLoaded),
        }
    }

    /// Add bytes read from `offset`; parse the directories, and decode the
    /// blocks whose bytes are now complete.
    pub(crate) fn insert(&self, offset: u64, data: Vec<u8>) -> Result<(), String> {
        if self.is_tin() {
            // Requested whole: parse what came.
            if offset != 0 {
                return Err("a triangulation is read whole, from its start".into());
            }
            let parsed = self.tin.get_or_init(|| Tin::parse(&data));
            return parsed.as_ref().map(|_| ()).map_err(Clone::clone);
        }
        let mut loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        loaded.insert(offset, data);
        if self.images.get().is_none() {
            match tiff::parse(&loaded) {
                Ok(images) => {
                    let _ = self.images.set(Ok(images));
                }
                Err(TiffError::Need { .. }) => return Ok(()),
                Err(TiffError::Invalid(e)) => {
                    let _ = self.images.set(Err(e.clone()));
                    return Err(e);
                }
            }
        }
        let Some(Ok(images)) = self.images.get() else {
            return Ok(());
        };
        for image in images {
            for block in 0..image.block_count() {
                if image.is_decoded(block) {
                    continue;
                }
                match image.decode(block, &loaded) {
                    Ok(()) | Err(TiffError::Need { .. }) => {}
                    Err(TiffError::Invalid(e)) => return Err(format!("{}: {e}", self.name)),
                }
            }
        }
        Ok(())
    }

    /// The byte ranges still needed to use the file within `region` (or
    /// anywhere, for `None`): offset and length (`None`: to the end of the
    /// file), merged where adjacent.
    pub(crate) fn requests(&self, region: Option<Region>) -> Vec<(u64, Option<u64>)> {
        if self.is_tin() {
            return match self.tin.get() {
                None => vec![(0, None)],
                Some(_) => Vec::new(),
            };
        }
        let images = match self.images.get() {
            Some(Ok(images)) => images,
            Some(Err(_)) => return Vec::new(),
            None => {
                let loaded = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
                // Another thread's `insert` may have read the directories
                // while this one waited for the lock: it sets them before
                // letting go of it.
                match self.images.get() {
                    Some(Ok(images)) => images,
                    Some(Err(_)) => return Vec::new(),
                    None if loaded.is_empty() => return vec![(0, Some(HEADER_CHUNK))],
                    None => {
                        return match tiff::parse(&loaded) {
                            Err(TiffError::Need { offset, length }) => {
                                vec![(offset, Some(length.max(HEADER_CHUNK)))]
                            }
                            _ => Vec::new(),
                        };
                    }
                }
            }
        };
        let mut wanted: Vec<(u64, u64)> = Vec::new();
        for image in images {
            // Reduced-resolution images aren't read.
            if image.subfile_type != 0 && image.subfile_type != 2 {
                continue;
            }
            let blocks = match region {
                None => (0..image.block_count()).collect(),
                Some(r) => match nodes_in(&image.extent, image.width, image.height, r) {
                    Some((x0, y0, x1, y1)) => image.blocks_for(x0, y0, x1, y1),
                    None => Vec::new(),
                },
            };
            for b in blocks {
                if !image.is_decoded(b) {
                    wanted.push(image.block_range(b));
                }
            }
        }
        wanted.sort_unstable();
        wanted.dedup();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (offset, length) in wanted {
            match merged.last_mut() {
                Some((o, l)) if *o + *l == offset => *l += length,
                _ => merged.push((offset, length)),
            }
        }
        merged.into_iter().map(|(o, l)| (o, Some(l))).collect()
    }
}

/// The nodes of a grid within `region`, two nodes beyond on every side
/// (interpolation reads the next node, biquadratic one more), as column and
/// row ranges counted from the west and the south.
fn nodes_in(
    extent: &ExtentAndRes,
    width: usize,
    height: usize,
    r: Region,
) -> Option<(usize, usize, usize, usize)> {
    use std::f64::consts::PI;
    let rows = (
        ((r.south - extent.south) / extent.res_y).floor() - 2.0,
        ((r.north - extent.south) / extent.res_y).ceil() + 2.0,
    );
    if rows.1 < 0.0 || rows.0 > (height - 1) as f64 {
        return None;
    }
    let clamp_y = |v: f64| v.clamp(0.0, (height - 1) as f64) as usize;
    let (y0, y1) = (clamp_y(rows.0), clamp_y(rows.1));
    let span = |west: f64, east: f64| {
        (
            ((west - extent.west) / extent.res_x).floor() - 2.0,
            ((east - extent.west) / extent.res_x).ceil() + 2.0,
        )
    };
    let shifts: &[f64] = if extent.is_geographic {
        &[0.0, 2.0 * PI, -2.0 * PI]
    } else {
        &[0.0]
    };
    let mut cols: Option<(f64, f64)> = None;
    let east = if r.east < r.west {
        r.east + 2.0 * PI
    } else {
        r.east
    };
    for s in shifts {
        let (c0, c1) = span(r.west + s, east + s);
        if c1 < 0.0 || c0 > (width - 1) as f64 {
            continue;
        }
        cols = Some(match cols {
            None => (c0, c1),
            Some((a, b)) => (a.min(c0), b.max(c1)),
        });
    }
    let (c0, c1) = cols?;
    let clamp_x = |v: f64| v.clamp(0.0, (width - 1) as f64) as usize;
    Some((clamp_x(c0), y0, clamp_x(c1), y1))
}
