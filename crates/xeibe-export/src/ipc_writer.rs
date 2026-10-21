use std::io::Write;

use arrow_array::RecordBatch;
use arrow_ipc::writer::FileWriter;
use arrow_schema::Schema;

use crate::{Result, Summary};

/// Arrow IPC file: batches as read, GeoArrow types (curves included) as they are.
pub struct IpcWriter<W: Write> {
    writer: FileWriter<W>,
    rows: u64,
}

impl<W: Write> IpcWriter<W> {
    /// `metadata`: key-value pairs for the file's footer, such as the read's
    /// settings (`gml:settings`).
    pub fn new(sink: W, schema: &Schema, metadata: Vec<(String, String)>) -> Result<Self> {
        let mut writer = FileWriter::try_new(sink, schema)?;
        for (key, value) in metadata {
            writer.write_metadata(key, value);
        }
        Ok(IpcWriter { writer, rows: 0 })
    }

    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        self.writer.write(batch)?;
        self.rows += batch.num_rows() as u64;
        Ok(())
    }

    pub fn finish(mut self) -> Result<Summary> {
        self.writer.finish()?;
        Ok(Summary {
            rows: self.rows,
            warnings: Vec::new(),
        })
    }
}
