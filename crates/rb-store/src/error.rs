//! Internal conversions from foreign error types into `rb_types::Error`.

use rb_types::Error;

/// Map a `rusqlite::Error` to a storage error.
pub(crate) fn storage_err(e: rusqlite::Error) -> Error {
    Error::Storage(e.to_string())
}

/// Map a `rusqlite::Error` encountered during migration to a migration error.
pub(crate) fn migration_err(e: rusqlite::Error) -> Error {
    Error::Migration(e.to_string())
}

/// Map an I/O error to the IO variant.
pub(crate) fn io_err(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}
