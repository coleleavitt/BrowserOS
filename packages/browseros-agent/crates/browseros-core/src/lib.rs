pub mod browser;
pub mod connection;
pub mod content_markdown;
pub mod error;
pub mod frames;
pub mod input;
pub mod navigation;
pub mod observer;
pub mod pages;
pub mod screenshot;
pub mod snapshot;
pub mod timeouts;
pub mod types;
pub mod windows;

pub use browser::Browser;
pub use connection::{CdpConnection, ProtocolSession};
pub use error::CoreError;
pub use session::{BrowserSession, BrowserSessionHooks};
pub use types::{FrameId, PageId, Ref, SessionId, TabId, TargetId, WindowId};

mod session;
