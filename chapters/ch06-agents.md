# Chapter 6: Rig Agents and Multi-Turn Conversations

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` (772k downloads)  
> `futures = "0.3"` (Stream combinators for streaming output)  
> `tokio = "1"`, `anyhow = "1"`, `dotenvy = "0.15"`
>
> **Java reference:** `MessageChatMemoryAdvisor` / `ChatMemory` in Spring AI; `ChatMemory` / `MessageWindowChatMemory` in LangChain4j

---

## What You'll Learn

- How rig's `Agent` type manages system prompts, context, and LLM calls
- Two conversation patterns: manual `Vec<Message>` history with `.chat()`, and streaming with `FinalResponse::history()`
- The `AgentBuilder` configuration surface: preamble, context, temperature, max tokens
- Streaming agent output with `stream_prompt()` and `stream_chat()`
- How to write persona and guardrail logic in a preamble
- Build: a multi-turn customer support agent

---

## 6.1 What an Agent Is (and Isn't)

In rig, an `Agent<M>` is a thin wrapper around a completion model (`M: CompletionModel`) that adds:

- A **preamble** — the system prompt, set at build time
- **Context documents** — additional static background injected before each call
- **Tools** — callable functions the LLM can invoke (covered in Chapter 4)
- **Conversation history** — passed in on each call; management is the application's responsibility (§6.3, §6.4)

That's it. The `Agent` does not run a loop, plan, or take autonomous actions by default — those patterns come from the graph and multi-agent chapters. Here, "agent" means a configured LLM interface with a persona and optional memory.

> **Java parallel:** This is closest to Spring AI's `ChatClient` with a default system prompt and `defaultAdvisors(...)`. The preamble is the system message; context documents are injected via additional advisor messages. LangChain4j's `@AiService` with `@SystemMessage` is also a close match.

---

## 6.2 Building an Agent

The `AgentBuilder` is obtained via `client.agent(model)`. All configuration is optional:

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::providers::openai;

let agent = openai::Client::from_env()
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .build();

let response = agent.prompt("What is Rust?").await?;
println!("{response}");
```

Required imports:

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;  // brings .prompt() into scope
use rig::providers::openai;
```

`ProviderClient` is required for `openai::Client::from_env()` methods. `CompletionClient` is required for `.agent()`. `Prompt` brings the `.prompt()` method into scope on the built `Agent`.

### The Full Builder API

```rust
let agent = openai::Client::from_env()
    .agent(openai::GPT_4O)
    // System prompt — the agent's persona and instructions
    .preamble("You are an expert Rust developer. Be concise and precise.")
    // Static context documents injected before each request
    .context("Company policy: never reveal internal pricing.")
    .context("Product name: RustBot v2.1")
    // Model parameters
    .temperature(0.2)       // lower = more deterministic
    .max_tokens(1024)
    // Tools — see Chapter 4
    // .tool(my_tool)  // see Chapter 4
    .build();
```

| Builder method | Purpose |
|---|---|
| `.preamble(str)` | System prompt — sets the agent's persona and behavior |
| `.append_preamble(str)` | Appends to an existing preamble without replacing it |
| `.context(str)` | Adds a static context document injected before each request |
| `.temperature(f64)` | Sampling temperature (0.0–1.0; lower = more deterministic) |
| `.max_tokens(u64)` | Maximum tokens in the response |
| `.tool(tool)` | Register a callable tool (Chapter 4) |

---

## 6.3 Multi-Turn Conversations: Manual History

The simplest multi-turn pattern: maintain a `Vec<Message>` yourself and pass it to `.chat()` on each turn.

### The `Message` Type

```rust
use rig::completion::Message;

// Constructors
Message::user("Hello!");                    // user turn
Message::assistant("Hi, how can I help?"); // assistant turn
Message::system("You are helpful.");        // system message (rare — use preamble instead)
```

### Manual History with `.chat()`

```rust
use rig::client::CompletionClient;
use rig::completion::Chat;
use rig::completion::Message;
use rig::providers::openai;

let agent = openai::Client::from_env()
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .build();

let mut history: Vec<Message> = Vec::new();

// Turn 1 — pass history by reference (borrows, does not consume or mutate it)
let q1 = "My name is Alice.";
let r1 = agent.chat(q1, &history).await?;

// Append this exchange manually — chat() does NOT mutate history
history.push(Message::user(q1));
history.push(Message::assistant(r1.as_str()));

// Turn 2 — history now contains the previous exchange; agent knows Alice's name
let q2 = "What's my name?";
let r2 = agent.chat(q2, &history).await?;
history.push(Message::user(q2));
history.push(Message::assistant(r2.as_str()));

println!("{r2}"); // "Your name is Alice."
```

Key points:
- `.chat(prompt, chat_history)` — takes `impl IntoIterator<Item: Into<Message>>`. Passing `&history` works because `&Vec<T>` implements `IntoIterator`.
- `chat()` does **not** mutate history — you push `Message::user(prompt)` and `Message::assistant(reply)` yourself after each call
- `Message::user(text)` and `Message::assistant(text)` accept `impl Into<String>`
- History is held entirely in your application — rig makes no calls to store or retrieve it

### When Manual History Is Appropriate

Manual history works well when:
- You receive the full conversation on each request (stateless service, REST API)
- History is held in a database and you query it before each call
- You want to filter, truncate, or transform history before sending it

> **Java parallel:** Manual history is equivalent to building a `List<Message>` and passing it to Spring AI's `ChatClient.prompt().messages(history).call()`. LangChain4j's `UserMessage` / `AiMessage` types map directly to rig's `Message::user()` / `Message::assistant()`.

---

## 6.4 Multi-Turn Conversations: Streaming with History

For interactive applications, rig's streaming API provides a clean way to maintain history through the `FinalResponse` object returned at the end of a stream. The `FinalResponse::history()` method returns the updated message list — user turn + assistant response — ready to pass to the next call.

### Streaming Multi-Turn Pattern

```rust
use anyhow::Result;
use futures::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::client::CompletionClient;
use rig::completion::Message;
use rig::providers::openai;
use rig::streaming::StreamingChat;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let agent = openai::Client::from_env()
        .agent(openai::GPT_4O_MINI)
        .preamble("You are a helpful assistant.")
        .build();

    let mut history: Vec<Message> = Vec::new();

    // Turn 1
    let mut stream = agent.stream_chat("My name is Alice.", &history).await;
    while let Some(item) = stream.next().await {
        match item? {
            MultiTurnStreamItem::FinalResponse(fin) => {
                // extend history with [user("My name is Alice."), assistant(reply)]
                history.extend_from_slice(fin.history().unwrap_or_default());
            }
            _ => {}
        }
    }

    // Turn 2 — history contains the previous exchange
    let mut stream = agent.stream_chat("What's my name?", &history).await;
    while let Some(item) = stream.next().await {
        match item? {
            MultiTurnStreamItem::FinalResponse(fin) => {
                history.extend_from_slice(fin.history().unwrap_or_default());
                // Print the reply from history
                if let Some(last) = fin.history().and_then(|h| h.last()) {
                    println!("{last:?}"); // "Your name is Alice."
                }
            }
            _ => {}
        }
    }

    Ok(())
}
```

The `fin.history()` slice contains the messages added this turn — append them to your `Vec<Message>` for the next call.

### Non-Streaming Multi-Turn Pattern

When using `.chat()` (non-streaming), there is no `FinalResponse` — push user and assistant messages manually as shown in §6.3:

```rust
let q = "What is ownership?";
let reply = agent.chat(q, &history).await?;
history.push(Message::user(q));
history.push(Message::assistant(reply.as_str()));
```

### History Storage Strategies

| Approach | Where history lives | Good for |
|---|---|---|
| `Vec<Message>` in function | Stack / local scope | Single session, request-scoped handlers |
| `Arc<Mutex<Vec<Message>>>` | Shared heap | Multi-threaded server, one entry per session ID |
| Database (SQLite, Postgres) | External storage | Production agents, persistence across restarts |
| Redis `LPUSH/LRANGE` | External cache | Distributed services, TTL-based expiry |

For production, store history keyed by session ID in a database or Redis. Load it before each call, pass it to `.chat()` or `.stream_chat()`, then persist the updated history. Chapter 10 covers memory management strategies — window sizing, token budgets, and compaction — in depth.

> **Java parallel:** This matches Spring AI's `InMemoryChatMemory` with `MessageChatMemoryAdvisor` for prototype work, and a Redis- or JDBC-backed `ChatMemory` for production. The explicit `Vec<Message>` approach maps directly to LangChain4j's `MessageWindowChatMemory.messages()` — you manage the list, the framework just sends it.

---

## 6.5 Preambles and Personas

The preamble is the agent's system prompt — it shapes how the LLM interprets every subsequent message. Treat it as the agent's standing instructions.

### Writing Effective Preambles

```rust
const SUPPORT_PREAMBLE: &str = "\
You are a customer support agent for TechCorp. \
\n\nBehavior rules:\
\n- Be professional and empathetic at all times.\
\n- If the customer has a billing question, say you are escalating to the billing team.\
\n- Never invent product features or pricing you do not have information about.\
\n- If you cannot resolve the issue, offer to connect them with a human agent.\
\n\nYou do not have access to order management systems in this session.";

let agent = openai::Client::from_env()
    .agent(openai::GPT_4O_MINI)
    .preamble(SUPPORT_PREAMBLE)
    .build();
```

Tips:
- **Be explicit about what the agent should not do** — "never invent pricing" is more reliable than "be accurate"
- **State limitations clearly** — "You do not have access to order management systems" prevents the LLM from fabricating order lookups
- **Use numbered or bulleted rules** — structured preambles are easier for the LLM to follow than prose
- **Keep it under ~1000 tokens** — preamble counts against your context budget on every call

### Adding Context Documents

Use `.context()` for static background information that should be available on every call without being part of the conversation history:

```rust
let agent = openai::Client::from_env()
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a TechCorp support agent.")
    .context("TechCorp products: RustBot (IDE plugin), DataFlow (ETL tool), CloudSync (backup service).")
    .context("Support escalation policy: billing → billing@techcorp.com; technical → eng-support@techcorp.com")
    .build();
```

Context documents are injected before each request. They are useful for: product catalogs, FAQs, policy documents, and other reference material that's too large for the preamble but should always be available.

---

## 6.6 Guardrails — Manual Patterns

Rig does not have built-in content moderation. Guardrails are a pattern you implement — rig gives you the tools.

### Input Validation

The simplest guardrail: validate input before sending to the agent.

```rust
fn check_input(input: &str) -> Result<(), String> {
    if input.len() > 4000 {
        return Err("Input too long. Please limit your message to 4000 characters.".into());
    }
    // Check for prompt injection attempts (basic)
    let blocked = ["ignore previous instructions", "disregard your preamble", "system:"];
    for phrase in &blocked {
        if input.to_lowercase().contains(phrase) {
            return Err("I cannot process that request.".into());
        }
    }
    Ok(())
}

// In your request handler:
match check_input(&user_message) {
    Ok(()) => {
        let response = agent.prompt(&user_message).await?;
        println!("{response}");
    }
    Err(msg) => println!("Rejected: {msg}"),
}
```

### Output Classification with `Extractor`

For output moderation, use an `Extractor` to classify the agent's response before returning it to the user:

```rust
use rig::client::{CompletionClient, ProviderClient};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct SafetyVerdict {
    /// Whether the response is safe to show to the user
    safe: bool,
    /// Brief reason if not safe
    #[schemars(required)]
    reason: Option<String>,
}

let moderator = openai::Client::from_env()
    .extractor::<SafetyVerdict>(openai::GPT_4O_MINI)
    .preamble(
        "Classify whether the following text is safe to show to a customer support user. \
         Safe means: professional, not harmful, not revealing internal data. \
         Unsafe means: contains profanity, reveals internal system details, or is harmful."
    )
    .build();

let agent_response = agent.prompt(&user_message).await?;

let verdict = moderator.extract(&agent_response).await?;
if verdict.safe {
    println!("{agent_response}");
} else {
    println!("I'm sorry, I can't help with that right now.");
    eprintln!("Moderation blocked: {:?}", verdict.reason);
}
```

This is a two-call pattern (agent response + moderation check) — appropriate for high-stakes customer-facing applications. For most internal tools, a well-crafted preamble is sufficient.

> **Java parallel:** Spring AI's `SafeGuardAdvisor` intercepts prompt and response at the advisor layer. LangChain4j does not have a built-in moderation advisor — you implement similar logic manually in your `@Tool` or `@AiService` implementation, exactly as shown above.

---

## 6.7 Streaming Agent Output

For interactive applications (chat UIs, CLI tools), streaming the response character-by-character gives a much better user experience than waiting for the full response.

### `stream_prompt()` — Single-Shot Streaming

```rust
use anyhow::{Result, anyhow};
use futures::StreamExt;
use rig::agent::{MultiTurnStreamItem, StreamingResult};
use rig::client::{CompletionClient, ProviderClient};
use rig::streaming::StreamingChat;
use rig::providers::openai;

let agent = openai::Client::from_env()
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .build();

// stream_prompt returns StreamingPromptRequest immediately (not async)
let mut stream = agent.stream_prompt("Tell me a short story about a crab.").await;

// Collect the final response from the stream
let mut response = String::new();
while let Some(item) = stream.next().await {
    match item? {
        MultiTurnStreamItem::FinalResponse(r) => {
            response = r.response().to_owned();
        }
        _ => {} // intermediate chunks if any
    }
}
println!("{response}");
```

### `stream_chat()` — Streaming with History

```rust
use rig::completion::Message;

let history = vec![
    Message::user("What programming language should I learn first?"),
    Message::assistant("I recommend Rust for systems programming or Python for data science."),
];

let mut stream = agent.stream_chat("Tell me more about Rust.", &history).await;

let mut final_response = String::new();
while let Some(item) = stream.next().await {
    if let MultiTurnStreamItem::FinalResponse(r) = item? {
        final_response = r.response().to_owned();
    }
}
println!("{final_response}");
```

### Note on Streaming in Practice

For real-time display (printing tokens as they arrive), you would process intermediate chunks from `StreamAssistantItem` rather than waiting for `FinalResponse`. The `MultiTurnStreamItem` enum (marked `#[non_exhaustive]`) has three variants as of rig-core 0.37:

| Variant | Purpose |
|---|---|
| `FinalResponse` | Terminal — the complete assistant response |
| `StreamAssistantItem(StreamedAssistantContent<R>)` | Intermediate — partial tokens or tool calls from the assistant |
| `StreamUserItem(StreamedUserContent)` | Tool results injected into the stream |

The pattern above (collecting only `FinalResponse`) is the safe baseline. For token-level streaming to a UI, match on `StreamAssistantItem` and extract partial text to display as it arrives.

---

## 6.8 Hands-On: Customer Support Agent

The complete runnable example demonstrates the manual history pattern:

```rust
// code-examples/ch06-agents/src/main.rs
use anyhow::Result;
use rig::client::CompletionClient;
use rig::completion::{Chat, Prompt};
use rig::completion::Message;
use rig::providers::openai;

const PREAMBLE: &str = "\
You are a helpful customer support agent for TechCorp, a fictional software company. \
Your role is to help customers with their questions, troubleshoot issues politely, \
and escalate to a human agent when you cannot resolve the issue. \
Always be professional and empathetic. \
If a customer reports a billing issue, tell them you will escalate to the billing team. \
Never invent information about products you do not know about.";

// Manual Vec<Message> history — push user/assistant turns yourself after each call
async fn demo_manual_history(client: &openai::Client) -> Result<()> {
    println!("=== Manual History ===\n");
    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    let mut history: Vec<Message> = Vec::new();

    let q1 = "Hi, I'm having trouble logging into my account.";
    println!("User: {q1}");
    let r1 = agent.chat(q1, &history).await?;
    println!("Agent: {r1}\n");
    history.push(Message::user(q1));
    history.push(Message::assistant(r1.as_str()));

    let q2 = "I've already tried resetting my password twice.";
    println!("User: {q2}");
    let r2 = agent.chat(q2, &history).await?;
    println!("Agent: {r2}\n");

    Ok(())
}

async fn demo_prompt(client: &openai::Client) -> Result<()> {
    println!("=== Single-Shot Prompt ===\n");
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
```

### Running the Example

```bash
cd code-examples
export OPENAI_API_KEY=sk-...
cargo run -p ch06-agents
```

---

## 6.9 Choosing a History Storage Pattern

Rig's `Agent` does not manage conversation history for you — it sends whatever history you pass in and returns a response. Persistence is your responsibility. The choice is which data structure to use:

| Pattern | Where history lives | Good for |
|---|---|---|
| `Vec<Message>` in local scope | Stack | Single-session CLI tools, tests |
| `Arc<Mutex<Vec<Message>>>` | Shared heap (keyed by session ID) | In-process multi-user servers |
| SQLite / Postgres | External storage | Production agents needing persistence |
| Redis | External cache with TTL | Distributed services, session expiry |

The API is the same in every case: before each call, load or build your `Vec<Message>`, pass `&history` to `.chat()` or `.stream_chat()`, then persist the new messages after. Chapter 10 covers window truncation and token budget strategies for keeping histories within model context limits.

---

## Key Takeaways

- `Agent<M>` wraps a completion model with a preamble, context, and tools. Build one with `client.agent(model).preamble(...).build()`.
- Required imports: `rig::client::CompletionClient` (for `.agent()`), `rig::completion::Chat` (for `.chat()`), `rig::completion::Prompt` (for `.prompt()`).
- **History management is your responsibility** — rig provides no automatic conversation store. Maintain a `Vec<Message>`, pass `&history` to `.chat()`, then push `Message::user(q)` and `Message::assistant(reply)` after each call.
- `Message::user(text)` and `Message::assistant(text)` accept `impl Into<String>`.
- **Streaming history**: use `stream_chat()` and call `fin.history()` on the `FinalResponse` to get the appended messages for that turn — `history.extend_from_slice(fin.history().unwrap_or_default())`.
- Guardrails are manual: validate input before calling the agent; use an `Extractor<SafetyVerdict>` to classify output before returning it.
- Streaming: `agent.stream_chat(text, &history)` — iterate with `StreamExt::next()`, match `MultiTurnStreamItem::FinalResponse` for the complete response or `StreamAssistantItem` for incremental chunks.

---

## Further Reading

- [rig-core Agent docs](https://docs.rs/rig-core/latest/rig/agent/index.html) — `Agent`, `AgentBuilder`, `PromptRequest`
- [rig-core Message docs](https://docs.rs/rig-core/latest/rig/message/index.html) — `Message` enum and constructors
- [rig-core streaming docs](https://docs.rs/rig-core/latest/rig/streaming/index.html) — `StreamingChat`, `FinalResponse::history()`
- [Spring AI ChatClient advisors](https://docs.spring.io/spring-ai/reference/api/advisors.html) — Java reference: `MessageChatMemoryAdvisor`
- [LangChain4j ChatMemory](https://docs.langchain4j.dev/tutorials/chat-memory) — Java reference: `MessageWindowChatMemory`

---

*Next: Chapter 7 — Rig with Axum: Building a Streaming Web API*
