use serde::{Deserialize, Serialize};

/// Unified error type for all rig Tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError(pub String);

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ToolError {}
