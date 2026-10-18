//! The grids of one file arranged as PROJ arranges them: top-level grids
//! with nested subgrids, and the lookup of the grid a point falls in.
//!
//! Ported from PROJ 9.8.1 `src/grids.cpp` (MIT, see PROJ-NOTICE.md):
//! `ExtentAndRes`, `isPointInExtent`, `insertIntoHierarchy`,
//! `GTiffHGridShiftSet::open`, `GTiffVGridShiftSet::open`,
//! `GTiffGenericGridShiftSet::open` and the `gridAt` functions.

use std::collections::HashMap;
use std::f64::consts::PI;

use super::tiff::Image;

/// `REL_TOLERANCE_HGRIDSHIFT`.
pub(crate) const REL_TOLERANCE: f64 = 1e-5;

/// A grid's extent and resolution, in radians for a geographic grid. West
/// and south are the first node's centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExtentAndRes {
    pub(crate) is_geographic: bool,
    pub(crate) west: f64,
    pub(crate) south: f64,
    pub(crate) east: f64,
    pub(crate) north: f64,
    pub(crate) res_x: f64,
    pub(crate) res_y: f64,
    pub(crate) inv_res_x: f64,
    pub(crate) inv_res_y: f64,
}

impl ExtentAndRes {
    pub(crate) fn full_world_longitude(&self) -> bool {
        self.is_geographic && self.east - self.west + self.res_x >= 2.0 * PI - 1e-10
    }

    pub(crate) fn contains(&self, other: &ExtentAndRes) -> bool {
        other.west >= self.west
            && other.east <= self.east
            && other.south >= self.south
            && other.north <= self.north
    }

    /// PROJ's test, which is only true when `other` starts inside `self`
    /// (it is used for a debug message only).
    #[allow(dead_code)]
    pub(crate) fn intersects(&self, other: &ExtentAndRes) -> bool {
        other.west < self.east
            && self.west <= other.west
            && other.south < self.north
            && self.south <= other.north
    }

    /// The tolerance horizontal-shift lookups allow around a grid.
    pub(crate) fn epsilon(&self) -> f64 {
        (self.res_x + self.res_y) * REL_TOLERANCE
    }
}

/// `isPointInExtent`.
pub(crate) fn point_in_extent(mut x: f64, y: f64, extent: &ExtentAndRes, eps: f64) -> bool {
    if !(y + eps >= extent.south && y - eps <= extent.north) {
        return false;
    }
    if extent.full_world_longitude() {
        return true;
    }
    if extent.is_geographic {
        if x + eps < extent.west {
            x += 2.0 * PI;
        } else if x - eps > extent.east {
            x -= 2.0 * PI;
        }
    }
    x + eps >= extent.west && x - eps <= extent.east
}

/// One grid of the hierarchy: an image of the file, what the method needs
/// to read it, and its subgrids.
#[derive(Debug)]
pub(crate) struct Node<P> {
    pub(crate) image: usize,
    pub(crate) info: P,
    pub(crate) children: Vec<Node<P>>,
}

impl<P> Node<P> {
    /// `insertGrid`: into the first child that contains it, recursively,
    /// else as a new child. Returns where it went, relative to `self`.
    fn insert_grid(&mut self, sub: Node<P>, images: &[Image]) -> Vec<usize> {
        let extent = images[sub.image].extent;
        for (i, child) in self.children.iter_mut().enumerate() {
            if images[child.image].extent.contains(&extent) {
                let mut path = vec![i];
                path.extend(child.insert_grid(sub, images));
                return path;
            }
        }
        self.children.push(sub);
        vec![self.children.len() - 1]
    }

    /// `gridAt` below a grid: the deepest subgrid containing the point.
    pub(crate) fn grid_at(&self, images: &[Image], x: f64, y: f64, tolerant: bool) -> &Node<P> {
        for child in &self.children {
            let extent = &images[child.image].extent;
            let eps = if tolerant { extent.epsilon() } else { 0.0 };
            if point_in_extent(x, y, extent, eps) {
                return child.grid_at(images, x, y, tolerant);
            }
        }
        self
    }
}

fn node_at_mut<'a, P>(top: &'a mut [Node<P>], path: &[usize]) -> &'a mut Node<P> {
    let mut node = &mut top[path[0]];
    for &i in &path[1..] {
        node = &mut node.children[i];
    }
    node
}

fn node_at<'a, P>(top: &'a [Node<P>], path: &[usize]) -> &'a Node<P> {
    let mut node = &top[path[0]];
    for &i in &path[1..] {
        node = &node.children[i];
    }
    node
}

/// `insertIntoHierarchy`: by the `grid_name` and `parent_grid_name`
/// metadata when they place the grid, else by extents (among top grids of
/// the same `TYPE`).
fn insert_into_hierarchy<P>(
    top: &mut Vec<Node<P>>,
    names: &mut HashMap<String, Vec<usize>>,
    node: Node<P>,
    grid_name: &str,
    parent_name: &str,
    images: &[Image],
    type_of: &dyn Fn(&Node<P>) -> String,
) {
    let extent = images[node.image].extent;
    let path = 'placed: {
        if !parent_name.is_empty() {
            if let Some(parent) = names.get(parent_name).cloned()
                && images[node_at(top, &parent).image].extent.contains(&extent)
            {
                let p = node_at_mut(top, &parent);
                p.children.push(node);
                let mut path = parent;
                path.push(p.children.len() - 1);
                break 'placed path;
            }
        } else if !grid_name.is_empty() {
            top.push(node);
            break 'placed vec![top.len() - 1];
        }
        let ty = type_of(&node);
        for i in 0..top.len() {
            if !ty.is_empty() && type_of(&top[i]) != ty {
                continue;
            }
            if images[top[i].image].extent.contains(&extent) {
                let mut path = vec![i];
                path.extend(top[i].insert_grid(node, images));
                break 'placed path;
            }
        }
        top.push(node);
        vec![top.len() - 1]
    };
    if !grid_name.is_empty() {
        names.insert(grid_name.to_string(), path);
    }
}

/// How a horizontal-shift grid's samples are read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HInfo {
    pub(crate) lat: usize,
    pub(crate) lon: usize,
    /// From the grid's unit to radians.
    pub(crate) to_radians: f64,
    pub(crate) positive_east: bool,
}

/// FILETYPE_PAGE.
const PAGE: u32 = 2;

/// `GTiffHGridShiftSet::open`. The sample indices, unit and sign carry over
/// from one image to the next unless an image redefines them.
pub(crate) fn horizontal(images: &[Image]) -> Result<Vec<Node<HInfo>>, String> {
    let arc_second = (PI / 180.0) / 3600.0;
    let mut info = HInfo {
        lat: 0,
        lon: 1,
        to_radians: arc_second,
        positive_east: true,
    };
    let mut top = Vec::new();
    let mut names = HashMap::new();
    for (ifd, image) in images.iter().enumerate() {
        if image.subfile_type != 0 && image.subfile_type != PAGE {
            if ifd == 0 {
                return Err("invalid subfileType".into());
            }
            continue;
        }
        if image.samples < 2 {
            if ifd == 0 {
                return Err("at least 2 samples per pixel needed".into());
            }
            continue;
        }
        let mut any = false;
        let (mut found_lat, mut found_lon) = (false, false);
        for i in 0..image.samples {
            let desc = image.metadata("DESCRIPTION", i as i32).unwrap_or("");
            if !desc.is_empty() {
                any = true;
            }
            if desc == "latitude_offset" {
                info.lat = i;
                found_lat = true;
            } else if desc == "longitude_offset" {
                info.lon = i;
                found_lon = true;
            }
        }
        if any && !found_lat && !found_lon {
            if ifd > 0 {
                continue;
            }
            return Err("IFD 0 has channel descriptions, but no offsets".into());
        }
        if found_lat != found_lon {
            return Err("only one of latitude_offset and longitude_offset".into());
        }
        if info.lat >= image.samples || info.lon >= image.samples {
            return Err("invalid sample index".into());
        }
        if found_lon {
            match image.metadata("positive_value", info.lon as i32) {
                None | Some("") => {}
                Some("west") => info.positive_east = false,
                Some("east") => info.positive_east = true,
                Some(v) => return Err(format!("unsupported positive_value {v}")),
            }
        }
        let unit_lat = image.metadata("UNITTYPE", info.lat as i32).unwrap_or("");
        let unit_lon = image.metadata("UNITTYPE", info.lon as i32).unwrap_or("");
        if unit_lat != unit_lon {
            return Err("different units for longitude and latitude offsets".into());
        }
        match unit_lat {
            "" => {}
            "arc-second" | "arc-seconds per year" => info.to_radians = arc_second,
            "radian" => info.to_radians = 1.0,
            "degree" => info.to_radians = PI / 180.0,
            u => return Err(format!("unsupported unit {u}")),
        }
        let grid_name = image.metadata("grid_name", -1).unwrap_or("").to_string();
        let parent = image
            .metadata("parent_grid_name", -1)
            .unwrap_or("")
            .to_string();
        let node = Node {
            image: ifd,
            info,
            children: Vec::new(),
        };
        let type_of = |n: &Node<HInfo>| {
            images[n.image]
                .metadata("TYPE", -1)
                .unwrap_or("")
                .to_string()
        };
        insert_into_hierarchy(
            &mut top, &mut names, node, &grid_name, &parent, images, &type_of,
        );
    }
    Ok(top)
}

/// `HorizontalShiftGridSet::gridAt`.
pub(crate) fn horizontal_grid_at<'a>(
    top: &'a [Node<HInfo>],
    images: &[Image],
    lon: f64,
    lat: f64,
) -> Option<&'a Node<HInfo>> {
    for grid in top {
        let extent = &images[grid.image].extent;
        if point_in_extent(lon, lat, extent, extent.epsilon()) {
            return Some(grid.grid_at(images, lon, lat, true));
        }
    }
    None
}

/// The sample of a vertical grid that holds the shift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VInfo {
    pub(crate) sample: usize,
}

/// `GTiffVGridShiftSet::open`. The sample carries over from one image to
/// the next unless an image names its own.
pub(crate) fn vertical(images: &[Image]) -> Result<Vec<Node<VInfo>>, String> {
    let mut sample = 0;
    let mut top = Vec::new();
    let mut names = HashMap::new();
    for (ifd, image) in images.iter().enumerate() {
        if image.subfile_type != 0 && image.subfile_type != PAGE {
            if ifd == 0 {
                return Err("invalid subfileType".into());
            }
            continue;
        }
        let (mut any, mut found) = (false, false);
        for i in 0..image.samples {
            let desc = image.metadata("DESCRIPTION", i as i32).unwrap_or("");
            if !desc.is_empty() {
                any = true;
            }
            if matches!(
                desc,
                "geoid_undulation"
                    | "vertical_offset"
                    | "hydroid_height"
                    | "ellipsoidal_height_offset"
            ) {
                sample = i;
                found = true;
            }
        }
        if any && !found {
            if ifd > 0 {
                continue;
            }
            return Err("IFD 0 has channel descriptions, but no vertical shift".into());
        }
        if sample >= image.samples {
            return Err("invalid sample index".into());
        }
        let grid_name = image.metadata("grid_name", -1).unwrap_or("").to_string();
        let parent = image
            .metadata("parent_grid_name", -1)
            .unwrap_or("")
            .to_string();
        let node = Node {
            image: ifd,
            info: VInfo { sample },
            children: Vec::new(),
        };
        let type_of = |n: &Node<VInfo>| {
            images[n.image]
                .metadata("TYPE", -1)
                .unwrap_or("")
                .to_string()
        };
        insert_into_hierarchy(
            &mut top, &mut names, node, &grid_name, &parent, images, &type_of,
        );
    }
    Ok(top)
}

/// `VerticalShiftGridSet::gridAt`.
pub(crate) fn vertical_grid_at<'a>(
    top: &'a [Node<VInfo>],
    images: &[Image],
    lon: f64,
    lat: f64,
) -> Option<&'a Node<VInfo>> {
    for grid in top {
        if point_in_extent(lon, lat, &images[grid.image].extent, 0.0) {
            return Some(grid.grid_at(images, lon, lat, false));
        }
    }
    None
}

/// A generic grid reads its metadata, when an image lacks it, from the
/// file's first grid (`GTiffGenericGrid::setFirstGrid`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GInfo {
    pub(crate) first: Option<usize>,
}

/// `GTiffGenericGrid::metadataItem`.
pub(crate) fn generic_metadata<'a>(
    images: &'a [Image],
    node: &Node<GInfo>,
    name: &str,
    sample: i32,
) -> &'a str {
    match images[node.image].metadata(name, sample) {
        Some(v) if !v.is_empty() => v,
        _ => node
            .info
            .first
            .and_then(|f| images[f].metadata(name, sample))
            .unwrap_or(""),
    }
}

/// `GTiffGenericGridShiftSet::open`.
pub(crate) fn generic(images: &[Image]) -> Result<Vec<Node<GInfo>>, String> {
    let mut top: Vec<Node<GInfo>> = Vec::new();
    let mut names = HashMap::new();
    for (ifd, image) in images.iter().enumerate() {
        if image.subfile_type != 0 && image.subfile_type != PAGE {
            if ifd == 0 {
                return Err("invalid subfileType".into());
            }
            continue;
        }
        let grid_name = image.metadata("grid_name", -1).unwrap_or("").to_string();
        let parent = image
            .metadata("parent_grid_name", -1)
            .unwrap_or("")
            .to_string();
        let mut node = Node {
            image: ifd,
            info: GInfo { first: None },
            children: Vec::new(),
        };
        if let Some(first) = top.first()
            && image.metadata("TYPE", -1).unwrap_or("").is_empty()
            && !generic_metadata(images, first, "TYPE", -1).is_empty()
        {
            node.info.first = Some(first.image);
        }
        let type_of = |n: &Node<GInfo>| generic_metadata(images, n, "TYPE", -1).to_string();
        insert_into_hierarchy(
            &mut top, &mut names, node, &grid_name, &parent, images, &type_of,
        );
    }
    Ok(top)
}

/// `GenericShiftGridSet::gridAt(type, x, y)`.
pub(crate) fn generic_grid_at_type<'a>(
    top: &'a [Node<GInfo>],
    images: &[Image],
    ty: &str,
    x: f64,
    y: f64,
) -> Option<&'a Node<GInfo>> {
    for grid in top {
        if generic_metadata(images, grid, "TYPE", -1) != ty {
            continue;
        }
        if point_in_extent(x, y, &images[grid.image].extent, 0.0) {
            return Some(grid.grid_at(images, x, y, false));
        }
    }
    None
}

/// `GenericShiftGridSet::gridAt(x, y)`.
pub(crate) fn generic_grid_at<'a>(
    top: &'a [Node<GInfo>],
    images: &[Image],
    x: f64,
    y: f64,
) -> Option<&'a Node<GInfo>> {
    for grid in top {
        if point_in_extent(x, y, &images[grid.image].extent, 0.0) {
            return Some(grid.grid_at(images, x, y, false));
        }
    }
    None
}
