//! Serde models for the Claude Code hook input and output payloads.

#![allow(dead_code)] // types used in Tasks 9-11 (post_hook, pre_hook)

use serde::{Deserialize, Serialize};

/// Subset of the Claude Code hook JSON payload used by both pre- and post-hooks.
#[derive(Debug, Deserialize)]
pub struct HookPayload {
    pub tool_name: Option<String>,
    pub tool_input: Option<ToolInput>,
    pub tool_response: Option<ToolResponse>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ToolInput {
    pub command: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ToolResponse {
    pub exit_code: Option<i64>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

/// Output from a post-hook that wants to send a system message to Claude.
#[derive(Debug, Serialize)]
pub struct PostHookOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(rename = "systemMessage", skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
}

impl PostHookOutput {
    /// Allows hook processing and sends `msg` to Claude as a system message.
    pub fn allow_with_message(msg: impl Into<String>) -> Self {
        Self {
            decision: Some("allow".to_string()),
            system_message: Some(msg.into()),
        }
    }

    /// Allows hook processing without sending a system message.
    pub fn silent() -> Self {
        Self {
            decision: Some("allow".to_string()),
            system_message: None,
        }
    }
}
