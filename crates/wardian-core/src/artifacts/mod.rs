//! Durable, inspectable artifact threads and immutable presented versions.

mod models;
mod store;

pub use models::{
    ArtifactIndexEntryV1, ArtifactIndexV1, ArtifactManifestV1, ArtifactOriginV1,
    ArtifactReviewStatus, ArtifactVersionV1,
};
pub use store::{
    AppendArtifactVersion, ArtifactStore, ArtifactStoreError, CreateArtifactThread,
    StoredArtifactVersion,
};
