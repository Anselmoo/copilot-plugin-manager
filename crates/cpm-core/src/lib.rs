//! Core logic for cpm: resolver, fetcher, installer, doctor, status, and auth.
//!
//! All public functions return [`Result<T, CpmError>`]. The CLI renders errors
//! with [`miette`] — coloured, with source spans and `help:` text.

#![deny(missing_docs)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]

pub mod auth;
pub mod config;
pub mod doctor;
pub mod error;
pub mod external;
pub mod fetcher;
pub mod installer;
pub mod license;
pub mod paths;
pub mod plugin_delegate;
pub mod plugin_index;
pub mod project;
pub mod resolver;
pub mod source;
pub mod status;

pub use error::CpmError;
