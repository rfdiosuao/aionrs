use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use aion_config::shell::{ResolvedShell, resolve_shell, shell_command_builder};
use aion_process::CommandRunner;
use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

pub struct ExecCommandTool {
    cwd: PathBuf,
    runtime_env: HashMap<String, String>,
    default_shell: Option<ResolvedShell>,
}

impl ExecCommandTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            runtime_env: HashMap::new(),
            default_shell: None,
        }
    }

    pub fn new_with_env(cwd: PathBuf, runtime_env: Vec<(String, String)>) -> Self {
        Self {
            cwd,
            runtime_env: runtime_env.into_iter().collect(),
            default_shell: None,
        }
    }

    /// Use the same default shell as the session's system prompt.
    pub fn with_default_shell(mut self, shell: ResolvedShell) -> Self {
        self.default_shell = Some(shell);
        self
    }
}

fn render_exit_result(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    format!("Exit code: {exit_code}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")
}

fn render_timeout_result(timeout_ms: u64, stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    format!("Command timed out after {timeout_ms}ms\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")
}

#[async_trait]
impl Tool for ExecCommandTool {
    fn name(&self) -> &str {
        "ExecCommand"
    }

    fn description(&self) -> &str {
        "Executes a shell command and returns its output.\n\n\
         IMPORTANT: Do NOT use ExecCommand when a dedicated tool is available:\n\
         - File search: use Glob (not find or ls)\n\
         - Content search: use Grep (not grep or rg)\n\
         - Read files: use Read (not cat, head, or tail)\n\
         - Edit files: use Edit (not sed or awk)\n\
         - Write files: use Write (not echo or cat with heredoc)\n\n\
         # Instructions\n\
         - Use absolute paths to avoid working directory confusion.\n\
         - Run independent commands separately. For dependent steps, use the selected shell's syntax. \
         Windows PowerShell 5.1 does not support `&&` or `||`; check `$?` explicitly or use separate calls.\n\
         - You may specify an optional timeout in milliseconds (default 120000, max 600000).\n\n\
         # Git safety\n\
         - Never force push, reset --hard, or use --no-verify unless explicitly asked.\n\
         - Prefer creating new commits over amending existing ones."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "cmd": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "shell": {
                    "type": "string",
                    "description": "Omit to use the session default. Overrides: auto (platform default), powershell (Windows PowerShell on Windows), pwsh, cmd, bash, zsh, sh, or an executable path."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 120000, max 600000)"
                }
            },
            "required": ["cmd"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(command) = input["cmd"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: cmd".to_string(),
                is_error: true,
            };
        };

        let requested = input["shell"].as_str().map(str::trim).filter(|shell| !shell.is_empty());
        let resolved = match (requested, &self.default_shell) {
            (None, Some(shell)) => Ok(shell.clone()),
            _ => resolve_shell(requested),
        };
        let shell = match resolved {
            Ok(shell) => shell,
            Err(err) => {
                return ToolResult {
                    content: format!("Invalid shell: {}", err),
                    is_error: true,
                };
            }
        };

        tracing::info!(
            cwd = %self.cwd.display(),
            shell_kind = shell.kind.name(),
            shell_path = %shell.path.display(),
            "ExecCommandTool executing"
        );

        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        let timeout = Duration::from_millis(timeout_ms);

        let cwd = self.cwd.clone();
        let mut command_builder = shell_command_builder(&shell, command, false);
        command_builder.envs(&self.runtime_env).current_dir(&cwd);

        let result = CommandRunner::new(command_builder).timeout(timeout).run().await;

        match result {
            Ok(result) if result.timed_out => ToolResult {
                content: render_timeout_result(timeout_ms, &result.stdout, &result.stderr),
                is_error: true,
            },
            Ok(result) => {
                let exit_code = result.exit_code.unwrap_or(-1);
                ToolResult {
                    content: render_exit_result(exit_code, &result.stdout, &result.stderr),
                    is_error: exit_code != 0,
                }
            }
            Err(err) => ToolResult {
                content: format!("Failed to execute command: {}", err),
                is_error: true,
            },
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    fn describe(&self, input: &Value) -> String {
        let cmd = input.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
        format!("Execute: {}", crate::truncate_utf8(cmd, 80))
    }
}

#[cfg(test)]
#[path = "exec_command_test.rs"]
mod exec_command_test;
