//! `read_gml(path, layer [, 'key=value' ...])`.

use datafusion::arrow::datatypes::DataType;
use xeibe_arrow::ReadOptions;
use xeibe_core::Source;
use xeibe_schema::ScanExtent;

use crate::support::{POINTS, PRG, context, count, file_url, path, query, rows, strings, temp_dir};

#[tokio::test(flavor = "multi_thread")]
async fn read_gml_counts_the_features_of_one_layer() {
    let ctx = context();
    let sql = format!("select count(*) from read_gml('{}', '{POINTS}')", path(PRG));
    assert_eq!(count(&ctx, &sql).await, 2);
    let sql = format!("select count(*) from read_gml('{}', 'AD_UlicaPlac')", path(PRG));
    assert_eq!(count(&ctx, &sql).await, 2);
}

#[tokio::test]
async fn read_gml_works_on_a_current_thread_runtime() {
    let ctx = context();
    let sql = format!("select count(*) from read_gml('{}', '{POINTS}')", path(PRG));
    assert_eq!(count(&ctx, &sql).await, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn read_gml_projects_and_filters() {
    let ctx = context();
    let sql = format!(
        r#"select "kodPocztowy", georeferencja from read_gml('{}', '{POINTS}')
           where "numerPorzadkowy" = 43"#,
        path(PRG)
    );
    let batches = query(&ctx, &sql).await;
    assert_eq!(strings(&batches, "kodPocztowy"), [Some("68-213".into())]);
    let geometry = batches[0].schema().field_with_name("georeferencja").unwrap().clone();
    assert!(
        geometry.metadata().get("ARROW:extension:name").is_some_and(|name| name.starts_with("geoarrow.")),
        "{geometry:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn read_gml_takes_a_file_url_and_an_array_of_paths() {
    let ctx = context();
    let sql = format!("select count(*) from read_gml('{}', '{POINTS}')", file_url(PRG));
    assert_eq!(count(&ctx, &sql).await, 2);
    let sql = format!("select count(*) from read_gml(['{0}', '{0}'], '{POINTS}')", path(PRG));
    assert_eq!(count(&ctx, &sql).await, 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn read_gml_uses_the_schema_of_a_settings_file() {
    let ctx = context();
    let mut settings = xeibe_arrow::scan(Source::file(path(PRG)).unwrap(), ScanExtent::Full, &ReadOptions::default())
        .unwrap()
        .to_settings()
        .unwrap();
    // Keep only two columns of the layer: the rest goes to `_overflow`.
    let columns = settings.layers.values_mut().find(|columns| columns.contains_key("kodPocztowy")).unwrap();
    columns.retain(|name, _| name == "kodPocztowy" || name == "georeferencja");
    let file = temp_dir("settings").join("prg.gml.json");
    settings.save(&file).unwrap();

    for sql in [
        format!("select * from read_gml('{}', '{POINTS}', '{}')", path(PRG), file.display()),
        format!("select * from read_gml('{}', '{POINTS}', 'settings={}')", path(PRG), file.display()),
    ] {
        let batches = query(&ctx, &sql).await;
        assert_eq!(rows(&batches), 2);
        let schema = batches[0].schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, ["georeferencja", "kodPocztowy", "_overflow"]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn read_gml_applies_a_preset() {
    let ctx = context();
    let sql = format!("select * from read_gml('{}', '{POINTS}', 'preset=strings')", path(PRG));
    let df = ctx.sql(&sql).await.unwrap();
    let field = df.schema().field_with_unqualified_name("numerPorzadkowy").unwrap();
    assert_eq!(field.data_type(), &DataType::Utf8View);
}

#[tokio::test(flavor = "multi_thread")]
async fn read_gml_rejects_bad_arguments() {
    let ctx = context();
    for (sql, message) in [
        (format!("select * from read_gml('{}')", path(PRG)), "needs a path and a layer"),
        (format!("select * from read_gml('{}', '{POINTS}', 'colour=red')", path(PRG)), "unknown option"),
        (format!("select * from read_gml('{}', '{POINTS}', 'preset=fancy')", path(PRG)), "invalid preset"),
        (format!("select * from read_gml('{}', 'NoSuchLayer')", path(PRG)), "NoSuchLayer"),
    ] {
        let error = match ctx.sql(&sql).await {
            Ok(df) => df.collect().await.expect_err(&sql).to_string(),
            Err(error) => error.to_string(),
        };
        assert!(error.contains(message), "{sql}: {error}");
    }
}
