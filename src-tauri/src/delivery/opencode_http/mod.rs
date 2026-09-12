//! HTTP delivery into an already running OpenCode TUI backend.
//!
//! This module deliberately owns no OpenCode process. The interactive spawn
//! path must launch the normal TUI with an explicit loopback listener and pass
//! the resulting launch proof to [`OpenCodeHttpOwner::bind`]. A caller that
//! cannot prove that the listener, generation, working directory, and exact
//! OpenCode session belong to the same interactive launch must retain the
//! canonical composer.
//!
//! Integration contract:
//!
//! * The interactive lifecycle owner creates one [`OpenCodeHttpLaunchPlan`]
//!   before spawn, reserves its port, applies only `network_args` and
//!   `environment` to that child, and preserves the original TUI arguments,
//!   config, model, agent, auto mode, resume/new identity, and working
//!   directory.
//! * After listener ownership and exact session discovery are proven for the
//!   same process/runtime generation, it calls `bind` on the plan and then
//!   [`OpenCodeHttpOwner::verify_binding`]. The caller must also perform the
//!   negative unauthenticated probe and retain the composer when any identity
//!   or configuration proof is missing.
//! * The delivery owner opens and filters SSE before claiming canonical work,
//!   checks authoritative busy state at admission, and calls
//!   [`OpenCodeHttpOwner::submit_once`] at most once for a persisted message
//!   ID. A 204 is only HTTP acknowledgement; message, assistant-parent, and
//!   terminal evidence are reconciled separately.
//! * Every submit error after request start is uncertain and forbids both HTTP
//!   replay and composer fallback. [`OpenCodeHttpOwner::close`] revokes the
//!   machine writer without touching the interactive TUI or another server.

mod owner;
mod protocol;
mod sse;

pub use owner::{
    OpenCodeHttpError, OpenCodeHttpErrorCode, OpenCodeHttpOwner, OpenCodeHttpReceipt,
    OpenCodeHttpVerification, OpenCodeSessionActivity, OpenCodeUserMessageProof,
};
pub use protocol::{
    OpenCodeHttpBinding, OpenCodeHttpLaunchPlan, OpenCodeHttpLaunchProof, OpenCodeModel,
    OpenCodePrompt, OpenCodePromptOptions, OpenCodeStoredMessage,
};
pub use sse::{OpenCodeEvent, OpenCodeEventStream};
