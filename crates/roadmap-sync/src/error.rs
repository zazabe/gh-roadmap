use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("validation failed with {0} error(s)")]
    Validation(usize),

    #[error("github authentication or permissions are insufficient (run `roadmap doctor`)")]
    Auth,

    #[error("config: {0}")]
    Config(String),

    #[error("parse {path}: {message}")]
    Parse { path: String, message: String },

    #[error("platform: {0}")]
    Platform(#[from] PlatformError),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("{message}")]
    CommandFailed { program: String, message: String },

    #[error("command `{program}` returned invalid JSON: {source}")]
    InvalidJson {
        program: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("command `{program}` succeeded but output was not a GitHub issue URL: {stdout}")]
    UnexpectedOutput { program: String, stdout: String },

    #[error("unsupported platform: {0}")]
    #[allow(dead_code)]
    Unsupported(String),
}
