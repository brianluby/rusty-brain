//! Core memory engine (Mind) for rusty-brain.
//!
//! This crate provides the [`Mind`] struct — the central API for storing,
//! searching, and retrieving observations from a memvid-backed `.mv2` memory
//! file. It also exposes [`estimate_tokens`] for token budget estimation.

mod backend;
mod context_builder;
mod file_guard;
mod memvid_store;
pub mod mind;
pub mod token;
