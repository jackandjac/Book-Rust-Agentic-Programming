// Chapter 7: Rig with Axum — Building a Streaming Web API
// See chapters/ch07-axum-api.md for the full explanation.
//
// Run: cargo run -p ch07-axum-api
// Then test with:
//   curl -N http://localhost:3000/chat/stream -H "Content-Type: application/json" \
//        -d '{"message": "What is ownership in Rust?", "conversation_id": "user-1"}'
//
// Requires: OPENAI_API_KEY env var (or .env file)

use std::convert::Infallible;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    Router,
    extract::State,
    http::{HeaderValue, Method},
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

// ── Shared state ─────────────────────────────────────────────────────────────

/// Application state shared across all request handlers.
///
/// `Agent<M>` is `Clone + Send + Sync` when the underlying model is, so we
/// can store it directly in `Arc<AppState>` without an extra Mutex.
struct AppState {
    agent: openai::Agent,
}

// ── Request/response types ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChatRequest {
    message: String,
    conversation_id: String,
}

// ── SSE streaming handler ─────────────────────────────────────────────────────

/// POST /chat/stream
///
/// Accepts a JSON body `{"message": "...", "conversation_id": "..."}`.
/// Streams the assistant's response as Server-Sent Events, one `data:` line
/// per text chunk. Sends a final `event: done` when the stream is complete.
async fn chat_stream(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ChatRequest>,
) -> impl IntoResponse {
    // We bridge the rig async stream into a channel so we can hand a
    // synchronous ReceiverStream to Axum's SSE response.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);

    let agent = state.agent.clone();
    let message = req.message.clone();
    let conv_id = req.conversation_id.clone();

    // Spawn a task to drive the rig stream and forward events to the channel.
    tokio::spawn(async move {
        // stream_prompt returns a StreamingPromptRequest that implements
        // IntoFuture — awaiting it yields a pinned stream of MultiTurnStreamItem.
        let stream = match agent
            .stream_prompt(&message)
            .conversation(&conv_id)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let err_event = Event::default()
                    .event("error")
                    .data(e.to_string());
                let _ = tx.send(Ok(err_event)).await;
                return;
            }
        };

        // Pin the stream so we can call .next() on it in the async loop.
        tokio::pin!(stream);

        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
                    // StreamedAssistantContent::Text carries incremental text chunks.
                    if let StreamedAssistantContent::Text(text) = content {
                        let event = Event::default().data(text.text);
                        if tx.send(Ok(event)).await.is_err() {
                            // Client disconnected.
                            break;
                        }
                    }
                    // ToolCall and Reasoning variants are silently skipped here —
                    // a production handler would surface them differently.
                }
                Ok(MultiTurnStreamItem::FinalResponse(_)) => {
                    // Stream complete — send a sentinel so the client can close.
                    let done = Event::default().event("done").data("{}");
                    let _ = tx.send(Ok(done)).await;
                    break;
                }
                Ok(_) => {} // StreamUserItem (tool results) — not relevant for text chat
                Err(e) => {
                    let err_event = Event::default()
                        .event("error")
                        .data(e.to_string());
                    let _ = tx.send(Ok(err_event)).await;
                    break;
                }
            }
        }
    });

    // Wrap the receiver as a stream and hand it to Axum's SSE response.
    let stream = ReceiverStream::new(rx);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ── Router ────────────────────────────────────────────────────────────────────

fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin(Any)
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ]);

    Router::new()
        .route("/chat/stream", post(chat_stream))
        .with_state(state)
        .layer(cors)
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = openai::Client::from_env()?;

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(
            "You are a helpful Rust programming assistant. \
             Answer questions clearly and concisely. \
             Use code examples where appropriate.",
        )
        .build();

    let state = Arc::new(AppState { agent });
    let app = build_router(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("Listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
