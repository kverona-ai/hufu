//! Errors and the exit codes they map to.

use std::fmt;

/// The documented, stable exit codes of the `hufu` command.
pub mod exit {
    pub const OK: i32 = 0;
    pub const INTERNAL: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const JOB_FAILED: i32 = 3;
    pub const JOB_CANCELLED: i32 = 4;
    pub const JOB_TIMED_OUT: i32 = 5;
    pub const WAIT_TIMED_OUT: i32 = 6;
    pub const JOB_LOST: i32 = 7;
    pub const JOB_REJECTED: i32 = 8;
    pub const AGENT_UNAVAILABLE: i32 = 10;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The request was wrong: bad flag, bad spec, unknown profile, unsupported tier. Nothing ran.
    Usage,
    /// Something inside Hufu failed: I/O, a corrupt record.
    Internal,
    /// The agent is not installed or not logged in.
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct HufuError {
    pub kind: ErrorKind,
    pub message: String,
}

impl HufuError {
    pub fn usage(message: impl Into<String>) -> HufuError {
        HufuError { kind: ErrorKind::Usage, message: message.into() }
    }

    pub fn internal(message: impl Into<String>) -> HufuError {
        HufuError { kind: ErrorKind::Internal, message: message.into() }
    }

    pub fn unavailable(message: impl Into<String>) -> HufuError {
        HufuError { kind: ErrorKind::Unavailable, message: message.into() }
    }

    pub fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::Usage => exit::USAGE,
            ErrorKind::Internal => exit::INTERNAL,
            ErrorKind::Unavailable => exit::AGENT_UNAVAILABLE,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self.kind {
            ErrorKind::Usage => "usage",
            ErrorKind::Internal => "internal",
            ErrorKind::Unavailable => "agent_unavailable",
        }
    }
}

impl fmt::Display for HufuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HufuError {}

impl From<std::io::Error> for HufuError {
    fn from(e: std::io::Error) -> Self {
        HufuError::internal(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, HufuError>;

/// Adds the path to an I/O error, which on its own rarely says what it was about.
pub trait IoContext<T> {
    fn at(self, path: &std::path::Path) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: &std::path::Path) -> Result<T> {
        self.map_err(|e| HufuError::internal(format!("{}: {e}", path.display())))
    }
}
