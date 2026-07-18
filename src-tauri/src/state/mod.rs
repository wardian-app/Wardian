pub mod active_agent;
pub mod agent_watch;
pub mod app_state;
pub mod conversation_archive;
pub mod file_resources;
pub mod interactions;
pub mod mailbox;
pub mod terminal_session;
pub mod terminal_text;
pub mod user_terminal;

pub use active_agent::ActiveAgent;
pub use agent_watch::AgentWatchState;
pub use app_state::{AppState, ExplorerWatchRegistration, LibraryWatchRegistration};
pub use conversation_archive::ConversationArchiveState;
pub use file_resources::{
    FileRecoveryCheckpointV1, FileRecoveryCleanupV1, FileRecoveryMergeResultV1,
    FileRecoverySummaryV1, FileRecoveryV1, FileResourceEventV1, FileResourceRangeRead,
    FileResourceRuntime, FileResourceSaveAsResultV1, FileResourceSaveResultV1,
    FileResourceSnapshotV1, FileResourceTextV1, FileResourceTicketV1, SaveTargetGrantV1,
    UserFileGrantV1, FILE_RESOURCE_REVISION_EVENT,
};
pub use interactions::InteractionState;
pub use mailbox::{
    MailboxDeliveryPhase, MailboxMessageDraft, MailboxMessageRecord, MailboxMessageStatus,
    MailboxState,
};
pub use terminal_session::TerminalSessionBroker;
pub use user_terminal::UserTerminalSession;
