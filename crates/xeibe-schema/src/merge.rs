/// Associative and commutative merge. Enables parallel scans (one tree per
/// chunk) and multi-file inputs and WFS pages (one tree per source).
pub trait Merge {
    fn merge(&mut self, other: Self);
}
