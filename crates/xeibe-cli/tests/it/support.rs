use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use parquet::file::reader::{FileReader, SerializedFileReader};

/// A sample under `tests/data/samples/`.
pub fn sample(relative: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/samples")
        .join(relative)
        .display()
        .to_string()
}

/// PRG: three point/polygon layers in EPSG:2180, two features each.
pub const PRG: &str = "pl/prg-address-points.gml";
/// RCN: `RCN_Budynek` has curve polygons (arcs).
pub const RCN_ARCS: &str = "pl/rcn-price-register-arcs.gml";

/// An empty directory for one test's outputs.
pub fn out_dir(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("xeibe-cli").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub struct Run {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn xeibe(args: &[&str]) -> Run {
    let Output { status, stdout, stderr } = Command::new(env!("CARGO_BIN_EXE_xeibe"))
        .args(args)
        .output()
        .expect("running xeibe");
    Run {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    }
}

/// Runs and expects success; both streams go into the panic message otherwise.
pub fn xeibe_ok(args: &[&str]) -> Run {
    let run = xeibe(args);
    assert!(run.success, "xeibe {args:?} failed\nstdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
    run
}

pub fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

pub fn parquet_reader(path: &Path) -> SerializedFileReader<File> {
    SerializedFileReader::new(File::open(path).expect("output file")).expect("a Parquet file")
}

/// A zip archive at `path` with the given members (name, sample).
pub fn zip(path: &Path, members: &[(&str, &str)]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for (name, relative) in members {
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file(*name, options).unwrap();
        std::io::Write::write_all(&mut zip, &std::fs::read(sample(relative)).unwrap()).unwrap();
    }
    zip.finish().unwrap();
}

/// The file's GeoParquet `geo` metadata.
pub fn geo_metadata(path: &Path) -> serde_json::Value {
    let reader = parquet_reader(path);
    let kv = reader.metadata().file_metadata().key_value_metadata().cloned().unwrap_or_default();
    let geo = kv.iter().find(|kv| kv.key == "geo").expect("`geo` key-value metadata");
    serde_json::from_str(geo.value.as_deref().expect("a value")).expect("`geo` is JSON")
}
