//! Optional extension point for graph annotations; ordinary VCF has no graph adapter.
use super::model::{VcfHeader, VcfRecord};

/// Named annotation properties returned by a future graph-aware adapter.
#[derive(Clone, Debug)]
pub struct GraphAnnotation {
    /// Adapter-supplied annotation names and values.
    pub properties: Vec<(String, String)>,
}

/// Return no graph annotation; ordinary VCF syntax does not establish a graph model.
pub fn inspect_graph(_header: &VcfHeader, _record: &VcfRecord) -> Option<GraphAnnotation> {
    None
}
