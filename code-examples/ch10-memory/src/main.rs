// Chapter 10: Memory and State in Rust Agents
// See chapters/ch10-memory.md for the full explanation.
//
// Run: cargo run -p ch10-memory
// Requires: OPENAI_API_KEY env var (or .env file)
//
// Demonstrates three memory patterns:
//   1. Manual history — Vec<Message> + Agent::chat(&mut history)
//   2. Managed memory — InMemoryConversationMemory + .conversation(id)
//   3. Sliding-window policy — rig-memory SlidingWindowMemory limits history size

use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::memory::InMemoryConversationMemory;
use rig::message::Message;
use rig::providers::openai;
use rig_memory::{InMemoryConversationMemory as PolicyMemoryStore, SlidingWindowMemory};

const PREAMBLE: &str = "\
You are a helpful personal assistant with a good memory. \
Refer back to earlier parts of the conversation when relevant. \
Keep answers concise — one to three sentences.";

// ── Pattern 1: Manual Vec<Message> history ────────────────────────────────────

/// Demonstrates manual history management.
///
/// `Agent::chat(prompt, &mut Vec<Message>)` in rig-core 0.37 automatically
/// appends both the user turn and the assistant response after each call.
/// You pass in the full accumulated history on every turn.
async fn demo_manual_history(client: &openai::Client) -> Result<()> {
    println!("━━━ Pattern 1: Manual Vec<Message> history ━━━\n");

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    // Start with an empty history — no prior context.
    let mut history: Vec<Message> = Vec::new();

    // Turn 1 — agent sees no prior messages
    let q1 = "My name is Alice and I'm learning Rust.";
    println!("User:  {q1}");
    let r1 = agent.chat(q1, &mut history).await?;
    println!("Agent: {r1}\n");
    // history now contains: [User("My name is Alice..."), Assistant(r1)]

    // Turn 2 — agent sees the previous exchange; knows the user's name
    let q2 = "What topic am I studying?";
    println!("User:  {q2}");
    let r2 = agent.chat(q2, &mut history).await?;
    println!("Agent: {r2}\n");
    // history now contains 4 messages

    // Turn 3 — demonstrate that history is growing
    let q3 = "What's my name again?";
    println!("User:  {q3}");
    let r3 = agent.chat(q3, &mut history).await?;
    println!("Agent: {r3}");
    println!("(history length: {} messages)\n", history.len());

    Ok(())
}

// ── Pattern 2: Managed memory with InMemoryConversationMemory ────────────────

/// Demonstrates rig-managed memory.
///
/// `InMemoryConversationMemory` stores per-conversation history in a HashMap.
/// The agent loads history before processing and appends after responding —
/// the caller just provides a `conversation_id` string per request.
async fn demo_managed_memory(client: &openai::Client) -> Result<()> {
    println!("━━━ Pattern 2: InMemoryConversationMemory ━━━\n");

    // Create the in-memory backend and attach it to the agent at build time.
    let memory = InMemoryConversationMemory::new();

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .memory(memory)
        .build();

    // Two separate conversations on the same agent instance.
    // Each conversation_id is completely isolated.
    let alice_id = "alice-session-1";
    let bob_id = "bob-session-1";

    // Alice's conversation
    let r1 = agent
        .prompt("Hi! I'm Alice. I prefer short answers.")
        .conversation(alice_id)
        .await?;
    println!("[Alice] Turn 1: {r1}\n");

    // Bob's conversation — agent has no knowledge of Alice's session
    let r2 = agent
        .prompt("Hello! My favourite language is Haskell.")
        .conversation(bob_id)
        .await?;
    println!("[Bob]   Turn 1: {r2}\n");

    // Alice's second turn — agent remembers she prefers short answers
    let r3 = agent
        .prompt("Can you recommend a Rust book?")
        .conversation(alice_id)
        .await?;
    println!("[Alice] Turn 2: {r3}\n");

    // Bob's second turn — agent remembers his language preference
    let r4 = agent
        .prompt("Does Rust feel similar to my favourite language?")
        .conversation(bob_id)
        .await?;
    println!("[Bob]   Turn 2: {r4}\n");

    Ok(())
}

// ── Pattern 3: Sliding-window policy via rig-memory ──────────────────────────

/// Demonstrates bounded history with SlidingWindowMemory.
///
/// Without a window policy, history grows unboundedly, eventually exceeding the
/// model's context window. `SlidingWindowMemory::new(n)` keeps only the most
/// recent `n` messages, discarding older ones as new turns arrive.
///
/// This uses `rig_memory::InMemoryConversationMemory` (from the `rig-memory`
/// crate) which accepts a policy via `.with_filter()`. It is the same logical
/// type as `rig::memory::InMemoryConversationMemory` but with policy support.
async fn demo_sliding_window(client: &openai::Client) -> Result<()> {
    println!("━━━ Pattern 3: Sliding-window (last 4 messages) ━━━\n");

    // Keep at most 4 messages in the active window (2 turns).
    // Older messages are silently dropped when the window fills.
    let memory = PolicyMemoryStore::new()
        .with_filter(SlidingWindowMemory::new(4));

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .memory(memory)
        .build();

    let conv = "sliding-demo";

    // Establish a fact early in the conversation.
    let _ = agent
        .prompt("I'm working on a project called Titan.")
        .conversation(conv)
        .await?;
    println!("Turn 1: established project name 'Titan'\n");

    // Add more turns to push the first message out of the window.
    let _ = agent
        .prompt("The project uses PostgreSQL for storage.")
        .conversation(conv)
        .await?;
    println!("Turn 2: added storage detail\n");

    let _ = agent
        .prompt("We're deploying to Kubernetes.")
        .conversation(conv)
        .await?;
    println!("Turn 3: added deployment detail (window now at 4 messages)\n");

    // By now Turn 1 has been evicted from the window (window = 4 messages = 2 turns).
    // The agent should NOT know the project name anymore.
    let r = agent
        .prompt("What is the name of my project?")
        .conversation(conv)
        .await?;
    println!("Turn 4 (project name query): {r}");
    println!("(Expected: agent cannot recall 'Titan' — it was evicted from the window)\n");

    Ok(())
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = openai::Client::from_env()?;

    demo_manual_history(&client).await?;
    println!("────────────────────────────────────────────────\n");
    demo_managed_memory(&client).await?;
    println!("────────────────────────────────────────────────\n");
    demo_sliding_window(&client).await?;

    Ok(())
}
