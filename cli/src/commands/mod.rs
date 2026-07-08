//! One module per CLI command family; `main` just parses args and dispatches.

pub(crate) mod contacts;
#[cfg(unix)]
pub(crate) mod daemon;
pub(crate) mod identity;
pub(crate) mod offline;
pub(crate) mod receive;
pub(crate) mod send;
pub(crate) mod sessions;
pub(crate) mod transfers;
