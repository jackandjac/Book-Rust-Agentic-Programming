# Chapter 6: Rig Agents and Multi-Turn Conversations

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` (772k downloads — memory module added in 0.37)  
> `futures = "0.3"` (Stream combinators for streaming output)  
> `tokio = "1"`, `anyhow = "1"`, `dotenvy = "0.15"`
>
> **Java reference:** `MessageChatMemoryAdvisor` / `ChatMemory` in Spring AI; `ChatMemory` / `MessageWindowChatMemory` in LangChain4j

---

## What You'll Learn

- How rig's `Agent` type manages system prompts, context, and LLM calls
- Two conversation patterns: manual `Vec<Message>` history vs rig-managed `InMemoryConversationMemory`
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
- **Memory** — optional managed conversation history (covered in §6.4)

That's it. The `Agent` does not run a loop, plan, or take autonomous actions by default — those patterns come from the graph and multi-agent chapters. Here, "agent" means a configured LLM interface with a persona and optional memory.

> **Java parallel:** This is closest to Spring AI's `ChatClient` with a default system prompt and `defaultAdvisors(...)`. The preamble is the system message; context documents are injected via additional advisor messages. LangChain4j's `@AiService` with `@SystemMessage` is also a close match.

---

## 6.2 Building an Agent

The `AgentBuilder` is obtained via `client.agent(model)`. All configuration is optional:

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::providers::openai;

let agent = openai::Client::from_env()?
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
let agent = openai::Client::from_env()?
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
    // .tool(my_tool)
    // Managed memory — see §6.4
    // .memory(memory)
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
| `.memory(mem)` | Enable managed conversation memory (§6.4) |

---

## 6.3 Multi-Turn Conversations: Manual History

The simplest multi-turn pattern: maintain a `Vec<Message>` yourself and pass it to `.chat()` on each turn.

### The `Message` Type

```rust
use rig::message::Message;

// Constructors
Message::user("Hello!");                    // user turn
Message::assistant("Hi, how can I help?"); // assistant turn
Message::system("You are helpful.");        // system message (rare — use preamble instead)
```

### Manual History with `.chat()`

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::message::Message;
use rig::providers::openai;

let agent = openai::Client::from_env()?
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .build();

let mut history: Vec<Message> = Vec::new();

// Turn 1 — history starts empty; chat() appends [user("My name..."), assistant(r1)]
let q1 = "My name is Alice.";
let r1 = agent.chat(q1, &mut history).await?;

// Turn 2 — history now contains the previous exchange; agent knows Alice's name
let q2 = "What's my name?";
let r2 = agent.chat(q2, &mut history).await?;

println!("{r2}"); // "Your name is Alice."
```

Key points:
- `.chat(prompt, &mut Vec<Message>)` — takes a mutable reference; rig-core 0.37 automatically appends both the user turn and the assistant response after each call
- You no longer need to push messages manually — the mutation is handled inside `chat()`
- History is held entirely in your application — rig makes no calls to store or retrieve it
- This is the right pattern when: conversation scope is request-scoped, history is short, or you want full control

### When Manual History Is Appropriate

Manual history works well when:
- You receive the full conversation on each request (stateless service, REST API)
- History is held in a database and you query it before each call
- You want to filter, truncate, or transform history before sending it

The downside: you must manage appending, truncating, and persisting history yourself. For persistent stateful agents, rig provides managed memory.

> **Java parallel:** Manual history is equivalent to building a `List<Message>` and passing it to Spring AI's `ChatClient.prompt().messages(history).call()`. LangChain4j's `UserMessage` / `AiMessage` types map directly to rig's `Message::user()` / `Message::assistant()`.

---

## 6.4 Multi-Turn Conversations: Managed Memory

For persistent agents that maintain conversation history across calls without manual tracking, rig provides the `memory()` builder option combined with per-prompt conversation scoping.

### `InMemoryConversationMemory`

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::memory::InMemoryConversationMemory;
use rig::providers::openai;

// Build memory store (lives for the duration of the program)
let memory = InMemoryConversationMemory::new();

// Attach memory to the agent at build time
let agent = openai::Client::from_env()?
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant with persistent memory.")
    .memory(memory)
    .build();
```

Once `.memory()` is set, use `.conversation(id)` on each prompt to scope history:

```rust
// A conversation identified by "user-42"
let r1 = agent
    .prompt("My name is Alice.")
    .conversation("user-42")
    .await?;

let r2 = agent
    .prompt("What's my name?")
    .conversation("user-42")
    .await?;

println!("{r2}"); // "Your name is Alice."
```

The conversation ID scopes history — `"user-42"` and `"user-99"` have completely independent conversation histories. This means one `Agent` instance can serve many concurrent users without cross-contamination.

```rust
// Two users, isolated histories, same agent
let alice_r = agent.prompt("My name is Alice.").conversation("user-42").await?;
let bob_r   = agent.prompt("My name is Bob.").conversation("user-99").await?;

// Alice's history does not contain Bob's messages
let alice_q = agent.prompt("What's my name?").conversation("user-42").await?;
// → "Your name is Alice."
```

### Constraint: `.conversation()` Requires `.memory()`

`.conversation(id)` is only valid on a prompt when the agent was built with `.memory(...)`. Without it, `.conversation()` has no effect — the agent has nowhere to store or retrieve history. Build the agent with `.memory()` first.

### `InMemoryConversationMemory` Limitations

`InMemoryConversationMemory` stores all history in process memory (a `HashMap` behind a `Mutex`). This means:
- **Not persistent** — history is lost on process restart
- **Not distributed** — not shared across multiple service instances
- **Unbounded** — long conversations grow indefinitely

For production use, the `rig-memory` companion crate adds:
- `SlidingWindowMemory` — retains the N most recent messages
- `TokenWindowMemory` — keeps messages within a token budget
- `CompactingMemory` — summarizes evicted messages into a condensed artifact

These are covered in Chapter 10 (Memory and State).

> **Java parallel:** `InMemoryConversationMemory` maps to Spring AI's `InMemoryChatMemory` backend, combined with `MessageChatMemoryAdvisor`:
> ```java
> ChatMemory chatMemory = new InMemoryChatMemory();
> ChatClient chatClient = ChatClient.builder(chatModel)
>     .defaultAdvisors(
>         MessageChatMemoryAdvisor.builder(chatMemory).build()
>     )
>     .build();
> ```
> In LangChain4j, the equivalent is `MessageWindowChatMemory` + `AiServices`.

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

let agent = openai::Client::from_env()?
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
let agent = openai::Client::from_env()?
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

let moderator = openai::Client::from_env()?
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

let agent = openai::Client::from_env()?
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
use rig::message::Message;

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

The complete runnable example demonstrates both history patterns:

```rust
// code-examples/ch06-agents/src/main.rs
use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::memory::InMemoryConversationMemory;
use rig::message::Message;
use rig::providers::openai;

const PREAMBLE: &str = "\
You are a helpful customer support agent for TechCorp, a fictional software company. \
Your role is to help customers with their questions, troubleshoot issues politely, \
and escalate to a human agent when you cannot resolve the issue. \
Always be professional and empathetic. \
If a customer reports a billing issue, tell them you will escalate to the billing team. \
Never invent information about products you do not know about.";

// Pattern 1: manual Vec<Message> history with .chat()
async fn demo_manual_history(client: &openai::Client) -> Result<()> {
    println!("=== Manual History ===\n");
    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .build();

    let mut history: Vec<Message> = Vec::new();

    let q1 = "Hi, I'm having trouble logging into my account.";
    println!("User: {q1}");
    let r1 = agent.chat(q1, &mut history).await?;
    println!("Agent: {r1}\n");
    // history now has [user(q1), assistant(r1)] — appended automatically

    let q2 = "I've already tried resetting my password twice.";
    println!("User: {q2}");
    let r2 = agent.chat(q2, &mut history).await?;
    println!("Agent: {r2}\n");

    Ok(())
}

// Pattern 2: rig-managed memory with .memory() + .conversation(id)
async fn demo_managed_memory(client: &openai::Client) -> Result<()> {
    println!("=== Managed Memory ===\n");
    let memory = InMemoryConversationMemory::new();
    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(PREAMBLE)
        .memory(memory)
        .build();

    let conv_id = "user-42";

    let r1 = agent
        .prompt("I ordered a laptop last week but it hasn't arrived.")
        .conversation(conv_id)
        .await?;
    println!("Turn 1: {r1}\n");

    let r2 = agent
        .prompt("The order number is ORD-88291.")
        .conversation(conv_id)
        .await?;
    println!("Turn 2: {r2}\n");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let client = openai::Client::from_env()?;
    demo_manual_history(&client).await?;
    demo_managed_memory(&client).await?;
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

## 6.9 Choosing Between the Two History Patterns

| | Manual `Vec<Message>` + `.chat()` | Managed `.memory()` + `.conversation(id)` |
|---|---|---|
| **Who stores history** | Your code | rig (`InMemoryConversationMemory`) |
| **Persistence** | You control (DB, Redis, etc.) | In-process only — lost on restart |
| **Multi-instance** | Works if you load from shared storage | Not distributed |
| **History filtering** | Full control — truncate, transform, filter | Requires `rig-memory` policies |
| **Best for** | Stateless services, DB-backed history | Single-process prototypes, dev tools |
| **Conversation scoping** | Implicit (your Vec per session) | Explicit `conversation_id` string |

In production: load history from a database before each call and use `.chat()`. `InMemoryConversationMemory` is the right tool for prototypes and single-process services.

---

## Key Takeaways

- `Agent<M>` wraps a completion model with a preamble, context, tools, and optional memory. Build one with `client.agent(model).preamble(...).build()`.
- Two required imports: `rig::client::CompletionClient` (for `.agent()`), `rig::client::ProviderClient` (for client construction), `rig::completion::Prompt` (for `.prompt()`).
- **Manual history**: maintain a `Vec<Message>` yourself, pass it to `.chat(prompt, &history)`. You append user/assistant turns after each exchange.
- **Managed memory**: build with `.memory(InMemoryConversationMemory::new())`, then scope each prompt with `.conversation(id)`. The ID isolates history per-user. Requires `.memory()` to be set — `.conversation()` has no effect without it.
- `Message::user(str)` and `Message::assistant(str)` are the constructors for history entries.
- Guardrails are manual: validate input before calling the agent; use an `Extractor<SafetyVerdict>` to classify output before returning it.
- Streaming: `agent.stream_prompt(text).await` or `agent.stream_chat(text, &history).await` — iterate with `StreamExt::next()`, match `MultiTurnStreamItem::FinalResponse` for the complete response or `StreamAssistantItem` for incremental chunks.

---

## Further Reading

- [rig-core Agent docs](https://docs.rs/rig-core/latest/rig/agent/index.html) — `Agent`, `AgentBuilder`, `PromptRequest`
- [rig-core Message docs](https://docs.rs/rig-core/latest/rig/message/index.html) — `Message` enum and constructors
- [rig-core memory docs](https://docs.rs/rig-core/latest/rig/memory/index.html) — `InMemoryConversationMemory`
- [Spring AI ChatClient advisors](https://docs.spring.io/spring-ai/reference/api/advisors.html) — Java reference: `MessageChatMemoryAdvisor`
- [LangChain4j ChatMemory](https://docs.langchain4j.dev/tutorials/chat-memory) — Java reference: `MessageWindowChatMemory`

---

*Next: Chapter 7 — Rig with Axum: Building a Streaming Web API*
