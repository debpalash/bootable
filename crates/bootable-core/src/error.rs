use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("required tool `{0}` is not installed")]
    MissingTool(&'static str),

    #[error("command `{program}` failed: {message}")]
    CommandFailed { program: String, message: String },

    #[error("device `{0}` was not found")]
    DeviceNotFound(String),

    #[error("unsafe target: {0}")]
    UnsafeTarget(String),

    #[error("unsupported image: {0}")]
    UnsupportedImage(String),

    #[error("image needs {required} bytes but the device has {available} bytes")]
    ImageTooLarge { required: u64, available: u64 },

    #[error("confirmation phrase does not match; expected `{expected}`")]
    ConfirmationMismatch { expected: String },

    #[error("the target changed after the plan was created: {0}")]
    StalePlan(String),

    #[error("writing requires administrator/root privileges")]
    NotPrivileged,

    #[error("administrator authentication was cancelled or denied")]
    PrivilegeDenied,

    #[error("privileged writer is unavailable: {0}")]
    PrivilegedWriterUnavailable(String),

    #[error("privileged write failed: {0}")]
    PrivilegedWriteFailed(String),

    #[error("this platform adapter is not implemented yet: {0}")]
    PlatformUnavailable(String),

    #[error("invalid data from `{program}`: {message}")]
    InvalidToolOutput { program: String, message: String },

    #[error("network request to {url} failed: {message}")]
    Network { url: String, message: String },

    #[error("invalid distribution catalog data: {0}")]
    InvalidCatalog(String),

    #[error("download refused: {0}")]
    InvalidDownload(String),

    #[error("download manager error: {0}")]
    DownloadManager(String),

    #[error("could not open the default browser: {0}")]
    BrowserOpen(String),

    #[error("operation cancelled safely")]
    OperationCancelled,

    #[error("not enough free space: {required} bytes required, {available} bytes available")]
    InsufficientSpace { required: u64, available: u64 },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> Error {
    Error::Io {
        path: path.into(),
        source,
    }
}
