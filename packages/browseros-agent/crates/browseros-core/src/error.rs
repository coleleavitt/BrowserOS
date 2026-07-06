use crate::{PageId, Ref};
use browseros_cdp::CdpError;

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum CoreError {
    #[error("Unknown page {0}. List pages to see what is open.")]
    UnknownPage(PageId),
    #[error("Unknown page {0}.")]
    UnknownPageShort(PageId),
    #[error("Unknown ref {0}; take a new snapshot.")]
    UnknownRef(Ref),
    #[error("Stale ref {ref_id} ({role} \"{name}\"); take a new snapshot.")]
    StaleRef {
        ref_id: Ref,
        role: String,
        name: String,
    },
    #[error("Page document changed during snapshot capture; retry.")]
    DocumentChanged,
    #[error("Drag across frame sessions is not supported.")]
    CrossFrameDrag,
    #[error("Provide either target element or both targetX and targetY.")]
    InvalidDragTarget,
    #[error(transparent)]
    Cdp(#[from] CdpError),
    #[error("{0}")]
    Message(String),
}

impl CoreError {
    #[must_use]
    pub fn is_retryable_session_loss(&self) -> bool {
        matches!(
            self,
            Self::Cdp(CdpError::SessionGone | CdpError::ConnectionLost | CdpError::NotConnected)
        )
    }
}

impl From<String> for CoreError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for CoreError {
    fn from(value: &str) -> Self {
        Self::Message(value.to_string())
    }
}
