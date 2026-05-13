# Chapter 12: Graph-Based Workflows with graph-flow

> **Framework versions in this chapter:**  
> `graph-flow = "0.5.1"` (6.6k downloads — small project, 312 GitHub stars, API may change)  
> `tokio = "1"`, `anyhow = "1"`, `async-trait = "0.1"`
>
> **⚠️ Maturity note:** `graph-flow` is a small, pre-1.0 project. It is included because it is the most complete graph-workflow library in the Rust ecosystem today. Check https://github.com/a-agmon/rs-graph-llm for the latest API before using in production.
>
> **Java reference:** LangGraph4j — `StateGraph`, `NodeAction`, `EdgeAction` (Chapter 15 of the Java book)

---

Parts I–III covered the building blocks: LLM calls, tools, structured output, RAG, memory. A real agent application connects these into a *workflow* — a series of steps where each step can make decisions, call tools, update state, and hand off to the next step.

Graph-based workflows model this as a directed graph: nodes are tasks, edges are routing decisions. State flows through the graph as a shared context object. The runner executes one node at a time, persisting state between steps — which means long-running workflows survive process restarts.

---

## 12.1 Why Graphs?

The simplest agent is a loop: think → act → observe → repeat. That's a cycle — and cycles don't fit a straight function call chain. A graph handles them naturally.

```
    ┌─────────┐       ┌──────────┐
    │  Think   │──────▶│   Act    │
    └─────────┘       └──────────┘
         ▲                  │
         │                  ▼
    ┌─────────┐       ┌──────────┐
    │  Done?   │◀──────│ Observe  │
    └─────────┘       └──────────┘
         │ (yes)
         ▼
       [END]
```

Graphs also enable **parallelism** (fan-out to multiple tasks), **conditional branching** (route based on output), and **human-in-the-loop** (pause and wait for input).

### Java comparison

LangGraph4j models the same idea with `StateGraph`:

```java
// LangGraph4j
StateGraph<AgentState> graph = new StateGraph<>(AgentState.class)
    .addNode("think", this::thinkAction)
    .addNode("act", this::actAction)
    .addEdge(START, "think")
    .addConditionalEdges("think", this::shouldContinue,
        Map.of("continue", "act", "end", END))
    .addEdge("act", "think");
```

`graph-flow`'s API is structurally similar but Rust-idiomatic: nodes are `Task` trait implementations, edges are method calls on `GraphBuilder`, and state is a `Context` (equivalent to LangGraph4j's `AgentState`).

---

## 12.2 Core Types

### `Context`

`Context` is the shared state container. It is thread-safe (backed by a `DashMap`) and holds arbitrary typed values under string keys.

```rust
// Write
context.set("result", "hello".to_string()).await;

// Read async
let val: Option<String> = context.get("result").await;

// Read sync (for closures and non-async contexts)
let val: Option<String> = context.get_sync("result");

// Convenience methods for chat history
context.add_user_message("What is 2+2?").await;
context.add_assistant_message("4").await;
let history = context.get_messages().await;  // Vec<Message>
```

`Context` implements `Serialize + Deserialize + Clone + Default` — these bounds are required for storage backends.

### `Task` trait

Every node in the graph implements `Task`:

```rust
#[async_trait]
pub trait Task: Send + Sync {
    async fn run(&self, context: Context) -> Result<TaskResult>;

    fn id(&self) -> &str {
        std::any::type_name::<Self>()  // Default: fully-qualified type name
    }
}
```

By default, `id()` returns the fully-qualified type name (`"my_crate::ValidateTask"`). Override it for shorter names:

```rust
fn id(&self) -> &str { "validate" }
```

### `TaskResult`

The return type from `Task::run()`:

```rust
pub struct TaskResult {
    pub response: Option<String>,   // output of this step (can be None)
    pub next_action: NextAction,    // what the runner should do next
}
```

`NextAction` controls graph execution:

| Variant | Effect |
|---------|--------|
| `NextAction::Continue` | Execute the next task in the edge chain |
| `NextAction::End` | Terminate the graph execution |
| `NextAction::ContinueAndExecute` | (fan-out) Execute all connected tasks in parallel |

### `Graph` and `GraphBuilder`

```rust
let graph = GraphBuilder::new("pipeline-name")
    .add_task(Arc::new(MyTask))           // Arc<dyn Task>
    .set_start_task("MyTask")             // task id of first node
    .add_edge("MyTask", "NextTask")       // unconditional edge
    .add_conditional_edge(               // conditional edge
        "DecideTask",
        |ctx: &Context| ctx.get_sync::<String>("sentiment")
            .map(|s| s == "positive")
            .unwrap_or(false),
        "PositiveTask",   // "yes" branch
        "NegativeTask",   // "no" branch
    )
    .build();
```

---

## 12.3 Conditional Routing

Conditional edges read a value from the context and route to one of two tasks:

```rust
.add_conditional_edge(
    "sentiment-analysis",
    |ctx: &Context| {
        ctx.get_sync::<String>("sentiment")
            .map(|s| s == "positive")
            .unwrap_or(false)
    },
    "positive-handler",
    "negative-handler",
)
```

The predicate closure is `Fn(&Context) -> bool + Send + Sync + 'static`. It can inspect any value stored in the context.

Rust's pattern matching makes complex routing readable:

```rust
.add_conditional_edge(
    "classify",
    |ctx: &Context| {
        matches!(
            ctx.get_sync::<String>("category").as_deref(),
            Some("urgent") | Some("critical")
        )
    },
    "escalate",
    "standard-reply",
)
```

---

## 12.4 Running the Graph

`FlowRunner` executes the graph one step at a time, persisting state to a storage backend between steps.

```rust
use graph_flow::{FlowRunner, InMemorySessionStorage};
use std::sync::Arc;

let runner = FlowRunner::new(
    Arc::new(graph),
    Arc::new(InMemorySessionStorage::new()),
);

// Each call to run() executes exactly ONE task.
// Loop until the graph signals completion.
loop {
    let result = runner.run("session-abc").await?;

    match result.status {
        ExecutionStatus::Completed => break,
        ExecutionStatus::Error(msg) => return Err(anyhow::anyhow!(msg)),
        ExecutionStatus::WaitingForInput => {
            // Human-in-the-loop: wait for external input
            runner.set_input("session-abc", read_user_input()).await?;
        }
        ExecutionStatus::Paused { .. } => {} // continue on next loop iteration
    }
}
```

The step-by-step design is intentional: each call to `run()` is atomic — it loads the session, executes one task, saves the updated session. If the process crashes between steps, the session is recoverable from storage.

### Storage backends

| Backend | Type | Use case |
|---------|------|---------|
| `InMemorySessionStorage` | RAM | Development, testing |
| `PostgresSessionStorage` | PostgreSQL (via sqlx) | Production |

> **Note:** `graph-flow` 0.5 does not include a SQLite backend. For production use, either use `PostgresSessionStorage` or implement the `SessionStorage` trait against your preferred store.

---

## 12.5 Fan-Out (Parallel Tasks)

`FanOutTask` executes multiple child tasks in parallel and aggregates their results:

```rust
use graph_flow::FanOutTask;

let fanout = FanOutTask::new(
    "parallel-enrichment",
    vec![
        Arc::new(KeywordsTask),
        Arc::new(SummaryTask),
        Arc::new(SentimentTask),
    ],
)
.with_prefix("enrichment");  // Results stored as "enrichment.KeywordsTask", etc.

let graph = GraphBuilder::new("enrich")
    .add_task(Arc::new(fanout))
    .set_start_task("parallel-enrichment")
    .build();
```

After the fan-out completes, the context contains the results of all three tasks under prefixed keys. The next task can read any of them.

---

## 12.6 Hands-On: Text Processing Pipeline

The complete example in `code-examples/ch08-graph-workflows/` (crate name is a pre-renumbering scaffold; content maps to Chapter 12) implements a three-node pipeline:

```
Validate → Summarise → Classify
```

```bash
cd code-examples
cargo run -p ch12-graph-workflows
```

Expected output:

```
[Validate] Input accepted: 182 chars
Step response: Validation passed
[Summarise] Summary: Rust is a systems programming language that runs blazingly fast
Step response: Rust is a systems programming language that runs blazingly fast
[Classify] Category: long
Step response: Category: long

Pipeline complete!
```

The pipeline is trivial on purpose — the goal is to show the graph structure. Chapter 13 replaces the stub `SummariseTask` with a real LLM call and adds conditional routing.

---

## 12.7 Key Takeaways

- **`Task` trait** — one `async fn run(ctx: Context) -> Result<TaskResult>` method; override `id()` for a short name.
- **`Context`** — thread-safe `DashMap`-backed state; `set()` / `get()` / `get_sync()` for typed values; built-in chat history helpers.
- **`NextAction::Continue`** — proceed to next node; `NextAction::End` — stop.
- **`GraphBuilder`** — `.add_task(Arc<dyn Task>)`, `.set_start_task()`, `.add_edge()`, `.add_conditional_edge()`, `.build()`.
- **`FlowRunner::run(session_id)`** — executes exactly ONE task per call; loop until `Completed`.
- **No streaming API** — graph-flow is step-by-step; stream results by printing inside `Task::run()` or reading `result.response` after each step.
- **No START/END sentinels** — start task set with `.set_start_task()`; graph ends when task returns `NextAction::End`.
- **Storage**: `InMemorySessionStorage` for development; `PostgresSessionStorage` for production.

---

## What's Next

Chapter 13 builds a ReAct agent inside graph-flow: the graph has a Think node that calls an LLM, an Act node that executes tools, and a conditional edge that loops back to Think until the agent decides to stop.

---

*→ Java reference: LangGraph4j `StateGraph`, `NodeAction`, `EdgeAction` (Ch 15–16 of Java book)*
