// Chapter 3: LLM Basics in Rust — streaming chat CLI
// See chapters/ch03-llm-basics-in-rust.md for the full explanation.
//
// Run: cargo run -p ch03-llm-basics
// Requires: OPENAI_API_KEY env var (or .env file)

use anyhow::Result;
use async_openai::{
    types::chat::{
        ChatCompletionRequestAssistantMessage,
        ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessage,
        ChatCompletionRequestUserMessage,
        CreateChatCompletionRequestArgs,
    },
    Client,
};
use futures::StreamExt;
use std::io::{self, stdout, BufRead, Write};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let client = Client::new(); // reads OPENAI_API_KEY from environment

    let system_prompt = "You are a helpful assistant for Rust developers. \
        Be concise and practical. Use code examples when helpful.";

    let mut history: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestSystemMessage::from(system_prompt).into(),
    ];

    println!("Rust Chat CLI — type your message and press Enter. Ctrl+C to exit.\n");

    let stdin = io::stdin();
    let mut stdout = stdout();

    loop {
        print!("You: ");
        stdout.flush()?;

        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim().to_string();

        if input.is_empty() {
            continue;
        }

        history.push(ChatCompletionRequestUserMessage::from(input.as_str()).into());

        let request = CreateChatCompletionRequestArgs::default()
            .model("gpt-4o-mini")
            .max_completion_tokens(1024u32)
            .messages(history.clone())
            .build()?;

        print!("Assistant: ");
        stdout.flush()?;

        let mut stream = client.chat().create_stream(request).await?;
        let mut full_reply = String::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(response) => {
                    for choice in &response.choices {
                        if let Some(content) = &choice.delta.content {
                            print!("{content}");
                            stdout.flush()?;
                            full_reply.push_str(content);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("\nError: {err}");
                    break;
                }
            }
        }

        println!("\n");

        history.push(
            ChatCompletionRequestAssistantMessage::from(full_reply.as_str()).into(),
        );
    }
}
