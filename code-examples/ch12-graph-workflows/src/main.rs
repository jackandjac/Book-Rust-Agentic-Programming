// Chapter 12: Graph-Based Workflows with graph-flow
// See chapters/ch12-graph-workflows.md for the full explanation.
//
// Run: cargo run -p ch12-graph-workflows
//
// Demonstrates a three-node text-processing pipeline:
//   Validate → Summarise → Classify
// State is held in a graph-flow Context; routing is unconditional.
//
// NOTE: graph-flow is a small project (312 GitHub stars, v0.5.1).
//       API may change — always check https://github.com/a-agmon/rs-graph-llm

use anyhow::Result;
use async_trait::async_trait;
use graph_flow::{
    Context, FlowRunner, Graph, GraphBuilder,
    InMemorySessionStorage, NextAction, Task, TaskResult,
};
use std::sync::Arc;

// ── Task definitions ──────────────────────────────────────────────────────────
//
// Each task is a struct that implements the `Task` trait.
// The `run()` method receives the shared Context, modifies it, and
// returns a TaskResult containing the next action and an optional response.

/// Step 1: Validate that the input text is non-empty.
struct ValidateTask;

#[async_trait]
impl Task for ValidateTask {
    async fn run(&self, context: Context) -> Result<TaskResult> {
        let input: String = context.get_sync("input")
            .unwrap_or_default();

        if input.trim().is_empty() {
            context.set("error", "Input is empty".to_string()).await;
            return Ok(TaskResult {
                response: Some("Validation failed: empty input".to_string()),
                next_action: NextAction::End,
            });
        }

        println!("[Validate] Input accepted: {} chars", input.len());
        context.set("validated", true).await;

        Ok(TaskResult {
            response: Some("Validation passed".to_string()),
            next_action: NextAction::Continue,
        })
    }
}

/// Step 2: Summarise the input (stub — in production, call an LLM here).
struct SummariseTask;

#[async_trait]
impl Task for SummariseTask {
    async fn run(&self, context: Context) -> Result<TaskResult> {
        let input: String = context.get_sync("input").unwrap_or_default();

        // Production version would call rig agent here.
        // For this example, we produce a simple first-sentence summary.
        let summary = input
            .split('.')
            .next()
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| input.chars().take(80).collect());

        println!("[Summarise] Summary: {summary}");
        context.set("summary", summary.clone()).await;

        Ok(TaskResult {
            response: Some(summary),
            next_action: NextAction::Continue,
        })
    }
}

/// Step 3: Classify the summary as short (<= 50 chars) or long.
struct ClassifyTask;

#[async_trait]
impl Task for ClassifyTask {
    async fn run(&self, context: Context) -> Result<TaskResult> {
        let summary: String = context.get_sync("summary").unwrap_or_default();
        let category = if summary.len() <= 50 { "short" } else { "long" };

        println!("[Classify] Category: {category}");
        context.set("category", category.to_string()).await;

        Ok(TaskResult {
            response: Some(format!("Category: {category}")),
            next_action: NextAction::End,
        })
    }
}

// ── Graph construction ────────────────────────────────────────────────────────

fn build_pipeline() -> Graph {
    GraphBuilder::new("text-pipeline")
        .add_task(Arc::new(ValidateTask))
        .add_task(Arc::new(SummariseTask))
        .add_task(Arc::new(ClassifyTask))
        .set_start_task("ValidateTask")
        .add_edge("ValidateTask", "SummariseTask")
        .add_edge("SummariseTask", "ClassifyTask")
        .build()
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let graph = build_pipeline();
    let storage = InMemorySessionStorage::new();
    let runner = FlowRunner::new(Arc::new(graph), Arc::new(storage));

    let session_id = "demo-session-1";

    // Seed the context with the input text.
    // In graph-flow, state is seeded by running a session with a pre-populated context,
    // or by storing initial values before the first run.
    runner.init_session(session_id, |ctx| {
        ctx.set_sync("input",
            "Rust is a systems programming language that runs blazingly fast, \
             prevents segfaults, and guarantees thread safety. It achieves memory \
             safety without a garbage collector through its ownership system."
        );
    }).await?;

    // Execute the pipeline step-by-step.
    // Each call to runner.run() executes one task and persists state.
    loop {
        let result = runner.run(session_id).await?;

        if let Some(response) = &result.response {
            println!("Step response: {response}");
        }

        match result.status {
            graph_flow::ExecutionStatus::Completed => {
                println!("\nPipeline complete!");
                break;
            }
            graph_flow::ExecutionStatus::Error(msg) => {
                eprintln!("Pipeline error: {msg}");
                break;
            }
            _ => {} // Continue to next step
        }
    }

    Ok(())
}
