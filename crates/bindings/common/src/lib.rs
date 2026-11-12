#![forbid(unsafe_code)]
//! Shared abstractions for Obrain language bindings.
//!
//! This crate provides language-agnostic implementations of common binding
//! logic: entity extraction from query results, error classification, and
//! JSON-to-Value conversion. Each language binding (Python, Node.js, C, WASM)
//! depends on this crate and maps the generic types to its FFI layer.

pub mod bail;
pub mod entity;
pub mod error;
pub mod json;
