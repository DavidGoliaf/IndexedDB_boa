//! Error types for the IndexedDB core engine.

use thiserror::Error;

/// General IndexedDB specification errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum IdbError {
    /// Transaction was aborted.
    #[error("Transaction was aborted")]
    Abort,

    /// Constraint violation (e.g. unique index, duplicate name).
    #[error("Constraint violation: {0}")]
    Constraint(String),

    /// Data clone error (value cannot be serialized).
    #[error("Data clone error: {0}")]
    DataClone(String),

    /// Data error (invalid key, key path, etc.).
    #[error("Data error: {0}")]
    Data(String),

    /// Invalid access (e.g. empty store list, wrong cursor type).
    #[error("Invalid access: {0}")]
    InvalidAccess(String),

    /// Invalid state (e.g. calling method on deleted object).
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Not found (e.g. unknown store/index name).
    #[error("Not found: {0}")]
    NotFound(String),

    /// Not readable (e.g. corrupted data).
    #[error("Not readable: {0}")]
    NotReadable(String),

    /// Syntax error (e.g. invalid key path).
    #[error("Syntax error: {0}")]
    Syntax(String),

    /// Transaction is read-only.
    #[error("Transaction is read-only")]
    ReadOnly,

    /// Transaction is inactive.
    #[error("Transaction is inactive")]
    TransactionInactive,

    /// Unknown error.
    #[error("Unknown error: {0}")]
    Unknown(String),

    /// Version error.
    #[error("Version error: {0}")]
    Version(String),

    /// Storage quota exceeded.
    #[error("Quota exceeded: needed {needed} bytes, available {available} bytes")]
    QuotaExceeded {
        /// Bytes needed for the operation.
        needed: u64,
        /// Bytes currently available.
        available: u64,
    },
}

/// Errors during key validation and encoding.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyError {
    /// Invalid key type (e.g. function, symbol).
    #[error("Invalid key type: {0}")]
    InvalidType(String),

    /// Invalid key value (e.g. NaN).
    #[error("Invalid key value: {0}")]
    InvalidValue(String),

    /// Encoded key exceeds maximum allowed length.
    #[error("Key exceeds maximum allowed length of {limit} bytes (got {actual})")]
    KeyTooLarge {
        /// Maximum allowed length in bytes.
        limit: usize,
        /// Actual encoded length in bytes.
        actual: usize,
    },

    /// Key array nesting depth exceeds maximum.
    #[error("Key array nesting depth exceeds maximum allowed of {0}")]
    MaxDepthExceeded(usize),

    /// Corrupted key encoding.
    #[error("Corrupted key encoding: {0}")]
    InvalidEncoding(String),

    /// Unexpected end of key buffer.
    #[error("Unexpected end of key buffer")]
    UnexpectedEof,
}

/// Errors in key path syntax and application.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyPathError {
    /// Invalid key path syntax.
    #[error("Invalid key path syntax: {0}")]
    InvalidSyntax(String),

    /// Key path array cannot be empty.
    #[error("Key path array cannot be empty")]
    EmptyArray,

    /// Cannot inject key into a primitive value.
    #[error("Cannot inject key into primitive value")]
    CannotInjectIntoPrimitive,
}

/// Errors during SCF-v1 serialization/deserialization.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScError {
    /// Data clone error (unsupported value type).
    #[error("Data clone error: {0}")]
    DataClone(String),

    /// Unsupported format version.
    #[error("Format version {0} is not supported (expected v1)")]
    UnsupportedVersion(u8),

    /// Invalid SCF magic bytes.
    #[error("Invalid SCF magic bytes: expected 'IDB1'")]
    InvalidMagic,

    /// CRC32C checksum mismatch.
    #[error("CRC32C checksum mismatch: expected {expected:#010x}, calculated {calculated:#010x}")]
    ChecksumMismatch {
        /// Expected checksum from the trailer.
        expected: u32,
        /// Calculated checksum from the data.
        calculated: u32,
    },

    /// Value nesting depth exceeds maximum.
    #[error("Value nesting depth exceeds maximum of {0}")]
    MaxDepthExceeded(usize),

    /// Serialized value size exceeds limit.
    #[error("Serialized value size exceeds limit of {limit} bytes (got {actual})")]
    ValueTooLarge {
        /// Maximum allowed size in bytes.
        limit: usize,
        /// Actual size in bytes.
        actual: usize,
    },

    /// Invalid memo reference ID.
    #[error("Invalid memo reference ID: {0}")]
    InvalidMemoRef(usize),

    /// Unknown or reserved SCF tag.
    #[error("Unknown or reserved SCF tag: {0:#04x}")]
    UnknownTag(u8),

    /// Corrupted SCF payload.
    #[error("Corrupted SCF payload: {0}")]
    CorruptedPayload(String),

    /// Unexpected end of SCF stream.
    #[error("Unexpected end of SCF stream")]
    UnexpectedEof,
}

impl From<KeyError> for IdbError {
    fn from(e: KeyError) -> Self {
        IdbError::Data(e.to_string())
    }
}

impl From<KeyPathError> for IdbError {
    fn from(e: KeyPathError) -> Self {
        IdbError::Data(e.to_string())
    }
}

impl From<ScError> for IdbError {
    fn from(e: ScError) -> Self {
        IdbError::DataClone(e.to_string())
    }
}
