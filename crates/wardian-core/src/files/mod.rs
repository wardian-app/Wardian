//! Canonical file authorization, content descriptors, and renderer limits.
//!
//! Callers first authorize an existing file with [`AuthorizedRootService`],
//! then pass the resulting [`AuthorizedPath`] to [`FileContentDescriptorV1`].

mod authorized_roots;
mod descriptor;

pub use authorized_roots::{
    AuthorizedPath, AuthorizedRootService, FileRevisionToken, GuardedFileWrite,
};
pub use descriptor::{
    FileContentDescriptorV1, FileRendererKind, FileResourceCapabilitiesV1, FileResourceErrorV1,
    FileResourceLimits, VerifiedFileSnapshot,
};
