// SPDX-License-Identifier: MIT OR Apache-2.0

//! ripple-reader — AI-powered academic paper discovery and reading tool.
//!
//! Fetches papers from arXiv and OpenReview, generates summaries and insights
//! using LLMs, and provides a web interface for browsing and managing
//! your reading list.

pub mod author_reputation;
pub mod config;
pub mod db;
pub mod env_editor;
pub mod extractor;
pub mod figure;
pub mod hires;
pub mod mdfmt;
pub mod mineru;
pub mod pdf;
pub mod pipeline;
pub mod processor;
pub mod query;
pub mod source;
pub mod web;
