//! `xeibe scan` (`docs/architecture.md`, "CLI" and "Settings file";
//! `docs/schema-inference.md` §7).

use crate::support::*;

#[test]
fn scan_lists_layers_with_counts_geometry_crs_and_columns() {
    let run = xeibe_ok(&["scan", &sample(PRG)]);
    for layer in ["prgad:AD_Miejscowosc", "prgad:AD_UlicaPlac", "prgad:AD_PunktAdresowy"] {
        let line = run
            .stdout
            .lines()
            .find(|line| line.starts_with(&format!("{layer}:")))
            .unwrap_or_else(|| panic!("no line for {layer} in\n{}", run.stdout));
        assert!(line.contains("2 features"), "{line}");
        assert!(line.contains("crs EPSG:2180"), "{line}");
    }
    assert!(run.stdout.contains("geometry georeferencja"), "{}", run.stdout);
    assert!(run.stdout.contains("geometry(Point)"), "{}", run.stdout);
    assert!(run.stdout.contains("geometry(Polygon)"), "{}", run.stdout);
    // Schemas are flat, with the shortest unique names: the type wrapper is
    // left out and `lokalnyId` is unique.
    assert!(run.stdout.contains("lokalnyId"), "{}", run.stdout);
    assert!(!run.stdout.contains("idIIP.lokalnyId"), "{}", run.stdout);
}

#[test]
fn scan_writes_a_settings_file_with_options_and_one_schema_per_layer() {
    let dir = out_dir("scan_settings");
    let path = dir.join("prg.json");
    xeibe_ok(&["scan", &sample(PRG), "-o", path_str(&path)]);

    let text = std::fs::read_to_string(&path).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(settings["format_version"], 1);
    let layers = settings["layers"].as_object().expect("layers");
    assert_eq!(layers.len(), 3);
    // Layers in input order (`serde_json::Value` sorts keys, so check the text).
    let position = |name: &str| text.find(&format!("\"{name}\": {{")).unwrap_or_else(|| panic!("{name} in\n{text}"));
    assert!(position("prgad:AD_Miejscowosc") < position("prgad:AD_UlicaPlac"));
    assert!(position("prgad:AD_UlicaPlac") < position("prgad:AD_PunktAdresowy"));
    let points = &layers["prgad:AD_PunktAdresowy"];
    // Types are written as aliases; a path only where the name isn't the path.
    assert_eq!(points["georeferencja"], "geometry(Point)");
    assert_eq!(points["dataNadania"], "date");
    assert_eq!(points["lokalnyId"]["type"], "text");
    assert_eq!(points["lokalnyId"]["path"], "idIIP/*/lokalnyId");
    assert_eq!(points["miejscowosc"]["path"], "miejscowosc/@href");
    // The axis decision is written as a plain mode, so reads with the file
    // gather no evidence. Geometry options are read options, not inference.
    assert_eq!(settings["options"]["geometry"]["axis"], "XY");
    assert!(settings["options"]["inference"].get("geometry").is_none(), "{}", settings["options"]);
}

#[test]
fn scan_preset_and_axis_order_go_into_the_settings_file() {
    // The presets are `default` and `strings` (`docs/schema-inference.md` §3.6).
    let dir = out_dir("scan_preset");
    let path = dir.join("prg.json");
    xeibe_ok(&["scan", &sample(PRG), "--preset", "strings", "--axis-order", "yx", "-o", path_str(&path)]);
    let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(settings["options"]["geometry"]["axis"], "YX");
    let points = &settings["layers"]["prgad:AD_PunktAdresowy"];
    assert_eq!(points["dataNadania"], "text", "every scalar as text");
}

#[test]
fn only_the_documented_presets_exist() {
    for gone in ["flat", "gdal-like", "spark-xml-like"] {
        let run = xeibe(&["scan", &sample(PRG), "--preset", gone]);
        assert!(!run.success, "--preset {gone} is still accepted");
    }
}

#[test]
fn scan_explain_prints_every_field_with_its_reasons() {
    let run = xeibe_ok(&["scan", &sample(PRG), "--explain"]);
    let line = run
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("georeferencja"))
        .unwrap_or_else(|| panic!("no georeferencja row in\n{}", run.stdout));
    assert!(line.contains("geoarrow.point"), "{line}");
    assert!(line.contains("kinds={Point}"), "{line}");
    assert!(run.stdout.contains("encoding Auto → native point"), "{}", run.stdout);
    assert!(run.stdout.contains("typed values rejected"), "{}", run.stdout);
}

#[test]
fn scan_sample_reads_only_the_first_features() {
    let run = xeibe_ok(&["scan", &sample(PRG), "--sample", "1"]);
    assert!(run.stdout.contains("sampled scan"), "{}", run.stdout);
    assert!(!run.stdout.contains("AD_PunktAdresowy"), "{}", run.stdout);
}

#[test]
fn scan_prints_axis_order_conflicts_first() {
    let run = xeibe_ok(&["scan", &sample("wfs/de-bfn-inspire-sd-wfs100-short-latlon.xml")]);
    let first = run.stderr.lines().next().unwrap_or_default();
    assert!(first.starts_with("axis-order conflict:"), "{}", run.stderr);
    assert!(first.contains("EPSG:4258"), "{first}");
    assert!(run.stderr.contains("decided y/x (swapped)"), "{}", run.stderr);
    assert!(run.stderr.contains("against: "), "{}", run.stderr);
}

#[test]
fn scan_reads_zip_members_selected_by_glob_or_path() {
    let dir = out_dir("scan_zip");
    let archive = dir.join("both.zip");
    zip(&archive, &[("data/prg.gml", PRG), ("data/rcn.gml", RCN_ARCS)]);

    let all = xeibe_ok(&["scan", path_str(&archive)]);
    assert!(all.stdout.contains("AD_PunktAdresowy") && all.stdout.contains("RCN_Budynek"), "{}", all.stdout);

    let member = xeibe_ok(&["scan", path_str(&archive), "--member", "data/rcn*"]);
    assert!(member.stdout.contains("RCN_Budynek"), "{}", member.stdout);
    assert!(!member.stdout.contains("AD_PunktAdresowy"), "{}", member.stdout);

    let path = format!("{}!/data/prg.gml", archive.display());
    let single = xeibe_ok(&["scan", &path]);
    assert!(single.stdout.contains("AD_PunktAdresowy"), "{}", single.stdout);
    assert!(!single.stdout.contains("RCN_Budynek"), "{}", single.stdout);
}

#[test]
fn scan_of_a_missing_file_fails() {
    let run = xeibe(&["scan", "does/not/exist.gml"]);
    assert!(!run.success);
    assert!(run.stderr.starts_with("error:"), "{}", run.stderr);
}
