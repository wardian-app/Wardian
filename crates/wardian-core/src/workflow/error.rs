use thiserror::Error;

/// Errors returned by the workflow library's IO and (de)serialization paths.
/// Semantic problems with an otherwise-parseable blueprint are reported as
/// [`crate::workflow::validate::Diagnostic`]s, not as `WorkflowError`.
#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("blueprint is missing the `---` front-matter block")]
    MissingFrontMatter,

    #[error("front-matter is not valid YAML: {0}")]
    Yaml(String),

    #[error("could not serialize blueprint: {0}")]
    Serialize(String),
}

pub type Result<T> = std::result::Result<T, WorkflowError>;
