// Chapter 6: Rig Agents and Multi-Turn Conversations
// See chapters/ch06-agents.md for the full explanation.
//
// Run: cargo run -p ch06-agents
// Requires: OPENAI_API_KEY env var (or .env file)

use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{Chat, Message, Prompt};
use rig::providers::openai;

const PREAMBLE: &str = "\
You are a helpful customer support agent for TechCorp, a fictional software company. \
Your role is to help customers with their questions, troubleshoot issues politely, \
and escalate to a human agent when you cannot resolve the issue. \
Always be professional and empathetic. \
If a customer reports a billing issue, tell them you will escalate to the billing team. \
Never invent information about products you do not know about.";

// Pattern 1: manual Vec<Message> history with .chat()
// chat() takes impl IntoIterator<Item: Into<Message>> — pass &history by reference.
// chat() does NOT mutate history — push user and assistant messages yourself.
async fn demo_manual_history(client: &openai::Client) -> Result<()> {
    println!("=== Manual History (Vec<Message> + .chat()) ===\n");

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    let mut history: Vec<Message> = Vec::new();

    // Turn 1
    let q1 = "Hi, I'm having trouble logging into my account.";
    println!("User: {q1}");
    let r1 = agent.chat(q1, &history).await?;
    println!("Agent: {r1}\n");
    // Append this exchange to history manually
    history.push(Message::user(q1));
    history.push(Message::assistant(r1.as_str()));

    // Turn 2 — history carries the previous exchange
    let q2 = "I've already tried resetting my password twice.";
    println!("User: {q2}");
    let r2 = agent.chat(q2, &history).await?;
    println!("Agent: {r2}\n");
    history.push(Message::user(q2));
    history.push(Message::assistant(r2.as_str()));

    Ok(())
}

// Pattern 2: single-shot prompts (stateless — no history carried between calls)
async fn demo_prompt(client: &openai::Client) -> Result<()> {
    println!("=== Single-Shot Prompt (.prompt()) ===\n");

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    let response = agent
        .prompt("What is your return policy for laptops?")
        .await?;
    println!("Response: {response}\n");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = openai::Client::from_env();

    demo_manual_history(&client).await?;
    println!("---\n");
    demo_prompt(&client).await?;

    Ok(())
}
