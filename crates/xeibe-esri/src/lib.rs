//! Esri's names for EPSG's CRSs and datums, and Esri's old codes that are
//! EPSG codes now, from Esri's projection engine database
//! (`scripts/gen_esri_tables.py`; Apache-2.0, see `ESRI-NOTICE.md`).
//!
//! <div class="warning">
//!
//! **Experimental, not for production use.** This crate is AI-generated and
//! has not been thoroughly tested yet. Its interfaces are not stable and will
//! change in the near future, in breaking ways.
//!
//! </div>
//!
//! ESRI's WKT names objects its own way (`GCS_ETRS_1989`, `D_ETRS_1989`,
//! `ETRS_1989_UTM_Zone_33N`), and many of those names don't normalise to
//! EPSG's. These tables map them to EPSG codes, so `xeibe-reproject` reads
//! ESRI WKT as the EPSG objects it describes (`docs/crs-wkt.md`). Objects
//! only Esri defines (`ESRI:54009`, Mollweide) are not here.
//!
//! ```
//! assert_eq!(xeibe_esri::geodetic_crs("GCS_ETRS_1989").collect::<Vec<_>>(), [4258]);
//! assert_eq!(xeibe_esri::datum("D_ETRS_1989").collect::<Vec<_>>(), [6258]);
//! assert_eq!(xeibe_esri::epsg_code(102100), Some(3857));
//! ```

mod tables;

use tables::{DATUMS, GEODETIC_CRS, OLD_CODES, PROJECTED_CRS, VERTICAL_CRS, VERTICAL_DATUMS};

/// The codes `name` has in `table`, which is sorted by lowercased name.
fn codes(table: &'static [(&'static str, u32)], name: &str) -> impl Iterator<Item = u32> {
    let name = name.trim().to_lowercase();
    let start = table.partition_point(|(n, _)| *n < name.as_str());
    table[start..]
        .iter()
        .take_while(move |(n, _)| *n == name)
        .map(|&(_, code)| code)
}

/// EPSG codes of the geographic CRSs (2D and 3D) Esri calls `name`
/// (`GCS_ETRS_1989`), ignoring case.
pub fn geodetic_crs(name: &str) -> impl Iterator<Item = u32> {
    codes(GEODETIC_CRS, name)
}

/// EPSG codes of the projected CRSs Esri calls `name`
/// (`ETRS_1989_UTM_Zone_33N`), ignoring case.
pub fn projected_crs(name: &str) -> impl Iterator<Item = u32> {
    codes(PROJECTED_CRS, name)
}

/// EPSG codes of the vertical CRSs Esri calls `name`, ignoring case.
pub fn vertical_crs(name: &str) -> impl Iterator<Item = u32> {
    codes(VERTICAL_CRS, name)
}

/// EPSG codes of the geodetic datums (or ensembles) Esri calls `name`
/// (`D_ETRS_1989`), ignoring case.
pub fn datum(name: &str) -> impl Iterator<Item = u32> {
    codes(DATUMS, name)
}

/// EPSG codes of the vertical datums Esri calls `name`, ignoring case.
pub fn vertical_datum(name: &str) -> impl Iterator<Item = u32> {
    codes(VERTICAL_DATUMS, name)
}

/// The EPSG code a CRS Esri gave a code of its own has now:
/// `ESRI:102100` is EPSG:3857.
pub fn epsg_code(esri_code: u32) -> Option<u32> {
    OLD_CODES
        .binary_search_by_key(&esri_code, |&(old, _)| old)
        .ok()
        .map(|i| OLD_CODES[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_for_lookup() {
        for table in [
            GEODETIC_CRS,
            PROJECTED_CRS,
            VERTICAL_CRS,
            DATUMS,
            VERTICAL_DATUMS,
        ] {
            assert!(table.windows(2).all(|w| w[0] < w[1]));
        }
        assert!(OLD_CODES.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn names_and_codes() {
        let all = |i: &mut dyn Iterator<Item = u32>| i.collect::<Vec<_>>();
        assert_eq!(all(&mut projected_crs("ETRF2000-PL_CS92")), [2180]);
        assert_eq!(
            all(&mut projected_crs("WGS_1984_Web_Mercator_Auxiliary_Sphere")),
            [3857]
        );
        assert_eq!(all(&mut datum("d_wgs_1984")), [6326]);
        assert_eq!(all(&mut datum("not a datum")), []);
        assert_eq!(epsg_code(102100), Some(3857));
        assert_eq!(epsg_code(54009), None);
    }
}
