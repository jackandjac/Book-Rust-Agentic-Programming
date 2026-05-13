# Chapter 15: Multi-Agent Systems with AutoAgents

> **Framework versions in this chapter:**  
> `autoagents = "0.3"` (7.3k downloads — experimental, API evolving)  
> `rig-core = "0.37"` · `tokio = "1"`
>
> **⚠️ Maturity note:** `autoagents` 0.3.x is experimental. It has its own LLM abstraction layer (not rig-based). The patterns in this chapter are framework-agnostic; the code example uses rig for the hands-on to keep dependencies consistent.
>
> **Java reference:** LangGraph4j multi-agent graphs, supervisor pattern (Chapter 18 of Java book)

---

So far every example has used a single agent. Single agents work well for tasks with a clear linear flow — ask a question, get an answer. They break down when the task:
- Requires **parallel specialisation** — a researcher and a writer working simultaneously
- Involves **long context** — no single agent can hold all relevant information
- Benefits from **adversarial review** — one agent checks another's work
- Needs **isolated execution** — untrusted tool calls should not affect the main agent

Multi-agent systems solve these problems by dividing work across agents that communicate through message passing.

---

## 15.1 Multi-Agent Architectures

Three common patterns:

### Supervisor (hub-and-spoke)

One orchestrator agent routes tasks to specialist workers:

```
User ──▶ Supervisor ──▶ Researcher
                   ──▶ Summariser
                   ──▶ Fact-Checker
         ◀── collects results ───
```

### Parallel (fan-out/fan-in)

Multiple agents work on independent subtasks simultaneously, a coordinator synthesises:

```
               ┌──▶ Agent A (topic 1) ──┐
Input ──▶ Split │──▶ Agent B (topic 2) ──│──▶ Merge ──▶ Output
               └──▶ Agent C (topic 3) ──┘
```

### Pipeline (handoff)

Output of one agent becomes input to the next:

```
Planner ──▶ Researcher ──▶ Writer ──▶ Reviewer ──▶ Publisher
```

---

## 15.2 AutoAgents Architecture

`autoagents` is an event-driven multi-agent framework built on Tokio channels. Its key types:

| Type | Role |
|------|------|
| `AgentDeriveT` | Core async trait all agents implement |
| `BaseAgent<T>` | Runtime wrapper with LLM, memory, tools |
| `Environment` | Orchestrator — routes events between agents |
| `ActorID` | Unique agent address |
| `Event` | Message passed between agents |

### The `#[agent]` macro

```rust
use autoagents::{agent, tool, AgentOutput};
use serde::{Serialize, Deserialize};

#[derive(AgentOutput, Serialize, Deserialize)]
struct ResearchResult {
    findings: String,
    sources: Vec<String>,
}

#[agent]
struct ResearchAgent {
    topic: String,
}

// AgentDeriveT is auto-implemented by #[agent]:
// - name()        → "ResearchAgent"
// - description() → "" (override to customise)
// - tools()       → vec![] (add tools here)
// - output_schema() → None (Some(schema) for structured output)
```

### Defining tools

```rust
use autoagents::tool;

#[tool]
async fn search_web(query: String) -> String {
    // Call a search API
    format!("Search results for: {query}")
}
```

### Running agents in an Environment

```rust
use autoagents::{Environment, LLMBuilder, LLMProvider};

let llm = LLMBuilder::new()
    .backend("openai")
    .model("gpt-4o-mini")
    .build()?;

let mut env = Environment::new();
env.register_runtime(
    BaseAgent::new(ResearchAgent { topic: "Rust async".into() }, llm)
)?;

env.run().await?;
// Events flow through channels; subscribe to collect results
let mut rx = env.subscribe_events();
while let Some(event) = rx.recv().await {
    println!("{:?}", event);
}
```

---

## 15.3 Agent Communication Patterns

AutoAgents agents communicate through the `Environment`'s event bus, not direct method calls. This decoupling means:
- Agents don't need to know each other's concrete types
- New agents can be added without changing existing ones
- Events can be logged, replayed, or filtered centrally

### Routing pattern

A router agent examines the input and delegates to a specialist:

```rust
#[agent]
struct RouterAgent;

// In the router's run logic (via tools or prompt):
// - Classify the input
// - Emit an event addressed to the appropriate specialist's ActorID
```

### Parallel pattern

Multiple agents registered in the same `Environment` run concurrently. The `Parallel` design pattern in AutoAgents has a coordinator that fans out work and collects responses via the event bus.

### Supervisor via graph-flow

For complex orchestration, combining graph-flow (Chapter 12–14) with rig agents (Chapters 4–7) gives you the cleanest result in the current Rust ecosystem. Each graph node is a task that runs a rig agent:

```rust
struct ResearchNode { client: Arc<openai::Client> }

#[async_trait]
impl Task for ResearchNode {
    fn id(&self) -> &str { "research" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let topic: String = context.get_sync("topic").unwrap_or_default();
        let agent = self.client
            .agent(openai::GPT_4O_MINI)
            .preamble("You are a research specialist. Find key facts.")
            .build();
        let findings = agent.prompt(&format!("Research: {topic}")).await?;
        context.set("findings", findings.clone()).await;
        Ok(TaskResult { response: Some(findings), next_action: NextAction::Continue })
    }
}

struct WriterNode { client: Arc<openai::Client> }

#[async_trait]
impl Task for WriterNode {
    fn id(&self) -> &str { "write" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let findings: String = context.get_sync("findings").unwrap_or_default();
        let agent = self.client
            .agent(openai::GPT_4O_MINI)
            .preamble("You are a technical writer. Write clearly and concisely.")
            .build();
        let article = agent.prompt(
            &format!("Write an article based on these findings:\n{findings}")
        ).await?;
        context.set("article", article.clone()).await;
        Ok(TaskResult { response: Some(article), next_action: NextAction::Continue })
    }
}
```

This "rig agents as graph nodes" pattern is the most practical multi-agent approach in the current Rust ecosystem — it gives you graph-flow's state persistence and conditional routing alongside rig's battle-tested LLM integration.

---

## 15.4 The Supervisor Pattern

A supervisor agent decides which worker to invoke next based on the current state. In graph-flow terms, the supervisor is a conditional edge predicate:

```rust
GraphBuilder::new("supervisor")
    .add_task(Arc::new(SupervisorTask { client: client.clone() }))
    .add_task(Arc::new(ResearchNode { client: client.clone() }))
    .add_task(Arc::new(WriterNode { client: client.clone() }))
    .add_task(Arc::new(ReviewNode { client: client.clone() }))
    .set_start_task("supervisor")
    // Supervisor reads "next_agent" from context and routes
    .add_conditional_edge("supervisor",
        |ctx: &Context| ctx.get_sync::<String>("next_agent")
            .map(|a| a == "research")
            .unwrap_or(false),
        "research",
        "write",
    )
    .add_conditional_edge("research",
        |ctx: &Context| ctx.get_sync::<bool>("needs_review").unwrap_or(false),
        "review",
        "supervisor",
    )
    .add_edge("write", "supervisor")
    .add_edge("review", "supervisor")
    .build()
```

The `SupervisorTask` calls an LLM with the current state and asks: "Which agent should run next — research, write, review, or done?" The response is stored as `"next_agent"` in context, and the conditional edges route accordingly.

---

## 15.5 WASM Sandboxing for Tool Safety

AutoAgents supports `wasmtime` for executing tool code in a WebAssembly sandbox. This is relevant when:
- Tools are provided by untrusted third parties
- Tools execute arbitrary code (code-interpreter pattern)
- You need strict memory/CPU limits on tool execution

```toml
autoagents = { version = "0.3", features = ["full"] }  # enables wasmtime + codeact
```

```rust
// In a WASM-enabled agent, tools can execute WASM modules
// The runtime enforces memory limits and prevents host access
use autoagents::features::wasmtime::WasmTool;

let sandboxed_tool = WasmTool::from_bytes(wasm_bytes, "execute_code")?;
```

For most book readers this is advanced — note it as a capability and skip the implementation details unless building a code-interpreter agent.

---

## 15.6 Hands-On: Parallel Research Pipeline

The complete example uses rig agents inside graph-flow nodes (the practical pattern for the current ecosystem):

```rust
// code-examples/ch15-multiagent-pipeline/src/main.rs
// Two specialist agents run in sequence (parallel via FanOutTask in production)
// Researcher → Writer → Output
```

```bash
cd code-examples
export OPENAI_API_KEY="sk-..."
cargo run -p ch20-capstone-multiagent-pipeline
```

The example shows:
1. `ResearchNode` — rig agent with researcher persona, produces findings
2. `WriterNode` — rig agent with writer persona, turns findings into prose
3. graph-flow wires them together with state persistence

For true parallelism, wrap both nodes in a `FanOutTask` (Chapter 12 §12.5) and add a merge node that combines results.

---

## 15.7 Choosing a Multi-Agent Approach

| Approach | Best for | Tradeoffs |
|----------|---------|-----------|
| rig agents as graph-flow nodes | Production today — stable APIs | More setup; no native parallelism |
| `autoagents` 0.3 | Experimenting with actor model | Evolving API; own LLM layer |
| rig `FanOutTask` (graph-flow) | Parallel independent subtasks | Simple; no inter-agent messaging |
| Manual channels (tokio::mpsc) | Custom message-passing | Full control; most boilerplate |

For a new production system today: **rig agents + graph-flow** gives you the most stability. As `autoagents` matures toward 1.0 and rig adds native multi-agent support, the ecosystem picture will improve.

---

## 15.8 Key Takeaways

- **Multi-agent = parallel specialisation + isolated context + coordinated state**
- **AutoAgents** uses `#[agent]`, `#[tool]`, `Environment`, event-driven channels; has its own LLM layer (not rig)
- **Supervisor pattern** = one orchestrator agent routes work to specialists based on current state
- **Practical today**: rig agents inside graph-flow nodes — stable APIs, full state persistence, conditional routing
- **`FanOutTask`** enables parallel node execution within graph-flow
- **WASM sandboxing** available in `autoagents` via `features = ["full"]` for untrusted tool execution
- **AutoAgents is experimental** — for production systems, verify API stability before committing

---

## What's Next

Part IV is complete. Part V covers production: Chapter 16 adds structured logging, OpenTelemetry traces, prompt injection protection, rate limiting, and token cost tracking to everything we've built.

---

*→ Java reference: LangGraph4j multi-agent supervisor, `CompiledGraph.stream()`, parallel subgraph (Ch 18)*
