//! Routing and lifetime identity shared by all domain message payloads.

/// Optional routing IDs for a document, viewport generation and asynchronous operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessageIdentity {
    /// Stable tab ID, absent for session-wide messages.
    pub document: Option<usize>,
    /// Viewport generation used to reject superseded snapshots.
    pub view_token: Option<u64>,
    /// Identifier for long-running operations such as search or copy.
    pub operation_id: Option<u64>,
}

/// Carry domain payloads together with the IDs needed to route and validate replies.
#[derive(Clone, Debug)]
pub struct Envelope<T> {
    /// Routing and lifetime identifiers associated with the payload.
    pub identity: MessageIdentity,
    /// Domain-specific request or update.
    pub payload: T,
}
