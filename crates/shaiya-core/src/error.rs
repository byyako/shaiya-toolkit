//! The toolkit's one error type.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    Io { path: Option<PathBuf>, source: std::io::Error },
    /// A file did not parse. `offset` is the byte where reading failed.
    Format { file: String, offset: Option<u64>, what: String },
    /// Valid data this build does not handle (an unknown episode, a SAH version above 1).
    Unsupported(String),
    /// A safety rule stopped the operation (output exists, would write the input without a backup, ...).
    Refused(String),
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Io,
    Format,
    Unsupported,
    Refused,
    Cancelled,
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Io { .. } => ErrorKind::Io,
            Error::Format { .. } => ErrorKind::Format,
            Error::Unsupported(_) => ErrorKind::Unsupported,
            Error::Refused(_) => ErrorKind::Refused,
            Error::Cancelled => ErrorKind::Cancelled,
        }
    }

    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Error {
        Error::Io { path: Some(path.into()), source }
    }

    pub fn format(file: impl Into<String>, offset: Option<u64>, what: impl Into<String>) -> Error {
        Error::Format { file: file.into(), offset, what: what.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path: Some(p), source } => write!(f, "{}: {}", p.display(), source),
            Error::Io { path: None, source } => write!(f, "{source}"),
            Error::Format { file, offset: Some(o), what } => write!(f, "{file}: {what} (at byte {o})"),
            Error::Format { file, offset: None, what } => write!(f, "{file}: {what}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Refused(s) => write!(f, "refused: {s}"),
            Error::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Error {
        Error::Io { path: None, source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_forms() {
        let e = Error::format("Item.SData", Some(40), "SEED checksum failed");
        assert_eq!(e.to_string(), "Item.SData: SEED checksum failed (at byte 40)");
        assert_eq!(e.kind(), ErrorKind::Format);
        assert_eq!(Error::format("x", None, "bad").to_string(), "x: bad");
        let io = Error::io("a/b.txt", std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert_eq!(io.to_string(), format!("{}: gone", std::path::Path::new("a/b.txt").display()));
        assert_eq!(io.kind(), ErrorKind::Io);
        assert_eq!(Error::Refused("output exists".into()).to_string(), "refused: output exists");
        assert_eq!(Error::Unsupported("SAH version 7".into()).kind(), ErrorKind::Unsupported);
        assert_eq!(Error::Cancelled.to_string(), "cancelled");
        let plain: Error = std::io::Error::new(std::io::ErrorKind::Other, "boom").into();
        assert_eq!(plain.to_string(), "boom");
        assert!(std::error::Error::source(&plain).is_some());
    }
}
