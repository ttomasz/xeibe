use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::LazyLock;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// `-V` / `--version`: the build facts that change what xeibe outputs.
fn version() -> &'static str {
    static TEXT: LazyLock<String> = LazyLock::new(|| {
        let mut features = Vec::new();
        if cfg!(feature = "object-store") {
            features.push("object-store");
        }
        let arrow =
            parquet::file::properties::DEFAULT_CREATED_BY.trim_start_matches("parquet-rs version ");
        let mut text = format!(
            "{}\nEPSG dataset {} ({})\nfeatures: {}\narrow/parquet {arrow}",
            env!("CARGO_PKG_VERSION"),
            xeibe_crs::EPSG_VERSION,
            xeibe_crs::EPSG_DATE,
            if features.is_empty() {
                "none".to_owned()
            } else {
                features.join(", ")
            },
        );
        if let Some(commit) = option_env!("XEIBE_GIT_COMMIT") {
            text += &format!("\ncommit {commit}");
        }
        text
    });
    &TEXT
}

/// Help headings that group the flags `gml scan`, `gml convert` and
/// `wfs convert` share.
const OUTPUT: &str = "Output format";
const INFERENCE: &str = "Schema inference";
const GEOMETRY: &str = "Geometry";

#[derive(Debug, Parser)]
#[command(
    name = "xeibe",
    version = version(),
    about = "Convert Geography Markup Language (GML) and Web Feature Service (WFS) data to Arrow / Parquet"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// GML files: list the layers, convert one to Parquet or Arrow IPC.
    #[command(subcommand)]
    Gml(GmlCommand),
    /// WFS services: list the feature types, count or convert one.
    #[command(subcommand)]
    Wfs(WfsCommand),
}

// Parsed once per run: the size difference between variants doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum GmlCommand {
    /// List the layers of the input with their inferred schemas
    ///
    /// Prints every layer (feature type) with its feature count, geometry
    /// columns, CRS and columns. With `--output/-o`, the result is also saved as
    /// a settings file; `xeibe gml convert --settings` can use this file to get
    /// settings for conversion.
    ///
    /// By default scan command reads the entire file(s). With `--sample` you can specify
    /// max number of features to read.
    ///
    /// Examples:
    ///   xeibe gml scan parcels.gml
    ///   xeibe gml scan 'data/**/*.gml' -o parcels.json
    ///   xeibe gml scan parcels.zip --sample 1000 --explain
    ///   xeibe gml scan parcels.zip!/member.xml --sample 1000 --explain
    #[command(verbatim_doc_comment)]
    Scan {
        #[command(flatten)]
        input: InputArgs,
        /// Scan only the first N features of the input
        ///
        /// A quick look at a large input: layers that start after the
        /// first N features are missing, and the feature counts are lower
        /// bounds. Without it, the whole input is read.
        #[arg(long, value_name = "N", verbatim_doc_comment)]
        sample: Option<u64>,
        /// For every column, print why it got its type and from which values.
        #[arg(long)]
        explain: bool,
        /// Save the scan as a settings file (JSON)
        ///
        /// It holds the options the scan ran with and one schema per layer:
        /// column names, types and paths. Edit it to rename columns or
        /// change types, then pass it to `xeibe gml convert --settings`.
        #[arg(short, long, value_name = "FILE", verbatim_doc_comment)]
        output: Option<PathBuf>,
        /// Take the scan's options from a settings file (the one exported by running `scan --output``)
        ///
        /// Only the file's `options` are used. Its `layers` (the schemas)
        /// are ignored, because the scan infers them anew. Use it to scan
        /// new data the way earlier data was scanned, or to set options
        /// that have no flag. A flag given here wins over the file.
        #[arg(long, value_name = "FILE", verbatim_doc_comment)]
        settings: Option<PathBuf>,
        #[command(flatten)]
        read: ReadArgs,
    },
    /// Convert one layer of the input to Parquet or Arrow IPC
    ///
    /// One run writes one layer to one file; run it once per layer.
    /// Without `--settings`, the layer's columns and their types are
    /// inferred from its first features (`--sample-features`).
    ///
    /// Examples:
    ///   xeibe gml convert parcels.gml --layer Parcel -o parcels.parquet
    ///   xeibe gml convert 'data/*.gml' --layer Parcel -o parcels.parquet \
    ///       --settings parcels.json
    ///   xeibe gml convert roads.gml --layer Road -o roads.parquet --linearize
    #[command(verbatim_doc_comment)]
    Convert {
        #[command(flatten)]
        input: InputArgs,
        /// Layer to convert: the element name of its features
        ///
        /// In this file the layer is `AD_PunktAdresowy`:
        ///
        ///   <gml:FeatureCollection
        ///       xmlns:gml="http://www.opengis.net/gml/3.2"
        ///       xmlns:prgad="https://geoportal.gov.pl/schemas/prgad/1.0">
        ///     <gml:featureMember>
        ///       <prgad:AD_PunktAdresowy gml:id="PL.ZIPIN.3719.1">
        ///         ...
        ///
        /// The name can be written in three ways:
        ///
        ///   AD_PunktAdresowy
        ///       The local name alone, whatever its namespace. An error if
        ///       layers of two namespaces have this name.
        ///   prgad:AD_PunktAdresowy
        ///       With the prefix the input declares for the namespace
        ///       (`xmlns:prgad`). A prefix the input doesn't declare is
        ///       ignored, and the local name decides.
        ///   {https://geoportal.gov.pl/schemas/prgad/1.0}AD_PunktAdresowy
        ///       With the namespace URI itself, in braces.
        ///
        /// `xeibe gml scan` lists the layers of an input.
        #[arg(long, value_name = "NAME", verbatim_doc_comment)]
        layer: String,
        /// Write the rows in the order of the input
        ///
        /// Without it, rows are written in the order the threads finish them:
        /// the same rows, but runs of rows (each a few MB of the input, in
        /// order) from different parts of the input can be interleaved, and
        /// several input files are read at once. Keeping the order costs
        /// speed: the files are then read one after another, and runs finished
        /// early wait in memory for the ones before them. With `--threads 1`
        /// the order is always kept. A settings file can ask for it too:
        /// `"preserve_order": true` in its `options`.
        #[arg(long, help_heading = OUTPUT, verbatim_doc_comment)]
        preserve_order: bool,
        #[command(flatten)]
        output: OutputArgs,
        #[command(flatten)]
        schema: SchemaArgs,
        #[command(flatten)]
        read: ReadArgs,
    },
}

// Parsed once per run: the size difference between variants doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum WfsCommand {
    /// List the feature types (layers) a service offers
    ///
    /// One line per feature type of the service's GetCapabilities
    /// response: name, title, CRS and extent.
    ///
    /// Example:
    ///   xeibe wfs layers https://example.com/geoserver/wfs
    #[command(verbatim_doc_comment)]
    Layers {
        #[command(flatten)]
        service: ServiceArgs,
    },
    /// Count the features of one feature type
    ///
    /// Asks the server for the number (`resultType=hits`) and downloads
    /// no features. Prints `unknown` if the server doesn't say.
    ///
    /// Example:
    ///   xeibe wfs count https://example.com/geoserver/wfs --type-name app:Parcel
    #[command(verbatim_doc_comment)]
    Count {
        #[command(flatten)]
        service: ServiceArgs,
        #[command(flatten)]
        feature_type: TypeNameArgs,
    },
    /// Download one feature type into Parquet or Arrow IPC
    ///
    /// Requests the features page by page and converts them as they
    /// arrive. Without `--settings`, the columns and their types are
    /// inferred from the first features (`--sample-features`). Rows are
    /// written in the order the server sends them (sorted with
    /// `--sort-by`).
    ///
    /// Example:
    ///   xeibe wfs convert https://example.com/geoserver/wfs \
    ///       --type-name app:Parcel -o parcels.parquet
    #[command(verbatim_doc_comment)]
    Convert {
        #[command(flatten)]
        service: ServiceArgs,
        #[command(flatten)]
        feature_type: TypeNameArgs,
        /// Features per request. Default: the server's page size, or 5000
        ///
        /// A server that declares a page size of its own (`CountDefault`)
        /// never gets a larger one.
        #[arg(long, value_name = "N", verbatim_doc_comment)]
        page_size: Option<u64>,
        /// CRS to request the geometry in (`SRSNAME`)
        ///
        /// One of the CRSs `xeibe wfs layers` prints for the feature type,
        /// e.g. `urn:ogc:def:crs:EPSG::4326`. Default: the type's own CRS.
        #[arg(long, value_name = "SRSNAME", verbatim_doc_comment)]
        srs_name: Option<String>,
        /// Property to sort the features by (`SORTBY`), e.g. an id
        ///
        /// A server that doesn't sort may repeat or skip features between
        /// pages. Not sent unless given.
        #[arg(long, value_name = "PROPERTY", verbatim_doc_comment)]
        sort_by: Option<String>,
        /// Extra query parameter for every request, as `KEY=VALUE`
        ///
        /// For vendor parameters and API keys, e.g. `--param token=abc`.
        /// Give the flag once per parameter.
        #[arg(long = "param", value_name = "KEY=VALUE", verbatim_doc_comment)]
        params: Vec<String>,
        /// Name of the layer's schema in `--settings`. Default: the type name
        ///
        /// Only needed when the settings file was made from other data and
        /// names the layer differently, e.g. with another prefix.
        #[arg(long, value_name = "NAME", verbatim_doc_comment)]
        layer: Option<String>,
        #[command(flatten)]
        output: OutputArgs,
        #[command(flatten)]
        schema: SchemaArgs,
        #[command(flatten)]
        read: ReadArgs,
    },
}

#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// URL of the WFS service, e.g. `https://example.com/geoserver/wfs`
    ///
    /// `SERVICE`, `REQUEST` and the other parameters of a request are set
    /// by xeibe; parameters of the same name in the URL are replaced.
    #[arg(value_name = "URL", verbatim_doc_comment)]
    pub url: String,
}

#[derive(Debug, Args)]
pub struct TypeNameArgs {
    /// Feature type, as `xeibe wfs layers` prints it, e.g. `app:Parcel`.
    #[arg(long, value_name = "NAME")]
    pub type_name: String,
}

#[derive(Debug, Args)]
pub struct InputArgs {
    /// GML input: files, directories, globs, zip archives, `-` or URLs
    ///
    /// Several inputs are read as one dataset, one after another.
    ///
    ///   parcels.gml
    ///       One file: .gml or .xml, also gzip- or zstd-compressed
    ///       (.gml.gz, .gml.zst).
    ///   data/
    ///       Every .gml, .xml and .zip file below a directory.
    ///   'data/**/*.gml'
    ///       A glob. `*` stays within a directory, `**` doesn't. Quote
    ///       it, so that xeibe expands it and not the shell.
    ///   parcels.zip
    ///       Every .gml and .xml member of a local zip archive.
    ///   parcels.zip!/north/a.gml
    ///       One member of a zip archive.
    ///   'parcels.zip!/north/*.gml'
    ///       The members of a zip archive that match a glob.
    ///   -
    ///       Standard input.
    ///   https://example.com/parcels.gml
    ///       One file over HTTP(S). A URL names a single file: no globs,
    ///       and no zip archives (download those first).
    ///   s3://bucket/parcels.gml  s3://bucket/data/  's3://bucket/**/*.gml'
    ///       An object, a prefix or a glob in an object store (s3://,
    ///       gs://, az://). Needs a build with the `object-store` feature,
    ///       which `xeibe --version` lists.
    #[arg(required = true, value_name = "INPUTS", verbatim_doc_comment)]
    pub inputs: Vec<String>,
}

#[derive(Debug, Args)]
pub struct OutputArgs {
    /// Output file
    ///
    /// Overwritten if it exists. A conversion that fails partway removes
    /// it. The format is `--format`'s, whatever the file's extension.
    #[arg(short, long, value_name = "FILE", verbatim_doc_comment)]
    pub output: PathBuf,
    /// Format of the output file
    ///
    /// In Parquet, geometry is WKB with the Parquet GEOMETRY type and
    /// GeoParquet 1.1 metadata, so readers of either kind find it. Curves
    /// can't be written (see `--linearize`). An Arrow IPC file holds the
    /// columns as they were read: geometry in GeoArrow types, curves kept.
    #[arg(
        long,
        value_enum,
        default_value_t = OutputFormat::Parquet,
        help_heading = OUTPUT,
        verbatim_doc_comment
    )]
    pub format: OutputFormat,
    /// Add a bounding-box column per geometry column. Parquet only
    ///
    /// The column is named `<geometry column>_bbox` and holds the xmin,
    /// ymin, xmax and ymax of each row's geometry: the `bbox` covering of
    /// GeoParquet 1.1. Readers use it to skip the parts of the file that
    /// lie outside the area they ask for.
    #[arg(
        long,
        value_enum,
        value_name = "WHEN",
        default_value_t = BboxColumn::Auto,
        help_heading = OUTPUT,
        verbatim_doc_comment
    )]
    pub bbox_column: BboxColumn,
    /// Rows per Parquet row group. Parquet only
    ///
    /// A row group is the part of the file a reader can skip as a whole,
    /// and the part the writer keeps in memory until it is complete.
    /// Memory use therefore grows with this number times the size of a
    /// row. Lower it (to 10000, say) if a layer with very large
    /// geometries runs out of memory.
    #[arg(
        long,
        value_name = "ROWS",
        default_value = "128000",
        help_heading = OUTPUT,
        verbatim_doc_comment
    )]
    pub row_group_size: NonZeroUsize,
}

/// Where a conversion's schema comes from.
#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// Settings file (JSON): read options and/or layer schemas
    ///
    /// Written by `xeibe gml scan -o`, or by hand. It has two parts:
    ///
    ///   layers
    ///       With a schema for the layer, that schema says which columns
    ///       are read and what their types are. Nothing is inferred, so
    ///       `--preset` and `--override` don't change the columns.
    ///       Without one, the schema is inferred from the layer's first
    ///       features (`--sample-features`).
    ///   options
    ///       Defaults for the other flags. A flag given on the command
    ///       line wins over the file. `--axis-override` and `--override`
    ///       add to the file's entries; `--preset` replaces all of its
    ///       inference options.
    ///
    /// `--format`, `--bbox-column` and `--row-group-size` are not part of
    /// a settings file. The output's metadata records the settings a
    /// conversion ran with (`gml:settings`).
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    pub settings: Option<PathBuf>,
    /// Features read before the first row is written. Default: 10000
    ///
    /// Without a schema from `--settings`, the columns and their types
    /// are inferred from the first N features of the layer and then
    /// fixed: a later value that doesn't fit its column is an error.
    /// Types are chosen cautiously (a column needs 100 values to get a
    /// type other than text). Raise N when the first features aren't
    /// typical of the rest, or scan the whole input with
    /// `xeibe gml scan -o`. `--axis-order auto` decides on the same
    /// features.
    #[arg(
        long,
        value_name = "N",
        help_heading = INFERENCE,
        display_order = 1000,
        verbatim_doc_comment
    )]
    pub sample_features: Option<u64>,
}

/// Read parameters. They override the settings file's `options`.
#[derive(Debug, Args)]
pub struct ReadArgs {
    /// Threads to work on (default: one per CPU core)
    ///
    /// All of the work counts: decompressing the input, splitting it into
    /// features and parsing them. With 1, everything runs on one thread;
    /// with N, on N threads and no more (0 counts as 1). Inputs read over
    /// the network (http://, s3://, ...) add one or two threads that only
    /// download.
    ///
    /// Examples:
    ///   --threads 1    one thread, leaving the other cores to other work
    ///   --threads 4    four threads
    #[arg(long, value_name = "N", verbatim_doc_comment)]
    pub threads: Option<usize>,
    /// How column types are inferred from the values
    ///
    /// With `default`, a column is bigint, double, boolean, date,
    /// timestamp or time when all of its values are, and none of them
    /// changes by the conversion: `0012` stays text, because the zeros
    /// would be lost. Every other column is text. With `strings`, values
    /// are not interpreted at all.
    #[arg(long, value_enum, help_heading = INFERENCE, verbatim_doc_comment)]
    pub preset: Option<Preset>,
    /// Set the type or the handling of fields in an inferred schema
    ///
    /// PATTERN is the path of element names to the field, starting with
    /// the layer: `AD_PunktAdresowy/kodPocztowy`. `*` stands for any one
    /// name, `**` for any number of them, `@name` is an attribute and
    /// `{uri}name` a name in a namespace. ACTION is a column type (text,
    /// bigint, double, boolean, date, timestamp, text[], ...) or one of:
    ///
    ///   drop      no column for the field
    ///   raw-xml   the element's content as XML text
    ///   map       the element's children as one name -> value map
    ///   list      a list column, even if the field never repeats
    ///   scalar    a single-value column, even if the field repeats
    ///
    /// Examples:
    ///   --override '*/kodPocztowy=text'
    ///       `kodPocztowy` of every layer as text
    ///   --override 'AD_PunktAdresowy/numerPorzadkowy=text'
    ///       house numbers of one layer as text
    ///   --override '**/@uom=drop'
    ///       no columns for `uom` attributes, at any depth
    ///
    /// Give the flag once per pattern; of several that match a field, the
    /// most specific wins. A conversion ignores it for a layer whose
    /// schema comes from `--settings`: change the type in the file instead.
    #[arg(
        long = "override",
        value_name = "PATTERN=ACTION",
        help_heading = INFERENCE,
        verbatim_doc_comment
    )]
    pub overrides: Vec<String>,
    /// Order of the coordinates in the input: x/y or y/x
    ///
    /// x is the easting or longitude, y the northing or latitude. GML
    /// should follow the axis order of its Coordinate Reference System
    /// (CRS, usually used interchangeably with SRS - Spatial Reference System)
    /// (EPSG:4326 is lat/lon), but most files in the wild don't so you'd get entries like:
    /// ```
    /// <gml:Point gml:id="p1" srsName="http://www.opengis.net/def/crs/EPSG/0/4326">
    ///   <gml:coordinates>10.0, 20.0</gml:coordinates>
    /// </gml:Point>
    /// ```
    /// where 10.0 would be longitude or x, and 20.0 would be latitude or y despite specifying EPSG:4326 CRS.
    ///
    /// Value `auto` decides once per file, srsName and GML version, by the
    /// first of these that gives an answer:
    ///
    ///   1. `--axis-override` for the srsName
    ///   2. `axisLabels` on the geometry, e.g. "Lat Long"
    ///   3. the coordinates fit the CRS's area of use in one order only
    ///   4. a known quirk of the producer (FME, MapServer WFS 1.1)
    ///   5. GML 2-style geometry (`gml:coordinates`): x/y
    ///   6. `xeibe wfs`: the WFS version and the srsName form requested
    ///   7. the form of the srsName, as `crs-heuristic`
    ///
    /// `xeibe gml scan` prints the cases in which these contradict each
    /// other. The other modes apply one rule to the whole input:
    /// `crs-heuristic` is what GDAL does, reading `EPSG:4326` as x/y and
    /// `urn:ogc:def:crs:EPSG::4326` as the CRS says (lat/lon), and
    /// `gml-version` tells GML 2 from GML 3 by the geometry's elements
    /// (`gml:coordinates` against `gml:pos` and `gml:posList`).
    ///
    /// Default: the settings file's mode, or `auto`.
    #[arg(
        long,
        value_enum,
        value_name = "MODE",
        help_heading = GEOMETRY,
        verbatim_doc_comment
    )]
    pub axis_order: Option<AxisMode>,
    /// Coordinate order for one srsName, as `SRSNAME=MODE`
    ///
    /// This is useful only for an input that mixes srsNames which must be read differently.
    /// Otherwise `--axis-order` alone is enough. MODE is one of
    /// `--axis-order`'s values. SRSNAME is compared with the input's as
    /// written: case matters, surrounding whitespace doesn't.
    ///
    /// Example:
    ///   --axis-order auto --axis-override 'EPSG:4326=yx'
    #[arg(
        long = "axis-override",
        value_name = "SRSNAME=MODE",
        help_heading = GEOMETRY,
        verbatim_doc_comment
    )]
    pub axis_overrides: Vec<String>,
    /// Manually set CRS of the geometry
    ///
    /// For input without an srsName, or with a wrong or unknown one. It
    /// only labels the output: coordinates are not reprojected, and their
    /// order is still `--axis-order`'s to decide. Accepts every form an
    /// srsName may have:
    ///
    ///   EPSG:2180
    ///   urn:ogc:def:crs:EPSG::2180
    ///   http://www.opengis.net/def/crs/EPSG/0/2180
    ///   EPSG:25832+7837     horizontal + vertical CRS
    ///   PL-1992             a name EPSG lists as an alias (this is not recommended to use)
    #[arg(
        long,
        value_name = "CRS",
        help_heading = GEOMETRY,
        verbatim_doc_comment
    )]
    pub crs: Option<String>,
    /// Replace curves by straight segments. Needed for Parquet; lossy conversion.
    ///
    /// GML arcs and circles (Arc, ArcString, Circle, ArcByCenterPoint,
    /// ArcByBulge and their kin) are read as the curve types
    /// CircularString, CompoundCurve, CurvePolygon, MultiCurve and
    /// MultiSurface. Parquet and GeoParquet only allow points, lines and
    /// polygons, so converting a layer with curves to Parquet is an error
    /// without this flag. Arrow IPC output can keep the curves but you can
    /// use the flag for readers that can't handle them.
    ///
    /// Every arc becomes a line through points on the arc that are at
    /// most STEP_DEG degrees of arc apart (4 if not given, as in GDAL: a
    /// full circle then has 90 segments). The points that define the arc
    /// are kept. The original curves are not stored anywhere.
    #[arg(
        long,
        value_name = "STEP_DEG",
        num_args = 0..=1,
        default_missing_value = "4",
        help_heading = GEOMETRY,
        verbatim_doc_comment
    )]
    pub linearize: Option<f64>,
    /// With `--linearize`: longest segment an arc may be replaced by
    ///
    /// The angle step alone makes long segments on wide arcs: at 4 degrees
    /// they are 7 m long on a radius of 100 m and 70 m on a radius of
    /// 1 km. This flag adds points until no segment is longer than LEN.
    /// LEN is in the units of the input's CRS, so the same number means
    /// different things:
    ///
    ///   --arc-max-gap 1
    ///       EPSG:3857 (metres): segments of at most 1 m.
    ///       EPSG:4326 (degrees): at most 1 degree, about 111 km, which
    ///       limits nothing.
    ///   --arc-max-gap 0.00001
    ///       EPSG:4326: at most 0.00001 degrees, about 1.1 m.
    ///       EPSG:3857: at most 0.01 mm, far too many points.
    ///
    /// Without it, there is no limit.
    #[arg(
        long,
        value_name = "LEN",
        requires = "linearize",
        help_heading = GEOMETRY,
        verbatim_doc_comment
    )]
    pub arc_max_gap: Option<f64>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Preset {
    /// A type per column, chosen from its values; text where none fits.
    Default,
    /// Every column as text; only geometry is still parsed.
    Strings,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum AxisMode {
    /// x/y: nothing is swapped.
    XY,
    /// y/x: the first two numbers of every position are swapped.
    YX,
    /// The order the CRS defines, whatever the form of the srsName.
    Crs,
    /// x/y for `EPSG:4326` and legacy URLs, the CRS's order for URNs and URIs.
    CrsHeuristic,
    /// x/y for GML 2-style geometry, the CRS's order for GML 3-style.
    GmlVersion,
    /// Decided from evidence in the input, in the order above.
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BboxColumn {
    /// Every geometry column but point columns, whose box would repeat the point.
    Auto,
    /// Every geometry column, point columns too.
    Always,
    /// No bounding-box columns.
    Never,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    /// Parquet with GeoParquet metadata, ZSTD-compressed.
    Parquet,
    /// Arrow IPC file (Feather V2), uncompressed.
    Ipc,
}
