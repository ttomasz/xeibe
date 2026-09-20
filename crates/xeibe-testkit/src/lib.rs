//! Test support shared by the crates' test suites.
//!
//! Nothing here depends on the library crates, so it keeps working while their
//! bodies are `todo!()`, and it can serve as an independent oracle:
//!
//! - [`samples`]: the curated samples in `tests/data` (`samples.json`), with the
//!   verified axis order of each one;
//! - [`gdal`]: GDAL 3.13 reference output (`*.gdal.txt`, `gml_geometry_cases.jsonl`);
//! - [`wkt`]: a small geometry value ([`wkt::G`]) with a WKT reader, the
//!   canonicalization used when comparing against GDAL, and approximate comparison;
//! - [`wkb`]: an ISO WKB reader (curve types included) used to check what the
//!   library writes;
//! - [`gml`]: builders for synthetic GML documents.

pub mod gdal;
pub mod gml;
pub mod samples;
pub mod wkb;
pub mod wkt;

use std::path::PathBuf;

/// `tests/data` in the repository.
pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data")
        .canonicalize()
        .expect("tests/data must exist")
}

/// Read a file below `tests/data`.
pub fn read_data(relative: &str) -> Vec<u8> {
    let path = data_dir().join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Read a UTF-8 file below `tests/data`.
pub fn read_data_str(relative: &str) -> String {
    String::from_utf8(read_data(relative)).expect("UTF-8")
}
