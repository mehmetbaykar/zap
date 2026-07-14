//! Re-exports the error-reporting core from the `warp_errors` leaf crate, plus
//! `ErrorExt` implementations for third-party error types used across the app.
//!
//! The macros and machinery now live in `warp_errors` so any workspace crate can
//! report errors without a dependency cycle with `warp_core`. These re-exports
//! preserve the historical `warp_core::errors::…` and `report_error!`
//! paths.

pub use warp_errors::{
    register_error, report_error, report_if_error, AnyhowErrorExt, ErrorExt, ErrorRegistration,
    RegisteredError, ReportErrorLogMode, LOG_TARGET,
};
