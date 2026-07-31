use thiserror::Error;

/// Crate-wide result type. Defaults its error to [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors surfaced by the operator during reconciliation.
#[derive(Debug, Error)]
pub enum Error {
    #[error("Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error("target Service `{0}` not found in namespace `{1}`")]
    ServiceNotFound(String, String),

    #[error("resource is missing required field: {0}")]
    MissingField(&'static str),

    /// A recoverable failure returned by a [`crate::provider::TunnelProvider`].
    #[error("playit provider error: {0}")]
    Provider(String),

    /// A provider operation that has not been implemented yet.
    #[error("provider operation not yet implemented: {0}")]
    NotImplemented(&'static str),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}
