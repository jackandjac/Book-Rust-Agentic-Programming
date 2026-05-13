// Chapter 11: MCP Client — connects to the filesystem server
// See chapters/ch11-mcp.md for the full explanation.
//
// Run: cargo run --bin mcp-client -p ch11-mcp
//
// This client spawns the mcp-server binary as a child process and calls its
// tools over STDIO transport. It demonstrates the full MCP client loop:
//   1. Spawn the server
//   2. List available tools
//   3. Call a tool with parameters
//   4. Print the result

use anyhow::Result;
use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    service::RoleClient,
    transport::TokioChildProcess,
};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    // ── Step 1: Spawn the MCP server as a child process ───────────────────────
    //
    // TokioChildProcess launches the given command and connects over its stdio
    // pipes. The MCP handshake (initialize / initialized) happens automatically.
    let server_binary = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("mcp-server");

    tracing::info!("Spawning MCP server: {}", server_binary.display());

    let transport = TokioChildProcess::new(
        tokio::process::Command::new(&server_binary)
    )?;

    // serve() on RoleClient performs the MCP initialization handshake.
    // The returned RunningService has a .peer() handle for making requests.
    let client = ().serve(transport).await?;
    let peer = client.peer().clone();

    tracing::info!("Connected to MCP server");

    // ── Step 2: List available tools ──────────────────────────────────────────
    let tools = peer.list_tools(None).await?;
    println!("Available tools ({}):", tools.tools.len());
    for tool in &tools.tools {
        println!(
            "  {} — {}",
            tool.name,
            tool.description.as_deref().unwrap_or("(no description)")
        );
    }
    println!();

    // ── Step 3: Call list_dir ─────────────────────────────────────────────────
    println!("Calling list_dir(\".\")...");
    let result = peer.call_tool(
        CallToolRequestParams::new("list_dir")
            .with_arguments(json!({ "path": "." }).as_object().unwrap().clone()),
    ).await?;

    for content in &result.content {
        if let Some(text) = content.as_text() {
            println!("{text}");
        }
    }
    println!();

    // ── Step 4: Read Cargo.toml ───────────────────────────────────────────────
    println!("Calling read_file(\"Cargo.toml\")...");
    let result = peer.call_tool(
        CallToolRequestParams::new("read_file")
            .with_arguments(json!({ "path": "Cargo.toml" }).as_object().unwrap().clone()),
    ).await?;

    for content in &result.content {
        if let Some(text) = content.as_text() {
            // Print just the first 400 chars to keep output tidy
            let preview = &text[..text.len().min(400)];
            println!("{preview}");
            if text.len() > 400 { println!("... (truncated)"); }
        }
    }

    // ── Step 5: Clean up ─────────────────────────────────────────────────────
    client.close().await?;
    Ok(())
}
