// Chapter 13: Building Agents with graph-flow — ReAct Pattern
// See chapters/ch13-graph-agents.md for the full explanation.
//
// Run: cargo run -p ch13-graph-agents
// Requires: OPENAI_API_KEY env var (or .env file)
//
// Implements the ReAct (Reason + Act) pattern as a graph:
//
//   [Think] ──► [Act] ──► [Think] ──► ... ──► [Respond]
//      │                                            ▲
//      └──────────────── (done) ────────────────────┘
//
// Think calls the LLM. If the LLM wants a tool, Act runs it and loops
// back. When the LLM has a final answer, the graph terminates.

use anyhow::Result;
use async_trait::async_trait;
use graph_flow::{
    Context, FlowRunner, GraphBuilder, InMemorySessionStorage,
    NextAction, Task, TaskResult,
};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::providers::openai;
use std::sync::Arc;

// ── Simple tool registry ──────────────────────────────────────────────────────

fn run_tool(name: &str, args: &serde_json::Value) -> String {
    match name {
        "calculator" => {
            let a = args["a"].as_f64().unwrap_or(0.0);
            let b = args["b"].as_f64().unwrap_or(0.0);
            let op = args["op"].as_str().unwrap_or("+");
            let result = match op {
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                "/" if b != 0.0 => a / b,
                _ => f64::NAN,
            };
            format!("{result}")
        }
        "word_count" => {
            let text = args["text"].as_str().unwrap_or("");
            format!("{}", text.split_whitespace().count())
        }
        _ => format!("Unknown tool: {name}"),
    }
}

// ── Think node ────────────────────────────────────────────────────────────────

struct ThinkTask {
    client: Arc<openai::Client>,
}

#[async_trait]
impl Task for ThinkTask {
    fn id(&self) -> &str { "think" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let question: String = context.get_sync("question").unwrap_or_default();
        let tool_results: Vec<String> = context.get_sync("tool_results").unwrap_or_default();

        let mut prompt = format!(
            "You are a helpful assistant. Answer the user's question.\n\
             Available tools:\n\
             - calculator: {{\"tool\":\"calculator\",\"args\":{{\"a\":N,\"b\":N,\"op\":\"+|-|*|/\"}}}}\n\
             - word_count: {{\"tool\":\"word_count\",\"args\":{{\"text\":\"...\"}}}}\n\n\
             If you need a tool, respond with ONLY the JSON object shown above.\n\
             If you have the final answer, respond with ONLY: {{\"answer\":\"your answer\"}}\n\n\
             Question: {question}"
        );

        if !tool_results.is_empty() {
            prompt.push_str("\n\nTool results so far:");
            for r in &tool_results { prompt.push_str(&format!("\n  {r}")); }
        }

        let agent = self.client.agent(openai::GPT_4O_MINI).build();
        let response = agent.prompt(&prompt).await?;

        println!("[Think] {response}");
        context.set("llm_response", response.clone()).await;

        // Determine if done (has final answer) or needs a tool call
        let done = serde_json::from_str::<serde_json::Value>(response.trim())
            .map(|j| j.get("answer").is_some())
            .unwrap_or(false);
        context.set("done", done).await;

        Ok(TaskResult { response: Some(response), next_action: NextAction::Continue })
    }
}

// ── Act node ──────────────────────────────────────────────────────────────────

struct ActTask;

#[async_trait]
impl Task for ActTask {
    fn id(&self) -> &str { "act" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let llm_response: String = context.get_sync("llm_response").unwrap_or_default();
        let tool_call: serde_json::Value =
            serde_json::from_str(llm_response.trim()).unwrap_or(serde_json::Value::Null);

        let result = if let (Some(name), Some(args)) =
            (tool_call.get("tool").and_then(|n| n.as_str()),
             tool_call.get("args"))
        {
            let r = run_tool(name, args);
            println!("[Act] {name}({args}) → {r}");
            format!("{name}({args}) = {r}")
        } else {
            "No tool call found".to_string()
        };

        let mut results: Vec<String> = context.get_sync("tool_results").unwrap_or_default();
        results.push(result.clone());
        context.set("tool_results", results).await;

        Ok(TaskResult { response: Some(result), next_action: NextAction::Continue })
    }
}

// ── Respond node ──────────────────────────────────────────────────────────────

struct RespondTask;

#[async_trait]
impl Task for RespondTask {
    fn id(&self) -> &str { "respond" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let llm_response: String = context.get_sync("llm_response").unwrap_or_default();
        let answer = serde_json::from_str::<serde_json::Value>(llm_response.trim())
            .ok()
            .and_then(|v| v.get("answer").and_then(|a| a.as_str()).map(String::from))
            .unwrap_or(llm_response);

        println!("\n[Answer] {answer}");
        context.set("answer", answer.clone()).await;

        Ok(TaskResult { response: Some(answer), next_action: NextAction::End })
    }
}

// ── Graph ─────────────────────────────────────────────────────────────────────

fn build_react_graph(client: Arc<openai::Client>) -> graph_flow::Graph {
    GraphBuilder::new("react-agent")
        .add_task(Arc::new(ThinkTask { client }))
        .add_task(Arc::new(ActTask))
        .add_task(Arc::new(RespondTask))
        .set_start_task("think")
        .add_conditional_edge(
            "think",
            |ctx: &Context| ctx.get_sync::<bool>("done").unwrap_or(false),
            "respond", // done → final answer
            "act",     // not done → call tool
        )
        .add_edge("act", "think") // loop back after tool call
        .build()
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = Arc::new(openai::Client::from_env());
    let graph = build_react_graph(client);
    let runner = FlowRunner::new(
        Arc::new(graph),
        Arc::new(InMemorySessionStorage::new()),
    );

    let session_id = "react-demo";
    let question = "What is 17 multiplied by 23, then add 99?";
    println!("Question: {question}\n");

    runner.init_session(session_id, |ctx| {
        ctx.set_sync("question", question.to_string());
    }).await?;

    for _ in 0..10 {
        let result = runner.run(session_id).await?;
        match result.status {
            graph_flow::ExecutionStatus::Completed => break,
            graph_flow::ExecutionStatus::Error(msg) => {
                eprintln!("Error: {msg}");
                break;
            }
            _ => {}
        }
    }

    Ok(())
}
