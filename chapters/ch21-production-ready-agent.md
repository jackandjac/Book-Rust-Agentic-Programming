# Chapter 21: The Production-Ready Rust AI Agent

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` · `swiftide = "0.32"` · `graph-flow = "0.5.1"` · `rmcp = "1.6"`  
> `tracing = "0.1"` · `governor = "0.10"` · `axum = "0.8"` · `tokio = "1"`
>
> **Java reference:** Spring Boot production hardening, Micrometer, Spring Security (Chapter 24 of Java book)

---

Building an agent that works in development is one thing. Shipping it — and keeping it running — is another. This final chapter consolidates every production concern from the book into a single reference architecture.

The chapter is structured as a checklist: each section identifies the risk, shows the mitigation, and points back to the earlier chapter where the full implementation lives.

---

## 21.1 The Production Checklist

Before any Rust AI agent goes to production, it should pass all of these checks:

| Category | Check | Chapter |
|---|---|---|
| **Build** | `[profile.release]` with `lto`, `strip`, `panic="abort"` | Ch 17 |
| **Build** | Multi-stage Docker; runtime image ≤ 30 MB | Ch 17 |
| **Observability** | Structured JSON logs via `tracing` + `fmt().json()` | Ch 16 |
| **Observability** | Distributed traces via `tracing-opentelemetry` | Ch 16 |
| **Observability** | Token usage logged on every LLM call | Ch 16 |
| **Security** | Prompt injection detection on all user inputs | Ch 16 |
| **Security** | Input length limits enforced before LLM call | Ch 16 |
| **Security** | API keys read from env / secrets manager, never hardcoded | Ch 17 |
| **Reliability** | Rate limiting via `governor` | Ch 16 |
| **Reliability** | Concurrency limits via `tokio::sync::Semaphore` | Ch 17 |
| **Reliability** | Error handling with `anyhow` / `thiserror`; no `.unwrap()` in prod paths | Ch 2 |
| **Cost** | Token budget with `AtomicU64` counter | Ch 16 |
| **Cost** | Cost estimation logged per request | Ch 16 |
| **Scaling** | Stateless design or shared external state (Redis / PostgreSQL) | Ch 17 |
| **Deployment** | Health check endpoint returning 200 | This chapter |
| **Deployment** | Graceful shutdown on `SIGTERM` | This chapter |

---

## 21.2 Health Checks

Every production service needs a health endpoint. Load balancers, Kubernetes, and Cloud Run all probe `/health` before routing traffic.

```rust
use axum::{Router, routing::get, response::IntoResponse};
use serde_json::json;

async fn health() -> impl IntoResponse {
    axum::Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// Add to your router
let app = Router::new()
    .route("/health", get(health))
    // ... other routes
```

For a deeper health check that verifies the LLM API is reachable:

```rust
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    use rig::completion::CompletionRequestBuilder;
    // Cheap API probe — just check auth works
    let probe = CompletionRequestBuilder::new(
        rig::completion::Message::user("ping"),
    ).max_tokens(1).build();

    match state.openai_client.completion_model(openai::GPT_4O_MINI)
        .completion(probe)
        .await
    {
        Ok(_) => (
            axum::http::StatusCode::OK,
            axum::Json(json!({ "status": "ok", "llm": "reachable" })),
        ),
        Err(e) => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({ "status": "degraded", "llm": "unreachable", "error": e.to_string() })),
        ),
    }
}
```

Kubernetes liveness vs readiness:
- **Liveness**: `/health` — is the process alive? Simple `200 OK`.
- **Readiness**: `/ready` — is it ready to serve traffic? Check LLM API + vector store.

---

## 21.3 Graceful Shutdown

Tokio applications should handle `SIGTERM` (sent by Kubernetes during pod termination) by finishing in-flight requests before stopping.

```rust
use tokio::signal;

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c    => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received — draining connections");
}

// Axum: attach the shutdown handler
let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
axum::serve(listener, app)
    .with_graceful_shutdown(shutdown_signal())
    .await?;
```

With `with_graceful_shutdown`, Axum stops accepting new connections immediately but waits for all active handlers to complete before exiting. Default Kubernetes `terminationGracePeriodSeconds` is 30 seconds — a Rust service typically drains in under 1 second.

---

## 21.4 Configuration Management

Never hardcode values that change between environments. Use a configuration struct loaded from environment variables:

```rust
use std::time::Duration;

#[derive(Debug)]
struct Config {
    openai_api_key:    String,
    port:              u16,
    max_concurrency:   usize,
    token_budget:      u64,
    rate_limit_rps:    u32,
    docs_path:         String,
    log_level:         String,
    database_url:      Option<String>,
}

impl Config {
    fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            openai_api_key:  std::env::var("OPENAI_API_KEY")
                                 .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY not set"))?,
            port:            std::env::var("PORT")
                                 .unwrap_or_else(|_| "3000".into())
                                 .parse()?,
            max_concurrency: std::env::var("MAX_CONCURRENCY")
                                 .unwrap_or_else(|_| "20".into())
                                 .parse()?,
            token_budget:    std::env::var("TOKEN_BUDGET_PER_HOUR")
                                 .unwrap_or_else(|_| "500000".into())
                                 .parse()?,
            rate_limit_rps:  std::env::var("RATE_LIMIT_RPS")
                                 .unwrap_or_else(|_| "10".into())
                                 .parse()?,
            docs_path:       std::env::var("DOCS_PATH")
                                 .unwrap_or_else(|_| "docs".into()),
            log_level:       std::env::var("RUST_LOG")
                                 .unwrap_or_else(|_| "info".into()),
            database_url:    std::env::var("DATABASE_URL").ok(),
        })
    }
}
```

Load once at startup:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    // All subsequent code uses &config, not std::env::var()
}
```

---

## 21.5 AppState: The Production Struct

Everything shared across requests lives in one struct:

```rust
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use governor::{Quota, RateLimiter};
use tokio::sync::Semaphore;

#[derive(Clone)]
struct AppState {
    client:         Arc<rig::providers::openai::Client>,
    limiter:        Arc<governor::DefaultDirectRateLimiter>,
    semaphore:      Arc<Semaphore>,
    tokens_used:    Arc<AtomicU64>,
    token_budget:   u64,
}

impl AppState {
    fn new(config: &Config) -> anyhow::Result<Self> {
        let client = Arc::new(
            rig::providers::openai::Client::from_env()
        );
        let quota   = Quota::per_second(
            std::num::NonZeroU32::new(config.rate_limit_rps).unwrap()
        );
        let limiter = Arc::new(RateLimiter::direct(quota));
        let sem     = Arc::new(Semaphore::new(config.max_concurrency));

        Ok(Self {
            client,
            limiter,
            semaphore:   sem,
            tokens_used: Arc::new(AtomicU64::new(0)),
            token_budget: config.token_budget,
        })
    }

    fn check_budget(&self, tokens: u64) -> anyhow::Result<()> {
        let used = self.tokens_used.fetch_add(tokens, Ordering::Relaxed);
        if used + tokens > self.token_budget {
            anyhow::bail!(
                "Token budget exhausted ({}/{} used)",
                used + tokens,
                self.token_budget
            );
        }
        Ok(())
    }
}
```

This struct is cloned cheaply into every Axum handler — all inner values are `Arc`-wrapped.

---

## 21.6 Request Handler: All Guards in One Place

A production Axum handler applies all protections in order:

```rust
use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct ChatRequest {
    message: String,
}

#[derive(Serialize)]
struct ChatResponse {
    reply:        String,
    tokens_used:  u64,
    cost_usd:     f64,
}

async fn chat(
    State(state): State<AppState>,
    Json(req):    Json<ChatRequest>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, String)> {
    // 1. Input validation
    if req.message.len() > 4096 {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Message too long (max 4096 chars)".to_string(),
        ));
    }

    // 2. Prompt injection detection
    if let Some(pattern) = detect_injection(&req.message) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("Blocked: '{pattern}'"),
        ));
    }

    // 3. Rate limit
    if state.limiter.check().is_err() {
        return Err((
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded".to_string(),
        ));
    }

    // 4. Concurrency limit
    let _permit = state.semaphore.acquire().await.map_err(|e| (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        e.to_string(),
    ))?;

    // 5. LLM call with token tracking
    let (reply, tokens) = instrumented_prompt(
        &state.client,
        &req.message,
    ).await.map_err(|e| (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        e.to_string(),
    ))?;

    // 6. Budget check
    state.check_budget(tokens).map_err(|e| (
        axum::http::StatusCode::PAYMENT_REQUIRED,
        e.to_string(),
    ))?;

    let cost_usd = estimate_cost_usd(tokens);

    Ok(Json(ChatResponse { reply, tokens_used: tokens, cost_usd }))
}
```

Each guard is a separate, ordered step. If any fails, the request is rejected before reaching the LLM — saving tokens and protecting your system.

---

## 21.7 Performance Profiling

### Where to look first

Rust AI agents spend the vast majority of their time in one place: **waiting for the LLM API**. Before profiling CPU, measure your request latency distribution:

```rust
use std::time::Instant;

let start = Instant::now();
let response = model.completion(request).await?;
let latency = start.elapsed();

tracing::info!(
    latency_ms = latency.as_millis(),
    tokens     = response.usage.total_tokens,
    "LLM call complete"
);
```

If p99 latency is 3–5 seconds, that's the API, not your code. Profile CPU only if you see unexpectedly high CPU usage alongside fast LLM responses.

### CPU profiling with `cargo-flamegraph`

```bash
cargo install flamegraph
cargo flamegraph --bin your-agent -- --bench-mode
```

Common hotspots in Rust AI agents:
- JSON serialization/deserialization (`serde_json`) — use `simd-json` for large payloads
- String allocations in prompt construction — use `format!` once, avoid repeated concatenation
- Embedding computation (CPU-bound) — parallelise with `rayon` or `tokio::spawn`

### Memory profiling

Rust agents rarely have memory problems (no GC, no leaks if you don't use `Rc` carelessly), but embedding models can consume significant RAM:

```bash
# Check RSS after startup
cargo build --release
valgrind --tool=massif ./target/release/your-agent
```

For production, track memory via your container runtime:

```bash
docker stats --format "{{.MemUsage}}" your-container
```

A rig-based agent with a Qdrant vector store typically uses 20–50 MB RSS. If you see significantly more, check for:
- Large `Vec<Document>` held in memory after indexing (drop them after `then_store_with`)
- Session state accumulating in `InMemorySessionStorage` (switch to PostgreSQL and evict old sessions)

---

## 21.8 Security Hardening Checklist

### Input validation (non-negotiable)

```rust
fn validate_input(input: &str) -> Result<(), String> {
    // Length limit
    if input.len() > 4096 {
        return Err(format!("Input too long: {} chars (max 4096)", input.len()));
    }
    // Injection detection
    if let Some(pattern) = detect_injection(input) {
        return Err(format!("Blocked pattern: '{pattern}'"));
    }
    // No null bytes (can confuse some models)
    if input.contains('\0') {
        return Err("Null bytes not allowed".to_string());
    }
    Ok(())
}
```

### Secret handling

```rust
// ✅ DO: read from environment
let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");

// ❌ DON'T: hardcode
let api_key = "sk-...";  // This will end up in git

// ✅ DO: use a secrets manager in production
// AWS Secrets Manager via aws-sdk-secretsmanager crate
// Vault via vaultrs crate
```

### Container hardening

```dockerfile
# Run as non-root
RUN useradd -r -s /bin/false agent
USER agent

# Read-only filesystem except /tmp
# (Set in Kubernetes securityContext or docker run --read-only)

# Drop all capabilities
# Kubernetes: securityContext.capabilities.drop: ["ALL"]
```

### TLS in production

Axum doesn't handle TLS directly — use a TLS termination proxy (nginx, envoy, or the cloud load balancer). Never run an agent API on plain HTTP in production.

---

## 21.9 Cost Controls at Scale

### Token budget with hourly reset

The `AtomicU64` in `AppState` accumulates tokens. Reset it hourly:

```rust
// Spawn a background task that resets the counter every hour
tokio::spawn(async move {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        interval.tick().await;
        let used = tokens_used.swap(0, std::sync::atomic::Ordering::Relaxed);
        tracing::info!(tokens_reset = used, "Hourly token counter reset");
    }
});
```

### Model tiering

Use cheaper models for tasks that don't require GPT-4-level reasoning:

```rust
fn select_model(task: &str) -> &'static str {
    match task {
        "classify" | "extract" | "summarise" => openai::GPT_4O_MINI,
        "reason"   | "plan"    | "critique"  => openai::GPT_4O,
        _ => openai::GPT_4O_MINI,
    }
}
```

gpt-4o-mini is ~10× cheaper than gpt-4o for most tasks. Use gpt-4o only where quality differences matter.

### Prompt caching

For agents with long, stable system prompts, enable prompt caching (Anthropic Claude, OpenAI):

```rust
// Anthropic prompt caching reduces input token costs by ~90% for cached sections
// Build the agent with a stable preamble — the provider caches it on the first call:
let agent = client
    .agent(openai::GPT_4O_MINI)
    .preamble(&long_stable_system_prompt)  // stable → cached by the provider
    .build();

let response = agent.prompt(user_message).await?;
// Check response.usage.cached_input_tokens to verify cache hits
```

### Alerting

Set up cost alerts in your cloud provider (OpenAI usage dashboard, AWS Cost Explorer). Add a Prometheus metric for token costs:

```rust
// With prometheus crate
use prometheus::{register_counter_vec, CounterVec};

lazy_static::lazy_static! {
    static ref TOKEN_COUNTER: CounterVec = register_counter_vec!(
        "llm_tokens_total",
        "Total LLM tokens consumed",
        &["model", "type"]  // type = input | output
    ).unwrap();
}

// After each LLM call:
TOKEN_COUNTER.with_label_values(&[model, "input"])
    .inc_by(usage.input_tokens as f64);
TOKEN_COUNTER.with_label_values(&[model, "output"])
    .inc_by(usage.output_tokens as f64);
```

---

## 21.10 Final Architecture Review

Here is the reference architecture synthesising all patterns from the book:

```
                    ┌─────────────────────────────────────────────┐
                    │  Rust Axum Service                           │
                    │                                             │
  HTTP/SSE          │  ┌──────────────┐    ┌──────────────────┐  │
  ──────────────────┼─►│  Validation  │───►│  Rate Limiter    │  │
                    │  │  (Ch 16)     │    │  (governor)      │  │
                    │  └──────────────┘    └───────┬──────────┘  │
                    │                             │               │
                    │  ┌──────────────────────────▼────────────┐ │
                    │  │  Semaphore (concurrency limit, Ch 17)  │ │
                    │  └──────────────────────────┬────────────┘ │
                    │                             │               │
                    │  ┌──────────────────────────▼────────────┐ │
                    │  │  rig Agent (Ch 4–6)                    │ │
                    │  │  Tools · Memory · Structured Output    │ │
                    │  └──────┬──────────────────┬─────────────┘ │
                    │         │                  │                │
                    │  ┌──────▼──────┐  ┌────────▼──────────┐   │
                    │  │  swiftide   │  │  graph-flow        │   │
                    │  │  RAG store  │  │  Workflow sessions │   │
                    │  │  (Ch 9)     │  │  (Ch 12–15)       │   │
                    │  └──────┬──────┘  └────────┬──────────┘   │
                    │         │                  │                │
                    │  ┌──────▼──────────────────▼─────────────┐ │
                    │  │  OpenAI / Anthropic / Local LLM        │ │
                    │  │  Qdrant / Redis / PostgreSQL            │ │
                    │  └────────────────────────────────────────┘ │
                    │                                             │
                    │  ┌──────────────────────────────────────┐  │
                    │  │  tracing + OTel (Ch 16)               │  │
                    │  │  Jaeger / Datadog / CloudWatch         │  │
                    │  └──────────────────────────────────────┘  │
                    └─────────────────────────────────────────────┘
                                        │
                               MCP (rmcp, Ch 11)
                                        │
                    ┌───────────────────▼────────────────────┐
                    │  Java Spring Boot (Auth, Business Logic) │
                    │  or Claude Desktop / other MCP clients   │
                    └─────────────────────────────────────────┘
```

Each layer maps to a chapter. No single component is a monolith — you can replace any layer independently:
- Swap `openai` for `anthropic` in rig — no graph-flow changes
- Swap `MemoryStorage` for Qdrant in swiftide — no rig changes  
- Swap `InMemorySessionStorage` for PostgreSQL in graph-flow — no node changes
- Swap Axum for a Lambda handler — same business logic

This composability is the Rust agentic stack's strongest architectural property.

---

## 21.11 What Goes Wrong in Production (And How to Fix It)

### The LLM returns unexpected JSON

```rust
// Use Extractor<M, T> with retries instead of manual JSON parsing
let extractor = client
    .extractor::<MyOutput>(openai::GPT_4O_MINI)
    .retries(3)
    .build();
```

### Token costs spike overnight

- Check for runaway loops in graph-flow (conditional edge never terminates)
- Add `max_iterations` guard in any ReAct-style Think→Act cycle
- Set token budget `AtomicU64` with hourly reset (§21.9)

### Cold starts on Lambda are slow

- Use `cargo-lambda` with `--release` and `[profile.release]` settings
- Rust cold starts on Lambda are 5–50 ms — if you see >100 ms, check for blocking calls at startup (file I/O, synchronous HTTP)

### Swiftide indexing OOMs

- Don't load all documents into memory before passing to the pipeline
- `FileLoader` is lazy — the pipeline streams; you shouldn't OOM unless a single chunk is enormous
- Add `with_chunk_range(100..1024)` to limit chunk size

### Graph-flow sessions accumulate

- `InMemorySessionStorage` never evicts — for long-running services, switch to PostgreSQL and add a TTL cleanup job

---

## 21.12 Key Takeaways

- **Health + graceful shutdown**: non-negotiable for Kubernetes/Cloud Run — add `/health` endpoint and `with_graceful_shutdown`
- **Single `AppState`** with `Arc`-wrapped rate limiter, semaphore, and token counter — clone cheaply into every handler
- **Guard order**: validate input → check injection → rate limit → concurrency limit → LLM call → budget check
- **Model tiering**: gpt-4o-mini for classification/extraction (~10× cheaper); gpt-4o only for complex reasoning
- **Token budget**: `AtomicU64` + hourly reset task; alert at 80% of budget
- **Profiling target**: LLM API latency dominates; profile CPU only if you see anomalies
- **Composability**: each layer (rig, swiftide, graph-flow, rmcp) replaces independently — design for this
- **MCP boundary**: expose rig agents via rmcp to decouple Rust internals from Java/Python clients

---

## Closing Words

You've built a complete mental model for Rust agentic programming — from ownership semantics to multi-agent pipelines, from Docker images to WASM edge deployment. The Rust agentic ecosystem is young, but it's moving fast, and the foundational patterns you've learned here will transfer as the crates evolve.

The Java parallels in every chapter were deliberate: the patterns aren't new, the implementation is. A Spring AI `ChatClient` and a rig `Agent` solve the same problem with the same mental model. The difference is in the operational characteristics — binary size, cold start, memory footprint — and in the ownership discipline that Rust enforces.

That discipline is the real payoff. When your agent runs in production for six months without a memory leak, without a data race, and with predictable performance under load — that's Rust's systems-language heritage working for you.

Build something real with it.

---

*→ Java reference: Spring Boot production hardening, Micrometer, Spring Security, GraalVM native (Ch 24)*
