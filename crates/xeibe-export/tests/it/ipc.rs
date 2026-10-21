use arrow_ipc::reader::FileReader;
use xeibe_arrow::ReadOptions;
use xeibe_export::IpcWriter;

use crate::support::*;

#[test]
fn ipc_keeps_curves_and_the_settings() {
    let (schema, batches, settings) = read(RCN_ARCS, "RCN_Budynek", &ReadOptions::default());
    let mut out = Vec::new();
    let mut writer = IpcWriter::new(&mut out, &schema, settings_metadata(settings)).unwrap();
    for batch in &batches {
        writer.write(batch).unwrap();
    }
    let summary = writer.finish().unwrap();
    assert_eq!(
        summary.rows,
        batches.iter().map(|b| b.num_rows() as u64).sum::<u64>()
    );

    let reader = FileReader::try_new(std::io::Cursor::new(out), None).unwrap();
    assert!(reader.custom_metadata().contains_key("gml:settings"));
    let field = reader
        .schema()
        .field_with_name("geometria")
        .unwrap()
        .clone();
    assert_eq!(
        field
            .metadata()
            .get("ARROW:extension:name")
            .map(String::as_str),
        Some("geoarrow.wkb"),
        "curves stay WKB with their curve types"
    );
    let rows: usize = reader.map(|batch| batch.unwrap().num_rows()).sum();
    assert_eq!(rows as u64, summary.rows);
}
