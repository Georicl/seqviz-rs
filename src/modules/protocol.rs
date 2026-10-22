//! Domain routing lives here rather than in the generic task runtime.
pub use super::sequence::protocol::{Request, Snapshot, Update, ViewRequest};
pub use crate::core::tasks::protocol::{Envelope, MessageIdentity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    Sequence,
    Vcf,
}

pub enum ModuleRequest {
    Open { file: usize },
    Close { file: usize },
    Quit,
    Sequence(Envelope<Request>),
    Vcf(Envelope<super::vcf::protocol::Request>),
}
pub enum ModuleUpdate {
    Opened { file: usize, module: ModuleKind },
    OpenFailed { file: usize, text: String },
    Sequence(Envelope<Update>),
    Vcf(Envelope<super::vcf::protocol::Update>),
}

impl ModuleRequest {
    pub fn vcf(request: super::vcf::protocol::Request) -> Self {
        use super::vcf::protocol::Request as VcfRequest;
        let identity = match &request {
            VcfRequest::View(view) => MessageIdentity {
                document: Some(view.file),
                view_token: Some(view.token),
                operation_id: None,
            },
            VcfRequest::Locate { file, token, .. } => MessageIdentity {
                document: Some(*file),
                view_token: Some(*token),
                operation_id: None,
            },
            VcfRequest::Cancel(file) | VcfRequest::Close(file) => MessageIdentity {
                document: Some(*file),
                ..MessageIdentity::default()
            },
            VcfRequest::Quit => MessageIdentity::default(),
        };
        Self::Vcf(Envelope {
            identity,
            payload: request,
        })
    }

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
    pub fn vcf(update: super::vcf::protocol::Update) -> Self {
        use super::vcf::protocol::Update as VcfUpdate;
        let identity = match &update {
            VcfUpdate::View(snapshot) => MessageIdentity {
                document: Some(snapshot.request.file),
                view_token: Some(snapshot.request.token),
                operation_id: None,
            },
            VcfUpdate::Located { file, token, .. }
            | VcfUpdate::LocateFailed { file, token, .. } => MessageIdentity {
                document: Some(*file),
                view_token: Some(*token),
                operation_id: None,
            },
            VcfUpdate::Message { file, .. } => MessageIdentity {
                document: Some(*file),
                ..MessageIdentity::default()
            },
        };
        Self::Vcf(Envelope {
            identity,
            payload: update,
        })
    }

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
            Self::Vcf(envelope) => envelope.identity.document,
            Self::Opened { file, .. } | Self::OpenFailed { file, .. } => Some(*file),
        }
    }
}

impl super::session::Worker {
    pub fn dispatch(&self, request: ModuleRequest) {
        self.send_module(request);
    }
    pub fn try_update(&self) -> Option<ModuleUpdate> {
        self.updates.try_recv().ok()
    }

    pub fn close(&self, file: usize) {
        self.dispatch(ModuleRequest::Close { file });
    }
}
