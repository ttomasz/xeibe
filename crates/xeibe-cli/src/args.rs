use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "xeibe",
    version,
    about = "Convert GML and WFS data to Arrow / Parquet"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List layers with their schemas; optionally save them as a settings file.
    Scan {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        read: ReadArgs,
        /// Scan only the first N features of the input (late layers may be missed).
        #[arg(long)]
        sample: Option<u64>,
        /// Print the reason and evidence for every field.
        #[arg(long)]
        explain: bool,
        /// Write the settings file (options + one schema per layer).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Convert one layer to Parquet or Arrow IPC.
    Convert {
        #[command(flatten)]
        input: InputArgs,
        #[command(flatten)]
        output: OutputArgs,
        #[command(flatten)]
        read: ReadArgs,
    },
    /// WFS operations.
    #[command(subcommand)]
    Wfs(WfsCommand),
}

// Parsed once per run: the size difference between variants doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum WfsCommand {
    /// List feature types from GetCapabilities.
    Layers { url: String },
    /// Count features (`resultType=hits`).
    Count {
        url: String,
        #[arg(long)]
        type_name: String,
    },
    /// Stream all pages of one feature type into Parquet or Arrow IPC.
    Convert {
        url: String,
        #[arg(long)]
        type_name: String,
        #[arg(long)]
        page_size: Option<u64>,
        #[arg(long)]
        srs_name: Option<String>,
        #[arg(long)]
        sort_by: Option<String>,
        /// Vendor parameter `key=value` (repeatable).
        #[arg(long = "param")]
        params: Vec<String>,
        #[command(flatten)]
        output: OutputArgs,
        #[command(flatten)]
        read: ReadArgs,
    },
}

#[derive(Debug, Args)]
pub struct InputArgs {
    /// Files, directories, globs, local zip archives (`a.zip!/member`), `-` for
    /// stdin, or http(s)/s3/gs/az URLs.
    #[arg(required = true)]
    pub inputs: Vec<String>,
    /// Zip members to read (glob, repeatable).
    #[arg(long)]
    pub member: Vec<String>,
}

#[derive(Debug, Args)]
pub struct OutputArgs {
    /// Layer (feature type) to convert. Required for `convert`.
    #[arg(long)]
    pub layer: Option<String>,
    #[arg(short, long)]
    pub output: PathBuf,
    #[arg(long, value_enum, default_value_t = OutputFormat::Parquet)]
    pub format: OutputFormat,
    /// A GeoParquet 1.1 `bbox` covering column per geometry column
    /// (`<column>_bbox`). Parquet only.
    #[arg(long, value_enum, default_value_t = BboxColumn::Auto)]
    pub bbox_column: BboxColumn,
}

/// Read parameters. They override the settings file's `options`.
#[derive(Debug, Args)]
pub struct ReadArgs {
    /// Settings file: options and/or layer schemas (from `xeibe scan -o`).
    /// Without a schema for the layer, the schema is sampled.
    #[arg(long)]
    pub settings: Option<PathBuf>,
    #[arg(long, value_enum)]
    pub preset: Option<Preset>,
    #[arg(long, value_enum)]
    pub axis_order: Option<AxisMode>,
    /// `srsName=mode`, e.g. `EPSG:4326=yx`, with the srsName exactly as
    /// written (repeatable). Only needed when one input mixes srsNames that
    /// must be read differently.
    #[arg(long = "axis-override")]
    pub axis_overrides: Vec<String>,
    #[arg(long)]
    pub crs: Option<String>,
    /// Lossy: linearize curves, splitting arcs into steps of at most STEP_DEG degrees
    /// (default 4, as GDAL). Required for Parquet output of columns with curves.
    #[arg(long, value_name = "STEP_DEG", num_args = 0..=1, default_missing_value = "4")]
    pub linearize: Option<f64>,
    /// With `--linearize`: largest distance between adjacent vertices (CRS units).
    #[arg(long, value_name = "LEN", requires = "linearize")]
    pub arc_max_gap: Option<f64>,
    /// Path pattern overrides for inferred schemas, e.g. `*/kod=text`
    /// (repeatable).
    #[arg(long = "override")]
    pub overrides: Vec<String>,
    /// Features sampled when there is no schema for the layer.
    #[arg(long)]
    pub sample_features: Option<u64>,
    #[arg(long)]
    pub threads: Option<usize>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Preset {
    /// Lossless by value, rich types.
    Default,
    /// Every scalar as text.
    Strings,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum AxisMode {
    /// x/y as written.
    XY,
    /// y/x: swap the first two ordinates.
    YX,
    /// The CRS's axis order, for every srsName form.
    Crs,
    /// Short/legacy srsName → x/y, URN/URI → CRS order (GDAL's default).
    CrsHeuristic,
    /// GML 2-style geometry elements x/y, GML 3-style by the CRS.
    GmlVersion,
    /// Evidence-based (default).
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BboxColumn {
    /// For every geometry column but point columns, where the bbox would
    /// repeat the coordinates.
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    /// Parquet with GeoParquet 1.1 metadata and the native GEOMETRY type.
    Parquet,
    Ipc,
}
