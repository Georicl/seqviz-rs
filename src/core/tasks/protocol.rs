//! Routing and lifetime identity shared by all domain message payloads.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessageIdentity {
    pub document: Option<usize>,
    pub view_token: Option<u64>,
    pub operation_id: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Envelope<T> {
    pub identity: MessageIdentity,
    pub payload: T,
}
