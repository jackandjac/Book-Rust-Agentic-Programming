// Chapter 6: Rig Agents and Multi-Turn Conversations
// See chapters/ch06-agents.md for the full explanation.
//
// Run: cargo run -p ch06-agents
// Requires: OPENAI_API_KEY env var (or .env file)

use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::memory::InMemoryConversationMemory;
use rig::message::Message; // Vec<Message> for manual history
use rig::providers::openai;

const PREAMBLE: &str = "\
You are a helpful customer support agent for TechCorp, a fictional software company. \
Your role is to help customers with their questions, troubleshoot issues politely, \
and escalate to a human agent when you cannot resolve the issue. \
Always be professional and empathetic. \
If a customer reports a billing issue, tell them you will escalate to the billing team. \
Never invent information about products you do not know about.";

// Demonstrate manual history management with .chat()
//
// In rig-core 0.37, Agent::chat() takes &mut Vec<Message> and automatically
// appends both the user turn and the assistant response to the vector.
// The caller no longer needs to push messages manually.
async fn demo_manual_history(client: &openai::Client) -> Result<()> {
    println!("=== Manual History (Vec<Message> + .chat()) ===\n");

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    // &mut Vec<Message> — chat() appends user + assistant messages automatically
    let mut history: Vec<Message> = Vec::new();

    // Turn 1 — history is empty; chat() will append [user("Hi..."), assistant(r1)]
    let q1 = "Hi, I'm having trouble logging into my account.";
    println!("User: {q1}");
    let r1 = agent.chat(q1, &mut history).await?;
    println!("Agent: {r1}\n");
    // history now contains: [user("Hi..."), assistant(r1)]

    // Turn 2 — history carries the previous exchange; agent remembers context
    let q2 = "I've already tried resetting my password twice.";
    println!("User: {q2}");
    let r2 = agent.chat(q2, &mut history).await?;
    println!("Agent: {r2}\n");
    // history now contains: [user("Hi..."), assistant(r1), user("I've..."), assistant(r2)]

    Ok(())
}

// Demonstrate rig-managed memory with .memory() + .conversation(id)
async fn demo_managed_memory(client: &openai::Client) -> Result<()> {
    println!("=== Managed Memory (InMemoryConversationMemory) ===\n");

    let memory = InMemoryConversationMemory::new();

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .memory(memory)
        .build();

    // conversation_id scopes history — "user-42" is isolated from "user-99"
    let conv_id = "user-42";

    let r1 = agent
        .prompt("Hello, I ordered a laptop last week but it hasn't arrived.")
        .conversation(conv_id)
        .await?;
    println!("Turn 1 → {r1}\n");

    let r2 = agent
        .prompt("The order number is ORD-88291.")
        .conversation(conv_id)
        .await?;
    println!("Turn 2 → {r2}\n");

    let r3 = agent
        .prompt("It was supposed to arrive in 3-5 business days.")
        .conversation(conv_id)
        .await?;
    println!("Turn 3 → {r3}\n");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = openai::Client::from_env()?;

    demo_manual_history(&client).await?;
    println!("---\n");
    demo_managed_memory(&client).await?;

    Ok(())
}
