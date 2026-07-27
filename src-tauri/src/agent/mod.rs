//! Agent orchestration: rig tools, wire protocol, streaming runner, and the
//! single public entry point [`run_agent_chat_stream`].
//!
//! (Migrated from lakemind's `agent/` module with the data-analysis specifics
//! stripped: the 16 SQL/OKF tools are gone, replaced by OA tools; the
//! `Segment` wire type no longer carries `SqlResult`/`Chart`; the streaming
//! runner, the rate-limit retry, the abort handling, and the usage accounting
//! are preserved verbatim — they are domain-agnostic.)
//!
//! Sub-modules:
//! - [`wire`]     — frontend-facing segment/event types
//! - [`events`]   — emit_* helpers + tool-call id generation
//! - [`config`]   — settings.json + provider lookup + endpoint sanitizing
//! - [`llm`]      — one-shot LLM completion + connection test
//! - [`error`]    — shared ToolError
//! - [`tools`]    — the OA rig Tool implementations
//! - [`runner`]   — streaming driver + agent assembly + public entry point

mod config;
mod error;
mod events;
mod llm;
mod runner;
mod tools;
mod wire;

// Re-export the public/crate-facing API so external callers keep using
// `crate::agent::<item>` unchanged (commands.rs). `first_enabled_model` and
// `complete_one_shot` are kept available for M2 (leave-title generation,
// summary archiving) even though M1 doesn't call them yet.
#[allow(unused_imports)]
pub(crate) use config::first_enabled_model;
#[allow(unused_imports)]
pub(crate) use llm::{complete_one_shot, test_connection};
pub(crate) use runner::run_agent_chat_stream;
pub(crate) use wire::AgentStreamEvent;
