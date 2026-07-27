//! OA domain layer: the swappable backend abstraction + a local demo
//! implementation backed by SQLite.
//!
//! This is the OA counterpart to lakemind's DuckDB layer: tools call the
//! [`OaBackend`] trait, and the concrete implementation ([`LocalOaBackend`] in
//! M1, a real DingTalk/Feishu/WeCom adapter later) decides where the data
//! actually lives. Swapping backends never touches the tool layer.

pub mod backend;
pub mod models;
pub mod seed;

// Public re-exports of the OA domain types. M1 consumes them via the trait
// methods on `OaBackend` rather than directly, but they're part of the module's
// public surface for future tools / commands / tests.
#[allow(unused_imports)]
pub use backend::{LocalOaBackend, OaBackend};
#[allow(unused_imports)]
pub use models::{Employee, LeaveRequest, LeaveStatus};
