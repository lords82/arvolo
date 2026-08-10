//! One module per CLI command family; `main` just parses args and dispatches.

pub(crate) mod cancel;
pub(crate) mod contacts;
pub(crate) mod daemon;
pub(crate) mod history;
pub(crate) mod identity;
pub(crate) mod offline;
pub(crate) mod pair;
pub(crate) mod receive;
pub(crate) mod resume;
pub(crate) mod send;
pub(crate) mod status;
