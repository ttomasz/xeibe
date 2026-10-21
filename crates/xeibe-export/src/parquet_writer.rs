use std::io::Write;
use std::num::NonZeroUsize;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;

use crate::geo_columns::GeoColumns;
use crate::{Result, Summary};

/// Which geometry columns get a GeoParquet 1.1 `bbox` covering column, a
/// struct of `xmin`, `ymin`, `xmax`, `ymax` after the column. Readers use it
/// to skip the parts of the file outside an area.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BboxColumn {
    /// Every geometry column but point columns, whose box would repeat the point.
    #[default]
    Auto,
    /// Every geometry column, point columns too.
    Always,
    /// No bounding-box columns.
    Never,
}

#[derive(Debug, Clone)]
pub struct ParquetOptions {
    pub bbox_column: BboxColumn,
    /// Rows per row group. The writer keeps a row group in memory until it
    /// is complete, so memory grows with this number times the size of a row.
    pub row_group_size: NonZeroUsize,
    /// The primary geometry column (the `primary` geometry read option); else
    /// the first geometry column.
    pub primary: Option<String>,
}

impl Default for ParquetOptions {
    fn default() -> Self {
        ParquetOptions {
            bbox_column: BboxColumn::Auto,
            row_group_size: NonZeroUsize::new(128_000).expect("not zero"),
            primary: None,
        }
    }
}

/// Parquet, ZSTD-compressed: every geometry column as WKB with the Parquet
/// `GEOMETRY` logical type and GeoParquet 1.1 `geo` metadata. Curves are an
/// error ([`crate::Error::Curve`]).
pub struct ParquetWriter<W: Write + Send> {
    schema: SchemaRef,
    options: ParquetOptions,
    metadata: Vec<(String, String)>,
    /// The sink until the first batch: it tells `BboxColumn::Auto` which WKB
    /// columns hold points.
    sink: Option<W>,
    open: Option<(ArrowWriter<W>, GeoColumns)>,
    rows: u64,
}

impl<W: Write + Send> ParquetWriter<W> {
    /// `metadata`: key-value pairs for the file's metadata, such as the
    /// read's settings (`gml:settings`), after GeoParquet's `geo`.
    pub fn new(
        sink: W,
        schema: SchemaRef,
        options: ParquetOptions,
        metadata: Vec<(String, String)>,
    ) -> Self {
        ParquetWriter {
            schema,
            options,
            metadata,
            sink: Some(sink),
            open: None,
            rows: 0,
        }
    }

    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        let (writer, geo) = self.open(Some(batch))?;
        writer.write(&geo.convert(batch)?)?;
        self.rows += batch.num_rows() as u64;
        Ok(())
    }

    pub fn finish(mut self) -> Result<Summary> {
        self.open(None)?;
        let (mut writer, geo) = self.open.take().expect("opened");
        if let Some(metadata) = geo.metadata() {
            writer.append_key_value_metadata(KeyValue::new("geo".to_string(), metadata));
        }
        for (key, value) in self.metadata {
            writer.append_key_value_metadata(KeyValue::new(key, value));
        }
        writer.close()?;
        Ok(Summary {
            rows: self.rows,
            warnings: geo.warnings,
        })
    }

    fn open(&mut self, first: Option<&RecordBatch>) -> Result<&mut (ArrowWriter<W>, GeoColumns)> {
        if self.open.is_none() {
            let geo = GeoColumns::new(
                &self.schema,
                self.options.primary.as_deref(),
                self.options.bbox_column,
                first,
            )?;
            let properties = WriterProperties::builder()
                .set_compression(Compression::ZSTD(ZstdLevel::default()))
                .set_max_row_group_row_count(Some(self.options.row_group_size.get()))
                .build();
            let sink = self.sink.take().expect("a sink until the writer opens");
            let writer = ArrowWriter::try_new(sink, geo.schema(), Some(properties))?;
            self.open = Some((writer, geo));
        }
        Ok(self.open.as_mut().expect("opened"))
    }
}
