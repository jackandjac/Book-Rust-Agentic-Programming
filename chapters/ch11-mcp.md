# Chapter 11: MCP — Model Context Protocol in Rust

> **Framework versions in this chapter:**  
> `rmcp = "1.6"` (9.7M downloads — the only stable 1.x Rust MCP crate)  
> `schemars = "1"`, `serde = "1"`, `tokio = "1"`, `anyhow = "1"`
>
> **Java reference:** Spring AI MCP starters (`spring-ai-mcp-server-spring-boot-starter`, `spring-ai-mcp-client-spring-boot-starter`)

---

The Model Context Protocol (MCP) is an open standard from Anthropic that defines how AI agents discover and call tools exposed by external processes. Where rig's `#[rig_tool]` attribute wires tools directly into an agent binary, MCP separates the tool server from the agent: a Python script, a Rust binary, or a remote HTTP service can all expose the same standardised tool interface, and any MCP-capable client can call them.

This matters for production systems. Your tools may be maintained by different teams, written in different languages, deployed as microservices, or shared across multiple agents. MCP is the standardisation layer that makes this composition possible.

---

## 11.1 MCP Concepts

MCP defines four primitive types:

| Primitive | Description |
|-----------|-------------|
| **Tool** | A callable function with a JSON schema for parameters |
| **Resource** | A readable data source (file, database record, API response) |
| **Prompt** | A reusable prompt template with parameters |
| **Sampling** | (advanced) The server can request an LLM completion from the client |

For agentic applications, **Tools** are the primary concern — everything else is secondary.

### Protocol flow

```
Client                          Server
  │──── initialize ────────────▶  │  (handshake — name, version, capabilities)
  │◀─── initialized ────────────  │
  │                               │
  │──── tools/list ─────────────▶ │  (discovery)
  │◀─── [Tool, Tool, Tool] ──────  │
  │                               │
  │──── tools/call ─────────────▶ │  (invocation)
  │◀─── CallToolResult ──────────  │
```

The handshake and discovery steps happen automatically — you see only the tool definition and tool call in application code.

### Transports

MCP is transport-agnostic. The `rmcp` crate provides:

| Transport | Feature flag | Use case |
|-----------|-------------|---------|
| STDIO | `transport-io` (server) / `transport-child-process` (client) | Local tools — client spawns server as child process |
| HTTP streaming | `transport-streamable-http-server` / `transport-streamable-http-client-reqwest` | Remote tools over HTTP |

STDIO is the standard for local development and CLI tools. HTTP is used for deployed services.

### Java comparison

Spring AI's MCP support:

```java
// Spring AI — MCP server (Java)
@Bean
public McpSyncServerExchange toolServer() {
    return McpSyncServerExchange.builder()
        .serverInfo("filesystem-server", "1.0.0")
        .tool(new ReadFileTool(), new ListDirTool())
        .build();
}
```

The rmcp equivalent uses the `#[tool_router(server_handler)]` macro to achieve the same thing with less boilerplate.

---

## 11.2 Building an MCP Server

An MCP server in rmcp is a Rust struct that implements `ServerHandler`. In practice, you almost never write that implementation by hand — the `#[tool_router(server_handler)]` macro generates it for you.

### Minimal server

```toml
[dependencies]
rmcp = { version = "1.6", features = ["server", "macros", "transport-io"] }
schemars = "1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
```

```rust
use rmcp::{ServiceExt, handler::server::wrapper::Parameters, tool, tool_router, transport::stdio};
use schemars::JsonSchema;
use serde::Deserialize;

// Parameter types derive JsonSchema — rmcp generates the tool's input_schema from this.
#[derive(Debug, Deserialize, JsonSchema)]
struct AddParams {
    /// First number
    a: i64,
    /// Second number
    b: i64,
}

#[derive(Clone)]
struct Calculator;

// #[tool_router(server_handler)] generates the full ServerHandler implementation:
//   - list_tools()  — builds the tool catalogue from #[tool] methods
//   - call_tool()   — dispatches requests to the correct method
//   - get_info()    — returns server name/version
#[tool_router(server_handler)]
impl Calculator {
    #[tool(description = "Add two integers and return their sum")]
    fn add(&self, Parameters(AddParams { a, b }): Parameters<AddParams>) -> String {
        (a + b).to_string()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdio() reads MCP protocol messages from stdin; writes responses to stdout.
    let service = Calculator.serve(stdio()).await?;
    service.waiting().await?;   // Block until the client disconnects
    Ok(())
}
```

The `Parameters<P>` wrapper is rmcp's mechanism for injecting tool parameters. The pattern `Parameters(AddParams { a, b })` destructures the params struct inline.

> **Log to stderr.** In STDIO mode, stdout carries MCP protocol messages. Any `println!` or log output on stdout will corrupt the protocol. Always configure your logger to write to stderr:
> ```rust
> tracing_subscriber::fmt()
>     .with_writer(std::io::stderr)
>     .init();
> ```

### The `#[tool]` attribute

Each `#[tool]`-annotated method becomes a callable MCP tool. The macro:
- Derives the `input_schema` from the `Parameters<T>` type using `schemars`
- Uses the `description` string as the tool's human-readable description
- Routes `call_tool` requests by matching the tool name

Optional `#[tool]` fields:
- `description = "..."` — tool description (recommended)
- `name = "..."` — override the tool name (defaults to the method name)

### Returning errors

Return a `String` from `#[tool]` methods — for errors, return an error string rather than using `Result`. The MCP protocol has an `is_error` flag in the response; rmcp sets it automatically when the result starts with `"Error:"`.

For cleaner error handling, return `Result<String, String>` — rmcp maps `Err(msg)` to an error response:

```rust
#[tool(description = "Divide a by b")]
fn divide(
    &self,
    Parameters(DivideParams { a, b }): Parameters<DivideParams>,
) -> Result<String, String> {
    if b == 0 {
        Err("Division by zero".to_string())
    } else {
        Ok((a / b).to_string())
    }
}
```

---

## 11.3 The ServerHandler Trait

When you need capabilities beyond tools (resources, prompts), implement `ServerHandler` manually alongside `#[tool_router]`:

```rust
use rmcp::{
    handler::server::ServerHandler,
    model::*,
    service::RequestContext,
    tool_router,
};

#[tool_router]
impl MyServer { /* #[tool] methods here */ }

impl ServerHandler for MyServer {
    // get_info() provides the server's identity to connecting clients.
    fn get_info(&self) -> Implementation {
        Implementation {
            name: "my-server".to_string(),
            version: "1.0.0".to_string(),
        }
    }

    // Override any methods you need; all have default no-op implementations.
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult { resources: vec![], next_cursor: None, meta: None })
    }
}
```

`ServerHandler` is not dyn-compatible; you use it as a concrete type, not a trait object.

---

## 11.4 Building an MCP Client

An MCP client spawns or connects to a server, discovers its tools, and calls them.

```toml
[dependencies]
rmcp = { version = "1.6", features = ["client", "macros", "transport-child-process"] }
```

### STDIO client (child process)

```rust
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // TokioChildProcess spawns the server and connects over its stdio pipes.
    let transport = TokioChildProcess::new(
        tokio::process::Command::new("./target/debug/mcp-server")
    )?;

    // serve() performs the MCP initialize / initialized handshake.
    let client = ().serve(transport).await?;
    let peer = client.peer().clone();

    // Discover available tools
    let tools = peer.list_tools(None).await?;
    for tool in &tools.tools {
        println!("Tool: {} — {}", tool.name, tool.description.as_deref().unwrap_or(""));
    }

    // Call the "add" tool
    let result = peer.call_tool(
        CallToolRequestParams::new("add")
            .with_arguments(
                json!({ "a": 21, "b": 21 }).as_object().unwrap().clone()
            ),
    ).await?;

    for content in &result.content {
        if let Some(text) = content.as_text() {
            println!("Result: {text}");  // "42"
        }
    }

    client.close().await?;
    Ok(())
}
```

### Reading `CallToolResult`

```rust
pub struct CallToolResult {
    pub content: Vec<Content>,   // tool output
    pub is_error: Option<bool>,  // true if the tool returned an error
    pub meta: Option<Value>,
}
```

Each `Content` can be text, an image, an embedded resource, or a tool result. For text-only tools:

```rust
for content in &result.content {
    match content.as_text() {
        Some(text) => println!("{text}"),
        None => println!("(non-text content)"),
    }
}
```

---

## 11.5 Using MCP Tools from a Rig Agent

MCP and rig serve complementary roles. There is no native rig→MCP bridge in rmcp 1.6 — the pattern is to call MCP tools from rig tools:

```rust
use rig::providers::openai;
use rig::client::CompletionClient;
use rig_derive::rig_tool;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use std::sync::Arc;
use tokio::sync::Mutex;

// Wrap the MCP peer in a rig tool.
// The tool spawns (or reuses) the MCP server connection and calls a tool.
struct McpFilesystemTool {
    peer: Arc<rmcp::service::Peer<rmcp::service::RoleClient>>,
}

#[rig_tool(
    description = "Read a file using the MCP filesystem server",
    params(path = "Relative path to the file")
)]
async fn read_file_via_mcp(
    tool: &McpFilesystemTool,
    path: String,
) -> Result<String, String> {
    let result = tool.peer.call_tool(
        CallToolRequestParams::new("read_file")
            .with_arguments(
                serde_json::json!({ "path": path })
                    .as_object().unwrap().clone()
            ),
    ).await.map_err(|e| e.to_string())?;

    Ok(result.content
        .iter()
        .filter_map(|c| c.as_text())
        .collect::<Vec<_>>()
        .join("\n"))
}
```

Then add `McpFilesystemTool` to a rig agent as a tool (Chapter 4 pattern). This bridges MCP's standardised protocol into rig's tool-calling system.

> **Note:** A native rig–MCP integration is planned for a future rig-core release. For production systems requiring deep integration, check the rig-core changelog for updates.

---

## 11.6 Hands-On: Filesystem MCP Server + Client

The complete example in `code-examples/ch11-mcp/` has two binaries:
- `mcp-server` — exposes `read_file` and `list_dir` tools with path sandboxing
- `mcp-client` — spawns the server, lists tools, and calls them

### Building and running

```bash
cd code-examples
cargo build -p ch11-mcp

# Terminal 1: you don't need to start the server manually —
# the client spawns it. But you can test the server directly:
cargo run --bin mcp-server -p ch11-mcp

# Terminal 2: run the client (it spawns the server automatically)
cargo run --bin mcp-client -p ch11-mcp
```

Expected output:

```
Available tools (2):
  read_file — Read a file from the filesystem. Path is relative to the server root.
  list_dir  — List files in a directory. Path is relative to the server root. Use '.' for the root.

Calling list_dir(".")...
Cargo.toml
src/

Calling read_file("Cargo.toml")...
[package]
name = "ch11-mcp"
...
```

### Path sandboxing

The server's `resolve()` method normalises `../` components and rejects any path that escapes the allowed root. This is a minimal but essential security boundary for filesystem tools:

```rust
fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
    let candidate = self.allowed_root.join(rel);
    let resolved = candidate.components().fold(PathBuf::new(), |mut acc, c| {
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
```

---

## 11.7 HTTP Transport

For deployed services, rmcp supports HTTP streaming transport:

```toml
rmcp = { version = "1.6", features = [
    "server",
    "macros",
    "transport-streamable-http-server",
    "transport-streamable-http-client-reqwest",
] }
```

The HTTP server integrates with Axum:

```rust
use rmcp::transport::streamable_http_server::{
    StreamableHttpService, StreamableHttpServiceConfig,
};
use axum::Router;

let mcp_service = StreamableHttpService::new(
    || async { Ok(MyServer::new()) },
    StreamableHttpServiceConfig::default(),
);

let app = Router::new()
    .nest_service("/mcp", mcp_service);

// Standard Axum serve (same as Chapter 7)
let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
axum::serve(listener, app).await?;
```

The client connects with a URL:

```rust
use rmcp::transport::StreamableHttpClientTransport;

let transport = StreamableHttpClientTransport::from_uri("http://localhost:8080/mcp");
let client = ().serve(transport).await?;
```

---

## 11.8 Key Takeaways

- **MCP** standardises how AI agents discover and call tools across process and language boundaries.
- **`#[tool_router(server_handler)]`** on an `impl` block generates the full `ServerHandler` — you only write the tool methods.
- **`#[tool(description = "...")]`** marks a method as an MCP tool; `Parameters<T>` injects the deserialized params.
- **`schemars::JsonSchema`** on the params struct generates the input schema automatically — same pattern as rig's structured output (Chapter 5).
- **Log to stderr** in STDIO servers — stdout carries MCP protocol frames.
- **`TokioChildProcess`** spawns the server binary and connects over its stdio pipes.
- **`peer.list_tools()`** discovers tools; **`peer.call_tool(CallToolRequestParams::new(name).with_arguments(...))`** calls them.
- **`content.as_text()`** extracts text from a `CallToolResult`.
- **No native rig→MCP bridge** in rmcp 1.6 — bridge via a rig tool that calls the MCP peer.
- **STDIO** (`transport-io` / `transport-child-process`) for local tools; **HTTP** (`transport-streamable-http-*`) for deployed services.

---

## What's Next

This chapter showed how to expose tools via a standard protocol. Part IV shifts to orchestration: Chapter 12 introduces `graph-flow`, which lets you build multi-step agent workflows as directed graphs — each node is a task, edges are routing decisions, and the runner manages state persistence across steps.

---

*→ Java reference: Spring AI `spring-ai-mcp-server-spring-boot-starter` and `spring-ai-mcp-client-spring-boot-starter`; Claude Desktop MCP configuration*
