use crate::utils::GenericError;
use std::borrow::Cow;
use std::convert::Into;
use std::fmt;
use std::fmt::Formatter;

/// Error type for conversion related problems (e.g. Rust <-> Protobuf)
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("missing field '{0}'")]
    MissingField(&'static str),
    #[error("invalid data: {0}")]
    InvalidData(GenericError),
}

impl ConversionError {
    pub fn invalid_data(source: impl Into<GenericError>) -> Self {
        ConversionError::InvalidData(source.into())
    }

    pub fn missing_field(field: &'static str) -> Self {
        ConversionError::MissingField(field)
    }
}

/// This error code set matches the [gRPC error code set](https://github.com/grpc/grpc/blob/master/doc/statuscodes.md#status-codes-and-their-use-in-grpc),
/// representing all the error codes visible to the user code.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u16)]
pub enum UserErrorCode {
    /// The operation completed successfully.
    Ok = 0,

    /// The operation was cancelled.
    Cancelled = 1,

    /// Unknown error.
    Unknown = 2,

    /// Client specified an invalid argument.
    InvalidArgument = 3,

    /// Deadline expired before operation could complete.
    DeadlineExceeded = 4,

    /// Some requested entity was not found.
    NotFound = 5,

    /// Some entity that we attempted to create already exists.
    AlreadyExists = 6,

    /// The caller does not have permission to execute the specified operation.
    PermissionDenied = 7,

    /// Some resource has been exhausted.
    ResourceExhausted = 8,

    /// The system is not in a state required for the operation's execution.
    FailedPrecondition = 9,

    /// The operation was aborted, for example due to killing an invocation forcefully.
    Aborted = 10,

    /// Operation was attempted past the valid range.
    OutOfRange = 11,

    /// Operation is not implemented or not supported.
    Unimplemented = 12,

    /// Internal error.
    Internal = 13,

    /// The service is currently unavailable.
    Unavailable = 14,

    /// Unrecoverable data loss or corruption.
    DataLoss = 15,

    /// The request does not have valid authentication credentials.
    Unauthenticated = 16,
}

impl fmt::Display for UserErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl From<u32> for UserErrorCode {
    fn from(value: u32) -> Self {
        use UserErrorCode::*;

        match value {
            0 => Ok,
            1 => Cancelled,
            2 => Unknown,
            3 => InvalidArgument,
            4 => DeadlineExceeded,
            5 => NotFound,
            6 => AlreadyExists,
            7 => PermissionDenied,
            8 => ResourceExhausted,
            9 => FailedPrecondition,
            10 => Aborted,
            11 => OutOfRange,
            12 => Unimplemented,
            13 => Internal,
            14 => Unavailable,
            15 => DataLoss,
            16 => Unauthenticated,
            _ => Unknown,
        }
    }
}

impl From<UserErrorCode> for u32 {
    fn from(value: UserErrorCode) -> Self {
        value as u32
    }
}

/// Error codes used by Restate to carry Restate specific error codes.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u16)]
pub enum RestateErrorCode {
    // The invocation cannot be replayed due to the mismatch between the journal and the actual code.
    JournalMismatch = 32,

    // Violation of the protocol state machine.
    ProtocolViolation = 33,

    // The invocation was killed by Restate
    Killed = 64,
}

impl fmt::Display for RestateErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl From<RestateErrorCode> for u32 {
    fn from(value: RestateErrorCode) -> Self {
        value as u32
    }
}

/// Error codes representing the possible error variants that can arise during an invocation.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum InvocationErrorCode {
    User(UserErrorCode),
    Restate(RestateErrorCode),
    Unknown(u32),
}

impl fmt::Debug for InvocationErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            InvocationErrorCode::User(c) => fmt::Debug::fmt(c, f),
            InvocationErrorCode::Restate(c) => fmt::Debug::fmt(c, f),
            InvocationErrorCode::Unknown(c) => write!(f, "{}", c),
        }
    }
}

impl fmt::Display for InvocationErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl From<u32> for InvocationErrorCode {
    fn from(value: u32) -> Self {
        use InvocationErrorCode::*;
        use RestateErrorCode::*;

        if value <= 16 {
            return User(value.into());
        }
        match value {
            32 => Restate(JournalMismatch),
            33 => Restate(ProtocolViolation),
            64 => Restate(Killed),

            c => Unknown(c),
        }
    }
}

impl From<InvocationErrorCode> for u32 {
    fn from(value: InvocationErrorCode) -> Self {
        match value {
            InvocationErrorCode::User(c) => c as u32,
            InvocationErrorCode::Restate(c) => c as u32,
            InvocationErrorCode::Unknown(c) => c,
        }
    }
}

impl From<InvocationErrorCode> for UserErrorCode {
    fn from(value: InvocationErrorCode) -> Self {
        match value {
            InvocationErrorCode::User(u) => u,
            InvocationErrorCode::Restate(RestateErrorCode::Killed) => UserErrorCode::Aborted,
            _ => UserErrorCode::Unknown,
        }
    }
}

impl From<UserErrorCode> for InvocationErrorCode {
    fn from(value: UserErrorCode) -> Self {
        InvocationErrorCode::User(value)
    }
}

impl From<RestateErrorCode> for InvocationErrorCode {
    fn from(value: RestateErrorCode) -> Self {
        InvocationErrorCode::Restate(value)
    }
}

/// This struct represents errors arisen when processing a service invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationError {
    code: InvocationErrorCode,
    message: Cow<'static, str>,
    description: Option<Cow<'static, str>>,
}

pub const UNKNOWN_INVOCATION_ERROR: InvocationError =
    InvocationError::new_static(InvocationErrorCode::User(UserErrorCode::Unknown), "unknown");

impl Default for InvocationError {
    fn default() -> Self {
        UNKNOWN_INVOCATION_ERROR
    }
}

impl fmt::Display for InvocationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "[{:?}] {}", self.code(), self.message())?;
        if self.description.is_some() {
            write!(f, ".\n{}", self.description().unwrap())?;
        }
        Ok(())
    }
}

impl std::error::Error for InvocationError {}

impl InvocationError {
    pub const fn new_static(code: InvocationErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: Cow::Borrowed(message),
            description: None,
        }
    }

    pub fn new(code: impl Into<InvocationErrorCode>, message: String) -> Self {
        Self {
            code: code.into(),
            message: Cow::Owned(message),
            description: None,
        }
    }

    pub fn new_with_description(
        code: impl Into<InvocationErrorCode>,
        message: String,
        description: String,
    ) -> Self {
        Self {
            code: code.into(),
            message: Cow::Owned(message),
            description: Some(Cow::Owned(description)),
        }
    }

    pub fn code(&self) -> InvocationErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

#[cfg(feature = "tonic_conversions")]
mod tonic_conversions_impl {
    use super::{InvocationError, InvocationErrorCode};
    use crate::errors::UserErrorCode;
    use tonic::{Code, Status};

    impl From<UserErrorCode> for Code {
        fn from(value: UserErrorCode) -> Self {
            match value {
                UserErrorCode::Ok => Code::Ok,
                UserErrorCode::Cancelled => Code::Cancelled,
                UserErrorCode::Unknown => Code::Unknown,
                UserErrorCode::InvalidArgument => Code::InvalidArgument,
                UserErrorCode::DeadlineExceeded => Code::DeadlineExceeded,
                UserErrorCode::NotFound => Code::NotFound,
                UserErrorCode::AlreadyExists => Code::AlreadyExists,
                UserErrorCode::PermissionDenied => Code::PermissionDenied,
                UserErrorCode::ResourceExhausted => Code::ResourceExhausted,
                UserErrorCode::FailedPrecondition => Code::FailedPrecondition,
                UserErrorCode::Aborted => Code::Aborted,
                UserErrorCode::OutOfRange => Code::OutOfRange,
                UserErrorCode::Unimplemented => Code::Unimplemented,
                UserErrorCode::Internal => Code::Internal,
                UserErrorCode::Unavailable => Code::Unavailable,
                UserErrorCode::DataLoss => Code::DataLoss,
                UserErrorCode::Unauthenticated => Code::Unauthenticated,
            }
        }
    }

    impl From<InvocationErrorCode> for Code {
        fn from(value: InvocationErrorCode) -> Self {
            UserErrorCode::from(value).into()
        }
    }

    impl From<InvocationError> for Status {
        fn from(value: InvocationError) -> Self {
            Status::new(value.code().into(), value.message)
        }
    }
}
