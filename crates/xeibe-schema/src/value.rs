use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::Merge;

bitflags::bitflags! {
    /// Types a value can be parsed as. `STRING` is always set.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct TypeSet: u16 {
        const BOOL = 1 << 0;
        const INT = 1 << 1;
        const FLOAT = 1 << 2;
        const DATE = 1 << 3;
        const DATETIME = 1 << 4;
        const TIME = 1 << 5;
        const STRING = 1 << 6;
    }
}

/// Candidate types of one text value at each losslessness level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidates {
    pub exact_text: TypeSet,
    pub exact_value: TypeSet,
    pub lossy: TypeSet,
}

/// Classify a single value (leading-zero rule, f64 round-trip, temporal forms).
pub fn classify(value: &str) -> Candidates {
    todo!()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueStats {
    pub count: u64,
    /// ∩ over values: parses and prints back identically.
    pub exact_text: TypeSet,
    /// ∩ over values: the value survives a round trip through the Arrow type.
    pub exact_value: TypeSet,
    /// ∩ over values: parses at all.
    pub lossy: TypeSet,
    pub int_range: Option<(i64, i64)>,
    pub float_shape: Option<FloatShape>,
    pub temporal: Option<TemporalShape>,
    pub max_len: u32,
    pub distinct: BoundedSet,
}

impl Default for ValueStats {
    fn default() -> Self {
        todo!()
    }
}

impl ValueStats {
    /// Update with one value. Skips parsing once only `STRING` remains.
    pub fn observe(&mut self, value: &str) {
        todo!()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatShape {
    pub max_significant_digits: u8,
    /// Largest number of fractional digits seen → `gml:max_scale`.
    pub max_scale: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TzShape {
    #[default]
    Absent,
    /// Offset in minutes, the same for every value.
    Fixed(i16),
    /// Different offsets (e.g. summer/winter time).
    Mixed,
    /// Some values with a time zone, some without → string.
    Inconsistent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporalShape {
    pub tz: TzShape,
    pub max_fraction_digits: u8,
}

/// Up to `capacity` distinct values; cleared once it overflows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoundedSet {
    pub values: Vec<Arc<str>>,
    pub overflowed: bool,
    pub capacity: u16,
}

impl BoundedSet {
    pub fn insert(&mut self, value: &str) {
        todo!()
    }
}

impl Merge for ValueStats {
    fn merge(&mut self, other: Self) {
        todo!()
    }
}

impl Merge for BoundedSet {
    fn merge(&mut self, other: Self) {
        todo!()
    }
}
