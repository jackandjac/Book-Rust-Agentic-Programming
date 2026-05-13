// Chapter 14: Stateful Workflows and Persistence
// See chapters/ch14-stateful-workflows.md for the full explanation.
//
// Run: cargo run -p ch14-stateful-workflows
//
// Demonstrates:
//   1. Session persistence — a workflow that survives process restart
//   2. Human-in-the-loop — graph pauses and waits for user approval
//
// Uses InMemorySessionStorage for the example (swappable with PostgresSessionStorage).

use anyhow::Result;
use async_trait::async_trait;
use graph_flow::{
    Context, ExecutionStatus, FlowRunner, GraphBuilder,
    InMemorySessionStorage, NextAction, Task, TaskResult,
};
use std::sync::Arc;

// ── Step 1: Fetch data ────────────────────────────────────────────────────────

struct FetchDataTask;

#[async_trait]
impl Task for FetchDataTask {
    fn id(&self) -> &str { "fetch-data" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        // Simulate fetching records from a database or API
        let records = vec![
            "Record A: sales=1200, region=north",
            "Record B: sales=800,  region=south",
            "Record C: sales=2100, region=east",
        ];
        println!("[fetch-data] Fetched {} records", records.len());
        context.set("records", records.join("\n")).await;
        context.set("step", "fetch-data-done".to_string()).await;

        Ok(TaskResult {
            response: Some(format!("Fetched {} records", 3)),
            next_action: NextAction::Continue,
        })
    }
}

// ── Step 2: Process data ─────────────────────────────────────────────────────

struct ProcessDataTask;

#[async_trait]
impl Task for ProcessDataTask {
    fn id(&self) -> &str { "process-data" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let records: String = context.get_sync("records").unwrap_or_default();
        let total_records = records.lines().count();

        // Simulate processing
        let summary = format!("Processed {total_records} records. Highest: Record C (2100 sales).");
        println!("[process-data] {summary}");
        context.set("summary", summary.clone()).await;
        context.set("step", "process-data-done".to_string()).await;

        Ok(TaskResult {
            response: Some(summary),
            next_action: NextAction::Continue,
        })
    }
}

// ── Step 3: Human-in-the-loop approval ───────────────────────────────────────

struct ApprovalTask;

#[async_trait]
impl Task for ApprovalTask {
    fn id(&self) -> &str { "await-approval" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let approved: Option<bool> = context.get_sync("approved");

        match approved {
            None => {
                // First pass — request approval
                let summary: String = context.get_sync("summary").unwrap_or_default();
                println!("[await-approval] Pausing for human approval.");
                println!("  Summary to approve: {summary}");
                context.set("step", "awaiting-approval".to_string()).await;

                Ok(TaskResult {
                    response: Some("Awaiting approval".to_string()),
                    next_action: NextAction::End, // pause; runner will stop here
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
                println!("[await-approval] Rejected — aborting pipeline.");
                context.set("step", "rejected".to_string()).await;
                Ok(TaskResult {
                    response: Some("Rejected".to_string()),
                    next_action: NextAction::End,
                })
            }
        }
    }
}

// ── Step 4: Publish results ───────────────────────────────────────────────────

struct PublishTask;

#[async_trait]
impl Task for PublishTask {
    fn id(&self) -> &str { "publish" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let summary: String = context.get_sync("summary").unwrap_or_default();
        println!("[publish] Publishing: {summary}");
        context.set("step", "published".to_string()).await;

        Ok(TaskResult {
            response: Some("Published successfully".to_string()),
            next_action: NextAction::End,
        })
    }
}

// ── Graph ─────────────────────────────────────────────────────────────────────

fn build_pipeline() -> graph_flow::Graph {
    GraphBuilder::new("report-pipeline")
        .add_task(Arc::new(FetchDataTask))
        .add_task(Arc::new(ProcessDataTask))
        .add_task(Arc::new(ApprovalTask))
        .add_task(Arc::new(PublishTask))
        .set_start_task("fetch-data")
        .add_edge("fetch-data", "process-data")
        .add_edge("process-data", "await-approval")
        // Conditional: approved → publish; rejected → ApprovalTask returns
        // NextAction::End before this edge fires, so the false branch is unreachable.
        // graph-flow requires both branches to be declared; "publish" is the fallback.
        .add_conditional_edge(
            "await-approval",
            |ctx: &Context| ctx.get_sync::<bool>("approved").unwrap_or(false),
            "publish", // true  → approved; continue to publish
            "publish", // false → rejection handled by NextAction::End in ApprovalTask
        )
        .build()
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let storage = Arc::new(InMemorySessionStorage::new());
    let graph = Arc::new(build_pipeline());
    let runner = FlowRunner::new(graph.clone(), storage.clone());

    let session_id = "report-2026-q1";

    println!("=== Run 1: Start pipeline ===\n");
    runner.init_session(session_id, |_| {}).await?;

    // Run steps 1 and 2 (fetch + process), stop at approval gate
    loop {
        let result = runner.run(session_id).await?;
        if let Some(r) = &result.response {
            println!("  → {r}");
        }
        match result.status {
            ExecutionStatus::Completed => { println!("Complete."); break; }
            ExecutionStatus::Error(e) => { eprintln!("Error: {e}"); break; }
            _ => {
                // Check if we're paused at the approval gate
                let step: Option<String> = {
                    // Peek at context via a read of the session
                    // In production you'd load the session from storage here
                    None // simplified — in real code: storage.get(session_id).context.get_sync("step")
                };
                // Stop if paused at approval (ApprovalTask returns NextAction::End on first pass)
                if result.response.as_deref() == Some("Awaiting approval") {
                    println!("\n  [Pipeline paused — awaiting human approval]\n");
                    break;
                }
            }
        }
    }

    // ── Simulate: human approves and resumes ──────────────────────────────────
    println!("=== Run 2: Human approves, resume pipeline ===\n");

    // In a real system the session is loaded from persistent storage.
    // Here we inject "approved = true" and restart the runner from the approval node.
    let runner2 = FlowRunner::new(graph, storage);
    runner2.update_session(session_id, |ctx| {
        ctx.set_sync("approved", true);
    }).await?;

    loop {
        let result = runner2.run(session_id).await?;
        if let Some(r) = &result.response {
            println!("  → {r}");
        }
        match result.status {
            ExecutionStatus::Completed => { println!("Pipeline complete."); break; }
            ExecutionStatus::Error(e) => { eprintln!("Error: {e}"); break; }
            _ => {}
        }
    }

    Ok(())
}
