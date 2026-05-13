# Chapter 14: Stateful Workflows and Persistence

> **Framework versions in this chapter:**  
> `graph-flow = "0.5.1"` · `async-trait = "0.1"`
>
> **Java reference:** LangGraph4j checkpointing, `MemorySaver`, `PostgresSaver`, human-in-the-loop (Chapter 17 of Java book)

---

The ReAct graph from Chapter 13 ran entirely in memory. When the process exits, the session is gone. For production workflows — document processing pipelines, multi-step approvals, research jobs that take hours — you need two things:

1. **Persistence** — sessions survive process restarts
2. **Human-in-the-loop** — the graph can pause and wait for external input

This chapter covers both.

---

## 14.1 Why Persistence Matters

Consider a document processing pipeline with five steps:

```
Fetch → Extract → Summarise → Review (human) → Publish
```

- Step 3 (`Summarise`) calls an LLM — it costs money and takes time
- Step 4 (`Review`) waits for a human — could be hours or days
- If the process crashes between steps 3 and 4, without persistence you re-run (and re-pay for) steps 1–3

With persistence, each step is a checkpoint. A crash between steps 3 and 4 loses nothing — the session is loaded from storage and execution resumes at step 4.

### Java comparison

LangGraph4j calls this **checkpointing** and provides `MemorySaver` and `PostgresSaver`:

```java
// LangGraph4j
var graph = new StateGraph<>(AgentState.class)
    .addNode("summarise", this::summarise)
    .addNode("review", this::review)
    .addEdge("summarise", "review");

var checkpointer = new PostgresSaver(dataSource);
var app = graph.compile(checkpointer);
app.invoke(state, new RunnableConfig("thread-123"));
```

graph-flow's equivalent is the `SessionStorage` trait and `PostgresSessionStorage` implementation.

---

## 14.2 Storage Backends

### `SessionStorage` trait

Any persistent backend implements:

```rust
pub trait SessionStorage: Send + Sync {
    async fn save(&self, session: Session) -> Result<()>;
    async fn get(&self, id: &str) -> Result<Option<Session>>;
    async fn delete(&self, id: &str) -> Result<()>;
}
```

`Session` wraps a session ID and a `Context`. Because `Context` implements `Serialize + Deserialize`, any storage backend that can persist JSON works.

### `InMemorySessionStorage` (development)

```rust
use graph_flow::InMemorySessionStorage;
let storage = Arc::new(InMemorySessionStorage::new());
```

Fast, zero-dependency, disappears on process exit. Use for development and tests.

### `PostgresSessionStorage` (production)

```toml
graph-flow = { version = "0.5", features = ["postgres"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres"] }
```

```rust
use graph_flow::PostgresSessionStorage;

let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
let storage = Arc::new(PostgresSessionStorage::new(pool));

// Creates the sessions table if it doesn't exist
storage.migrate().await?;

let runner = FlowRunner::new(Arc::new(graph), storage);
```

Sessions are stored as JSON in a `sessions` table. The session ID is the primary key — resuming a session is just `runner.run(session_id)` with the same ID.

> **SQLite note:** graph-flow 0.5 does not ship a SQLite backend. For single-process deployments where you want durability, implement `SessionStorage` over `sqlx` with the `sqlite` feature — it's about 30 lines of code following the Redis pattern in Section 10.4.

---

## 14.3 Resuming a Session

Once a session is persisted, resuming is transparent:

```rust
// Process A: start the pipeline, stops at an approval gate
let runner = FlowRunner::new(graph.clone(), storage.clone());
runner.init_session("job-42", |ctx| {
    ctx.set_sync("document_path", "/docs/report.pdf".to_string());
}).await?;

// Run until paused
loop {
    let result = runner.run("job-42").await?;
    if result.response.as_deref() == Some("Awaiting approval") || 
       matches!(result.status, ExecutionStatus::Completed | ExecutionStatus::Error(_)) {
        break;
    }
}

// ... hours later, in Process B (or after a restart):
// The session is loaded from PostgreSQL automatically
let runner = FlowRunner::new(graph.clone(), storage.clone());

// Inject the approval decision
runner.update_session("job-42", |ctx| {
    ctx.set_sync("approved", true);
}).await?;

// Resume — the graph picks up exactly where it left off
loop {
    let result = runner.run("job-42").await?;
    if matches!(result.status, ExecutionStatus::Completed | ExecutionStatus::Error(_)) {
        break;
    }
}
```

The key is `runner.update_session()` — it loads the session, applies the closure, and saves it back without executing any tasks.

---

## 14.4 Human-in-the-Loop

Human-in-the-loop means the graph pauses at a designated node and waits for external input before continuing. In graph-flow, this is implemented by:

1. A task that checks whether approval has been given
2. If not yet given, return `NextAction::End` (stop the runner)
3. Externally inject the approval decision into the session
4. Re-run the graph — it loads the session, sees the approval, and continues

```rust
struct ApprovalTask;

#[async_trait]
impl Task for ApprovalTask {
    fn id(&self) -> &str { "await-approval" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        match context.get_sync::<bool>("approved") {
            None => {
                // No decision yet — pause the pipeline
                println!("[await-approval] Waiting for human review...");
                Ok(TaskResult {
                    response: Some("Awaiting approval".to_string()),
                    next_action: NextAction::End,  // ← pause here
                })
            }
            Some(true) => {
                println!("[await-approval] Approved — continuing.");
                Ok(TaskResult {
                    response: Some("Approved".to_string()),
                    next_action: NextAction::Continue,
                })
            }
            Some(false) => {
                println!("[await-approval] Rejected.");
                Ok(TaskResult {
                    response: Some("Rejected".to_string()),
                    next_action: NextAction::End,
                })
            }
        }
    }
}
```

### Wiring the approval gate into a web service

In a real system the approval comes from a human clicking a button:

```rust
// Axum handler: POST /sessions/{id}/approve
async fn approve_session(
    Path(session_id): Path<String>,
    State(runner): State<Arc<FlowRunner>>,
) -> impl IntoResponse {
    runner.update_session(&session_id, |ctx| {
        ctx.set_sync("approved", true);
    }).await.unwrap();

    // Optionally kick off the next run in a background task
    tokio::spawn(async move {
        loop {
            let result = runner.run(&session_id).await.unwrap();
            if matches!(result.status, ExecutionStatus::Completed | ExecutionStatus::Error(_)) {
                break;
            }
        }
    });

    axum::http::StatusCode::ACCEPTED
}
```

This is the same pattern as LangGraph4j's `interrupt_before` / `Command(resume=value)` — you pause, collect external input, then resume with the injected value.

---

## 14.5 Replay and Audit

Because every session step is persisted, you can reconstruct what happened at each step by storing step metadata in the context:

```rust
async fn run(&self, context: Context) -> Result<TaskResult> {
    // Record that this task ran and when
    let mut audit_log: Vec<String> = context.get_sync("audit_log").unwrap_or_default();
    audit_log.push(format!("{} ran at {}", self.id(), chrono::Utc::now()));
    context.set("audit_log", audit_log).await;
    // ... task logic
}
```

After the pipeline completes, the audit log is available in the session context. For compliance use-cases, write it to a separate audit table alongside the session save.

---

## 14.6 Hands-On: Report Pipeline with Approval Gate

The complete example in `code-examples/ch10-react/` (scaffold crate, content maps to Ch14) implements:

```
Fetch → Process → Approve (gate) → Publish
```

```bash
cd code-examples
cargo run -p ch14-stateful-workflows
```

Expected output:

```
=== Run 1: Start pipeline ===

[fetch-data] Fetched 3 records
  → Fetched 3 records
[process-data] Processed 3 records. Highest: Record C (2100 sales).
  → Processed 3 records. Highest: Record C (2100 sales).
[await-approval] Pausing for human approval.
  Summary to approve: Processed 3 records. Highest: Record C (2100 sales).
  → Awaiting approval

  [Pipeline paused — awaiting human approval]

=== Run 2: Human approves, resume pipeline ===

[await-approval] Approved — continuing.
  → Approved
[publish] Publishing: Processed 3 records. Highest: Record C (2100 sales).
  → Published successfully
Pipeline complete.
```

---

## 14.7 Key Takeaways

- **`SessionStorage` trait** — `save()`, `get()`, `delete()`; implement it for any backend.
- **`InMemorySessionStorage`** — development; **`PostgresSessionStorage`** — production (graph-flow 0.5 only ships these two).
- **`FlowRunner::init_session(id, init_fn)`** — creates a new session with seed data.
- **`FlowRunner::update_session(id, update_fn)`** — injects data without running tasks (use for approval decisions, external events).
- **Human-in-the-loop** = task returns `NextAction::End` on first pass → external system calls `update_session` → re-run picks up from the same task.
- **No SQLite backend** in graph-flow 0.5 — implement `SessionStorage` yourself or use PostgreSQL.
- **Idempotency matters** — if a task is re-run after a crash, it should produce the same result. Check `context.get_sync("step")` at the start of expensive tasks to skip if already done.

---

## What's Next

Chapter 15 steps back from graph-flow and covers multi-agent systems with AutoAgents — where multiple independent agents collaborate on a task, coordinate via events, and are supervised by an orchestrator.

---

*→ Java reference: LangGraph4j `MemorySaver`, `PostgresSaver`, `interrupt_before`, human-in-the-loop `Command(resume=value)` (Ch 17)*
