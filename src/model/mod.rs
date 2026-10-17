//! Plain data and pure reducers shared by the GPUI widgets.
//!
//! Nothing in this module creates entities or touches a window. The widgets in
//! the crate project these values; hosts can unit-test their own flows
//! against the same reducers.

mod attachment;
mod chat;
mod component;
mod session;
mod settings;

pub use attachment::{Attachment, AttachmentKind};
pub use chat::{
    ChatMessage, ChatRole, ChatState, MessageId, MessageMeta, MessageState, NewMessage, SourceRef,
};
pub use component::{AgentComponent, ComponentKind, ComponentStatus};
pub use session::SessionEntry;
pub use settings::ChatSettings;
