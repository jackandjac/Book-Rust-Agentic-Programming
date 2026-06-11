// Chapter 10: Memory and State in Rust Agents
// See chapters/ch10-memory.md for the full explanation.
//
// Run: cargo run -p ch10-memory
// Requires: OPENAI_API_KEY env var (or .env file)
//
// Demonstrates three memory patterns:
//   1. Manual Vec<Message> — push user + assistant turns after each call
//   2. In-process HashMap — lightweight multi-session store with no external deps
//   3. Sliding-window truncation — keep only the last N messages per session

use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{Chat, Prompt};
use rig::completion::Message;
use rig::providers::openai;
use std::collections::HashMap;
use std::sync::Mutex;

const PREAMBLE: &str = "\
You are a helpful personal assistant with a good memory. \
Refer back to earlier parts of the conversation when relevant. \
Keep answers concise — one to three sentences.";

// ── Pattern 1: Manual Vec<Message> history ────────────────────────────────────

/// Demonstrates manual history management.
///
/// `Agent::chat(prompt, &history)` takes an immutable borrow of history.
/// After each call, push user + assistant turns manually.
async fn demo_manual_history(client: &openai::Client) -> Result<()> {
    println!("━━━ Pattern 1: Manual Vec<Message> history ━━━\n");

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    let mut history: Vec<Message> = Vec::new();

    // Turn 1
    let q1 = "My name is Alice and I'm learning Rust.";
    println!("User:  {q1}");
    let r1 = agent.chat(q1, &history).await?;
    println!("Agent: {r1}\n");
    history.push(Message::user(q1));
    history.push(Message::assistant(r1.as_str()));

    // Turn 2 — agent sees the previous exchange
    let q2 = "What topic am I studying?";
    println!("User:  {q2}");
    let r2 = agent.chat(q2, &history).await?;
    println!("Agent: {r2}\n");
    history.push(Message::user(q2));
    history.push(Message::assistant(r2.as_str()));

    // Turn 3
    let q3 = "What's my name again?";
    println!("User:  {q3}");
    let r3 = agent.chat(q3, &history).await?;
    println!("Agent: {r3}");
    history.push(Message::user(q3));
    history.push(Message::assistant(r3.as_str()));
    println!("(history length: {} messages)\n", history.len());

    Ok(())
}

// ── Pattern 2: In-process session store ──────────────────────────────────────

/// A lightweight in-process store keyed by session ID.
///
/// This is the simplest way to serve multiple concurrent users from one process
/// without any external dependency. History is lost on restart.
struct SessionStore {
    sessions: Mutex<HashMap<String, Vec<Message>>>,
}

impl SessionStore {
    fn new() -> Self {
        Self { sessions: Mutex::new(HashMap::new()) }
    }

    fn load(&self, id: &str) -> Vec<Message> {
        self.sessions.lock().unwrap()
            .get(id).cloned().unwrap_or_default()
    }

    fn save(&self, id: &str, history: Vec<Message>) {
        self.sessions.lock().unwrap().insert(id.to_string(), history);
    }
}

// Agent<M>: Chat when M: CompletionModel + 'static (from rig-core impl).
// Using a type-erased reference keeps the calling code simple.
async fn send_with_session<M: rig::completion::CompletionModel + 'static>(
    agent: &rig::agent::Agent<M>,
    store: &SessionStore,
    session_id: &str,
    prompt: &str,
) -> Result<String> {
    let history = store.load(session_id);
    let reply = agent.chat(prompt, &history).await?;

    let mut updated = history;
    updated.push(Message::user(prompt));
    updated.push(Message::assistant(reply.as_str()));
    store.save(session_id, updated);

    Ok(reply)
}

async fn demo_session_store(client: &openai::Client) -> Result<()> {
    println!("━━━ Pattern 2: In-process session store ━━━\n");

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    let store = SessionStore::new();

    // Alice's conversation
    let r1 = send_with_session(&agent, &store, "alice", "Hi! I'm Alice. I prefer short answers.").await?;
    println!("[Alice] Turn 1: {r1}\n");

    // Bob's conversation — completely isolated
    let r2 = send_with_session(&agent, &store, "bob", "Hello! My favourite language is Haskell.").await?;
    println!("[Bob]   Turn 1: {r2}\n");

    // Alice's second turn — agent remembers her preference
    let r3 = send_with_session(&agent, &store, "alice", "Can you recommend a Rust book?").await?;
    println!("[Alice] Turn 2: {r3}\n");

    // Bob's second turn — agent remembers Haskell
    let r4 = send_with_session(&agent, &store, "bob", "Does Rust feel similar to my favourite language?").await?;
    println!("[Bob]   Turn 2: {r4}\n");

    Ok(())
}

// ── Pattern 3: Sliding-window truncation ─────────────────────────────────────

/// Keep only the most recent `n` messages from history.
///
/// This prevents unbounded growth. Older messages are silently dropped.
/// Use when you want a simple, no-dependency guard against context overflow.
fn sliding_window(history: &[Message], max_messages: usize) -> Vec<Message> {
    if history.len() <= max_messages {
        history.to_vec()
    } else {
        history[history.len() - max_messages..].to_vec()
    }
}

async fn demo_sliding_window(client: &openai::Client) -> Result<()> {
    println!("━━━ Pattern 3: Sliding-window (last 4 messages) ━━━\n");

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    let mut history: Vec<Message> = Vec::new();
    const WINDOW: usize = 4; // keep last 4 messages = 2 turns

    // Turn 1 — establish a fact
    let q1 = "I'm working on a project called Titan.";
    let r1 = agent.chat(q1, &sliding_window(&history, WINDOW)).await?;
    history.push(Message::user(q1));
    history.push(Message::assistant(r1.as_str()));
    println!("Turn 1: established project name 'Titan' (history: {} msgs)\n", history.len());

    // Turn 2
    let q2 = "The project uses PostgreSQL for storage.";
    let r2 = agent.chat(q2, &sliding_window(&history, WINDOW)).await?;
    history.push(Message::user(q2));
    history.push(Message::assistant(r2.as_str()));
    println!("Turn 2: added storage detail (history: {} msgs)\n", history.len());

    // Turn 3 — window is now full; Turn 1 will be excluded from next call
    let q3 = "We're deploying to Kubernetes.";
    let r3 = agent.chat(q3, &sliding_window(&history, WINDOW)).await?;
    history.push(Message::user(q3));
    history.push(Message::assistant(r3.as_str()));
    println!("Turn 3: added deployment detail (history: {} msgs, window passes last {})\n",
        history.len(), WINDOW);

    // Turn 4 — window excludes Turn 1 ("Titan"); agent should not recall it
    let q4 = "What is the name of my project?";
    let windowed = sliding_window(&history, WINDOW);
    println!("(Sending {} messages to model — Turn 1 excluded)", windowed.len());
    let r4 = agent.chat(q4, &windowed).await?;
    println!("Turn 4 (project name query): {r4}");
    println!("(Expected: agent cannot recall 'Titan' — it was outside the window)\n");

    Ok(())
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = openai::Client::from_env();

    demo_manual_history(&client).await?;
    println!("────────────────────────────────────────────────\n");
    demo_session_store(&client).await?;
    println!("────────────────────────────────────────────────\n");
    demo_sliding_window(&client).await?;

    Ok(())
}
