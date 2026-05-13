// Chapter 11: MCP Server — Filesystem Tools
// See chapters/ch11-mcp.md for the full explanation.
//
// Run: cargo run --bin mcp-server -p ch11-mcp
//
// This binary implements an MCP server that exposes two filesystem tools:
//   - read_file: read a file from the allowed directory
//   - list_dir: list files in the allowed directory
//
// Transport: STDIO — the client spawns this binary as a child process.

use anyhow::Result;
use rmcp::{
    ServiceExt,
    handler::server::wrapper::Parameters,
    tool, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::{Path, PathBuf};

// ── Parameter types ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadFileParams {
    /// Path to the file, relative to the allowed root directory.
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListDirParams {
    /// Directory path relative to the allowed root. Use "." for root.
    path: String,
}

// ── Server implementation ─────────────────────────────────────────────────────

/// Filesystem MCP server.
///
/// All paths are sandboxed inside `ALLOWED_ROOT` — any attempt to escape via
/// `../` traversal is rejected. This is the minimal security boundary for a
/// filesystem tool server.
#[derive(Clone)]
struct FilesystemServer {
    allowed_root: PathBuf,
}

impl FilesystemServer {
    fn new(root: impl Into<PathBuf>) -> Self {
        Self { allowed_root: root.into() }
    }

    /// Resolve and validate a relative path against the allowed root.
    /// Returns an error if the resolved path escapes the root.
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let candidate = self.allowed_root.join(rel);
        // Canonicalize with Path::components() to remove `..` without
        // requiring the path to exist on disk yet.
        let resolved = candidate
            .components()
            .fold(PathBuf::new(), |mut acc, c| {
                match c {
                    std::path::Component::ParentDir => { acc.pop(); }
                    other => acc.push(other),
                }
                acc
            });

        if resolved.starts_with(&self.allowed_root) {
            Ok(resolved)
        } else {
            Err(format!("Path escape attempt: {rel}"))
        }
    }
}

// ── Tool definitions (using #[tool_router(server_handler)] macro) ─────────────
//
// This macro generates the full ServerHandler implementation:
//   - list_tools()  — returns the tool catalogue
//   - call_tool()   — dispatches to the annotated methods
//   - get_info()    — returns server name/version
//
// The #[tool] attribute on each method generates:
//   - A Tool definition with JSON schema derived from the Parameters type
//   - Routing logic in call_tool()

#[tool_router(server_handler)]
impl FilesystemServer {
    /// Read the contents of a file as UTF-8 text.
    #[tool(description = "Read a file from the filesystem. Path is relative to the server root.")]
    fn read_file(
        &self,
        Parameters(ReadFileParams { path }): Parameters<ReadFileParams>,
    ) -> String {
        match self.resolve(&path) {
            Err(e) => format!("Error: {e}"),
            Ok(full_path) => {
                std::fs::read_to_string(&full_path)
                    .unwrap_or_else(|e| format!("Error reading {path}: {e}"))
            }
        }
    }

    /// List files and directories at the given path.
    #[tool(description = "List files in a directory. Path is relative to the server root. Use '.' for the root.")]
    fn list_dir(
        &self,
        Parameters(ListDirParams { path }): Parameters<ListDirParams>,
    ) -> String {
        match self.resolve(&path) {
            Err(e) => format!("Error: {e}"),
            Ok(full_path) => {
                match std::fs::read_dir(&full_path) {
                    Err(e) => format!("Error reading directory {path}: {e}"),
                    Ok(entries) => {
                        let names: Vec<String> = entries
                            .filter_map(|e| e.ok())
                            .map(|e| {
                                let name = e.file_name().to_string_lossy().into_owned();
                                if e.path().is_dir() {
                                    format!("{name}/")
                                } else {
                                    name
                                }
                            })
                            .collect();
                        if names.is_empty() {
                            "(empty directory)".to_string()
                        } else {
                            names.join("\n")
                        }
                    }
                }
            }
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr so stdout stays clean for MCP protocol messages.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("info")
        .init();

    // Allow files under the current working directory only.
    let root = std::env::current_dir()?;
    tracing::info!("MCP filesystem server starting, root = {}", root.display());

    // stdio() reads from stdin and writes to stdout.
    // The client spawns this process and communicates over its stdio pipes.
    let service = FilesystemServer::new(root)
        .serve(stdio())
        .await?;

    service.waiting().await?;
    Ok(())
}
