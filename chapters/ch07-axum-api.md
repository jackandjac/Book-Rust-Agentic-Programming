# Chapter 7: Rig with Axum — Building a Streaming Web API

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` · `axum = "0.8"` (74M downloads, updated Apr 2026)  
> `tower-http = "0.6"` · `tokio-stream = "0.1"` · `tokio = "1"`
>
> **Java reference:** Spring Boot + Spring AI's `ChatClient` streaming; LangChain4j with Spring Boot REST controller

---

## What You'll Learn

- Why Axum is the natural pairing for Rig in async Rust web services
- How to expose an LLM agent as an HTTP endpoint
- How to bridge rig's streaming output to Server-Sent Events (SSE)
- Shared state: injecting `Agent` into Axum handlers with `State<T>`
- Session scoping with `conversation_id` — same agent serving many concurrent users
- CORS and production wiring with `tower-http`
- Build: a streaming chat API — test with `curl -N`, consume from any browser via `EventSource`

---

## 7.1 Why Axum?

When you want to serve an LLM agent over HTTP in Java, you reach for Spring Boot. In Rust, the equivalent is **Axum** — the dominant async web framework (74 million downloads), built by the same team as Tokio.

Axum's core design is:

| Axum concept | Spring Boot equivalent |
|---|---|
| `Router::new().route(path, handler)` | `@RestController` + `@GetMapping` |
| `State<T>` extractor | `@Autowired` / `@Bean` injection |
| `axum::Json<T>` extractor | `@RequestBody` |
| `impl IntoResponse` return type | `ResponseEntity<T>` |
| `tower::Layer` middleware | `HandlerInterceptor` / Servlet filters |
| `Sse<S>` response | `SseEmitter` / `Flux<ServerSentEvent>` |

Axum handlers are plain async functions. No reflection, no annotations, no startup time. The same Tokio runtime that drives your `Agent` drives your HTTP server — there is no impedance mismatch.

---

## 7.2 A Minimal Axum Handler

Before adding rig, here's the shape of an Axum service:

```rust
use axum::{Router, routing::get};

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/health", get(health));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

That's the full skeleton. Axum infers everything from the handler's type signature — the return type `&'static str` is automatically converted to a 200 OK response with a text body.

> **Java parallel:** This is a Spring Boot `@GetMapping` method with no `ResponseEntity` wrapper. Axum's type inference does what Spring MVC's `HttpMessageConverter` does, but at compile time.

---

## 7.3 Server-Sent Events in Axum

Server-Sent Events (SSE) is the standard transport for streaming LLM responses to browsers. Unlike WebSockets, SSE is a unidirectional HTTP response — the server pushes events, the client reads them.

The relevant types are in `axum::response::sse`:

```rust
use axum::response::sse::{Event, KeepAlive, Sse};
```

### Constructing an `Event`

```rust
// A plain data event (the default type is "message")
Event::default().data("Hello, world!");

// A named event with a data payload
Event::default()
    .event("token")         // event type — client filters on this
    .data("some text");     // the payload

// A sentinel to signal stream completion
Event::default().event("done").data("{}");
```

### SSE Handler Signature

An SSE handler returns `Sse<S>` where `S: TryStream<Ok = Event> + Send + 'static`:

```rust
use std::convert::Infallible;
use axum::response::sse::{Event, Sse};
use futures::stream::{self, Stream};

// The simplest possible SSE endpoint — sends three events and closes
async fn simple_stream() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = vec![
        Ok(Event::default().data("first")),
        Ok(Event::default().data("second")),
        Ok(Event::default().event("done").data("{}")),
    ];
    Sse::new(stream::iter(events))
}
```

`.keep_alive(KeepAlive::default())` adds periodic comment lines (`: keepalive`) to prevent proxy timeouts:

```rust
Sse::new(my_stream).keep_alive(KeepAlive::default())
```

> **Java parallel:** This maps to Spring WebFlux's `Flux<ServerSentEvent<String>>` return type from a `@GetMapping(produces = MediaType.TEXT_EVENT_STREAM_VALUE)` controller. Axum's `Sse<S>` serves the same role. For Spring MVC (non-reactive), the equivalent is `SseEmitter`.

---

## 7.4 Bridging Rig Streaming to SSE

This is the core of the chapter. Rig's `stream_prompt()` returns a `StreamingPromptRequest` that, when awaited, yields a stream of `MultiTurnStreamItem` values. We need to map those into `Event` values for Axum's `Sse` response.

### The Type Bridge

The chain is:

```
agent.stream_prompt(text).conversation(id).await
    → Pin<Box<dyn Stream<Item = Result<MultiTurnStreamItem, _>> + Send>>

        ↓ map each StreamAssistantItem::Text → Event::default().data(text.text)
        ↓ on FinalResponse → Event::default().event("done").data("{}")

    → impl Stream<Item = Result<Event, Infallible>>
        → Sse::new(stream)
```

### The Channel Bridge Pattern

Because `stream_prompt` is async and Axum's `Sse::new()` needs a stream it can poll from a synchronous context, the cleanest pattern is an `mpsc` channel: spawn a task to drive the rig stream, forward events through the channel, and wrap the receiver as a `ReceiverStream` for Axum:

```rust
use std::convert::Infallible;
use futures::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::streaming::{StreamedAssistantContent, StreamingPrompt};
use tokio_stream::wrappers::ReceiverStream;
use axum::response::sse::{Event, KeepAlive, Sse};

async fn sse_handler(agent: &openai::Agent, message: &str, conv_id: &str)
    -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>>
{
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);

    let agent = agent.clone();   // Agent<M> is Clone when M: Clone
    let message = message.to_owned();
    let conv_id = conv_id.to_owned();

    tokio::spawn(async move {
        let stream = match agent.stream_prompt(&message).conversation(&conv_id).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Ok(Event::default().event("error").data(e.to_string()))).await;
                return;
            }
        };

        tokio::pin!(stream);

        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
                    if let StreamedAssistantContent::Text(text) = content {
                        // text.text: String — the incremental text chunk
                        let event = Event::default().data(text.text);
                        if tx.send(Ok(event)).await.is_err() {
                            break; // Client disconnected
                        }
                    }
                }
                Ok(MultiTurnStreamItem::FinalResponse(_)) => {
                    let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
                    break;
                }
                Ok(_) => {} // StreamUserItem (tool results) — not text
                Err(e) => {
                    let _ = tx.send(Ok(Event::default().event("error").data(e.to_string()))).await;
                    break;
                }
            }
        }
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}
```

### Key Details

**`StreamedAssistantContent::Text(text)`** — the `text` field is `text.text: String`, containing the incremental text chunk. Other variants (`ToolCall`, `ToolCallDelta`, `Reasoning`, `ReasoningDelta`, `Final`) are skipped in a plain chat endpoint.

**`MultiTurnStreamItem` is `#[non_exhaustive]`** — always include a `_ => {}` arm. New variants may appear in future rig releases.

**`Agent<M>: Clone`** when `M: Clone` — `openai::Agent` satisfies this, so `agent.clone()` inside the `tokio::spawn` closure is valid. No `Arc<Mutex<Agent>>` needed.

**`tokio::pin!(stream)`** — rig's stream is returned as a boxed `Pin<Box<dyn Stream + Send>>`. `tokio::pin!` re-pins it in the stack frame so `stream.next()` works correctly.

---

## 7.5 Shared Agent State with `State<T>`

In a real service, you build the `Agent` once at startup and share it across all request handlers. Axum's `State<T>` extractor is the idiomatic way to do this.

### Defining `AppState`

```rust
use std::sync::Arc;
use rig::providers::openai;

struct AppState {
    agent: openai::Agent,
}
```

`openai::Agent` is `Clone + Send + Sync`, so no `Mutex` is needed for read-only access. Wrap in `Arc` for cheap cloning across request handlers:

```rust
let state = Arc::new(AppState { agent });
let app = Router::new()
    .route("/chat/stream", post(chat_stream))
    .with_state(state);
```

### Extracting State in a Handler

```rust
async fn chat_stream(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ChatRequest>,
) -> impl IntoResponse {
    // state.agent is the shared agent
    // req.message and req.conversation_id come from the request body
    // ...
}
```

`State<Arc<AppState>>` requires `AppState: Clone`; wrapping in `Arc` satisfies this — `Arc<T>: Clone` for any `T`.

> **Java parallel:** `State<T>` in Axum is analogous to `@Autowired` injection in Spring. The key difference: Axum makes the dependency explicit in the handler signature (visible at a glance), while Spring injects it invisibly. This is closer to constructor injection, which is also the Spring best practice.

---

## 7.6 Session Management with `conversation_id`

Multiple concurrent users can share one `Agent` instance because rig's managed memory is scoped by conversation ID. Each request carries a `conversation_id` that isolates its history from other users' conversations.

```rust
#[derive(serde::Deserialize)]
struct ChatRequest {
    message: String,
    conversation_id: String,   // client-generated, e.g. a UUID
}
```

The agent was built with `.memory(InMemoryConversationMemory::new())` at startup. Each call to `.conversation(&conv_id)` retrieves and stores history under that key:

```rust
let stream = agent
    .stream_prompt(&req.message)
    .conversation(&req.conversation_id)  // scope history to this user
    .await?;
```

### Limitations of In-Process Memory

`InMemoryConversationMemory` stores all history in a `HashMap` inside the process. This means:

- History is **lost on restart** — sessions don't survive deploys
- Not shared across **multiple service instances** — incompatible with horizontal scaling
- **Unbounded** — long conversations accumulate indefinitely

For production, replace in-process memory with a database-backed store:

| Approach | Description |
|---|---|
| **Managed memory** | rig's `InMemoryConversationMemory` — fine for prototypes and single-instance services |
| **External cache** | Store `Vec<Message>` in Redis; serialize with `serde_json`; key by `conversation_id` |
| **Database** | Persist messages in PostgreSQL; load the last N messages before each call with `.chat()` |

The Redis approach: before each call, load history from Redis → call `.chat(prompt, history)` → append the new exchange → write back to Redis. This is the same manual history pattern from Chapter 6, just with Redis as the storage backend instead of an in-memory `Vec`.

---

## 7.7 CORS and Middleware

Real-world services need CORS headers so browser clients can make cross-origin requests. The `tower-http` crate provides a `CorsLayer`:

```rust
use tower_http::cors::{Any, CorsLayer};
use axum::http::Method;

let cors = CorsLayer::new()
    .allow_methods([Method::GET, Method::POST])
    .allow_origin(Any)
    .allow_headers([
        axum::http::header::CONTENT_TYPE,
        axum::http::header::AUTHORIZATION,
    ]);

let app = Router::new()
    .route("/chat/stream", post(chat_stream))
    .with_state(state)
    .layer(cors);    // layers apply to all routes
```

`Any` for `allow_origin` is appropriate for local development. In production, replace `Any` with specific allowed origins:

```rust
use axum::http::HeaderValue;

.allow_origin("https://myapp.example.com".parse::<HeaderValue>().unwrap())
```

Other common `tower-http` layers:

| Layer | Purpose |
|---|---|
| `TraceLayer` | Request/response tracing with `tracing` crate |
| `CompressionLayer` | gzip/brotli response compression |
| `TimeoutLayer` | Request timeout |
| `RequestBodyLimitLayer` | Limit request body size (prevent large prompt injection attempts) |

---

## 7.8 Hands-On: Streaming Chat API

The complete runnable example:

```rust
// code-examples/ch07-axum-api/src/main.rs
use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::Method,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::post,
};
use futures::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::client::{CompletionClient, ProviderClient};
use rig::providers::openai;
use rig::streaming::{StreamedAssistantContent, StreamingPrompt};
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::cors::{Any, CorsLayer};

struct AppState {
    agent: openai::Agent,
}

#[derive(Deserialize)]
struct ChatRequest {
    message: String,
    conversation_id: String,
}

async fn chat_stream(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ChatRequest>,
) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);

    let agent = state.agent.clone();
    let message = req.message.clone();
    let conv_id = req.conversation_id.clone();

    tokio::spawn(async move {
        let stream = match agent.stream_prompt(&message).conversation(&conv_id).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Ok(Event::default().event("error").data(e.to_string()))).await;
                return;
            }
        };

        tokio::pin!(stream);

        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
                    if let StreamedAssistantContent::Text(text) = content {
                        let event = Event::default().data(text.text);
                        if tx.send(Ok(event)).await.is_err() {
                            break;
                        }
                    }
                }
                Ok(MultiTurnStreamItem::FinalResponse(_)) => {
                    let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.send(Ok(Event::default().event("error").data(e.to_string()))).await;
                    break;
                }
            }
        }
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = openai::Client::from_env()?;
    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble("You are a helpful Rust programming assistant.")
        .build();

    let state = Arc::new(AppState { agent });

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(Any)
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ]);

    let app = Router::new()
        .route("/chat/stream", post(chat_stream))
        .with_state(state)
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!("Listening on http://0.0.0.0:3000");
    axum::serve(listener, app).await?;

    Ok(())
}
```

### Running the Example

```bash
cd code-examples
export OPENAI_API_KEY=sk-...
cargo run -p ch07-axum-api
```

### Testing with curl

```bash
# Stream the response — -N disables buffering so tokens print as they arrive
curl -N http://localhost:3000/chat/stream \
     -H "Content-Type: application/json" \
     -d '{"message": "What is ownership in Rust?", "conversation_id": "user-1"}'

# Second turn — same conversation_id continues the history
curl -N http://localhost:3000/chat/stream \
     -H "Content-Type: application/json" \
     -d '{"message": "How does it differ from garbage collection?", "conversation_id": "user-1"}'
```

Each `data:` line in the response is one text chunk. A final `event: done` signals the stream is complete.

### Consuming from a Browser

The browser's built-in `EventSource` API reads SSE. For a POST endpoint (which `EventSource` doesn't support natively), use the `fetch` API with `ReadableStream`:

```javascript
const response = await fetch('/chat/stream', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ message: userInput, conversation_id: sessionId }),
});

const reader = response.body.getReader();
const decoder = new TextDecoder();

while (true) {
  const { done, value } = await reader.read();
  if (done) break;
  const chunk = decoder.decode(value);
  // Parse SSE format: "data: ...\n\n"
  for (const line of chunk.split('\n')) {
    if (line.startsWith('data: ')) {
      appendToChat(line.slice(6));
    }
  }
}
```

> **Java parallel:** Spring AI streaming looks like:
> ```java
> @PostMapping(value = "/chat/stream", produces = MediaType.TEXT_EVENT_STREAM_VALUE)
> public Flux<String> chatStream(@RequestBody ChatRequest req) {
>     return chatClient.prompt()
>         .user(req.getMessage())
>         .stream()
>         .content();
> }
> ```
> The rig + Axum pattern achieves the same result, but the stream bridging — which Spring WebFlux hides inside its `Flux` abstraction — is explicit in Rust. This explicitness is a double-edged sword: more boilerplate, but every step is visible and testable.

---

## 7.9 Architecture Notes

### Request Lifecycle

```
POST /chat/stream
    ↓
Axum extracts State<Arc<AppState>> + Json<ChatRequest>
    ↓
chat_stream handler spawns a tokio task
    ↓
Task: agent.stream_prompt(message).conversation(id).await
    → MultiTurnStreamItem stream
    → map Text chunks → mpsc channel
    ↓
ReceiverStream<Event> → Sse response headers sent immediately
    ↓
Client receives SSE events as they arrive
    ↓
FinalResponse → send "done" event → task exits → channel closes → SSE closes
```

### Why `mpsc` Instead of Mapping Directly?

The direct alternative — mapping the rig stream into an `Event` stream in the handler — runs into an issue: `async_fn_in_trait` and lifetime constraints make it difficult to return a `impl Stream` that borrows from local handler variables. The channel decouples the rig stream lifecycle from the Axum response lifecycle cleanly, at the cost of one allocation (the channel buffer).

In practice, this pattern is idiomatic in Axum SSE handlers and is how the official Axum SSE example is structured.

### Scaling Considerations

For horizontal scaling (multiple service instances):

1. Replace `InMemoryConversationMemory` with Redis-backed history storage
2. Use `.chat(prompt, history)` instead of `.prompt().conversation(id)` — load history from Redis before each call, write it back after
3. Any instance can serve any request because conversation state is in Redis, not the process

The `Agent` itself is stateless across requests — it's the memory that needs to be externalized.

---

## Key Takeaways

- Axum handlers are async functions; return types implement `IntoResponse`. `Sse<S>` is a built-in response type for Server-Sent Events.
- Bridge rig streaming to Axum SSE with an `mpsc` channel + `tokio::spawn`: the task drives the rig stream; the `ReceiverStream` is handed to `Sse::new()`.
- `MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(t))` — `t.text` is the incremental string chunk to send. `FinalResponse` signals the end.
- `Agent<M>: Clone` when `M: Clone` — `openai::Agent` can be cloned into `tokio::spawn` closures directly. No `Arc<Mutex<Agent>>` needed.
- Share the agent across handlers via `Arc<AppState>` + `State<Arc<AppState>>` extractor. Router state is set with `.with_state(state)`.
- `InMemoryConversationMemory` scoped by `conversation_id` handles multi-user sessions in a single process. For multi-instance deployments, externalize history to Redis or a database.
- `tower-http`'s `CorsLayer` handles CORS; add it last with `.layer(cors)` to apply to all routes.

---

## Further Reading

- [Axum docs](https://docs.rs/axum/latest/axum/) — `Router`, `State`, `Sse`, `Event`, `KeepAlive`
- [Axum SSE example](https://github.com/tokio-rs/axum/blob/main/examples/sse/src/main.rs) — official reference for SSE handler structure
- [tower-http docs](https://docs.rs/tower-http/latest/tower_http/) — `CorsLayer`, `TraceLayer`, `CompressionLayer`
- [tokio-stream ReceiverStream](https://docs.rs/tokio-stream/latest/tokio_stream/wrappers/struct.ReceiverStream.html) — bridging `mpsc::Receiver` to a `Stream`
- [rig streaming module](https://docs.rs/rig-core/latest/rig/streaming/) — `StreamedAssistantContent`, `MultiTurnStreamItem`
- [Spring AI Streaming](https://docs.spring.io/spring-ai/reference/api/clients/openai-chat.html#streaming) — Java reference for streaming comparison

---

*Next: Chapter 8 — RAG: Retrieval-Augmented Generation*
