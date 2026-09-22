//! `GmlTable`: one layer as a table (`docs/architecture.md`, "DataFusion").

use std::sync::Arc;

use datafusion::prelude::SessionContext;
use xeibe_arrow::ReadOptions;
use xeibe_core::Source;
use xeibe_datafusion::GmlTable;
use xeibe_schema::ScanExtent;

use crate::support::{POINTS, PRG, context, count, file_url, path, query, rows, strings};

async fn register(ctx: &SessionContext, inputs: Vec<String>) {
    let table = GmlTable::try_new(&ctx.state(), inputs, POINTS, None, ReadOptions::default())
        .await
        .expect("the layer is sampled");
    ctx.register_table("points", Arc::new(table)).expect("the table registers");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_layer_with_a_sampled_schema_counts_its_features() {
    let ctx = context();
    register(&ctx, vec![path(PRG)]).await;
    assert_eq!(count(&ctx, "select count(*) from points").await, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_projection_returns_only_the_columns_selected() {
    let ctx = context();
    register(&ctx, vec![path(PRG)]).await;
    let batches = query(&ctx, r#"select "kodPocztowy", "@id" from points"#).await;
    assert_eq!(rows(&batches), 2);
    assert_eq!(batches[0].num_columns(), 2);
    assert_eq!(batches[0].schema().field(0).name(), "kodPocztowy");
    assert_eq!(batches[0].schema().field(1).name(), "@id");
    assert_eq!(strings(&batches, "kodPocztowy"), [Some("68-213".into()), Some("68-213".into())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filter_is_applied_by_datafusion() {
    let ctx = context();
    register(&ctx, vec![path(PRG)]).await;
    let batches = query(
        &ctx,
        r#"select "numerPorzadkowy" from points where "@id" like '%f244dc6c%'"#,
    )
    .await;
    assert_eq!(strings(&batches, "numerPorzadkowy"), [Some("48".into())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_geometry_column_keeps_its_geoarrow_metadata() {
    let ctx = context();
    register(&ctx, vec![path(PRG)]).await;
    let df = ctx.sql("select georeferencja from points").await.unwrap();
    let field = df.schema().field_with_unqualified_name("georeferencja").unwrap().clone();
    let extension = field.metadata().get("ARROW:extension:name").cloned();
    assert!(
        extension.as_deref().is_some_and(|name| name.starts_with("geoarrow.")),
        "a GeoArrow extension type, got {extension:?}"
    );
    assert!(
        field.metadata().get("ARROW:extension:metadata").is_some_and(|m| m.contains("crs")),
        "the CRS is in the extension metadata: {:?}",
        field.metadata()
    );

    let batches = df.collect().await.unwrap();
    assert_eq!(rows(&batches), 2);
    let column = batches[0].schema().field(0).clone();
    assert_eq!(column.metadata().get("ARROW:extension:name"), extension.as_ref());
    assert_eq!(batches[0].column(0).null_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_limit_stops_early() {
    let ctx = context();
    register(&ctx, vec![path(PRG)]).await;
    assert_eq!(rows(&query(&ctx, "select * from points limit 1").await), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_given_schema_is_used_as_it_is() {
    let ctx = context();
    let scan = xeibe_arrow::scan(
        Source::file(path(PRG)).unwrap(),
        ScanExtent::Full,
        &ReadOptions::default(),
    )
    .unwrap();
    let schema = scan.arrow_schema(POINTS).unwrap();
    let table = GmlTable::try_new(&ctx.state(), vec![path(PRG)], POINTS, Some(schema.clone()), ReadOptions::default())
        .await
        .unwrap();
    ctx.register_table("points", Arc::new(table)).unwrap();

    let df = ctx.table("points").await.unwrap();
    let names: Vec<_> = df.schema().fields().iter().map(|f| f.name().clone()).collect();
    let mut expected: Vec<_> = schema.fields().iter().map(|f| f.name().clone()).collect();
    expected.push("_overflow".into());
    assert_eq!(names, expected, "the read adds `_overflow` to a given schema");
    assert_eq!(rows(&df.collect().await.unwrap()), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_source_is_one_partition() {
    let ctx = context();
    register(&ctx, vec![path(PRG), path(PRG)]).await;
    let plan = ctx.sql("select * from points").await.unwrap().create_physical_plan().await.unwrap();
    let text = datafusion::physical_plan::displayable(plan.as_ref()).indent(true).to_string();
    assert!(text.contains("StreamingTableExec: partition_sizes=2"), "{text}");
    assert_eq!(count(&ctx, "select count(*) from points").await, 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_urls_are_read_through_the_object_store_registry() {
    let ctx = context();
    register(&ctx, vec![file_url(PRG)]).await;
    assert_eq!(count(&ctx, "select count(*) from points").await, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_object_store_glob_lists_its_matches() {
    let ctx = context();
    register(&ctx, vec![file_url("samples/pl/prg-*.gml")]).await;
    let batches = query(&ctx, r#"select "kodPocztowy" from points"#).await;
    assert_eq!(rows(&batches), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_layer_fails_when_the_table_is_created() {
    let ctx = context();
    let error = GmlTable::try_new(&ctx.state(), vec![path(PRG)], "NoSuchLayer", None, ReadOptions::default())
        .await
        .expect_err("no such layer");
    assert!(error.to_string().contains("NoSuchLayer"), "{error}");
}
