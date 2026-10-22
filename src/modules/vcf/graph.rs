//! Optional extension point for graph annotations; ordinary VCF has no graph adapter.
use super::model::{VcfHeader, VcfRecord};

#[derive(Clone, Debug)]
pub struct GraphAnnotation {
    pub properties: Vec<(String, String)>,
}

pub fn inspect_graph(_header: &VcfHeader, _record: &VcfRecord) -> Option<GraphAnnotation> {
    None
}
