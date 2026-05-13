# Chapter 3: LLM Basics in Rust

> **Framework versions in this chapter:**  
> `async-openai = "0.38"` (4.8M downloads, updated May 11 2026)  
> `rig-core = "0.37"` (772k downloads — bumped from 0.36; all Ch3 APIs unchanged)  
> `tokio = "1"`, `serde = "1"`, `anyhow = "1"`, `dotenvy = "0.15"`  
>
> **Java reference:** "LangChain4j ChatModel basics" and "Spring AI ChatClient first application"

---

## What You'll Learn

- How to call the OpenAI API directly with `async-openai` — the low-level foundation
- How streaming chat works in Rust and why it matters for agent UX
- How `rig-core` provides a higher-level abstraction — and what it trades away for convenience
- How multi-turn conversation history is managed at the API level
- How to connect to Anthropic and Ollama as alternative providers
- Build: a streaming chat CLI — the "Hello, World" of LLM programming

---

## 3.1 Two Levels of Abstraction

Before writing any code, let's understand the landscape of this chapter.

In the Java AI ecosystem, you typically work at one level:

- **LangChain4j's `ChatModel`**: a clean abstraction over all providers
- **Spring AI's `ChatClient`**: a fluent builder over the same providers

In Rust, this chapter covers two levels intentionally:

| Level | Crate | Java parallel |
|-------|-------|--------------|
| Low-level | `async-openai` | Direct API calls (no Java parallel — Java devs rarely do this) |
| High-level | `rig-core` | `ChatModel` in LangChain4j / `ChatClient` in Spring AI |

**Why bother with the low level?**

Because `async-openai` is what every Rust AI application actually runs on at its core — including `rig-core` itself. Understanding it means you can:
- Debug issues that the higher-level abstraction hides
- Use features that `rig-core` hasn't wrapped yet
- Write code that doesn't break when `rig-core`'s pre-1.0 API changes

We'll start with `async-openai`, understand the raw API shape, then move to `rig-core` and see how it simplifies things.

---

## 3.2 Project Setup

The companion code for this chapter lives in `code-examples/ch03-llm-basics/`. Let's build it from scratch here so you understand every line.

Create the project:

```bash
cargo new ch03-llm-basics
cd ch03-llm-basics
```

`Cargo.toml`:

```toml
[package]
name = "ch03-llm-basics"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1", features = ["full"] }
async-openai = "0.38"
rig-core = "0.37"
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
futures = "0.3"        # for StreamExt trait on streaming responses
dotenvy = "0.15"       # loads .env file

[features]
default = []
```

Create `.env` in the project root (never commit this):

```bash
OPENAI_API_KEY=sk-...
# ANTHROPIC_API_KEY=sk-ant-...   # needed for §3.7
```

Add `.env` to `.gitignore`:

```bash
echo ".env" >> .gitignore
```

---

## 3.3 Your First LLM Call with async-openai

Let's make the simplest possible call: send a message, get a response.

```rust
// src/main.rs
use anyhow::Result;
use async_openai::{
    types::chat::{
        ChatCompletionRequestSystemMessage,
        ChatCompletionRequestUserMessage,
        CreateChatCompletionRequestArgs,
    },
    Client,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Load API key from .env file
    dotenvy::dotenv().ok();

    // Client reads OPENAI_API_KEY from environment automatically
    let client = Client::new();

    // Build the request using the builder pattern
    let request = CreateChatCompletionRequestArgs::default()
        .model("gpt-4o-mini")
        .max_tokens(256u32)
        .messages([
            ChatCompletionRequestSystemMessage::from(
                "You are a concise assistant. Answer in one sentence.",
            )
            .into(),
            ChatCompletionRequestUserMessage::from(
                "What is the main advantage of Rust over Java for network services?",
            )
            .into(),
        ])
        .build()?;

    // Send the request — this awaits the full response
    let response = client.chat().create(request).await?;

    // Extract the text from the first choice
    for choice in response.choices {
        if let Some(content) = choice.message.content {
            println!("{content}");
        }
    }

    Ok(())
}
```

Run it:

```bash
cargo run
```

Expected output (your response will vary):

```
Rust eliminates garbage collection pauses through compile-time memory management,
giving network services deterministic latency that Java's GC cannot guarantee.
```

### Mapping to Java

In LangChain4j, the equivalent is:

```java
// Java — LangChain4j
ChatLanguageModel model = OpenAiChatModel.builder()
    .apiKey(System.getenv("OPENAI_API_KEY"))
    .modelName("gpt-4o-mini")
    .maxTokens(256)
    .build();

String response = model.generate("What is the main advantage of Rust over Java?");
System.out.println(response);
```

In Spring AI:

```java
// Java — Spring AI
@Autowired ChatClient chatClient;

String response = chatClient.prompt()
    .system("You are a concise assistant. Answer in one sentence.")
    .user("What is the main advantage of Rust over Java?")
    .call()
    .content();
```

The Rust version is more verbose because there's no dependency injection framework managing the client. You create the client explicitly, build the request explicitly, and handle the response explicitly. This verbosity is a trade-off for transparency — there's no magic.

---

## 3.4 Understanding the Response Structure

The response from `client.chat().create(request)` returns a `CreateChatCompletionResponse`. Let's look at what's inside:

```rust
let response = client.chat().create(request).await?;

// Top-level fields
println!("Model: {}", response.model);
println!("Choices: {}", response.choices.len());

// Usage statistics (token counts)
if let Some(usage) = response.usage {
    println!("Prompt tokens: {}", usage.prompt_tokens);
    println!("Completion tokens: {}", usage.completion_tokens);
    println!("Total tokens: {}", usage.total_tokens);
}

// The actual response text is inside choices[0].message.content
let first_choice = &response.choices[0];
println!("Stop reason: {:?}", first_choice.finish_reason);
println!("Content: {:?}", first_choice.message.content);
```

**Why `content` is `Option<String>`:** The API can return a response with no text content — for example, when the model makes a tool call instead of generating text. The `Option` forces you to handle both cases. You'll see this pattern throughout async-openai: anything that might be absent is wrapped in `Option`.

**`finish_reason`** tells you why the model stopped:
- `Stop` — normal completion
- `Length` — hit `max_tokens` limit
- `ToolCalls` — model wants to call a tool (Chapter 4)
- `ContentFilter` — content was filtered

This maps to LangChain4j's `Response<AiMessage>` which has similar metadata, but in Rust it's modeled as a plain struct with `Option` fields rather than a wrapper object.

---

## 3.5 Streaming Responses

Streaming is critical for agent UX. Without it, users stare at a blank screen for 5–30 seconds waiting for a complete response. With streaming, tokens appear as they're generated — the same experience as ChatGPT's interface.

```rust
use anyhow::Result;
use async_openai::{
    types::chat::{
        ChatCompletionRequestSystemMessage,
        ChatCompletionRequestUserMessage,
        CreateChatCompletionRequestArgs,
    },
    Client,
};
use futures::StreamExt; // provides .next() on the stream
use std::io::{stdout, Write};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let client = Client::new();

    let request = CreateChatCompletionRequestArgs::default()
        .model("gpt-4o-mini")
        .max_tokens(512u32)
        .messages([
            ChatCompletionRequestSystemMessage::from(
                "You are a helpful assistant explaining Rust to Java developers.",
            )
            .into(),
            ChatCompletionRequestUserMessage::from(
                "Explain Rust's ownership model in 3 paragraphs.",
            )
            .into(),
        ])
        .build()?;

    // create_stream instead of create — returns a stream of chunks
    let mut stream = client.chat().create_stream(request).await?;

    // Lock stdout once — more efficient than locking on every write
    let mut lock = stdout().lock();

    while let Some(result) = stream.next().await {
        match result {
            Ok(response) => {
                for choice in &response.choices {
                    // Each chunk contains a delta — the incremental new text
                    if let Some(content) = &choice.delta.content {
                        write!(lock, "{content}")?;
                    }
                }
                // Flush ensures tokens appear immediately, not buffered
                stdout().flush()?;
            }
            Err(err) => {
                eprintln!("\nStream error: {err}");
                break;
            }
        }
    }

    // Print a newline after streaming completes
    println!();

    Ok(())
}
```

### What's different from the non-streaming version

| Non-streaming | Streaming |
|--------------|-----------|
| `client.chat().create(request)` | `client.chat().create_stream(request)` |
| Returns complete `CreateChatCompletionResponse` | Returns `Stream<Item = Result<CreateChatCompletionStreamResponse>>` |
| `choice.message.content` — the full text | `choice.delta.content` — incremental chunk |
| Wait for full response | Tokens appear as generated |

The key type change: instead of `message.content` (the full response), you get `delta.content` (each new piece). The `delta` is the diff since the last chunk — you accumulate them to build the full response if you need it.

### Java comparison

In LangChain4j, streaming uses a `StreamingChatLanguageModel`:

```java
// Java — LangChain4j streaming
StreamingChatLanguageModel model = OpenAiStreamingChatModel.builder()
    .apiKey(apiKey)
    .modelName("gpt-4o-mini")
    .build();

model.generate("Explain Rust ownership", new StreamingResponseHandler<AiMessage>() {
    @Override
    public void onNext(String token) {
        System.out.print(token); // each token as it arrives
    }

    @Override
    public void onComplete(Response<AiMessage> response) {
        System.out.println("\nDone.");
    }

    @Override
    public void onError(Throwable error) {
        error.printStackTrace();
    }
});
```

The Rust version uses a `Stream` (similar to Java's `Flow.Publisher`) processed with `StreamExt::next()`. Rust's approach is more composable — the stream is a value you can pass around, filter, or map before consuming it.

---

## 3.6 Multi-Turn Conversations

A single LLM call is stateless — the API has no memory between calls. Multi-turn conversation requires sending the entire history with every request. This is true in every language and framework.

```rust
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

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let client = Client::new();

    // History grows with each turn
    let mut history: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestSystemMessage::from(
            "You are a Rust tutor helping Java developers learn Rust."
        )
        .into(),
    ];

    // Turn 1: user asks a question
    let user_message = "What is ownership in Rust?";
    history.push(
        ChatCompletionRequestUserMessage::from(user_message).into()
    );

    let response = send_and_record(&client, &mut history).await?;
    println!("Assistant: {response}\n");

    // Turn 2: follow-up question — history includes previous turns
    let followup = "How does that differ from Java's garbage collector?";
    history.push(
        ChatCompletionRequestUserMessage::from(followup).into()
    );

    let response2 = send_and_record(&client, &mut history).await?;
    println!("Assistant: {response2}\n");

    Ok(())
}

/// Send the current history to the LLM, record the assistant's reply,
/// and return the reply text.
async fn send_and_record(
    client: &Client,
    history: &mut Vec<ChatCompletionRequestMessage>,
) -> Result<String> {
    let request = CreateChatCompletionRequestArgs::default()
        .model("gpt-4o-mini")
        .max_tokens(512u32)
        .messages(history.clone())
        .build()?;

    let response = client.chat().create(request).await?;

    let reply = response
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .unwrap_or_default();

    // Append the assistant's reply to history for the next turn
    history.push(
        ChatCompletionRequestAssistantMessage::from(reply.as_str()).into()
    );

    Ok(reply)
}
```

### The history management pattern

Notice that `history` is a `Vec<ChatCompletionRequestMessage>` that grows with each turn. This is exactly what LangChain4j's `MessageWindowChatMemory` does under the hood — it stores messages and truncates at a window size. In this raw form you control it explicitly.

**Memory considerations:** Tokens cost money and LLMs have context limits. A production agent needs to either:
1. Truncate old messages (sliding window — what LangChain4j's `MessageWindowChatMemory` does)
2. Summarize old messages (what LangChain4j's `ConversationSummaryMemory` does)

Chapter 7 covers memory management strategies in depth. For now, understand that history is just a `Vec` — there's nothing magic about it.

---

## 3.7 Switching to rig-core: The Higher-Level API

`async-openai` gives you full control but requires boilerplate. `rig-core` is the higher-level abstraction — think of it as LangChain4j's `ChatModel` level of abstraction in Rust.

Here's the same interaction from §3.3, rewritten with `rig-core`:

```rust
use anyhow::Result;
use rig::{
    client::{CompletionClient, ProviderClient},
    completion::Prompt,
    providers::openai,
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    // Creates client reading OPENAI_API_KEY from environment
    let client = openai::Client::from_env()?;

    // Build an agent with a system prompt (preamble)
    let agent = client
        .agent(openai::GPT_4O_MINI)              // named constant for model string
        .preamble("You are a concise assistant. Answer in one sentence.")
        .max_tokens(256)
        .build();

    // .prompt() returns the response text directly — no unwrapping required
    let response = agent
        .prompt("What is the main advantage of Rust over Java for network services?")
        .await?;

    println!("{response}");

    Ok(())
}
```

The reduction in boilerplate is significant:

| `async-openai` | `rig-core` |
|---------------|-----------|
| Build `CreateChatCompletionRequestArgs` | `.agent().preamble().build()` |
| `.client.chat().create(request).await?` | `.prompt("...").await?` |
| Unwrap `choices[0].message.content` | Returns `String` directly |
| ~25 lines | ~12 lines |

**What rig-core trades away:**
- Access to raw `usage` statistics (token counts)
- Access to `finish_reason`
- Fine-grained per-request configuration (some options not exposed)
- Direct access to streaming at this abstraction level (it uses a different streaming API)

For most agent code, the trade-off is worth it. When you need raw access, drop down to `async-openai`.

### Named model constants

`rig-core` provides named constants for model strings, reducing typos:

```rust
// Verified constants in rig-core 0.37 (rig::providers::openai::completion):
openai::GPT_4O          // "gpt-4o"
openai::GPT_4O_MINI     // "gpt-4o-mini"
openai::GPT_4_1         // "gpt-4.1"
openai::GPT_5_2         // "gpt-5.2"
openai::O4_MINI         // "o4-mini"
// Full list: https://docs.rs/rig-core/0.37.0/rig/providers/openai/completion/
```

> **Note:** These constants change as new models are released. Always check the `rig_core::providers::openai` module docs for the current list. If a constant doesn't exist for a model you need, you can pass the model string directly: `.agent("gpt-4o-2024-11-20")`.

---

## 3.8 Multi-Turn Chat with rig-core

`rig-core`'s `Agent` has a `chat()` method that accepts a prompt and an existing history:

```rust
use anyhow::Result;
use rig::{
    client::{CompletionClient, ProviderClient},
    completion::{Chat, Message},
    providers::openai,
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let client = openai::Client::from_env()?;

    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble("You are a Rust tutor helping Java developers.")
        .build();

    // History of previous messages (role + content pairs)
    let mut history: Vec<Message> = vec![];

    // Turn 1
    let reply1 = agent
        .chat("What is ownership in Rust?", history.clone())
        .await?;
    println!("Turn 1: {reply1}\n");

    // Record the exchange in history
    history.push(Message {
        role: "user".to_string(),
        content: "What is ownership in Rust?".to_string(),
    });
    history.push(Message {
        role: "assistant".to_string(),
        content: reply1,
    });

    // Turn 2 — history includes the previous turn
    let reply2 = agent
        .chat("How does that differ from Java's GC?", history.clone())
        .await?;
    println!("Turn 2: {reply2}");

    Ok(())
}
```

> **API note (rig-core 0.37):** `Agent::chat()` takes `&mut Vec<Message>` and **automatically appends** both the user turn and the assistant response after each call. You do not need to push messages manually — see Chapter 6 for the full multi-turn pattern. Full API: [`rig::agent`](https://docs.rs/rig-core/0.37.0/rig/agent/).

---

## 3.9 Alternative Providers

One of `rig-core`'s main advantages over `async-openai` is unified provider support. Switching from OpenAI to Anthropic or Ollama requires changing two lines.

### Anthropic Claude

```toml
# Cargo.toml — no extra dependency needed, included in rig-core
```

```rust
use rig::{
    client::{CompletionClient, ProviderClient},
    completion::Prompt,
    providers::anthropic,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    // Reads ANTHROPIC_API_KEY from environment
    let client = anthropic::Client::from_env()?;

    // Constant verified: rig-core 0.37 anthropic::completion::CLAUDE_SONNET_4_6
    // The path is anthropic::completion::CLAUDE_SONNET_4_6 or re-exported — check docs.rs
    let agent = client
        .agent(anthropic::completion::CLAUDE_SONNET_4_6)
        .preamble("You are a concise assistant.")
        .build();

    let response = agent.prompt("Explain Rust ownership briefly.").await?;
    println!("{response}");
    Ok(())
}
```

### Ollama (local LLMs)

Ollama lets you run models locally — no API key required, no data leaving your machine.

First, install and start Ollama: [ollama.ai](https://ollama.ai). Pull a model:

```bash
ollama pull llama3.2
```

Then in Rust:

```rust
use rig::{
    client::{CompletionClient, ProviderClient},
    completion::Prompt,
    providers::ollama,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Client::new() defaults to http://localhost:11434, no auth
    // Signature: Client::new(None) or Client::new(Nothing) depending on version
    // If compilation fails, try: ollama::Client::builder().build().unwrap()
    let client = ollama::Client::new(None).unwrap();

    let agent = client
        .agent("llama3.2")    // model name as string — must match pulled model
        .preamble("You are a concise assistant.")
        .build();

    let response = agent.prompt("What is Rust's ownership model?").await?;
    println!("{response}");
    Ok(())
}
```

The same code structure works regardless of provider. This is the key benefit of the `rig-core` abstraction — identical agent code runs against OpenAI, Anthropic, or local models with minimal changes.

### Provider comparison

| Provider | Crate constant | Env var | Local? |
|---------|---------------|---------|--------|
| OpenAI | `openai::GPT_4O_MINI` | `OPENAI_API_KEY` | ❌ |
| Anthropic | `anthropic::CLAUDE_3_5_SONNET` | `ANTHROPIC_API_KEY` | ❌ |
| Ollama | Model name as string | None | ✅ |
| Azure OpenAI | Via `openai::Client::from_url()` | `AZURE_OPENAI_API_KEY` | ❌ |

---

## 3.10 Hands-On Project: Streaming Chat CLI

Let's build a complete interactive streaming chat CLI — the equivalent of a minimal ChatGPT terminal interface. This pulls together everything in the chapter.

```rust
// src/bin/chat-cli.rs
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

    let client = Client::new();

    let system_prompt = "You are a helpful assistant for Rust developers. \
        Be concise and practical. Use code examples when helpful.";

    let mut history: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestSystemMessage::from(system_prompt).into(),
    ];

    println!("Rust Chat CLI — type your message and press Enter. Ctrl+C to exit.\n");

    let stdin = io::stdin();
    let mut stdout = stdout();

    loop {
        // Print prompt and flush
        print!("You: ");
        stdout.flush()?;

        // Read a line from stdin
        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim().to_string();

        if input.is_empty() {
            continue;
        }

        // Add user message to history
        history.push(
            ChatCompletionRequestUserMessage::from(input.as_str()).into()
        );

        // Build streaming request with full history
        let request = CreateChatCompletionRequestArgs::default()
            .model("gpt-4o-mini")
            .max_tokens(1024u32)
            .messages(history.clone())
            .build()?;

        print!("Assistant: ");
        stdout.flush()?;

        // Stream the response
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

        println!("\n"); // newline after response

        // Record assistant's full reply in history
        history.push(
            ChatCompletionRequestAssistantMessage::from(full_reply.as_str()).into()
        );
    }
}
```

Add the binary to `Cargo.toml`:

```toml
[[bin]]
name = "chat-cli"
path = "src/bin/chat-cli.rs"
```

Run it:

```bash
cargo run --bin chat-cli
```

Sample session:

```
Rust Chat CLI — type your message and press Enter. Ctrl+C to exit.

You: What is the ? operator in Rust?
Assistant: The `?` operator is shorthand for error propagation. In a function
returning `Result<T, E>`, appending `?` to a fallible call will either unwrap
the `Ok` value or return the `Err` immediately to the caller...

You: How does it compare to Java's try-catch?
Assistant: The key difference is that `?` makes the error path visible in the
function's return type — callers know the function can fail. Java's unchecked
exceptions don't appear in method signatures, so failures can be invisible...
```

Each user message and assistant reply is accumulated in `history`, so follow-up questions have full context.

---

## 3.11 What We Didn't Cover (and Where to Find It)

This chapter focused on the essentials. Here's what's next:

| Topic | Chapter |
|-------|---------|
| Tool calling (function calling) | Chapter 4 |
| Structured output with serde | Chapter 5 |
| Embeddings and vector search | Chapter 6 |
| Memory management (truncation, summarization) | Chapter 7 |
| Anthropic API details | Chapter 4 (tool calling) |
| Local LLMs with Kalosm (full local inference) | Chapter 13 |

**async-openai features not covered here:**
- Vision / multimodal inputs (image in the message)
- Audio transcription and speech generation
- Fine-tuning API
- Embeddings (covered in Chapter 6)
- Batch API for offline processing

All of these follow the same builder pattern you've learned — consult [docs.rs/async-openai](https://docs.rs/async-openai) for their APIs.

---

## Key Takeaways

- **`async-openai`** is the low-level foundation — verbose but transparent. It gives you full control over request/response structure and token usage.
- **`rig-core`** is the higher-level abstraction — less boilerplate, unified provider API, but pre-1.0 API stability.
- **Streaming** uses `create_stream()` instead of `create()` and processes `delta.content` chunks — critical for agent UX.
- **Multi-turn conversation** is just a growing `Vec` of messages sent with every request — the LLM API itself is stateless.
- **Provider switching** in `rig-core` requires changing only the client and model constant — the agent code is identical.
- **`Option<String>` for content** is intentional: the model might respond with a tool call instead of text. Handle both cases.

---

## Further Reading

- [async-openai docs](https://docs.rs/async-openai) — full API reference with all request types
- [async-openai examples](https://github.com/64bit/async-openai/tree/main/examples) — 60+ working examples covering every API feature
- [rig-core docs](https://docs.rs/rig-core) — Agent, completion, and provider APIs
- [Ollama](https://ollama.ai) — running local LLMs for development without API costs
- [OpenAI API reference](https://platform.openai.com/docs/api-reference/chat) — the underlying API that async-openai wraps

---

*Next: Chapter 4 — Tool Calling with Rig: the `#[rig_tool]` Macro vs Java's `@Tool`*
