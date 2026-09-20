//! The curated samples in `tests/data`, read from the generated `samples.json`.
//!
//! The manifest carries what tests need as ground truth, above all the
//! **verified axis order** of every sample (`axis_order`, `axis.expected_first_xy`),
//! which `scripts/corpus/axis_evidence.py` checked against the CRS area of use and
//! places the features are known to lie in. GDAL disagrees with 4 of the 17
//! samples, so tests assert against this, never against GDAL
//! (`docs/geometry.md`, "Observed in real services").

use std::path::PathBuf;

use serde::Deserialize;

use crate::data_dir;
use crate::gdal::GdalReport;

#[derive(Debug, Clone, Deserialize)]
pub struct Sample {
    /// e.g. `pl-prg-address-points`.
    pub name: String,
    /// Path relative to `tests/data`.
    pub file: String,
    /// `"x/y"` or `"y/x"`: how the coordinates are written in this file.
    pub axis_order: String,
    /// What this sample is here for.
    #[serde(default)]
    pub covers: Vec<String>,
    /// Extraction rules and counts.
    #[serde(default)]
    pub extraction: serde_json::Value,
    pub gdal: GdalSummary,
    pub axis: AxisFacts,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GdalSummary {
    /// Path of the `*.gdal.txt` file, relative to `tests/data`.
    pub file: String,
    pub driver: String,
    pub layers: usize,
    /// Features per layer, in the order GDAL lists the layers.
    pub feature_counts: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AxisFacts {
    /// `"x/y"` or `"y/x"`.
    pub source_order: String,
    /// The first position of the first geometry, exactly as written in the file.
    pub first_position_as_written: Vec<f64>,
    /// The same position in output order (easting/longitude first). This is what
    /// a correct read must produce.
    pub expected_first_xy: Vec<f64>,
    /// What GDAL 3.13.3 made of it.
    pub gdal_first_xy: Option<Vec<f64>>,
    /// `false` for the 4 samples GDAL reads in the wrong order.
    pub gdal_agrees: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    samples: Vec<Sample>,
}

impl Sample {
    pub fn path(&self) -> PathBuf {
        data_dir().join(&self.file)
    }

    pub fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.path()).unwrap_or_else(|e| panic!("reading {}: {e}", self.file))
    }

    pub fn text(&self) -> String {
        String::from_utf8(self.bytes()).expect("samples are UTF-8")
    }

    /// GDAL's `ogrinfo -al` output for this sample.
    pub fn gdal_report(&self) -> GdalReport {
        crate::gdal::report(&self.gdal.file)
    }

    /// `true` if the file writes coordinates in the CRS's (y/x) order.
    pub fn is_yx(&self) -> bool {
        self.axis_order == "y/x"
    }

    /// Features in the whole file, over all layers.
    pub fn total_features(&self) -> u64 {
        self.gdal.feature_counts.iter().sum()
    }

    /// `true` if this sample is a saved WFS response.
    pub fn is_wfs(&self) -> bool {
        self.file.contains("/wfs/")
    }
}

/// Every sample in `tests/data/samples.json`.
pub fn samples() -> Vec<Sample> {
    let text = crate::read_data_str("samples.json");
    let manifest: Manifest = serde_json::from_str(&text).expect("samples.json parses");
    manifest.samples
}

/// One sample by name; panics if it is not in the manifest.
pub fn sample(name: &str) -> Sample {
    samples()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no sample named {name:?} in samples.json"))
}
