//! Batch Query Generator (`bqg`) core library.
//!
//! The binary is a thin shell over this library: the CLI and the web UI are two
//! front ends onto exactly the same engine. Zero external dependencies; std only.

pub mod android;
pub mod checkpoint;
pub mod config;
pub mod csv;
pub mod exec;
pub mod json;
pub mod server;
pub mod target;
pub mod template;
