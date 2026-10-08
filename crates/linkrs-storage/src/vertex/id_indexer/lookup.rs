//! Visibility-aware primary-key lookup result.

/// Collapses the old two-step read (`get_index` plus a timestamp check in
/// the caller) into one call: committed bindings passing the caller's
/// visibility predicate report as [`PkLookup::Visible`], everything else is
/// [`PkLookup::Missing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkLookup {
    /// Committed binding visible at the read timestamp.
    Visible(u32),
    /// No binding, or a committed binding invisible at the read timestamp.
    Missing,
}

impl PkLookup {
    /// Visible id, if any.
    pub fn visible_id(self) -> Option<u32> {
        match self {
            Self::Visible(id) => Some(id),
            Self::Missing => None,
        }
    }
}
