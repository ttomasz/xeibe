/// Why a CRS or a pair of CRSs can't be set up for conversion.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// The EPSG code has no PROJJSON in `xeibe-crs`.
    #[error("EPSG:{0} has no definition in the CRS table")]
    UnknownCode(u32),
    /// The PROJJSON is malformed or lacks something the conversion needs.
    #[error("{crs}: invalid PROJJSON: {message}")]
    Projjson { crs: String, message: String },
    /// The CRS is valid but uses something this crate doesn't implement: a
    /// projection method, a CRS type, an axis direction.
    #[error("{crs}: {what} is not supported")]
    Unsupported { crs: String, what: String },
    /// The two CRSs are on different datums and no transformation between
    /// them can be used: EPSG has none that xeibe can apply (grid-based ones
    /// are not supported yet). A ballpark approximation is never used instead.
    #[error(
        "no usable transformation from {src} to {dst}: EPSG has none that xeibe \
         can apply, and ballpark approximations are not used"
    )]
    NoTransformation { src: String, dst: String },
    /// A coordinate could not be converted. `index` is its position in the
    /// buffer, counted in coordinates (not ordinates).
    #[error("coordinate {index} ({x}, {y}): {reason}")]
    Coordinate {
        index: usize,
        x: f64,
        y: f64,
        reason: CoordinateError,
    },
}

/// Why one coordinate could not be converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CoordinateError {
    /// NaN or infinite input.
    #[error("not a finite number")]
    NotFinite,
    /// A latitude beyond ±90°, or a longitude beyond ±10 radians (PROJ's limit).
    #[error("latitude or longitude out of range")]
    InvalidGeographic,
    /// The point is outside the area where the projection is defined, e.g.
    /// more than 150° from a Transverse Mercator central meridian or at the
    /// pole a conic projection can't reach.
    #[error("outside the projection's domain")]
    OutsideDomain,
    /// An iterative inverse did not converge.
    #[error("did not converge")]
    NoConvergence,
}
