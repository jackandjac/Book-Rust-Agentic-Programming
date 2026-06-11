// Chapter 20 (Capstone): Multi-Agent Pipeline — Research → Synthesis → Review → Approval
// See chapters/ch20-capstone-multiagent-pipeline.md for the full walkthrough.
//
// Run: cargo run -p ch20-capstone-multiagent-pipeline
// Requires: OPENAI_API_KEY env var (or .env file)
//
// Pipeline:
//   ResearchNode → SynthesisNode → ReviewNode → ApprovalNode (human gate)
//   graph-flow manages state; rig agents do the LLM work.

use anyhow::Result;
use async_trait::async_trait;
use graph_flow::{
    Context, ExecutionStatus, FlowRunner, GraphBuilder,
    InMemorySessionStorage, NextAction, Task, TaskResult,
};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::providers::openai;
use std::sync::Arc;

// ── ResearchNode ──────────────────────────────────────────────────────────────

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

        println!("[Research]\n{findings}\n");
        context.set("findings", findings.clone()).await;

        Ok(TaskResult {
            response: Some(findings),
            next_action: NextAction::Continue,
        })
    }
}

// ── SynthesisNode ─────────────────────────────────────────────────────────────

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

        println!("[Synthesis]\n{report}\n");
        context.set("report", report.clone()).await;

        Ok(TaskResult {
            response: Some(report),
            next_action: NextAction::Continue,
        })
    }
}

// ── ReviewNode ────────────────────────────────────────────────────────────────

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
                "You are a critical reviewer. Identify any factual inaccuracies, \
                 logical gaps, or missing context in the provided report. \
                 Rate quality 1-10 and give 1-3 specific improvement suggestions.",
            )
            .build();

        let review = agent
            .prompt(&format!("Review this report for quality and accuracy:\n\n{report}"))
            .await?;

        println!("[Review]\n{review}\n");
        context.set("review", review.clone()).await;

        Ok(TaskResult {
            response: Some(review),
            // Hand off to human approval gate
            next_action: NextAction::Continue,
        })
    }
}

// ── ApprovalNode (human-in-the-loop) ─────────────────────────────────────────

/// Returns End (waiting) until `approved` is set to true via update_session.
struct ApprovalNode;

#[async_trait]
impl Task for ApprovalNode {
    fn id(&self) -> &str { "approve" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let approved: Option<bool> = context.get_sync("approved");

        match approved {
            Some(true) => {
                println!("[Approval] Approved — pipeline complete.");
                Ok(TaskResult {
                    response: Some("approved".to_string()),
                    next_action: NextAction::End,
                })
            }
            Some(false) => {
                println!("[Approval] Rejected — stopping pipeline.");
                Ok(TaskResult {
                    response: Some("rejected".to_string()),
                    next_action: NextAction::End,
                })
            }
            None => {
                println!("[Approval] Waiting for human approval...");
                println!("  → Run again after setting 'approved' in the session.");
                Ok(TaskResult {
                    response: None,
                    // End the current run; caller re-runs after injecting approval
                    next_action: NextAction::End,
                })
            }
        }
    }
}

// ── Graph ─────────────────────────────────────────────────────────────────────

fn build_pipeline(client: Arc<openai::Client>) -> graph_flow::Graph {
    GraphBuilder::new("research-pipeline")
        .add_task(Arc::new(ResearchNode  { client: client.clone() }))
        .add_task(Arc::new(SynthesisNode { client: client.clone() }))
        .add_task(Arc::new(ReviewNode    { client }))
        .add_task(Arc::new(ApprovalNode))
        .set_start_task("research")
        .add_edge("research",  "synthesise")
        .add_edge("synthesise","review")
        .add_edge("review",    "approve")
        .build()
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = Arc::new(openai::Client::from_env());
    let storage = Arc::new(InMemorySessionStorage::new());
    let graph = Arc::new(build_pipeline(client));
    let runner = FlowRunner::new(graph, storage);

    let session_id = "capstone-demo";
    let topic = "The trade-offs between Rust and Java for production AI agent systems";

    println!("=== Research Pipeline ===");
    println!("Topic: {topic}\n");

    // ── First run: research → synthesise → review → approve (waits) ──────────
    runner.init_session(session_id, |ctx| {
        ctx.set_sync("topic", topic.to_string());
    }).await?;

    loop {
        let result = runner.run(session_id).await?;
        match result.status {
            ExecutionStatus::Completed => break,
            ExecutionStatus::Error(e) => {
                eprintln!("Pipeline error: {e}");
                return Ok(());
            }
            _ => {}
        }
    }

    // ── Simulate human approval ───────────────────────────────────────────────
    println!("\n--- Human reviews the report, then approves ---\n");
    runner.update_session(session_id, |ctx| {
        ctx.set_sync("approved", true);
    }).await?;

    // ── Second run: approval gate sees approved=true → End ────────────────────
    loop {
        let result = runner.run(session_id).await?;
        match result.status {
            ExecutionStatus::Completed => {
                println!("\n=== Pipeline complete ===");
                break;
            }
            ExecutionStatus::Error(e) => {
                eprintln!("Pipeline error: {e}");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}
