//! Areas of use: geographic bounding boxes in degrees, with PROJ's rules for
//! the antimeridian, containment and intersection.
//!
//! Ported from PROJ 9.8.1 `src/iso19111/metadata.cpp` (`GeographicBoundingBox`
//! and `Extent`; MIT, see PROJ-NOTICE.md). The name rides along because the
//! choice of operation looks at it ("- offshore", unknown names), and PROJ
//! keeps it only when an intersection returns one of its inputs unchanged.

/// A geographic bounding box, west may exceed east across the antimeridian.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Extent {
    pub(crate) name: Option<String>,
    pub(crate) west: f64,
    pub(crate) south: f64,
    pub(crate) east: f64,
    pub(crate) north: f64,
}

/// The whole world, as PROJ's `Extent::WORLD`.
pub(crate) fn world() -> Extent {
    Extent {
        name: Some("World".to_owned()),
        west: -180.0,
        south: -90.0,
        east: 180.0,
        north: 90.0,
    }
}

impl Extent {
    /// As `GeographicBoundingBox::create`: a box reduced to a line or a point
    /// is widened by one ulp so it isn't degenerate.
    pub(crate) fn new(
        name: Option<String>,
        mut west: f64,
        mut south: f64,
        mut east: f64,
        mut north: f64,
    ) -> Extent {
        if west == east {
            if west > -180.0 {
                west = next_down(west);
            }
            if east < 180.0 {
                east = next_up(east);
            }
        }
        if south == north {
            if south > -90.0 {
                south = next_down(south);
            }
            if north < 90.0 {
                north = next_up(north);
            }
        }
        Extent {
            name,
            west,
            south,
            east,
            north,
        }
    }

    pub(crate) fn is_world(&self) -> bool {
        self.west == -180.0 && self.south == -90.0 && self.east == 180.0 && self.north == 90.0
    }

    /// PROJ's `getPseudoArea`: proportional to the area on a sphere.
    pub(crate) fn pseudo_area(&self) -> f64 {
        let (w, s, mut e, n) = (self.west, self.south, self.east, self.north);
        if w > e {
            e += 360.0;
        }
        (e - w) * (n.to_radians().sin() - s.to_radians().sin())
    }

    pub(crate) fn contains(&self, other: &Extent) -> bool {
        let (w, e, n, s) = (self.west, self.east, self.north, self.south);
        let (ow, oe, on, os) = (other.west, other.east, other.north, other.south);
        if !(s <= os && n >= on) {
            return false;
        }
        if w == -180.0 && e == 180.0 {
            return ow != oe;
        }
        if ow == -180.0 && oe == 180.0 {
            return false;
        }
        if w < e {
            ow < oe && w <= ow && e >= oe
        } else if ow < oe {
            ow >= w || oe <= e
        } else {
            w <= ow && e >= oe
        }
    }

    pub(crate) fn intersects(&self, other: &Extent) -> bool {
        let (w, e, n, s) = (self.west, self.east, self.north, self.south);
        let (ow, oe, on, os) = (other.west, other.east, other.north, other.south);
        if n < os || s > on {
            return false;
        }
        if w == -180.0 && e == 180.0 && ow > oe {
            return true;
        }
        if ow == -180.0 && oe == 180.0 && w > e {
            return true;
        }
        if w <= e {
            if ow <= oe {
                return w.max(ow) < e.min(oe);
            }
            if ow > 180.0 || oe < -180.0 {
                return false;
            }
            self.intersects(&bbox(ow, os, 180.0, on)) || self.intersects(&bbox(-180.0, os, oe, on))
        } else if ow <= oe {
            other.intersects(self)
        } else {
            true
        }
    }

    /// The area covering several areas of use (an operation's usages): the
    /// one that contains all the others, else their bounding box, named
    /// after the largest. Areas across the antimeridian that no single one
    /// contains fall back to the largest.
    pub(crate) fn covering(areas: &[&Extent]) -> Option<Extent> {
        let largest = areas
            .iter()
            .max_by(|a, b| a.pseudo_area().total_cmp(&b.pseudo_area()))?;
        if areas.iter().all(|a| largest.contains(a)) || areas.iter().any(|a| a.west > a.east) {
            return Some((*largest).clone());
        }
        let fold = |f: fn(f64, f64) -> f64, g: fn(&Extent) -> f64| {
            areas.iter().map(|a| g(a)).reduce(f).unwrap_or(0.0)
        };
        Some(Extent::new(
            largest.name.clone(),
            fold(f64::min, |a| a.west),
            fold(f64::min, |a| a.south),
            fold(f64::max, |a| a.east),
            fold(f64::max, |a| a.north),
        ))
    }

    /// `Extent::intersection`: one of the two unchanged (name included) when
    /// it contains the other, else the box intersection without a name.
    pub(crate) fn intersection(&self, other: &Extent) -> Option<Extent> {
        if self.contains(other) {
            return Some(other.clone());
        }
        if other.contains(self) {
            return Some(self.clone());
        }
        let b = box_intersection(self, other)?;
        Some(Extent::new(None, b.west, b.south, b.east, b.north))
    }
}

fn bbox(west: f64, south: f64, east: f64, north: f64) -> Extent {
    Extent {
        name: None,
        west,
        south,
        east,
        north,
    }
}

/// `GeographicBoundingBox::Private::intersection`.
fn box_intersection(a: &Extent, b: &Extent) -> Option<Extent> {
    let (w, e, n, s) = (a.west, a.east, a.north, a.south);
    let (ow, oe, on, os) = (b.west, b.east, b.north, b.south);
    if n < os || s > on {
        return None;
    }
    if w == -180.0 && e == 180.0 && ow > oe {
        return Some(bbox(ow, s.max(os), oe, n.min(on)));
    }
    if ow == -180.0 && oe == 180.0 && w > e {
        return Some(bbox(w, s.max(os), e, n.min(on)));
    }
    if w <= e {
        if ow <= oe {
            let res_w = w.max(ow);
            let res_e = e.min(oe);
            if res_w < res_e {
                return Some(bbox(res_w, s.max(os), res_e, n.min(on)));
            }
            return None;
        }
        if ow > 180.0 || oe < -180.0 {
            return None;
        }
        let inter1 = box_intersection(a, &bbox(ow, os, 180.0, on));
        let inter2 = box_intersection(a, &bbox(-180.0, os, oe, on));
        match (inter1, inter2) {
            (None, x) | (x, None) => x,
            (Some(i1), Some(i2)) => {
                if i1.east - i1.west > i2.east - i2.west {
                    Some(i1)
                } else {
                    Some(i2)
                }
            }
        }
    } else if ow <= oe {
        box_intersection(b, a)
    } else {
        Some(bbox(w.max(ow), s.max(os), e.min(oe), n.min(on)))
    }
}

fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return f64::from_bits(1);
    }
    let bits = x.to_bits();
    f64::from_bits(if x > 0.0 { bits + 1 } else { bits - 1 })
}

fn next_down(x: f64) -> f64 {
    -next_up(-x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_keeps_the_contained_extent_and_its_name() {
        let poland = Extent::new(Some("Poland".into()), 14.14, 49.0, 24.15, 55.93);
        let europe = Extent::new(Some("Europe".into()), -16.1, 32.88, 40.18, 84.73);
        assert_eq!(europe.intersection(&poland), Some(poland.clone()));
        let east = Extent::new(Some("East".into()), 20.0, 40.0, 30.0, 60.0);
        let cut = poland.intersection(&east).unwrap();
        assert_eq!((cut.name, cut.west, cut.east), (None, 20.0, 24.15));
    }

    #[test]
    fn covering_takes_the_containing_area_else_the_bounding_box() {
        let world = world();
        let vietnam = Extent::new(Some("Vietnam".into()), 102.2, 5.67, 112.55, 21.45);
        assert_eq!(Extent::covering(&[&vietnam, &world]), Some(world));
        let south = Extent::new(Some("South".into()), -141.0, 41.0, -52.0, 60.0);
        let north = Extent::new(Some("North".into()), -141.0, 60.0, -60.0, 84.0);
        let both = Extent::covering(&[&south, &north]).unwrap();
        assert_eq!(
            (
                both.name.as_deref(),
                both.west,
                both.south,
                both.east,
                both.north
            ),
            (Some("South"), -141.0, 41.0, -52.0, 84.0)
        );
    }

    #[test]
    fn across_the_antimeridian() {
        let fiji = Extent::new(None, 172.0, -22.0, -178.0, -12.0);
        let pacific = Extent::new(None, 170.0, -30.0, 180.0, 0.0);
        assert!(fiji.intersects(&pacific));
        assert!(fiji.pseudo_area() > 0.0);
    }
}
