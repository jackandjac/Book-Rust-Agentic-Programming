# Chapter 20: Capstone — Building a Multi-Agent Pipeline

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` · `graph-flow = "0.5.1"` · `tokio = "1"`
>
> **Java reference:** LangGraph4j multi-agent stateful pipeline (Chapter 22 of Java book)

---

This chapter builds a stateful multi-agent research pipeline with a human approval gate. Four specialised agents — Researcher, Synthesiser, Reviewer, and an Approval gate — run as graph-flow nodes, passing work through a shared context that persists across sessions.

By the end you have a pipeline where:
1. A researcher collects raw findings
2. A synthesiser structures them into a report
3. A reviewer critiques the report
4. A human approves or rejects before finalisation

This is the pattern behind real-world document review, content moderation, and multi-stage approval workflows.

---

## 20.1 Why Graph-Flow for Multi-Agent?

In Chapter 15 we saw the Researcher → Writer pipeline. That was a simple linear DAG. This capstone adds:

- **Four nodes** instead of two
- **Human-in-the-loop** (the approval gate pauses execution and waits)
- **Session persistence** (the graph remembers where it stopped)
- **Two run phases** (first run processes; second run resumes after human input)

graph-flow's `InMemorySessionStorage` handles all of this. For production, swap it for `PostgresSessionStorage` — the graph code is identical.

---

## 20.2 The Four-Node Pipeline

```
ResearchNode → SynthesisNode → ReviewNode → ApprovalNode
                                              ↑
                                   (waits for human input)
```

Each node is a Rust struct implementing the `Task` trait. Each holds an `Arc<openai::Client>` for LLM calls and reads/writes a shared `Context`.

---

## 20.3 Researcher Node

The researcher's job: given a topic, produce 5 key facts with evidence.

```rust
use anyhow::Result;
use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use rig::{client::{CompletionClient, ProviderClient}, completion::Prompt, providers::openai};
use std::sync::Arc;

struct ResearchNode {
    client: Arc<openai::Client>,
}

#[async_trait]
impl Task for ResearchNode {
    fn id(&self) -> &str { "research" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let topic: String = context.get_sync("topic").unwrap_or_default();

        let agent = self.client
            .agent(openai::GPT_4O_MINI)
            .preamble(
                "You are a research specialist. Given a topic, produce 5 key facts \
                 with supporting evidence. Use bullet points. Be precise.",
            )
            .build();

        let findings = agent
            .prompt(&format!("Research this topic in depth:\n\n{topic}"))
            .await?;

        context.set("findings", findings.clone()).await;

        Ok(TaskResult {
            response: Some(findings),
            next_action: NextAction::Continue,
        })
    }
}
```

Key pattern: `context.set("findings", ...)` stores the output for the next node. `context.get_sync("topic")` reads the initial input. All context values are strings in this example — for structured data, serialize to JSON before storing.

---

## 20.4 Synthesis Node

The synthesiser turns raw findings into a structured report:

```rust
struct SynthesisNode {
    client: Arc<openai::Client>,
}

#[async_trait]
impl Task for SynthesisNode {
    fn id(&self) -> &str { "synthesise" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let topic: String    = context.get_sync("topic").unwrap_or_default();
        let findings: String = context.get_sync("findings").unwrap_or_default();

        let agent = self.client
            .agent(openai::GPT_4O_MINI)
            .preamble(
                "You are a technical writer synthesising research into a structured \
                 report. Format: executive summary (2 sentences), 3-5 key insights, \
                 one recommended action. Audience: software architects.",
            )
            .build();

        let report = agent
            .prompt(&format!(
                "Topic: {topic}\n\nRaw research:\n{findings}\n\n\
                 Produce a structured synthesis report."
            ))
            .await?;

        context.set("report", report.clone()).await;

        Ok(TaskResult {
            response: Some(report),
            next_action: NextAction::Continue,
        })
    }
}
```

The synthesis node reads `findings` (set by research) and writes `report` (read by review). Each node is responsible for exactly one transformation — the Single Responsibility Principle applied to AI agents.

---

## 20.5 Review Node

The reviewer critiques the report before it reaches the human:

```rust
struct ReviewNode {
    client: Arc<openai::Client>,
}

#[async_trait]
impl Task for ReviewNode {
    fn id(&self) -> &str { "review" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let report: String = context.get_sync("report").unwrap_or_default();

        let agent = self.client
            .agent(openai::GPT_4O_MINI)
            .preamble(
                "You are a critical reviewer. Identify factual inaccuracies, \
                 logical gaps, or missing context. Rate quality 1-10 and give \
                 1-3 specific improvement suggestions.",
            )
            .build();

        let review = agent
            .prompt(&format!("Review this report:\n\n{report}"))
            .await?;

        context.set("review", review.clone()).await;

        Ok(TaskResult {
            response: Some(review),
            next_action: NextAction::Continue,
        })
    }
}
```

The review is stored in context. A human (or a downstream process) can read it alongside the report when making the approval decision.

---

## 20.6 Approval Gate (Human-in-the-Loop)

The approval node pauses the pipeline until a human sets `approved = true`:

```rust
struct ApprovalNode;

#[async_trait]
impl Task for ApprovalNode {
    fn id(&self) -> &str { "approve" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let approved: Option<bool> = context.get_sync("approved");

        match approved {
            Some(true) => Ok(TaskResult {
                response: Some("approved".to_string()),
                next_action: NextAction::End,
            }),
            Some(false) => Ok(TaskResult {
                response: Some("rejected".to_string()),
                next_action: NextAction::End,
            }),
            None => {
                // No decision yet — end the current run
                // The session retains its state; the next run resumes here
                Ok(TaskResult {
                    response: None,
                    next_action: NextAction::End,
                })
            }
        }
    }
}
```

When `approved` is `None`, the node returns `NextAction::End`. The session is saved. The pipeline is not complete — it's paused at this node. The next call to `runner.run(session_id)` resumes from `approve`.

This is the fundamental HITL pattern in graph-flow: **pause by returning `End` without completing**.

---

## 20.7 Wiring the Graph

```rust
use graph_flow::{FlowRunner, GraphBuilder, InMemorySessionStorage};

fn build_pipeline(client: Arc<openai::Client>) -> graph_flow::Graph {
    GraphBuilder::new("research-pipeline")
        .add_task(Arc::new(ResearchNode  { client: client.clone() }))
        .add_task(Arc::new(SynthesisNode { client: client.clone() }))
        .add_task(Arc::new(ReviewNode    { client }))
        .add_task(Arc::new(ApprovalNode))
        .set_start_task("research")
        .add_edge("research",   "synthesise")
        .add_edge("synthesise", "review")
        .add_edge("review",     "approve")
        .build()
}
```

The edges are data-flow declarations. graph-flow executes nodes in topological order, passing the shared `Context` through each. No node knows about its neighbours — it only reads and writes context keys.

---

## 20.8 Running the Pipeline

```rust
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = Arc::new(openai::Client::from_env()?);
    let storage = Arc::new(InMemorySessionStorage::new());
    let runner = FlowRunner::new(Arc::new(build_pipeline(client)), storage);

    let session_id = "capstone-demo";
    let topic = "The trade-offs between Rust and Java for production AI agent systems";

    // Phase 1: research → synthesise → review → approve (pauses)
    runner.init_session(session_id, |ctx| {
        ctx.set_sync("topic", topic.to_string());
    }).await?;

    loop {
        let result = runner.run(session_id).await?;
        match result.status {
            ExecutionStatus::Completed => break,
            ExecutionStatus::Error(e) => { eprintln!("Error: {e}"); return Ok(()); }
            _ => {}
        }
    }

    // At this point: research, synthesis, and review are done.
    // The approval node returned End without completing — waiting for human.
    println!("\n--- Human reviews the report and approves ---\n");

    // Phase 2: inject approval decision, re-run
    runner.update_session(session_id, |ctx| {
        ctx.set_sync("approved", true);
    }).await?;

    loop {
        let result = runner.run(session_id).await?;
        match result.status {
            ExecutionStatus::Completed => { println!("Pipeline complete."); break; }
            ExecutionStatus::Error(e) => { eprintln!("Error: {e}"); break; }
            _ => {}
        }
    }

    Ok(())
}
```

Run it:

```bash
cd code-examples
export OPENAI_API_KEY="sk-..."
RUST_LOG=info cargo run -p ch20-capstone-multiagent-pipeline
```

Expected flow:

```
=== Research Pipeline ===
Topic: The trade-offs between Rust and Java for production AI agent systems

[Research]
• Rust binaries are 5-30 MB vs 80-200 MB for Spring Boot fat JARs
• Rust cold starts on Lambda: 5-50ms vs 3-8s for JVM
...

[Synthesis]
Executive summary: Rust offers significant advantages for LLM-intensive...
Key insights:
  1. Memory footprint: 10-30 MB idle vs 150-400 MB for Spring Boot
...

[Review]
Quality rating: 8/10
Suggestions:
  1. Add benchmarks for specific workloads (embedding throughput, not just cold start)
...

[Approval] Waiting for human approval...

--- Human reviews the report and approves ---

[Approval] Approved — pipeline complete.
=== Pipeline complete ===
```

---

## 20.9 Adding PostgreSQL Persistence

Replace `InMemorySessionStorage` with `PostgresSessionStorage` for sessions that survive restarts and scale across multiple instances:

```toml
graph-flow = { version = "0.5", features = ["postgres"] }
```

```rust
use graph_flow::PostgresSessionStorage;

let database_url = std::env::var("DATABASE_URL")?;
let storage = Arc::new(
    PostgresSessionStorage::new(&database_url).await?
);
let runner = FlowRunner::new(Arc::new(build_pipeline(client)), storage);
```

The graph code — every node, every edge, the `init_session` / `update_session` / `run` calls — is unchanged. The storage backend is the only difference.

In production this means:
- Approval requests survive server restarts
- Multiple web workers can serve the API; any can call `runner.run(session_id)`
- Historical sessions are auditable in PostgreSQL

---

## 20.10 Production Extensions

### Parallel research with Tokio

When you have independent research subtasks, fan them out:

```rust
async fn run(&self, context: Context) -> Result<TaskResult> {
    let topic: String = context.get_sync("topic").unwrap_or_default();

    let (findings1, findings2) = tokio::join!(
        research_subtopic(&self.client, &format!("{topic}: technical aspects")),
        research_subtopic(&self.client, &format!("{topic}: business aspects")),
    );

    let combined = format!("{}\n\n{}", findings1?, findings2?);
    context.set("findings", combined).await;
    // ...
}
```

### Retry node for review failures

```rust
async fn run(&self, context: Context) -> Result<TaskResult> {
    let report: String = context.get_sync("report").unwrap_or_default();
    let attempts: u32 = context.get_sync("review_attempts").unwrap_or(0);

    if attempts >= 3 {
        // Give up after 3 LLM failures
        context.set("review", "Review unavailable after 3 attempts.".to_string()).await;
        return Ok(TaskResult { response: None, next_action: NextAction::Continue });
    }

    match self.do_review(&report).await {
        Ok(review) => {
            context.set("review", review.clone()).await;
            Ok(TaskResult { response: Some(review), next_action: NextAction::Continue })
        }
        Err(e) => {
            tracing::warn!(error = %e, attempt = attempts + 1, "Review failed, will retry");
            context.set_sync("review_attempts", attempts + 1);
            // Loop back to retry — requires a conditional edge back to "review"
            Ok(TaskResult { response: None, next_action: NextAction::Continue })
        }
    }
}
```

### Structured context with serde_json

For complex inter-node data, store JSON blobs:

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct ResearchFindings {
    facts: Vec<String>,
    sources: Vec<String>,
    confidence: f32,
}

// Store
let json = serde_json::to_string(&findings)?;
context.set("findings_json", json).await;

// Retrieve
let json: String = context.get_sync("findings_json").unwrap_or_default();
let findings: ResearchFindings = serde_json::from_str(&json)?;
```

---

## 20.11 Java Comparison

The LangGraph4j equivalent uses `StateGraph<AgentState>`:

```java
// LangGraph4j
StateGraph<AgentState> graph = new StateGraph<>(AgentState::new)
    .addNode("research",   researchAgent)
    .addNode("synthesise", synthesisAgent)
    .addNode("review",     reviewAgent)
    .addNode("approve",    approvalNode)
    .addEdge("research",   "synthesise")
    .addEdge("synthesise", "review")
    .addEdge("review",     "approve")
    .addEdge(END, END);
```

The structure is nearly identical. Key differences:
- **Types**: LangGraph4j uses a typed `AgentState` class with explicit field definitions; graph-flow uses a string-keyed `Context` map — more flexible but less type-safe at compile time
- **Persistence**: LangGraph4j has built-in SQLite and PostgreSQL checkpointers; graph-flow has `PostgresSessionStorage` (requires the `postgres` feature)
- **Streaming**: LangGraph4j supports streaming node outputs; graph-flow runs nodes to completion before returning — no streaming
- **HITL**: Both use the same pattern: interrupt the graph, inject external state, resume

---

## 20.12 Key Takeaways

- **Four-node pattern**: Research → Synthesise → Review → Approve separates concerns into single-purpose agents
- **Human-in-the-loop**: `ApprovalNode` returns `NextAction::End` with no completion signal; `runner.update_session(id, ...)` injects the decision; re-run resumes
- **Context as the message bus**: nodes communicate only via `context.set` / `context.get_sync` — no direct coupling
- **Storage swap**: `InMemorySessionStorage` → `PostgresSessionStorage` without changing any node or graph code
- **Parallel subtasks**: `tokio::join!` inside a node for independent LLM calls — free concurrency
- **Retry logic**: store attempt count in context; conditional edge loops back to the failing node

---

## What's Next

Chapter 21 closes the book with the production checklist: performance profiling, security hardening, cost controls at scale, and a final architecture review synthesising all the patterns.

---

*→ Java reference: LangGraph4j multi-agent stateful pipeline with PostgreSQL checkpointing (Ch 22)*
