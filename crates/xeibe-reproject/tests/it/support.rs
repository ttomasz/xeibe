//! Test vectors written by `scripts/gen_reprojection_vectors.py`, with PROJ as
//! the oracle.

use std::path::Path;

use serde_json::Value;
use xeibe_reproject::{Crs, Error};

pub struct Vector {
    pub code: u32,
    /// `"projected"` or `"geocentric"`.
    pub kind: String,
    /// Projection method name, for projected CRSs.
    pub method: String,
    /// The geographic CRS the points start in.
    pub src: Value,
    /// Points in `src`, authority order and units.
    pub geog: Vec<Vec<f64>>,
    /// PROJ's conversion of each point into EPSG:`code`, authority order and
    /// units, or `None` where PROJ fails.
    pub proj: Vec<Option<Vec<f64>>>,
}

impl Vector {
    pub fn source(&self) -> Result<Crs, Error> {
        match &self.src {
            Value::Number(code) => Crs::from_epsg(code.as_u64().unwrap() as u32),
            doc => Crs::from_projjson(&doc.to_string()),
        }
    }
}

/// Read a vector file. The first line is a header naming the generator and
/// the PROJ version.
pub fn load(path: &Path) -> Vec<Vector> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run scripts/gen_reprojection_vectors.py",
            path.display()
        )
    });
    let mut lines = text.lines();
    let header: Value = serde_json::from_str(lines.next().expect("header")).unwrap();
    assert_eq!(header["generator"], "scripts/gen_reprojection_vectors.py");
    lines
        .map(|line| {
            let v: Value = serde_json::from_str(line).unwrap();
            let points = |key: &str| -> Vec<Option<Vec<f64>>> {
                v[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        p.as_array()
                            .map(|p| p.iter().map(|x| x.as_f64().unwrap()).collect())
                    })
                    .collect()
            };
            Vector {
                code: v["code"].as_u64().unwrap() as u32,
                kind: v["kind"].as_str().unwrap().to_owned(),
                method: v["method"].as_str().unwrap_or("").to_owned(),
                src: v["src"].clone(),
                geog: points("geog").into_iter().map(Option::unwrap).collect(),
                proj: points("proj"),
            }
        })
        .collect()
}
