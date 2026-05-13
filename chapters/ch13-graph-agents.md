# Chapter 13: Building Agents with graph-flow

> **Framework versions in this chapter:**  
> `graph-flow = "0.5.1"` · `rig-core = "0.37"` · `async-trait = "0.1"`
>
> **Java reference:** LangGraph4j ReAct agent pattern (Chapter 16 of Java book)

---

Chapter 12 introduced graph-flow's primitives. This chapter puts them together into a **ReAct agent** — the most common agentic pattern in practice.

ReAct stands for *Reason + Act*. The agent alternates between two steps:
1. **Reason** — call the LLM with the current context; it either requests a tool or produces a final answer
2. **Act** — if a tool was requested, execute it; append the result to context; loop back to Reason

In a graph this is a cycle:

```
START → [Think] ──(done?)──▶ [Respond] → END
              └──(need tool)──▶ [Act] ──▶ [Think]
```

---

## 13.1 The ReAct Pattern

ReAct was described in a 2022 paper as an interleaving of reasoning traces (chain-of-thought) and action steps (tool calls). Every major agentic framework implements it:

| Framework | Pattern name |
|-----------|-------------|
| LangChain4j | `ReActAgent`, `AiServices` with tools |
| Spring AI | `ToolCallingChatOptions` loop |
| LangGraph4j | `ReactAgent` utility / custom graph |
| rig-core | `Agent` with tools (automatic loop) |
| graph-flow | Manual Think→Act→Think cycle (this chapter) |

The graph-flow version is more verbose than rig's `Agent` (which hides the loop), but it gives you complete visibility into each step — which matters for debugging, human-in-the-loop, and observability.

---

## 13.2 Graph Structure

```rust
GraphBuilder::new("react-agent")
    .add_task(Arc::new(ThinkTask { client }))
    .add_task(Arc::new(ActTask))
    .add_task(Arc::new(RespondTask))
    .set_start_task("think")
    .add_conditional_edge(
        "think",
        |ctx: &Context| ctx.get_sync::<bool>("done").unwrap_or(false),
        "respond",   // true  → LLM has final answer
        "act",       // false → LLM wants to call a tool
    )
    .add_edge("act", "think")  // always loop back after a tool call
    .build()
```

The cycle is `think → act → think → act → …` until `done = true`, then `think → respond → END`.

---

## 13.3 The Think Node

The Think node calls the LLM and parses its response to determine the next action.

```rust
struct ThinkTask {
    client: Arc<openai::Client>,
}

#[async_trait]
impl Task for ThinkTask {
    fn id(&self) -> &str { "think" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let question: String = context.get_sync("question").unwrap_or_default();
        let tool_results: Vec<String> = context.get_sync("tool_results").unwrap_or_default();

        // Build prompt including any prior tool results
        let prompt = build_react_prompt(&question, &tool_results);

        let agent = self.client.agent(openai::GPT_4O_MINI).build();
        let response = agent.prompt(&prompt).await?;

        // Parse: {"answer": "..."} → done; {"tool": "...", "args": {...}} → call tool
        let done = serde_json::from_str::<serde_json::Value>(response.trim())
            .map(|j| j.get("answer").is_some())
            .unwrap_or(false);

        context.set("llm_response", response.clone()).await;
        context.set("done", done).await;

        Ok(TaskResult { response: Some(response), next_action: NextAction::Continue })
    }
}
```

Key design decisions:

1. **The `client` is cloned into the task** at graph construction time — graph-flow tasks must be `Send + Sync`, and `Arc<openai::Client>` satisfies this.
2. **Prompt engineering** drives the tool-call protocol. Rather than using rig's native tool calling (which would be cleaner in a pure rig context), we use JSON-in-prompt here to keep the graph's control flow explicit. In production, you'd use rig's `Agent` with tools inside the Think node.
3. **Context stores both `done` and `llm_response`** so the conditional edge and Act node can read them without re-running the LLM.

---

## 13.4 The Act Node

The Act node reads the tool call from context, executes it, and appends the result for the next Think iteration.

```rust
struct ActTask;

#[async_trait]
impl Task for ActTask {
    fn id(&self) -> &str { "act" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let llm_response: String = context.get_sync("llm_response").unwrap_or_default();
        let tool_call: serde_json::Value =
            serde_json::from_str(llm_response.trim()).unwrap_or(serde_json::Value::Null);

        // Execute the requested tool
        let result = if let (Some(name), Some(args)) = (
            tool_call.get("tool").and_then(|n| n.as_str()),
            tool_call.get("args"),
        ) {
            let r = run_tool(name, args);
            format!("{name}({args}) = {r}")
        } else {
            "No tool call found".to_string()
        };

        // Append to running list of tool results
        let mut results: Vec<String> = context.get_sync("tool_results").unwrap_or_default();
        results.push(result.clone());
        context.set("tool_results", results).await;

        Ok(TaskResult { response: Some(result), next_action: NextAction::Continue })
    }
}
```

`ActTask` has no LLM dependency — it's pure computation. This is a key ReAct property: the Reason step and Act step are strictly separated.

---

## 13.5 Streaming from a Graph

`graph-flow` 0.5 has no built-in streaming API — the graph executes one task at a time and returns `ExecutionResult`. To stream output to a user interface, print inside `Task::run()` or collect responses after each step:

```rust
loop {
    let result = runner.run(session_id).await?;

    // Emit each step's response as a Server-Sent Event (if inside Axum)
    if let Some(response) = &result.response {
        tx.send(Event::default().data(response.clone())).await?;
    }

    if matches!(result.status, ExecutionStatus::Completed | ExecutionStatus::Error(_)) {
        break;
    }
}
```

This is less ergonomic than rig's `stream_prompt()`, but it gives you step-level observability — you can stream *task names* and *intermediate results*, not just LLM tokens.

---

## 13.6 Error Recovery and Retry Nodes

Graph-flow doesn't have built-in retry logic, but you can implement it with a retry counter in context:

```rust
struct RetryableActTask { max_retries: u32 }

#[async_trait]
impl Task for RetryableActTask {
    fn id(&self) -> &str { "act" }

    async fn run(&self, context: Context) -> Result<TaskResult> {
        let retries: u32 = context.get_sync("retries").unwrap_or(0);

        match attempt_tool_call(&context).await {
            Ok(result) => {
                context.set("retries", 0u32).await; // reset on success
                Ok(TaskResult { response: Some(result), next_action: NextAction::Continue })
            }
            Err(e) if retries < self.max_retries => {
                context.set("retries", retries + 1).await;
                context.set("last_error", e.to_string()).await;
                // Return to Think node to ask the LLM to try differently
                Ok(TaskResult {
                    response: Some(format!("Tool failed ({retries}/{} retries): {e}", self.max_retries)),
                    next_action: NextAction::Continue,
                })
            }
            Err(e) => {
                Ok(TaskResult {
                    response: Some(format!("Giving up after {} retries: {e}", self.max_retries)),
                    next_action: NextAction::End,
                })
            }
        }
    }
}
```

The conditional edge after the Act node can then route to a "handle error" task based on `context.get_sync::<String>("last_error")`.

---

## 13.7 Hands-On: ReAct Agent

```bash
cd code-examples
export OPENAI_API_KEY="sk-..."
cargo run -p ch13-graph-agents
```

Expected output for "What is 17 multiplied by 23, then add 99?":

```
Question: What is 17 multiplied by 23, then add 99?

[Think] {"tool":"calculator","args":{"a":17,"b":23,"op":"*"}}
[Act] calculator({"a":17,"b":23,"op":"*"}) → 391
[Think] {"tool":"calculator","args":{"a":391,"b":99,"op":"+"}}
[Act] calculator({"a":391,"b":99,"op":"+"}) → 490
[Think] {"answer":"The result of 17 × 23 + 99 is 490."}

[Answer] The result of 17 × 23 + 99 is 490.
```

The graph runs 5 steps (Think→Act→Think→Act→Think→Respond). Each step is logged; the full trace shows exactly what the LLM decided at each turn.

---

## 13.8 Key Takeaways

- **ReAct** = Think (LLM) → Act (tool) → Think (LLM) → … until done → Respond
- **`add_conditional_edge("think", done_predicate, "respond", "act")`** — the routing heart of the graph
- **`add_edge("act", "think")`** — the loop-back that makes the cycle work
- **Think node** owns the LLM client; Act node is pure computation — keep them separate
- **No streaming** in graph-flow 0.5; simulate by collecting `result.response` after each `runner.run()` call
- **`context.get_sync::<T>(key)`** is safe in sync code (closures, predicate lambdas); use `context.get(key).await` inside async tasks
- **Arc wrapping** — tasks passed to `add_task()` must be `Arc<dyn Task>`; clone lightweight handles into each task at construction

---

## What's Next

Chapter 14 covers persistence: how to wire `PostgresSessionStorage`, add checkpointing, and implement human-in-the-loop pausing — making the ReAct graph survive process restarts and wait for external input.

---

*→ Java reference: LangGraph4j `ReactAgent`, custom `StateGraph` with tool node (Ch 16)*
