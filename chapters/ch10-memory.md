# Chapter 10: Memory and State in Rust Agents

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` · `tokio = "1"` · `anyhow = "1"` · `dotenvy = "0.15"`
>
> **Java reference:** LangChain4j `ChatMemory`, `MessageWindowChatMemory`, `TokenWindowChatMemory`; Spring AI `MessageChatMemoryAdvisor`, `InMemoryChatMemory`

---

An agent that cannot remember previous turns is not an assistant — it's a calculator. Every real-world application needs at least basic conversational memory: the system must know whether the user said "Alice" ten messages ago.

But memory is also a resource. LLMs have finite context windows. Every message in history costs tokens, and tokens cost money and latency. Unbounded memory eventually fails; bounded memory must evict something. Deciding *what* to evict, *when*, and *how* to compensate is one of the core design decisions in agent architecture.

This chapter covers three memory patterns:

1. **Manual `Vec<Message>`** — you manage history explicitly; most flexible, most code
2. **In-process session store** — a `HashMap<String, Vec<Message>>` per session ID; zero dependencies, suitable for single-server services
3. **Sliding-window truncation** — a small helper function that keeps only the last N messages, preventing unbounded growth

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

In rig, the three patterns below cover the same ground.

---

## 10.2 Pattern 1 — Manual `Vec<Message>`

The simplest pattern gives you total control: hold a `Vec<Message>` and pass it to every `chat()` call.

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Chat;
use rig::completion::Message;
use rig::providers::openai;

let agent = openai::Client::from_env()
    .agent(openai::GPT_4O_MINI)
    .preamble("You are a helpful assistant.")
    .build();

let mut history: Vec<Message> = Vec::new();

// Turn 1 — pass history by immutable reference; chat() does not mutate it
let r1 = agent.chat("My name is Alice.", &history).await?;
// Now push both turns manually
history.push(Message::user("My name is Alice."));
history.push(Message::assistant(r1.as_str()));

// Turn 2 — history now carries the previous exchange
let r2 = agent.chat("What's my name?", &history).await?;
history.push(Message::user("What's my name?"));
history.push(Message::assistant(r2.as_str()));
```

`chat()` accepts `impl IntoIterator<Item: Into<Message>>`. Passing `&history` works because `&Vec<T>` implements `IntoIterator`. The method does **not** mutate history — you push turns yourself.

### When to use manual history

- **Stateless services** — receive the full conversation from the client on each request (REST API pattern), pass it to `chat()`, return the response. Nothing persists server-side.
- **Database-backed history** — load `Vec<Message>` from SQL/Redis before the call, save it after. Full control over serialisation format.
- **History transformation** — filter, truncate, or rewrite messages before sending. Manual gives you a hook between turns.

### Serialising history to JSON

`Message` derives `serde::Serialize` and `serde::Deserialize`, so you can persist history trivially:

```rust
use rig::completion::Message;
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

## 10.3 Pattern 2 — In-Process Session Store

When you need one agent to serve many concurrent users, wrapping a `HashMap<String, Vec<Message>>` in a `Mutex` gives you isolated per-session history with no external dependencies.

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use rig::completion::Message;

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
```

Usage per request:

```rust
// Agent<M>: Chat when M: CompletionModel + 'static.
// A generic bound accepts any agent regardless of provider:
async fn handle<M: rig::completion::CompletionModel + 'static>(
    agent: &rig::agent::Agent<M>,
    store: &SessionStore,
    session_id: &str,
    prompt: &str,
) -> anyhow::Result<String> {
    let history = store.load(session_id);
    let reply = agent.chat(prompt, &history).await?;

    let mut updated = history;
    updated.push(Message::user(prompt));
    updated.push(Message::assistant(reply.as_str()));
    store.save(session_id, updated);

    Ok(reply)
}
```

Two users with isolated histories on one agent:

```rust
let store = SessionStore::new();
handle(&agent, &store, "alice", "My favourite language is Haskell.").await?;
handle(&agent, &store, "bob",   "I prefer Lisp.").await?;

let r = handle(&agent, &store, "alice", "What's my favourite language?").await?;
// → "Your favourite language is Haskell."
```

### Limitations

The `SessionStore` lives inside the process. It disappears on restart and is not shared across multiple service instances. For durability, use JSON-on-disk or SQLite (§10.7).

> **Java parallel:** This pattern is equivalent to maintaining a `Map<String, ChatMemory>` in LangChain4j and looking up the right `ChatMemory` by session ID per request. Spring AI does the same with `InMemoryChatMemory` scoped by a conversation ID.

---

## 10.4 Custom Storage Backends

The `SessionStore` in §10.3 is an in-process pattern. For a Redis- or database-backed equivalent, define your own `load` / `save` abstraction. Rig doesn't provide a `ConversationMemory` trait — you implement the pattern yourself. Here is a Redis example that follows the same load-chat-push-save contract:

```rust
// redis = "0.27" in Cargo.toml
use redis::AsyncCommands;
use rig::completion::Message;

pub struct RedisSessionStore {
    client: redis::Client,
    ttl_secs: usize,
}

impl RedisSessionStore {
    pub async fn load(&self, id: &str) -> anyhow::Result<Vec<Message>> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let raw: Option<String> = conn.get(id).await?;
        match raw {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(Vec::new()),
        }
    }

    pub async fn save(&self, id: &str, history: &[Message]) -> anyhow::Result<()> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let json = serde_json::to_string(history)?;
        conn.set_ex(id, json, self.ttl_secs).await?;
        Ok(())
    }
}
```

Usage is identical to the in-process `SessionStore` — load, chat, push, save:

```rust
let history = redis_store.load(session_id).await?;
let reply = agent.chat(prompt, &history).await?;
let mut updated = history;
updated.push(Message::user(prompt));
updated.push(Message::assistant(reply.as_str()));
redis_store.save(session_id, &updated).await?;
```

> **Java parallel:** LangChain4j's `ChatMemoryStore` interface has `getMessages`, `updateMessages`, and `deleteMessages`. The Redis implementation above covers the same three operations — rig just doesn't prescribe a formal trait for them.

---

## 10.5 Pattern 3 — Bounded History

The previous patterns let history grow without limit. That's fine for short conversations, but will eventually exceed the model's context window or inflate per-turn cost. The fix is simple: slice history before passing it to `chat()`.

### Sliding-window truncation

```rust
/// Keep only the most recent `max_messages` from `history`.
fn sliding_window(history: &[Message], max_messages: usize) -> Vec<Message> {
    if history.len() <= max_messages {
        history.to_vec()
    } else {
        history[history.len() - max_messages..].to_vec()
    }
}
```

Usage:

```rust
const WINDOW: usize = 20; // 10 turns

let windowed = sliding_window(&history, WINDOW);
let reply = agent.chat(prompt, &windowed).await?;
history.push(Message::user(prompt));
history.push(Message::assistant(reply.as_str()));
```

The full history `Vec` still grows (useful if you later want to persist or summarise it), but only the last `WINDOW` messages are sent to the model on each call.

### Token-aware truncation

When messages vary widely in length (e.g. code blocks alongside short replies), a message count is a coarse proxy for tokens. A rough heuristic: estimate 1 token ≈ 4 characters of English text, or use `tiktoken-rs` for exact OpenAI counts:

```toml
# tiktoken-rs = "0.5"  (add to Cargo.toml if needed)
```

```rust
// Heuristic token budget — drop oldest messages until under budget.
// Serialises each message to JSON to measure its approximate byte size.
fn token_window(history: &[Message], max_chars: usize) -> Vec<Message> {
    let mut kept: Vec<&Message> = Vec::new();
    let mut total = 0usize;
    for msg in history.iter().rev() {
        // JSON length is a reasonable proxy for token count (1 token ≈ 4 chars)
        let len = serde_json::to_string(msg).unwrap_or_default().len();
        if total + len > max_chars { break; }
        total += len;
        kept.push(msg);
    }
    kept.into_iter().rev().cloned().collect()
}
```

### Choosing a budget

Rule of thumb for `gpt-4o-mini` (128k context):
- Reserve ~4k tokens for system prompt + tool schemas
- Reserve ~4k tokens for the response
- Budget ~8k–16k tokens (≈32k–64k chars) for conversation history

### Java comparison

LangChain4j's `MessageWindowChatMemory.withMaxMessages(n)` and `TokenWindowChatMemory` apply the same truncation strategy. In Rust there is no framework magic — the truncation is a plain function applied to your `Vec<Message>` before each call. This makes the behaviour explicit and testable.

---

## 10.6 Memory Compaction (Summarisation)

When old messages are evicted by a sliding window, context is permanently lost. For long-running agents — personal assistants, support bots, research agents — losing early context is unacceptable.

**Compaction** replaces evicted messages with a summary instead of discarding them. Rig doesn't provide a built-in compactor, but the pattern is straightforward to implement using your rig agent itself:

```rust
use rig::completion::Prompt;
use rig::completion::Message;

/// Summarise `to_evict` messages using the agent, then return a single
/// "Earlier in this conversation: …" message as their replacement.
async fn compact(
    agent: &impl rig::completion::Prompt,
    to_evict: &[Message],
) -> anyhow::Result<Message> {
    // Serialise to JSON for the prompt — Message implements Serialize
    let history_json = serde_json::to_string_pretty(to_evict)
        .unwrap_or_else(|_| "[history unavailable]".to_string());
    let summary_prompt = format!(
        "Summarise the following conversation history in 2-3 sentences, \
         capturing the key facts for future reference:\n\n{history_json}"
    );
    let summary = agent.prompt(&summary_prompt).await?;
    Ok(Message::user(format!("Earlier in this conversation: {summary}")))
}

/// Apply a sliding window with compaction: evict old messages as a summary.
async fn compact_window(
    agent: &impl rig::completion::Prompt,
    history: &mut Vec<Message>,
    max_messages: usize,
) -> anyhow::Result<()> {
    if history.len() > max_messages {
        let eviction_count = history.len() - max_messages;
        let to_evict = history[..eviction_count].to_vec();
        let summary = compact(agent, &to_evict).await?;
        history.drain(..eviction_count);
        history.insert(0, summary);
    }
    Ok(())
}
```

The flow:
1. History grows beyond `max_messages`
2. Evicted messages are summarised into one `Message::user("Earlier in this conversation: …")`
3. The summary is prepended; the agent always sees a compact history *plus* a digest of earlier context

> **When to compact:** for persistent personal assistants that resume across sessions. For session-scoped API handlers, plain `sliding_window()` is simpler and sufficient. Compaction adds one LLM call per eviction cycle.

---

## 10.7 Persistence Patterns

### Simple: JSON on disk

Suitable for single-user applications or prototypes:

```rust
use std::path::Path;
use rig::completion::Message;

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
let prompt = "Continue where we left off.";
let response = agent.chat(prompt, &history).await?;
history.push(Message::user(prompt));
history.push(Message::assistant(response.as_str()));
save_history("session.json", &history).await?;
```

### Production: SQLite

For multi-user servers, SQLite via `sqlx` gives you ACID transactions with no external service:

```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "json"] }
```

```rust
use sqlx::SqlitePool;
use rig::completion::Message;

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

### Using SQLite with the session pattern

Wrap these functions in a struct following the same load-chat-push-save contract from §10.4. The agent code doesn't change — only the storage backend does.

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

━━━ Pattern 2: In-process session store ━━━

[Alice] Turn 1: Hello Alice! I'll keep my answers concise.
[Bob]   Turn 1: Hello! Haskell is an excellent language.
[Alice] Turn 2: I recommend "The Rust Programming Language" (the Book).
[Bob]   Turn 2: Rust does share some concepts with Haskell ...

────────────────────────────────────────────────

━━━ Pattern 3: Sliding-window (last 4 messages) ━━━

Turn 1: established project name 'Titan' (history: 2 msgs)
Turn 2: added storage detail (history: 4 msgs)
Turn 3: added deployment detail (history: 6 msgs, window passes last 4)
(Sending 4 messages to model — Turn 1 excluded)
Turn 4 (project name query): I don't have that information in our conversation.
(Expected: agent cannot recall 'Titan' — it was outside the window)
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
| Single-server multi-user bot, no durability needed | In-process `SessionStore` (§10.3) |
| Long conversations — control context window cost | `sliding_window()` helper (§10.5) |
| Long-running personal assistant — preserve early context | Manual compaction with summarisation (§10.6) |
| Multi-server deployment — must survive restart | Redis or SQLite backend (§10.4, §10.7) |
| Semantic recall — "what did we say about X?" | `dynamic_context` + vector store (Chapter 8) |

The last row is important: conversational memory and RAG memory are **orthogonal**. A production agent often uses both:
- A `SessionStore` or sliding-window for recent turn history
- A vector index (Chapter 8's `dynamic_context`) for long-term semantic search over past exchanges or documents

---

## 10.10 Key Takeaways

- **LLM memory is faked** — every call re-sends prior messages; the model has no persistent state.
- **`Agent::chat(prompt, &history)`** — takes `impl IntoIterator<Item: Into<Message>>`; pass `&Vec<Message>`. Does NOT mutate history — push user + assistant turns yourself after each call.
- **Manual push**: `history.push(Message::user(q)); history.push(Message::assistant(reply.as_str()));`
- **In-process `SessionStore`** — `Mutex<HashMap<String, Vec<Message>>>` gives multi-user isolation with no external dependencies; lost on restart.
- **`sliding_window(history, n)`** — a plain function that returns the last `n` messages; pass the result to `chat()` to bound context cost.
- **Compaction** — summarise evicted messages into a digest using the agent itself; insert as a `Message::user("Earlier…")` at the front.
- **`Message` is serializable** — `Vec<Message>` round-trips through `serde_json`; persistence is just `to_string` + `from_str`.
- **Persistence = load → chat → push → save** — the same three-line pattern works regardless of whether the backend is a local `HashMap`, JSON file, SQLite, or Redis.

---

## What's Next

This chapter gave you the memory primitives. Chapter 11 moves to MCP — the Model Context Protocol — which standardises how agents discover and call tools exposed by external servers. The `rmcp` crate is Rust's official MCP SDK, and it complements rig's built-in tool system with a standardised network protocol.

---

*→ Java reference: LangChain4j `ChatMemory`, `MessageWindowChatMemory`, `TokenWindowChatMemory`, `ChatMemoryStore`; Spring AI `MessageChatMemoryAdvisor`, `InMemoryChatMemory`*
