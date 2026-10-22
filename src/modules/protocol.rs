//! Domain routing lives here rather than in the generic task runtime.
pub use super::sequence::protocol::{Request, Snapshot, Update, ViewRequest};
pub use crate::core::tasks::protocol::{Envelope, MessageIdentity};

pub enum ModuleRequest {
    Sequence(Envelope<Request>),
}
pub enum ModuleUpdate {
    Sequence(Envelope<Update>),
}

impl ModuleRequest {
    pub fn sequence(request: Request) -> Self {
        let identity = match &request {
            Request::View(view) => MessageIdentity {
                document: Some(view.file),
                view_token: Some(view.token),
                operation_id: None,
            },
            Request::Search {
                file, operation_id, ..
            }
            | Request::Copy {
                file, operation_id, ..
            } => MessageIdentity {
                document: Some(*file),
                operation_id: Some(*operation_id),
                ..MessageIdentity::default()
            },
            Request::Statistics { file } | Request::Cancel(file) | Request::Close(file) => {
                MessageIdentity {
                    document: Some(*file),
                    ..MessageIdentity::default()
                }
            }
            Request::Quit => MessageIdentity::default(),
        };
        Self::Sequence(Envelope {
            identity,
            payload: request,
        })
    }
}

impl ModuleUpdate {
    pub fn sequence(update: Update) -> Self {
        let identity = match &update {
            Update::View(snapshot) => MessageIdentity {
                document: Some(snapshot.request.file),
                view_token: Some(snapshot.request.token),
                operation_id: None,
            },
            Update::Selected {
                file, operation_id, ..
            }
            | Update::Copied {
                file, operation_id, ..
            }
            | Update::OperationFailed {
                file, operation_id, ..
            } => MessageIdentity {
                document: Some(*file),
                operation_id: Some(*operation_id),
                ..MessageIdentity::default()
            },
            Update::Message { file, .. } => MessageIdentity {
                document: Some(*file),
                ..MessageIdentity::default()
            },
        };
        Self::Sequence(Envelope {
            identity,
            payload: update,
        })
    }

    pub fn file(&self) -> Option<usize> {
        match self {
            Self::Sequence(envelope) => envelope.identity.document,
        }
    }
}

impl super::session::Worker {
    pub fn dispatch(&self, request: ModuleRequest) {
        match request {
            ModuleRequest::Sequence(envelope) => self.send(envelope.payload),
        }
    }
    pub fn try_update(&self) -> Option<ModuleUpdate> {
        self.updates.try_recv().ok().map(ModuleUpdate::sequence)
    }

    pub fn close(&self, file: usize) {
        self.dispatch(ModuleRequest::sequence(Request::Close(file)));
    }
}
