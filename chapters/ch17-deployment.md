# Chapter 17: Deployment — The Rust Advantage

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` · `axum = "0.8"` · `tokio = "1"`  
> Docker multi-stage builds · `wasm32-wasip2` target · `cargo-lambda`
>
> **Java reference:** Spring Boot fat JAR deployment, Docker Jib, GraalVM native image (Chapter 23 of Java book)

---

Deploying an AI agent is more than `git push`. You need to manage binary size, cold start time, secrets, and scale. This is where Rust's systems-language heritage becomes a concrete operational advantage.

This chapter covers:
1. Release builds and binary size
2. Docker multi-stage builds
3. Cloud deployment (Lambda, Cloud Run, Fly.io)
4. WASM for edge deployment
5. Scaling strategies for async Rust

---

## 17.1 Release Builds

By default, `cargo build` produces a debug binary. For deployment, always use `--release`:

```bash
cargo build --release -p ch07-axum-api
ls -lh target/release/ch07-axum-api
# → ~5 MB (vs ~25 MB debug)
```

### Binary size optimisation

Add to `Cargo.toml`:

```toml
[profile.release]
opt-level     = "z"   # optimise for size (vs "3" for speed)
lto           = true  # link-time optimisation — removes dead code across crates
codegen-units = 1     # single codegen unit — best LTO, slower compile
strip         = true  # strip debug symbols from binary
panic         = "abort" # smaller panic handler; no stack unwinding
```

With these settings, a typical rig-based Axum API compiles to **8–15 MB** as a static binary.

### Java comparison

| | Rust (release + LTO) | Spring Boot (fat JAR) | GraalVM native |
|--|---|---|---|
| Binary size | 8–15 MB | 80–200 MB | 50–80 MB |
| Cold start | 5–50 ms | 3–8 s | 50–200 ms |
| Memory (idle) | 10–30 MB | 150–400 MB | 50–100 MB |
| Compile time | 30–90 s | 10–30 s | 5–15 min |

These are representative figures — actual values depend heavily on workload. The key point: Rust's smaller binary and lower idle memory directly reduce container and serverless costs.

---

## 17.2 Docker Multi-Stage Builds

A single-stage Docker build for Rust is slow (full recompile on every change) and large (includes the compiler). Multi-stage solves both:

```dockerfile
# Stage 1: Build
FROM rust:1.87-slim AS builder

WORKDIR /app

# Cache dependency compilation — only rebuilds when Cargo.toml changes
COPY Cargo.toml Cargo.lock ./
COPY code-examples/Cargo.toml code-examples/

# Create dummy source files so cargo can compile dependencies
RUN find code-examples -name 'Cargo.toml' -exec sh -c \
    'mkdir -p "$(dirname {})/src" && echo "fn main(){}" > "$(dirname {})/src/main.rs"' \;
RUN cargo build --release -p ch07-axum-api 2>/dev/null || true

# Now copy real sources and build the actual binary
COPY . .
RUN touch code-examples/ch07-axum-api/src/main.rs
RUN cargo build --release -p ch07-axum-api

# Stage 2: Runtime — minimal image
FROM debian:bookworm-slim

# Only install what the binary needs at runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ch07-axum-api /usr/local/bin/agent

# Never run as root
RUN useradd -r -s /bin/false agent
USER agent

ENV RUST_LOG=info
EXPOSE 3000

CMD ["agent"]
```

The final image is:

```bash
docker build -t rust-agent .
docker images rust-agent
# → ~25 MB (debian-slim base + 12 MB binary)
```

Compare to a Spring Boot image: typically 180–300 MB.

### Using `scratch` for truly minimal images

If your binary is fully statically linked (musl target), you can use `FROM scratch`:

```bash
# Build with musl for static linking
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl -p ch07-axum-api
```

```dockerfile
FROM scratch
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/ch07-axum-api /agent
CMD ["/agent"]
```

The resulting image is just the binary — under 15 MB. This requires that all dependencies support static linking (most do; OpenSSL is the common exception — use `rustls` instead).

---

## 17.3 Cloud Deployment

### AWS Lambda with `cargo-lambda`

`cargo-lambda` packages Rust binaries as Lambda deployment archives:

```bash
cargo install cargo-lambda
cargo lambda build --release -p ch07-axum-api
cargo lambda deploy --region us-east-1 rust-agent
```

For HTTP APIs, use Lambda with Function URLs or API Gateway. Add the `lambda_http` adapter:

```toml
lambda_http = "0.14"
```

```rust
use lambda_http::{run, service_fn, Body, Error, Request, Response};

async fn handler(event: Request) -> Result<Response<Body>, Error> {
    // Your Axum app can be mounted here via tower-lambda
    Ok(Response::builder()
        .status(200)
        .body(Body::from("OK"))?)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    run(service_fn(handler)).await
}
```

Cold start for a Rust Lambda: **5–50 ms** (vs 3–8 seconds for Spring Boot with JVM warm-up).

### Google Cloud Run

Cloud Run serves any container that listens on `$PORT`. With Rust:

```bash
# Build and push
docker build -t gcr.io/my-project/rust-agent .
docker push gcr.io/my-project/rust-agent

# Deploy
gcloud run deploy rust-agent \
  --image gcr.io/my-project/rust-agent \
  --region us-central1 \
  --allow-unauthenticated \
  --set-env-vars OPENAI_API_KEY=... \
  --memory 128Mi \
  --cpu 1
```

128 MB memory is enough for most rig-based agents. For Java on Cloud Run, 512 MB–1 GB is typical minimum.

### Fly.io

```bash
fly launch --name rust-agent
# Edit fly.toml: set [[services.ports]] and [env]
fly secrets set OPENAI_API_KEY="sk-..."
fly deploy
```

Fly.io deploys close to users (edge-like), and Rust's small memory footprint means you can run on the smallest VM sizes (`shared-cpu-1x`, 256 MB RAM).

---

## 17.4 WASM for Edge Deployment

Rust's WASM support is one of its strongest differentiators. You can compile the same business logic to run at CDN edge nodes (Cloudflare Workers, Fastly Compute) with no JVM, no Docker.

### Targeting WASM

```bash
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release -p my-agent
```

WASM-compatible rig code must avoid:
- Blocking I/O (use async)
- Platform-specific crates

rig-core has WASM compatibility flags (`wasm_compat` module) for most providers.

### Cloudflare Workers (via `worker` crate)

```toml
worker = "0.5"
wasm-bindgen = "0.2"
```

```rust
use worker::*;

#[event(fetch)]
async fn main(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    // rig calls work here — the worker crate provides fetch-based HTTP
    Response::ok("Hello from Rust WASM!")
}
```

Deploy:

```bash
wrangler deploy
```

Cold start: **0 ms** (WASM modules are pre-compiled and cached at edge). Memory limit: 128 MB per request.

### When to use WASM vs Docker

| | Docker/Lambda | WASM (CF Workers) |
|--|--|--|
| Cold start | 5–50 ms | ~0 ms |
| Memory limit | 128 MB+ | 128 MB |
| Full async runtime | ✅ Tokio | ⚠️ Limited |
| External TCP | ✅ | ⚠️ HTTP only |
| Ideal for | Full agents, RAG, long-running | Lightweight routing, simple prompts |

---

## 17.5 Local LLMs with Kalosm

For offline, privacy-preserving, or cost-sensitive deployments, Rust has pure-Rust LLM inference via the `kalosm` crate:

```toml
kalosm = "0.3"
kalosm-language = "0.3"
```

```rust
use kalosm::language::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Downloads and caches the model on first run (~4 GB for Llama 3.2 8B)
    let model = Llama::new_chat().await?;
    let mut chat = model.chat();

    let response = chat
        .add_message(MessageType::UserMessage, "What is ownership in Rust?")
        .await?;

    println!("{response}");
    Ok(())
}
```

Kalosm uses llama.cpp under the hood, with Rust bindings. It runs on CPU (slower) or GPU (Metal on macOS, CUDA on Linux). No API key required.

> **When to use local inference:**
> - Privacy requirements (data never leaves the machine)
> - Cost at scale (no per-token fees)
> - Offline operation
> - Latency at edge (model co-located with code)

The tradeoff: model quality below GPT-4o for complex reasoning tasks; setup complexity vs a simple API call.

---

## 17.6 Scaling Async Rust

Tokio's multi-threaded runtime (the default with `#[tokio::main]`) scales vertically automatically — it uses one thread per CPU core. For horizontal scaling:

### Stateless scale-out

If your agent is stateless (no in-process memory), you can run multiple instances behind a load balancer. Each request is independent.

```bash
# Kubernetes: scale up replicas
kubectl scale deployment rust-agent --replicas=10
```

Rust's low memory footprint means you can run more replicas per node than an equivalent Java service.

### Stateful scale-out

For stateful agents using in-process session stores or in-memory vector stores, you need sticky sessions or shared external state:

- **Sticky sessions** — route all requests from a user to the same pod (simple but limits flexibility)
- **Redis-backed session store** — load/save `Vec<Message>` via Redis (Chapter 10 §10.4) — any pod can serve any user
- **graph-flow + PostgreSQL** — sessions in PostgreSQL; any pod can resume any session (Chapter 14)

### Backpressure

Tokio's async model provides natural backpressure: if the LLM API is slow, pending tasks queue in Tokio's scheduler rather than spawning unboundedly. For strict concurrency limits:

```rust
use tokio::sync::Semaphore;

let concurrency = Arc::new(Semaphore::new(10));  // max 10 concurrent LLM calls

async fn call_with_limit(sem: Arc<Semaphore>, ...) {
    let _permit = sem.acquire().await.unwrap();
    // LLM call here — permit released when _permit is dropped
}
```

---

## 17.7 Secrets Management

Never hardcode API keys. Use environment variables or a secrets manager:

```rust
// Read from environment (dotenvy loads .env in development)
let api_key = std::env::var("OPENAI_API_KEY")
    .expect("OPENAI_API_KEY must be set");
```

For production:
- **AWS Secrets Manager**: `aws-sdk-secretsmanager` crate
- **HashiCorp Vault**: `vaultrs` crate
- **Kubernetes Secrets**: mounted as environment variables (standard)
- **Doppler / Infisical**: load into env before process start

```bash
# Kubernetes Secret
kubectl create secret generic agent-secrets \
  --from-literal=OPENAI_API_KEY=sk-...

# Mount in deployment
env:
  - name: OPENAI_API_KEY
    valueFrom:
      secretKeyRef:
        name: agent-secrets
        key: OPENAI_API_KEY
```

---

## 17.8 Key Takeaways

- **`[profile.release]`** with `lto = true`, `strip = true`, `panic = "abort"` — typical result: 8–15 MB binary
- **Multi-stage Docker**: build stage (Rust toolchain) + runtime stage (debian-slim or scratch) → 15–30 MB image
- **musl target** (`x86_64-unknown-linux-musl`) + `FROM scratch` → static binary image under 15 MB
- **`cargo-lambda`** — packages Rust for AWS Lambda; cold start 5–50 ms
- **Cloud Run / Fly.io** — deploy the container; 128 MB memory is sufficient for most rig agents
- **WASM** (`wasm32-wasip2`) — edge deployment on Cloudflare Workers; ~0 ms cold start
- **Kalosm** — pure-Rust local LLM inference; no API key; ~4 GB model download
- **Horizontal scaling**: stateless → load balancer; stateful → Redis memory or PostgreSQL sessions
- **`Semaphore`** for concurrency limits on LLM calls

---

## What's Next

Chapter 18 steps back for a framework comparison: when to use rig vs swiftide vs graph-flow, decision criteria, hybrid architectures, and the Rust agentic ecosystem trajectory.

---

*→ Java reference: Spring Boot fat JAR, Docker Jib, GraalVM native image, Lambda SnapStart (Ch 23)*
