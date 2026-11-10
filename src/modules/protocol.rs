//! Domain routing lives here rather than in the generic task runtime.
pub use super::sequence::protocol::{Request, Snapshot, Update, ViewRequest};
pub use crate::core::tasks::protocol::{Envelope, MessageIdentity};

/// Built-in domain selected by probing decoded file content.
///
/// # Variants
///
/// - `Sequence`: FASTA, FASTQ, SAM or BAM sequence records.
/// - `Vcf`: VCF variant records and sample details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    /// FASTA, FASTQ, SAM or BAM sequence records.
    Sequence,
    /// VCF variant records and sample details.
    Vcf,
}

/// Session lifecycle messages and domain requests sent to the shared worker.
///
/// # Variants
///
/// - `Open`: Probe the input at the stable file/tab ID.
///   - `file`: Stable tab ID whose input needs format probing.
/// - `Close`: Close the tab, releasing its dataset when its last alias is gone.
///   - `file`: Stable tab ID to close while retaining any live aliases.
/// - `Quit`: Shut down both domain backends and the session runner.
/// - `Sequence`: Sequence-domain request with routing identity.
/// - `Vcf`: VCF-domain request with routing identity.
pub enum ModuleRequest {
    /// Probe the input at the stable file/tab ID.
    ///
    /// # Fields
    ///
    /// - `file`: Stable tab ID whose input needs format probing.
    Open {
        /// Stable tab ID whose input needs format probing.
        file: usize,
    },
    /// Close the tab, releasing its dataset when its last alias is gone.
    ///
    /// # Fields
    ///
    /// - `file`: Stable tab ID to close while retaining any live aliases.
    Close {
        /// Stable tab ID to close while retaining any live aliases.
        file: usize,
    },
    /// Shut down both domain backends and the session runner.
    Quit,
    /// Sequence-domain request with routing identity.
    Sequence(Envelope<Request>),
    /// VCF-domain request with routing identity.
    Vcf(Envelope<super::vcf::protocol::Request>),
}
/// Format-probe results and domain replies returned to document pages.
///
/// # Variants
///
/// - `Opened`: Detected module for a successfully probed file/tab ID.
///   - `file`: Stable tab ID whose format was detected.
///   - `module`: Domain selected from the decoded input prefix.
/// - `OpenFailed`: Probe failure text for the originating file/tab ID.
///   - `file`: Stable tab ID whose probe failed.
///   - `text`: Error text displayed by the loading page.
/// - `Sequence`: Sequence-domain reply with routing and operation identity.
/// - `Vcf`: VCF-domain reply with routing and viewport identity.
pub enum ModuleUpdate {
    /// Detected module for a successfully probed file/tab ID.
    ///
    /// # Fields
    ///
    /// - `file`: Stable tab ID whose format was detected.
    /// - `module`: Domain selected from the decoded input prefix.
    Opened {
        /// Stable tab ID whose format was detected.
        file: usize,
        /// Domain selected from the decoded input prefix.
        module: ModuleKind,
    },
    /// Probe failure text for the originating file/tab ID.
    ///
    /// # Fields
    ///
    /// - `file`: Stable tab ID whose probe failed.
    /// - `text`: Error text displayed by the loading page.
    OpenFailed {
        /// Stable tab ID whose probe failed.
        file: usize,
        /// Error text displayed by the loading page.
        text: String,
    },
    /// Sequence-domain reply with routing and operation identity.
    Sequence(Envelope<Update>),
    /// VCF-domain reply with routing and viewport identity.
    Vcf(Envelope<super::vcf::protocol::Update>),
}

impl ModuleRequest {
    /// Wrap a VCF request with its document and viewport generation identifiers.
    ///
    /// # Arguments
    ///
    /// - `request`: Request to dispatch to or handle in the owning backend.
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

    /// Wrap a sequence request with document, viewport or operation identity as applicable.
    ///
    /// # Arguments
    ///
    /// - `request`: Request to dispatch to or handle in the owning backend.
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
    /// Wrap a VCF reply with the identifiers required for page routing and stale-result checks.
    ///
    /// # Arguments
    ///
    /// - `update`: Backend reply to route or apply to the matching document and request.
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

    /// Wrap a sequence reply while preserving its viewport or asynchronous operation identity.
    ///
    /// # Arguments
    ///
    /// - `update`: Backend reply to route or apply to the matching document and request.
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

    /// Return the destination tab ID from a probe result or domain envelope.
    pub fn file(&self) -> Option<usize> {
        match self {
            Self::Sequence(envelope) => envelope.identity.document,
            Self::Vcf(envelope) => envelope.identity.document,
            Self::Opened { file, .. } | Self::OpenFailed { file, .. } => Some(*file),
        }
    }
}

impl super::session::Worker {
    /// Send a domain or lifecycle request through the session's request channel.
    ///
    /// # Arguments
    ///
    /// - `request`: Request to dispatch to or handle in the owning backend.
    pub fn dispatch(&self, request: ModuleRequest) {
        self.send_module(request);
    }
    /// Receive one pending reply without blocking, returning none if empty or disconnected.
    pub fn try_update(&self) -> Option<ModuleUpdate> {
        self.updates.try_recv().ok()
    }

    /// Queue closure of the specified tab and release its backend ownership when appropriate.
    ///
    /// # Arguments
    ///
    /// - `file`: Zero-based tab ID used to route requests and results.
    pub fn close(&self, file: usize) {
        self.dispatch(ModuleRequest::Close { file });
    }
}
