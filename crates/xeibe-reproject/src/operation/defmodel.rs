//! Deformation models (level 3): EPSG's "New Zealand Deformation Model",
//! from ITRF96 at the coordinate's epoch to NZGD2000, the plates' motion
//! and every earthquake's displacement since 2000 taken out.
//!
//! As PROJ 9.8.1 writes it (`SingleOperation::exportToPROJString`, MIT,
//! see PROJ-NOTICE.md): `+inv +proj=defmodel +model=…`. PROJ-data's model
//! goes from NZGD2000 to ITRF96, the other way round from EPSG's
//! operation. The model says where a point was at a given time, so the
//! operation needs the coordinate's epoch: PROJ fails without one, and so
//! does this ([`CoordinateError::MissingEpoch`]).

use std::sync::Arc;

use super::grid::state_error;
use super::{Method, MethodError};
use crate::crs::{Crs, Kind};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;
use crate::grid::store::{self, GridFile};
use crate::registry::Operation;

/// EPSG method codes handled here.
pub(super) const METHODS: &[u32] = &[NEW_ZEALAND_DEFORMATION_MODEL];

pub(crate) const NEW_ZEALAND_DEFORMATION_MODEL: u32 = 1079;
/// "Point motion velocity grid file": the model, for this method.
pub(super) const MODEL_FILE: u32 = 1050;

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    let name = op
        .file(MODEL_FILE)
        .ok_or(MethodError::MissingParameter(MODEL_FILE))?;
    let entry = crate::grid::on_cdn(name, &op.name)
        .ok_or_else(|| MethodError::GridUnavailable(name.to_string()))?;
    let prime_meridian = |crs: &Crs| match &crs.kind {
        Kind::Geographic { datum } => Ok(datum.prime_meridian),
        _ => Err(MethodError::WrongCrs("a geographic CRS")),
    };
    Ok(Arc::new(DeformationModel {
        file: store::file(entry.file),
        // The step has no ellipsoid of its own in PROJ's pipeline, which
        // makes it GRS 1980: NZGD2000's and ITRF96's.
        ellipsoid: Ellipsoid::from_inverse_flattening(6_378_137.0, 298.257_222_101),
        source_meridian: prime_meridian(src)?,
        target_meridian: prime_meridian(dst)?,
    }))
}

/// `defmodel` backwards between the operation's two geographic CRSs.
#[derive(Debug)]
struct DeformationModel {
    file: Arc<GridFile>,
    ellipsoid: Ellipsoid,
    source_meridian: f64,
    target_meridian: f64,
}

impl Method for DeformationModel {
    fn forward(&self, _: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Err(CoordinateError::MissingEpoch)
    }

    fn inverse(&self, _: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        Err(CoordinateError::MissingEpoch)
    }

    fn forward_3d_at(&self, c: [f64; 3], epoch: f64) -> Result<[f64; 3], CoordinateError> {
        let model = self.file.model().map_err(state_error)?;
        let [lon, lat, h] = model.inverse(
            [c[0] + self.source_meridian, c[1], c[2]],
            epoch,
            &self.ellipsoid,
        )?;
        Ok([lon - self.target_meridian, lat, h])
    }

    fn inverse_3d_at(&self, c: [f64; 3], epoch: f64) -> Result<[f64; 3], CoordinateError> {
        let model = self.file.model().map_err(state_error)?;
        let [lon, lat, h] = model.forward(
            [c[0] + self.target_meridian, c[1], c[2]],
            epoch,
            &self.ellipsoid,
        )?;
        Ok([lon - self.source_meridian, lat, h])
    }

    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        vec![self.file.clone()]
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{AxisOrder, CoordinateError, Transform};

    /// The model's master file comes first: it names the grids, which are
    /// then asked for like any others (their headers, then their blocks).
    /// No other test of this binary loads these files.
    #[test]
    fn a_models_grids_are_known_once_its_master_file_is_loaded() {
        const MASTER: &str = "nz_linz_nzgd2000-20180701.json";
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/data/reprojection/grids/cropped");
        let t = Transform::from_epsg(7907, 4959, AxisOrder::Authority).unwrap();
        let convert = || t.convert_at(-42.63, 173.01, Some(10.0), 2020.0);
        assert_eq!(t.grids(), [MASTER]);
        assert_eq!(convert(), Err(CoordinateError::GridNotLoaded));
        let mut rounds = Vec::new();
        loop {
            let requests = t.grid_requests(None);
            if requests.is_empty() {
                break;
            }
            rounds.push(requests.len());
            for r in requests {
                let bytes = std::fs::read(dir.join(&r.file)).unwrap();
                let start = (r.offset as usize).min(bytes.len());
                let end = r
                    .length
                    .map_or(bytes.len(), |n| (start + n as usize).min(bytes.len()));
                t.add_grid_data(&r.file, r.offset, bytes[start..end].to_vec())
                    .unwrap();
            }
            match rounds.len() {
                // The master file, whole.
                1 => {
                    assert_eq!(rounds, [1]);
                    // Its 20 grids are the transform's now, and not loaded.
                    let grids = t.grids();
                    assert_eq!(grids.len(), 21);
                    assert_eq!(grids[0], MASTER);
                    assert!(grids[1..].iter().all(|g| g.ends_with(".tif")), "{grids:?}");
                    assert_eq!(convert(), Err(CoordinateError::GridNotLoaded));
                }
                // Each grid's start.
                2 => assert_eq!(rounds, [1, 20]),
                _ => {}
            }
        }
        assert!(rounds.len() >= 2, "{rounds:?}");
        let (lat, lon, h) = convert().unwrap();
        // PROJ 9.8.1, on the whole model.
        assert!((lat + 42.630_003_992_318_97).abs() < 1e-10, "{lat}");
        assert!((lon - 173.010_007_238_755_86).abs() < 1e-10, "{lon}");
        assert_eq!(h, Some(10.0));
        // Asked for again: nothing.
        assert!(t.grid_requests(None).is_empty());
    }
}
