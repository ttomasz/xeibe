//! Types of the generated grid catalog (`catalog.rs`) and its lookup.

/// One row of PROJ's `grid_alternatives`.
#[derive(Debug)]
pub(crate) struct Entry {
    /// The file name EPSG gives in the operation's parameters.
    pub(crate) epsg: &'static str,
    /// PROJ-data's file.
    pub(crate) file: &'static str,
    /// PROJ-data stores the grid for the other direction than EPSG's
    /// operation.
    pub(crate) inverse: bool,
    /// Served openly from `cdn.proj.org`: what PROJ, with network access,
    /// counts as available.
    pub(crate) cdn: bool,
}

/// PROJ's `customizations.sql`: the Australian `*DAT.htm` geoid files of the
/// "GDA94 to AHD height" operations are AUSGeoid98.
static AUSGEOID98: Entry = Entry {
    epsg: "*DAT.htm",
    file: "au_ga_AUSGeoid98.tif",
    inverse: false,
    cdn: true,
};

/// The catalog row for a grid EPSG names in operation `op_name`.
pub(crate) fn lookup(epsg_name: &str, op_name: &str) -> Option<&'static Entry> {
    let entries = super::catalog::ENTRIES;
    if let Ok(i) = entries.binary_search_by(|e| e.epsg.cmp(epsg_name)) {
        return Some(&entries[i]);
    }
    if epsg_name.ends_with("DAT.htm")
        && op_name.starts_with("GDA94 to AHD")
        && op_name.contains("height")
    {
        return Some(&AUSGEOID98);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_sorted_for_binary_search() {
        let entries = super::super::catalog::ENTRIES;
        assert!(entries.windows(2).all(|w| w[0].epsg < w[1].epsg));
    }

    #[test]
    fn epsg_names_map_to_proj_data_files() {
        let e = lookup("conus.las", "NAD27 to NAD83 (1)").unwrap();
        assert_eq!((e.file, e.cdn), ("us_noaa_conus.tif", true));
        let e = lookup("SH55_DAT.htm", "GDA94 to AHD height (12)").unwrap();
        assert_eq!(e.file, "au_ga_AUSGeoid98.tif");
        assert!(lookup("SH55_DAT.htm", "something else").is_none());
    }
}
