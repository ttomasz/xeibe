//! Column builders driven by the read's schema.
//!
//! A feature is first read into a [`Value`] row (so that a feature that fails
//! half-way can be skipped without touching the builders), then appended.

use std::sync::Arc;

use arrow_array::builder::{
    ArrayBuilder, BooleanBuilder, Date32Builder, Date64Builder, Float32Builder, Float64Builder,
    Int8Builder, Int16Builder, Int32Builder, Int64Builder, LargeStringBuilder,
    NullBuilder,
    StringBuilder, StringViewBuilder, Time32MillisecondBuilder, Time32SecondBuilder,
    Time64MicrosecondBuilder, Time64NanosecondBuilder, TimestampMicrosecondBuilder,
    TimestampMillisecondBuilder, TimestampNanosecondBuilder, TimestampSecondBuilder,
    UInt8Builder, UInt16Builder, UInt32Builder, UInt64Builder,
};
use arrow_array::{
    ArrayRef, LargeListArray, LargeStringArray, ListArray, MapArray, RecordBatch,
    RecordBatchOptions, StringArray, StringViewArray, StructArray,
};
use arrow_buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow_schema::{ArrowError, DataType, FieldRef, SchemaRef, TimeUnit};
use xeibe_geom::Geometry;
use xeibe_geom::model::Envelope;

use crate::geometry_column::{BoxColumnBuilder, GeometryColumnBuilder};
use crate::value::{Scalar, parse_scalar};

/// One field's value in a row being read.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Scalar(Scalar),
    /// Already in the column's form (`GeometrySpec::prepare`).
    Geometry(Geometry),
    Box(Envelope),
    List(Vec<Value>),
    Map(Vec<(String, String)>),
}

/// Builds one layer's batches.
pub struct LayerBatchBuilder {
    schema: SchemaRef,
    columns: Vec<ColumnBuilder>,
    rows: usize,
}

/// A builder for one column (or a list column's items).
pub enum ColumnBuilder {
    Scalar(ScalarBuilder),
    List {
        /// The item field.
        item_field: FieldRef,
        large: bool,
        item: Box<ColumnBuilder>,
        offsets: Vec<i64>,
        validity: Vec<bool>,
    },
    Map {
        /// The `entries` field.
        entries: FieldRef,
        sorted: bool,
        keys: Vec<String>,
        values: Vec<String>,
        offsets: Vec<i32>,
        validity: Vec<bool>,
    },
    Geometry(GeometryColumnBuilder),
    Box(BoxColumnBuilder),
}

/// Typed scalar builder; parses text according to the target type.
pub struct ScalarBuilder {
    data_type: DataType,
    inner: Inner,
}

enum Inner {
    Null(NullBuilder),
    Bool(BooleanBuilder),
    Int8(Int8Builder),
    Int16(Int16Builder),
    Int32(Int32Builder),
    Int64(Int64Builder),
    UInt8(UInt8Builder),
    UInt16(UInt16Builder),
    UInt32(UInt32Builder),
    UInt64(UInt64Builder),
    Float32(Float32Builder),
    Float64(Float64Builder),
    Utf8(StringBuilder),
    LargeUtf8(LargeStringBuilder),
    Utf8View(StringViewBuilder),
    Date32(Date32Builder),
    Date64(Date64Builder),
    TimestampSecond(TimestampSecondBuilder),
    TimestampMillisecond(TimestampMillisecondBuilder),
    TimestampMicrosecond(TimestampMicrosecondBuilder),
    TimestampNanosecond(TimestampNanosecondBuilder),
    Time32Second(Time32SecondBuilder),
    Time32Millisecond(Time32MillisecondBuilder),
    Time64Microsecond(Time64MicrosecondBuilder),
    Time64Nanosecond(Time64NanosecondBuilder),
}

/// Run `$body` with `$b` bound to the builder, whatever its type.
macro_rules! each_builder {
    ($inner:expr, $b:ident => $body:expr) => {
        match $inner {
            Inner::Null($b) => $body,
            Inner::Bool($b) => $body,
            Inner::Int8($b) => $body,
            Inner::Int16($b) => $body,
            Inner::Int32($b) => $body,
            Inner::Int64($b) => $body,
            Inner::UInt8($b) => $body,
            Inner::UInt16($b) => $body,
            Inner::UInt32($b) => $body,
            Inner::UInt64($b) => $body,
            Inner::Float32($b) => $body,
            Inner::Float64($b) => $body,
            Inner::Utf8($b) => $body,
            Inner::LargeUtf8($b) => $body,
            Inner::Utf8View($b) => $body,
            Inner::Date32($b) => $body,
            Inner::Date64($b) => $body,
            Inner::TimestampSecond($b) => $body,
            Inner::TimestampMillisecond($b) => $body,
            Inner::TimestampMicrosecond($b) => $body,
            Inner::TimestampNanosecond($b) => $body,
            Inner::Time32Second($b) => $body,
            Inner::Time32Millisecond($b) => $body,
            Inner::Time64Microsecond($b) => $body,
            Inner::Time64Nanosecond($b) => $body,
        }
    };
}

impl ScalarBuilder {
    /// Panics on a type text can't be parsed into; [`is_scalar_type`] tells.
    pub fn new(data_type: DataType, capacity: usize) -> Self {
        let tz = |dt: &DataType| match dt {
            DataType::Timestamp(_, tz) => tz.clone(),
            _ => None,
        };
        let inner = match &data_type {
            DataType::Null => Inner::Null(NullBuilder::new()),
            DataType::Boolean => Inner::Bool(BooleanBuilder::with_capacity(capacity)),
            DataType::Int8 => Inner::Int8(Int8Builder::with_capacity(capacity)),
            DataType::Int16 => Inner::Int16(Int16Builder::with_capacity(capacity)),
            DataType::Int32 => Inner::Int32(Int32Builder::with_capacity(capacity)),
            DataType::Int64 => Inner::Int64(Int64Builder::with_capacity(capacity)),
            DataType::UInt8 => Inner::UInt8(UInt8Builder::with_capacity(capacity)),
            DataType::UInt16 => Inner::UInt16(UInt16Builder::with_capacity(capacity)),
            DataType::UInt32 => Inner::UInt32(UInt32Builder::with_capacity(capacity)),
            DataType::UInt64 => Inner::UInt64(UInt64Builder::with_capacity(capacity)),
            DataType::Float32 => Inner::Float32(Float32Builder::with_capacity(capacity)),
            DataType::Float64 => Inner::Float64(Float64Builder::with_capacity(capacity)),
            DataType::Utf8 => Inner::Utf8(StringBuilder::with_capacity(capacity, capacity * 8)),
            DataType::LargeUtf8 => Inner::LargeUtf8(LargeStringBuilder::with_capacity(capacity, capacity * 8)),
            DataType::Utf8View => Inner::Utf8View(StringViewBuilder::with_capacity(capacity)),
            DataType::Date32 => Inner::Date32(Date32Builder::with_capacity(capacity)),
            DataType::Date64 => Inner::Date64(Date64Builder::with_capacity(capacity)),
            DataType::Timestamp(TimeUnit::Second, _) => Inner::TimestampSecond(
                TimestampSecondBuilder::with_capacity(capacity).with_timezone_opt(tz(&data_type)),
            ),
            DataType::Timestamp(TimeUnit::Millisecond, _) => Inner::TimestampMillisecond(
                TimestampMillisecondBuilder::with_capacity(capacity).with_timezone_opt(tz(&data_type)),
            ),
            DataType::Timestamp(TimeUnit::Microsecond, _) => Inner::TimestampMicrosecond(
                TimestampMicrosecondBuilder::with_capacity(capacity).with_timezone_opt(tz(&data_type)),
            ),
            DataType::Timestamp(TimeUnit::Nanosecond, _) => Inner::TimestampNanosecond(
                TimestampNanosecondBuilder::with_capacity(capacity).with_timezone_opt(tz(&data_type)),
            ),
            DataType::Time32(TimeUnit::Second) => Inner::Time32Second(Time32SecondBuilder::with_capacity(capacity)),
            DataType::Time32(TimeUnit::Millisecond) => {
                Inner::Time32Millisecond(Time32MillisecondBuilder::with_capacity(capacity))
            }
            DataType::Time64(TimeUnit::Microsecond) => {
                Inner::Time64Microsecond(Time64MicrosecondBuilder::with_capacity(capacity))
            }
            DataType::Time64(TimeUnit::Nanosecond) => {
                Inner::Time64Nanosecond(Time64NanosecondBuilder::with_capacity(capacity))
            }
            other => panic!("{other} is not a scalar type text can be parsed into"),
        };
        ScalarBuilder { data_type, inner }
    }

    pub fn data_type(&self) -> &DataType {
        &self.data_type
    }

    pub fn len(&self) -> usize {
        each_builder!(&self.inner, b => b.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Parse and append; returns `Err(text)` if the value doesn't fit the type.
    pub fn append_text(&mut self, text: &str) -> Result<(), String> {
        match parse_scalar(&self.data_type, text) {
            Some(scalar) => {
                self.append(scalar);
                Ok(())
            }
            None => Err(text.to_string()),
        }
    }

    /// Append a value parsed for this builder's type ([`parse_scalar`]).
    /// A value of another physical type appends a null.
    pub fn append(&mut self, scalar: Scalar) {
        match (&mut self.inner, scalar) {
            (Inner::Bool(b), Scalar::Bool(v)) => b.append_value(v),
            (Inner::Int8(b), Scalar::Int(v)) => b.append_value(v as i8),
            (Inner::Int16(b), Scalar::Int(v)) => b.append_value(v as i16),
            (Inner::Int32(b), Scalar::Int(v)) => b.append_value(v as i32),
            (Inner::Int64(b), Scalar::Int(v)) => b.append_value(v),
            (Inner::UInt8(b), Scalar::UInt(v)) => b.append_value(v as u8),
            (Inner::UInt16(b), Scalar::UInt(v)) => b.append_value(v as u16),
            (Inner::UInt32(b), Scalar::UInt(v)) => b.append_value(v as u32),
            (Inner::UInt64(b), Scalar::UInt(v)) => b.append_value(v),
            (Inner::Float32(b), Scalar::Float(v)) => b.append_value(v as f32),
            (Inner::Float64(b), Scalar::Float(v)) => b.append_value(v),
            (Inner::Utf8(b), Scalar::Str(v)) => b.append_value(v),
            (Inner::LargeUtf8(b), Scalar::Str(v)) => b.append_value(v),
            (Inner::Utf8View(b), Scalar::Str(v)) => b.append_value(v),
            (Inner::Date32(b), Scalar::Int32(v)) => b.append_value(v),
            (Inner::Date64(b), Scalar::Int(v)) => b.append_value(v),
            (Inner::TimestampSecond(b), Scalar::Int(v)) => b.append_value(v),
            (Inner::TimestampMillisecond(b), Scalar::Int(v)) => b.append_value(v),
            (Inner::TimestampMicrosecond(b), Scalar::Int(v)) => b.append_value(v),
            (Inner::TimestampNanosecond(b), Scalar::Int(v)) => b.append_value(v),
            (Inner::Time32Second(b), Scalar::Int32(v)) => b.append_value(v),
            (Inner::Time32Millisecond(b), Scalar::Int32(v)) => b.append_value(v),
            (Inner::Time64Microsecond(b), Scalar::Int(v)) => b.append_value(v),
            (Inner::Time64Nanosecond(b), Scalar::Int(v)) => b.append_value(v),
            _ => self.append_null(),
        }
    }

    pub fn append_null(&mut self) {
        match &mut self.inner {
            Inner::Null(b) => b.append_null(),
            Inner::Bool(b) => b.append_null(),
            Inner::Int8(b) => b.append_null(),
            Inner::Int16(b) => b.append_null(),
            Inner::Int32(b) => b.append_null(),
            Inner::Int64(b) => b.append_null(),
            Inner::UInt8(b) => b.append_null(),
            Inner::UInt16(b) => b.append_null(),
            Inner::UInt32(b) => b.append_null(),
            Inner::UInt64(b) => b.append_null(),
            Inner::Float32(b) => b.append_null(),
            Inner::Float64(b) => b.append_null(),
            Inner::Utf8(b) => b.append_null(),
            Inner::LargeUtf8(b) => b.append_null(),
            Inner::Utf8View(b) => b.append_null(),
            Inner::Date32(b) => b.append_null(),
            Inner::Date64(b) => b.append_null(),
            Inner::TimestampSecond(b) => b.append_null(),
            Inner::TimestampMillisecond(b) => b.append_null(),
            Inner::TimestampMicrosecond(b) => b.append_null(),
            Inner::TimestampNanosecond(b) => b.append_null(),
            Inner::Time32Second(b) => b.append_null(),
            Inner::Time32Millisecond(b) => b.append_null(),
            Inner::Time64Microsecond(b) => b.append_null(),
            Inner::Time64Nanosecond(b) => b.append_null(),
        }
    }

    pub fn finish(&mut self) -> ArrayRef {
        each_builder!(&mut self.inner, b => Arc::new(b.finish()) as ArrayRef)
    }
}

/// Types [`ScalarBuilder`] builds.
pub fn is_scalar_type(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Null
            | DataType::Boolean
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float32
            | DataType::Float64
            | DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Utf8View
            | DataType::Date32
            | DataType::Date64
            | DataType::Timestamp(..)
            | DataType::Time32(TimeUnit::Second | TimeUnit::Millisecond)
            | DataType::Time64(TimeUnit::Microsecond | TimeUnit::Nanosecond)
    )
}

impl ColumnBuilder {
    /// The builder for `field`: GeoArrow extension types first, then by data type.
    pub fn for_field(field: &arrow_schema::Field, capacity: usize) -> crate::Result<Self> {
        match field.metadata().get("ARROW:extension:name").map(String::as_str) {
            Some("geoarrow.box") => return Ok(ColumnBuilder::Box(BoxColumnBuilder::for_field(field)?)),
            Some(name) if name.starts_with("geoarrow.") => {
                return Ok(ColumnBuilder::Geometry(GeometryColumnBuilder::for_field(field, capacity)?));
            }
            _ => {}
        }
        Ok(match field.data_type() {
            // `bytea`: the geometry as WKB, without a GeoArrow type.
            DataType::Binary => ColumnBuilder::Geometry(GeometryColumnBuilder::plain_wkb(capacity)),
            DataType::List(item) | DataType::LargeList(item) => ColumnBuilder::List {
                item_field: item.clone(),
                large: matches!(field.data_type(), DataType::LargeList(_)),
                item: Box::new(ColumnBuilder::for_field(item, capacity)?),
                offsets: vec![0],
                validity: Vec::with_capacity(capacity),
            },
            DataType::Map(entries, sorted) => ColumnBuilder::Map {
                entries: entries.clone(),
                sorted: *sorted,
                keys: Vec::new(),
                values: Vec::new(),
                offsets: vec![0],
                validity: Vec::with_capacity(capacity),
            },
            data_type if is_scalar_type(data_type) => {
                ColumnBuilder::Scalar(ScalarBuilder::new(data_type.clone(), capacity))
            }
            other => {
                return Err(ArrowError::NotYetImplemented(format!(
                    "column {}: {other} can't be filled from XML",
                    field.name()
                ))
                .into());
            }
        })
    }

    /// Values appended so far.
    pub fn len(&self) -> usize {
        match self {
            ColumnBuilder::Scalar(b) => b.len(),
            ColumnBuilder::List { validity, .. }
            | ColumnBuilder::Map { validity, .. } => validity.len(),
            ColumnBuilder::Geometry(b) => b.len(),
            ColumnBuilder::Box(b) => b.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append one value; `Value::Null` works for every column.
    pub fn append(&mut self, value: Value) -> crate::Result<()> {
        match (self, value) {
            (ColumnBuilder::Scalar(b), Value::Scalar(scalar)) => b.append(scalar),
            (ColumnBuilder::Scalar(b), _) => b.append_null(),
            (ColumnBuilder::List { item, offsets, validity, .. }, Value::List(items)) => {
                let count = items.len() as i64;
                for value in items {
                    item.append(value)?;
                }
                offsets.push(offsets.last().copied().unwrap_or(0) + count);
                validity.push(true);
            }
            (ColumnBuilder::List { offsets, validity, .. }, _) => {
                offsets.push(offsets.last().copied().unwrap_or(0));
                validity.push(false);
            }
            (ColumnBuilder::Map { keys, values, offsets, validity, .. }, Value::Map(pairs)) => {
                let count = pairs.len() as i32;
                for (key, value) in pairs {
                    keys.push(key);
                    values.push(value);
                }
                offsets.push(offsets.last().copied().unwrap_or(0) + count);
                validity.push(true);
            }
            (ColumnBuilder::Map { offsets, validity, .. }, _) => {
                offsets.push(offsets.last().copied().unwrap_or(0));
                validity.push(false);
            }
            (ColumnBuilder::Geometry(b), Value::Geometry(geometry)) => b.push(Some(&geometry))?,
            (ColumnBuilder::Geometry(b), _) => b.push(None)?,
            (ColumnBuilder::Box(b), Value::Box(envelope)) => b.push(Some(&envelope)),
            (ColumnBuilder::Box(b), _) => b.push(None),
        }
        Ok(())
    }

    /// The column so far; the builder starts over.
    pub fn finish(&mut self) -> crate::Result<ArrayRef> {
        Ok(match self {
            ColumnBuilder::Scalar(b) => b.finish(),
            ColumnBuilder::List { item_field, large, item, offsets, validity } => {
                let values = item.finish()?;
                let nulls = nulls(std::mem::take(validity));
                let offsets = std::mem::replace(offsets, vec![0]);
                if *large {
                    let offsets = OffsetBuffer::new(ScalarBuffer::from(offsets));
                    Arc::new(LargeListArray::try_new(item_field.clone(), offsets, values, nulls)?)
                } else {
                    let offsets: Vec<i32> = offsets.into_iter().map(|o| o as i32).collect();
                    let offsets = OffsetBuffer::new(ScalarBuffer::from(offsets));
                    Arc::new(ListArray::try_new(item_field.clone(), offsets, values, nulls)?)
                }
            }
            ColumnBuilder::Map { entries, sorted, keys, values, offsets, validity } => {
                let DataType::Struct(entry_fields) = entries.data_type() else {
                    return Err(ArrowError::InvalidArgumentError("map entries must be a struct".into()).into());
                };
                let key_type = entry_fields[0].data_type();
                let value_type = entry_fields[1].data_type();
                let key_array = string_array(key_type, std::mem::take(keys))?;
                let value_array = string_array(value_type, std::mem::take(values))?;
                let entries_array = StructArray::try_new(entry_fields.clone(), vec![key_array, value_array], None)?;
                let offsets = OffsetBuffer::new(ScalarBuffer::from(std::mem::replace(offsets, vec![0])));
                let nulls = nulls(std::mem::take(validity));
                Arc::new(MapArray::try_new(entries.clone(), offsets, entries_array, nulls, *sorted)?)
            }
            ColumnBuilder::Geometry(b) => b.finish(),
            ColumnBuilder::Box(b) => b.finish()?,
        })
    }
}

/// A validity buffer, or none if every value is valid.
fn nulls(validity: Vec<bool>) -> Option<NullBuffer> {
    if validity.iter().all(|valid| *valid) {
        return None;
    }
    Some(NullBuffer::from(validity))
}

/// A string array of the map's key or value type.
fn string_array(data_type: &DataType, values: Vec<String>) -> crate::Result<ArrayRef> {
    Ok(match data_type {
        DataType::Utf8View => Arc::new(StringViewArray::from_iter_values(values)),
        DataType::Utf8 => Arc::new(StringArray::from_iter_values(values)),
        DataType::LargeUtf8 => Arc::new(LargeStringArray::from_iter_values(values)),
        other => {
            return Err(ArrowError::InvalidArgumentError(format!(
                "map keys and values must be strings, not {other}"
            ))
            .into());
        }
    })
}

impl LayerBatchBuilder {
    pub fn new(schema: SchemaRef, capacity: usize) -> crate::Result<Self> {
        let columns = schema
            .fields()
            .iter()
            .map(|field| ColumnBuilder::for_field(field, capacity))
            .collect::<crate::Result<_>>()?;
        Ok(LayerBatchBuilder { schema, columns, rows: 0 })
    }

    pub fn len(&self) -> usize {
        self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    /// Append one row, one value per top-level column.
    pub fn append_row(&mut self, row: Vec<Value>) -> crate::Result<()> {
        let mut values = row.into_iter();
        for column in &mut self.columns {
            column.append(values.next().unwrap_or(Value::Null))?;
        }
        self.rows += 1;
        Ok(())
    }

    pub fn finish(&mut self) -> crate::Result<RecordBatch> {
        let columns = self
            .columns
            .iter_mut()
            .map(ColumnBuilder::finish)
            .collect::<crate::Result<Vec<_>>>()?;
        let options = RecordBatchOptions::new().with_row_count(Some(self.rows));
        self.rows = 0;
        Ok(RecordBatch::try_new_with_options(self.schema.clone(), columns, &options)?)
    }
}
