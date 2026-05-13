# Chapter 10: Memory and State in Rust Agents

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` — memory module added in 0.37; `chat()` signature changed  
> `rig-memory = "0.1"` — `SlidingWindowMemory`, `TokenWindowMemory` policies  
> `tokio = "1"`, `anyhow = "1"`, `dotenvy = "0.15"`
>
> **Java reference:** LangChain4j `ChatMemory`, `MessageWindowChatMemory`, `TokenWindowChatMemory`; Spring AI `MessageChatMemoryAdvisor`, `InMemoryChatMemory`

---

An agent that cannot remember previous turns is not an assistant — it's a calculator. Every real-world application needs at least basic conversational memory: the system must know whether the user said "Alice" ten messages ago.

But memory is also a resource. LLMs have finite context windows. Every message in history costs tokens, and tokens cost money and latency. Unbounded memory eventually fails; bounded memory must evict something. Deciding *what* to evict, *when*, and *how* to compensate is one of the core design decisions in agent architecture.

This chapter covers three memory patterns in rig-core 0.37:

1. **Manual `Vec<Message>`** — you manage history explicitly; most flexible, most code
2. **`InMemoryConversationMemory`** — rig manages per-conversation history; less code, same process only
3. **Policy-based history shaping** (`rig-memory`) — sliding window and token budget keep history bounded

---

## 10.1 How LLM Memory Works

Before examining rig's API, it's worth understanding what "memory" means at the protocol level.

Every LLM completion call takes a list of messages:

```
[system, user, assistant, user, assistant, user, …]
```

The model has no persistent state between calls. Every "memory" is faked by resending past messages on each request. This is the *stateless over stateful* pattern: the *service* (the LLM) is stateless; the *client* (your agent) manages state.

```
Turn 1:  [system] [user: "Hi, I'm Alice"]           → assistant: "Hello Alice!"
Turn 2:  [system] [user: "Hi, I'm Alice"]           ← these must be re-sent
         [assistant: "Hello Alice!"]
         [user: "What's my name?"]                   → assistant: "Your name is Alice."
```

This has two implications:
- **Token cost grows with history length.** Every turn re-sends all prior turns.
- **Context window limit.** Most models cap at 128k–1M tokens. Long conversations eventually hit this limit.

### Java comparison

LangChain4j's `ChatMemory` interface captures this exact pattern:

```java
// LangChain4j
ChatMemory memory = MessageWindowChatMemory.withMaxMessages(10);
AiService assistant = AiServices.builder(Assistant.class)
    .chatLanguageModel(model)
    .chatMemory(memory)
    .build();
```

Spring AI's equivalent is the `MessageChatMemoryAdvisor`:

```java
// Spring AI
ChatClient client = ChatClient.builder(chatModel)
    .defaultAdvisors(new MessageChatMemoryAdvisor(new InMemoryChatMemory()))
    .build();
```

In rig 0.37, the three patterns below cover the same ground.

---

## 10.2 Pattern 1 — Manual `Vec<Message>`

The simplest pattern gives you total control: you hold a `Vec<Message>` and pass it to every `chat()` call.

```rust
use rig::message::Message;

let mut history: Vec<Message> = Vec::new();

let agent = client
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .build();

// Turn 1 — history starts empty
let r1 = agent.chat("My name is Alice.", &mut history).await?;
// history = [User("My name is Alice."), Assistant(r1)]

// Turn 2 — history carries the previous exchange
let r2 = agent.chat("What's my name?", &mut history).await?;
// history = [User("My name..."), Assistant(r1), User("What's my name?"), Assistant(r2)]
```

**What changed in rig-core 0.37:** `chat()` now takes `&mut Vec<Message>` (not an immutable iterator). It appends both the user message and the assistant response automatically after each call. You no longer push messages manually.

### When to use manual history

- **Stateless services** — receive the full conversation from the client on each request (REST API pattern), pass it to `chat()`, return the response. Nothing persists server-side.
- **Database-backed history** — load `Vec<Message>` from SQL/Redis before the call, save it after. Full control over serialisation format.
- **History transformation** — filter, truncate, or rewrite messages before sending. Manual gives you a hook between turns.

### Serialising history to JSON

`Message` derives `serde::Serialize` and `serde::Deserialize`, so you can persist history trivially:

```rust
use rig::message::Message;
use std::fs;

// Save after each turn
let json = serde_json::to_string_pretty(&history)?;
fs::write("conversation.json", &json)?;

// Restore at next startup
let saved: String = fs::read_to_string("conversation.json")?;
let history: Vec<Message> = serde_json::from_str(&saved)?;
```

This is the foundation of simple persistence: write JSON to disk (or a `TEXT` column in SQLite), read it back on next launch.

---

## 10.3 Pattern 2 — `InMemoryConversationMemory`

Manual history is powerful but verbose. When all you need is "keep history for this conversation in process memory", rig-core 0.37 provides a built-in backend.

```rust
use rig::memory::InMemoryConversationMemory;

let memory = InMemoryConversationMemory::new();

let agent = client
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .memory(memory)
    .build();

// Use .conversation(id) to scope history per user/session
let r1 = agent
    .prompt("Hi, I'm Alice.")
    .conversation("alice-42")
    .await?;

let r2 = agent
    .prompt("What's my name?")
    .conversation("alice-42")
    .await?;
// r2 correctly refers back to "Alice"
```

The agent loads history for `"alice-42"` before processing, appends the new exchange, and stores it back — all transparently. Your code only specifies the conversation ID.

### Multiple conversations on one agent

The key advantage over manual history: a single agent instance handles many concurrent users, each with their own isolated history.

```rust
// Two users, same agent, different conversation IDs
let _ = agent.prompt("My favourite language is Haskell.").conversation("bob-1").await?;
let _ = agent.prompt("I prefer Lisp.").conversation("carol-2").await?;

// Each user's history is completely isolated
let r = agent.prompt("What's my favourite language?").conversation("bob-1").await?;
// → "Your favourite language is Haskell."
```

This is what LangChain4j achieves with a separate `ChatMemory` instance per user. In rig, one `InMemoryConversationMemory` handles all users — the ID is the key.

### Opting out of memory for a single request

Sometimes you want a one-off question without polluting the conversation history:

```rust
let r = agent
    .prompt("What year is it?")
    .without_memory()  // not recorded; not loaded
    .await?;
```

### Limitations

`InMemoryConversationMemory` stores everything in a `HashMap` inside the process. It disappears when the process exits. For durability across restarts, you need a persistent backend — Section 10.5 covers the pattern.

---

## 10.4 The `ConversationMemory` Trait

Both `InMemoryConversationMemory` (rig-core) and any custom backend implement the `ConversationMemory` trait from `rig::memory`:

```rust
pub trait ConversationMemory: Send + Sync {
    async fn load(&self, conversation_id: &str) -> Result<Vec<Message>>;
    async fn append(&self, conversation_id: &str, messages: Vec<Message>) -> Result<()>;
    async fn clear(&self, conversation_id: &str) -> Result<()>;
}
```

This is the extension point for custom backends. A Redis-backed implementation:

```rust
use rig::memory::ConversationMemory;
use rig::message::Message;

pub struct RedisMemory {
    client: redis::Client,
    ttl_secs: usize,
}

impl ConversationMemory for RedisMemory {
    async fn load(&self, id: &str) -> anyhow::Result<Vec<Message>> {
        let mut conn = self.client.get_async_connection().await?;
        let raw: Option<String> = redis::cmd("GET").arg(id).query_async(&mut conn).await?;
        match raw {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(Vec::new()),
        }
    }

    async fn append(&self, id: &str, messages: Vec<Message>) -> anyhow::Result<()> {
        let mut conn = self.client.get_async_connection().await?;
        // Load, merge, save
        let mut history = self.load(id).await?;
        history.extend(messages);
        let json = serde_json::to_string(&history)?;
        redis::cmd("SETEX")
            .arg(id).arg(self.ttl_secs).arg(json)
            .query_async(&mut conn).await?;
        Ok(())
    }

    async fn clear(&self, id: &str) -> anyhow::Result<()> {
        let mut conn = self.client.get_async_connection().await?;
        redis::cmd("DEL").arg(id).query_async(&mut conn).await?;
        Ok(())
    }
}
```

Attach it to an agent exactly like the built-in backend:

```rust
let memory = RedisMemory { client, ttl_secs: 3600 };
let agent = client.agent(openai::GPT_4O_MINI)
    .memory(memory)
    .build();
```

### Java comparison

LangChain4j's `ChatMemoryStore` interface is structurally identical:

```java
public interface ChatMemoryStore {
    List<ChatMessage> getMessages(Object memoryId);
    void updateMessages(Object memoryId, List<ChatMessage> messages);
    void deleteMessages(Object memoryId);
}
```

The `ConversationMemory` trait is Rust's version of the same contract.

---

## 10.5 Pattern 3 — Bounded History with `rig-memory`

The previous patterns let history grow without limit. That's fine for short conversations, but will eventually:
- Exceed the model's context window (hard failure)
- Inflate cost and latency on every turn (soft failure)

The `rig-memory` crate provides **memory policies** — functions that transform history before it is sent to the model:

```toml
[dependencies]
rig-memory = "0.1"
```

### `SlidingWindowMemory`

Keeps the most recent `n` messages, discarding older ones:

```rust
use rig_memory::{InMemoryConversationMemory, SlidingWindowMemory};

// Keep last 20 messages (10 turns)
let memory = InMemoryConversationMemory::new()
    .with_filter(SlidingWindowMemory::new(20));

let agent = client
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .memory(memory)
    .build();
```

> **Note:** `rig_memory::InMemoryConversationMemory` (from the `rig-memory` crate) is the policy-aware variant. It shares the same logical purpose as `rig::memory::InMemoryConversationMemory` (from rig-core) but adds the `.with_filter()` method. Use the `rig-memory` variant when you need a policy; use the rig-core variant for simple in-memory storage.

### `TokenWindowMemory`

Keeps messages that fit within a token budget, using a heuristic counter that requires no external API call:

```rust
use rig_memory::{
    InMemoryConversationMemory, TokenWindowMemory,
    HeuristicTokenCounter, TokenCounterPreset,
};

// Keep messages within 8192 tokens (OpenAI preset)
let counter = HeuristicTokenCounter::new(TokenCounterPreset::OpenAI);
let memory = InMemoryConversationMemory::new()
    .with_filter(TokenWindowMemory::new(8192, counter));

let agent = client
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .memory(memory)
    .build();
```

`TokenWindowMemory` is preferable to `SlidingWindowMemory` when conversations have variable message sizes — a long code block in one message can consume more tokens than twenty short exchanges.

### Choosing a budget

A practical rule of thumb for `gpt-4o-mini` (128k context window):
- Reserve ~4k tokens for the system prompt and tool schemas
- Reserve ~4k tokens for the response
- Budget ~8k–16k for conversation history

```rust
// Leaves ~16k for system prompt + response
let memory = InMemoryConversationMemory::new()
    .with_filter(TokenWindowMemory::new(16_384, counter));
```

### Java comparison

LangChain4j's `MessageWindowChatMemory` and `TokenWindowChatMemory` are direct parallels:

```java
// LangChain4j — message window
ChatMemory memory = MessageWindowChatMemory.withMaxMessages(20);

// LangChain4j — token window
ChatMemory memory = TokenWindowChatMemory.builder()
    .maxTokens(8192, tokenizer)
    .build();
```

The rig-memory API follows the same mental model: window type + size + optional tokenizer.

---

## 10.6 Memory Compaction (Summarisation)

When old messages are evicted by a sliding window, context is permanently lost. For long-running agents — personal assistants, support bots, research agents — losing early context is unacceptable.

**Compaction** replaces evicted messages with a summary instead of discarding them. The `rig-memory` crate provides `CompactingMemory` for this:

```rust
use rig_memory::{
    CompactingMemory, DemotingPolicyMemory,
    SlidingWindowMemory, TemplateCompactor,
};

// TemplateCompactor produces plain-text rollups without an LLM call.
// For LLM-driven summaries, implement the Compactor trait yourself.
let compactor = TemplateCompactor::default();
let memory = CompactingMemory::new(
    SlidingWindowMemory::new(20),
    compactor,
);
```

The compaction flow:
1. History grows beyond the window
2. The policy identifies messages to evict
3. The compactor synthesises a summary message: `"Earlier in this conversation: …"`
4. The summary replaces the evicted messages in the active window

The agent always sees a compact history that fits the window *and* retains a summary of earlier context.

> **When to compact:** compaction makes most sense for persistent personal assistants (session that resumes across days/weeks). For session-scoped support bots or API handlers, a simple `SlidingWindowMemory` is usually sufficient.

---

## 10.7 Persistence Patterns

### Simple: JSON on disk

Suitable for single-user applications or prototypes:

```rust
use std::path::Path;
use rig::message::Message;

async fn load_history(path: &str) -> Vec<Message> {
    if Path::new(path).exists() {
        let json = tokio::fs::read_to_string(path).await.unwrap_or_default();
        serde_json::from_str(&json).unwrap_or_default()
    } else {
        Vec::new()
    }
}

async fn save_history(path: &str, history: &[Message]) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(history)?;
    tokio::fs::write(path, json).await?;
    Ok(())
}

// Usage
let mut history = load_history("session.json").await;
let response = agent.chat("Continue where we left off.", &mut history).await?;
save_history("session.json", &history).await?;
```

### Production: SQLite

For multi-user servers, SQLite via `sqlx` gives you ACID transactions with no external service:

```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "json"] }
```

```rust
use sqlx::SqlitePool;
use rig::message::Message;

async fn load_history(pool: &SqlitePool, conv_id: &str) -> anyhow::Result<Vec<Message>> {
    let row = sqlx::query!(
        "SELECT messages FROM conversations WHERE id = ?",
        conv_id
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some(r) => Ok(serde_json::from_str(&r.messages)?),
        None => Ok(Vec::new()),
    }
}

async fn save_history(
    pool: &SqlitePool,
    conv_id: &str,
    history: &[Message],
) -> anyhow::Result<()> {
    let json = serde_json::to_string(history)?;
    sqlx::query!(
        "INSERT INTO conversations (id, messages) VALUES (?, ?)
         ON CONFLICT(id) DO UPDATE SET messages = excluded.messages",
        conv_id, json
    )
    .execute(pool)
    .await?;
    Ok(())
}
```

### Implementing `ConversationMemory` on top of SQLite

Wrap the SQLite functions in a struct that implements `ConversationMemory` (Section 10.4) and attach it to the agent with `.memory()`. This gives you the ergonomics of managed memory with durable storage.

---

## 10.8 Hands-On: Three-Pattern Demo

The complete example in `code-examples/ch10-memory/` exercises all three patterns in sequence.

```bash
cd code-examples
export OPENAI_API_KEY="sk-..."
cargo run -p ch10-memory
```

Expected output (assistant responses will vary):

```
━━━ Pattern 1: Manual Vec<Message> history ━━━

User:  My name is Alice and I'm learning Rust.
Agent: Nice to meet you, Alice! Rust is a great choice.

User:  What topic am I studying?
Agent: You're studying Rust.

User:  What's my name again?
Agent: Your name is Alice.
(history length: 6 messages)

────────────────────────────────────────────────

━━━ Pattern 2: InMemoryConversationMemory ━━━

[Alice] Turn 1: Hello Alice! I'll keep my answers concise.
[Bob]   Turn 1: Hello! Haskell is an excellent language.
[Alice] Turn 2: I recommend "The Rust Programming Language" (the Book).
[Bob]   Turn 2: Rust does share some concepts with Haskell ...

────────────────────────────────────────────────

━━━ Pattern 3: Sliding-window (last 4 messages) ━━━

Turn 1: established project name 'Titan'
Turn 2: added storage detail
Turn 3: added deployment detail (window now at 4 messages)
Turn 4 (project name query): I don't have that information in our conversation.
(Expected: agent cannot recall 'Titan' — it was evicted from the window)
```

### Walkthrough: sliding window

The key insight in Pattern 3:

```
Window = 4 messages

After Turn 1: [U:"Titan", A:"Got it"]                           (2 msgs)
After Turn 2: [U:"Titan", A:"Got it", U:"PostgreSQL", A:"Got it"] (4 msgs — full)
After Turn 3: [U:"PostgreSQL", A:"Got it", U:"Kubernetes", A:"Got it"]
              ↑ "Titan" was evicted when Turn 3 pushed window to 5
After Turn 4: agent has no knowledge of "Titan"
```

This demonstrates that sliding-window memory is **not transparent** to the user. If your application requires a graceful degradation story ("I recall you mentioned something earlier but can no longer access it"), either use compaction (Section 10.6) or increase the window size.

---

## 10.9 Choosing a Memory Strategy

| Scenario | Recommended Pattern |
|----------|---------------------|
| Stateless API — history sent on each request | Manual `Vec<Message>` |
| Single-server multi-user bot, no durability needed | `InMemoryConversationMemory` |
| Long conversations — must control context window cost | `SlidingWindowMemory` or `TokenWindowMemory` |
| Long-running personal assistant — can't lose early context | `CompactingMemory` |
| Multi-server deployment — must survive restart | Custom `ConversationMemory` over Redis/SQLite |
| Semantic recall — "what did we say about X?" | `dynamic_context` + vector store (Chapter 8) |

The last row is important: conversational memory and RAG memory are **orthogonal**. A production agent often uses both:
- `InMemoryConversationMemory` or `SlidingWindowMemory` for recent turn history
- A vector index (Chapter 8's `dynamic_context`) for long-term semantic search over past exchanges or documents

---

## 10.10 Key Takeaways

- **LLM memory is faked** — every call re-sends prior messages; the model has no persistent state.
- **`Agent::chat(prompt, &mut Vec<Message>)`** (rig-core 0.37) — auto-appends both turns; no manual push.
- **`InMemoryConversationMemory`** (rig-core) — zero-config per-conversation storage; use `.conversation(id)` to scope by user.
- **`.without_memory()`** — opt out of memory for a single request without changing the agent's configuration.
- **`SlidingWindowMemory` / `TokenWindowMemory`** (rig-memory crate) — bounded history; use `.with_filter()` on the policy-aware `InMemoryConversationMemory` from `rig-memory`.
- **`CompactingMemory`** — replaces evicted messages with a summary; preserves early context at the cost of an extra LLM call.
- **Custom `ConversationMemory`** — implement three async methods (`load`, `append`, `clear`) to use any storage backend.
- **`Message` is serializable** — `Vec<Message>` can be round-tripped through `serde_json` for any persistence layer.
- **`rig-memory` vs rig-core memory** — use `rig::memory::InMemoryConversationMemory` for simple storage; use `rig_memory::InMemoryConversationMemory` (from the `rig-memory` crate) when you need a policy.

---

## What's Next

This chapter gave you the memory primitives. Chapter 11 moves to MCP — the Model Context Protocol — which standardises how agents discover and call tools exposed by external servers. The `rmcp` crate is Rust's official MCP SDK, and it complements rig's built-in tool system with a standardised network protocol.

---

*→ Java reference: LangChain4j `ChatMemory`, `MessageWindowChatMemory`, `TokenWindowChatMemory`, `ChatMemoryStore`; Spring AI `MessageChatMemoryAdvisor`, `InMemoryChatMemory`*
