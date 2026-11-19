//! Rust contracts for planned RHAF management routes. These declarations do not install handlers.
pub mod models;
pub mod openapi;

/// OpenAPI generated from the Rust declarations during this crate's build.
pub const OPENAPI_JSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/openapi.json"));
