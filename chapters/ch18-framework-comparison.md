# Chapter 18: Comparing All Three Frameworks + What's Next

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` · `swiftide = "0.32"` · `graph-flow = "0.5.1"` · `rmcp = "1.6"`  
> `autoagents = "0.3.7"` · `kalosm = "0.3"`
>
> **Java reference:** Spring AI, LangChain4j, LangGraph4j comparison (Chapter 20 of Java book)

---

You've now seen the full Rust agentic stack in action. Before the capstone chapters, it's worth stepping back and mapping the terrain: what each framework does, how they fit together, where they're fragile, and where the ecosystem is heading.

This chapter covers:
1. Feature matrix: rig vs swiftide vs graph-flow vs rmcp
2. Decision guide: when to reach for each tool
3. Hybrid architectures: Rust agents + Java services
4. Maturity risks and how to manage them
5. The Rust agentic ecosystem in 2026 and beyond

---

## 18.1 The Landscape in One Sentence Each

The Rust agentic ecosystem is not a single monolith like Spring AI. It's a set of composable, single-purpose crates:

- **rig-core** — LLM clients, tool calling, structured output, agent conversations. The Spring AI equivalent.
- **swiftide** — Streaming document indexing pipelines for RAG. The LangChain4j EmbeddingStoreIngestor equivalent.
- **graph-flow** — Stateful graph-based workflow orchestration. The LangGraph4j equivalent.
- **rmcp** — Model Context Protocol server and client. The Spring AI MCP starters equivalent.
- **autoagents** — Event-driven multi-agent coordination. The experimental actor-model equivalent.
- **kalosm** — Pure-Rust local LLM inference. No Java equivalent.

A critical difference from Java: **these crates don't compete — they compose**. Spring AI bundles chat, RAG, tool calling, and vector stores into one framework. In Rust, you pick rig for LLM calls, swiftide for indexing, and graph-flow for orchestration — and wire them together yourself.

---

## 18.2 Feature Matrix

| Feature | rig-core 0.37 | swiftide 0.32 | graph-flow 0.5.1 | rmcp 1.6 |
|---------|:---:|:---:|:---:|:---:|
| **Purpose** | LLM agent layer | Document indexing | Workflow orchestration | MCP protocol |
| **Tool calling** | ✅ `#[rig_tool]` | ❌ | ❌ | ✅ (via MCP tools) |
| **Structured output** | ✅ `Extractor<M, T>` | ❌ | ❌ | ❌ |
| **RAG / vector search** | ✅ built-in stores | ✅ primary use case | ❌ | ❌ |
| **Streaming indexing** | ❌ | ✅ core design | ❌ | ❌ |
| **Stateful sessions** | ❌ | ❌ | ✅ in-memory/PostgreSQL | ❌ |
| **Graph workflows** | ❌ | ❌ | ✅ DAG + cycles | ❌ |
| **Conditional routing** | ❌ | ❌ | ✅ `add_conditional_edge` | ❌ |
| **Human-in-the-loop** | ❌ | ❌ | ✅ breakpoints | ❌ |
| **MCP server** | ❌ | ❌ | ❌ | ✅ |
| **MCP client** | ❌ | ❌ | ❌ | ✅ |
| **Multi-turn memory** | ✅ 0.37 | ❌ | ✅ (via context) | ❌ |
| **WASM compatible** | ⚠️ partial | ❌ | ❌ | ❌ |
| **Streaming responses** | ✅ SSE | ✅ pipeline | ❌ step-only | ❌ |
| **Local LLM inference** | ❌ (API only) | ❌ | ❌ | ❌ |
| **crates.io downloads** | 772k | 81k | 6.6k | 9.7M |
| **Pre-1.0** | ✅ | ✅ | ✅ | ✅ |

### What the download numbers tell you

rmcp's 9.7 million downloads reflects MCP adoption broadly — every tool that speaks MCP uses it, not just Rust agents. It's the infrastructure layer.

rig-core at 772k is healthy for a framework crate. swiftide at 81k is growing but niche. graph-flow at 6.6k is small — that's a risk to account for in your architecture (§18.4).

### Java comparison

| Java (Spring AI / LangChain4j) | Rust equivalent |
|---|---|
| `ChatClient` + advisors | `rig-core` agent |
| `EmbeddingStoreIngestor` | `swiftide` indexing pipeline |
| `LangGraph4j` `StateGraph` | `graph-flow` `GraphBuilder` |
| Spring AI MCP starters | `rmcp` |
| `MessageWindowChatMemory` | `rig_memory::SlidingWindowMemory` |
| `BeanOutputConverter` | `rig::Extractor<M, T>` |
| `@Tool` / `@ToolParam` | `#[rig_tool]` |

The Java stack is more integrated (Spring manages DI, config, health checks). The Rust stack is more composable but requires more explicit wiring.

---

## 18.3 Decision Guide

Use this as a flowchart:

**"I need to call an LLM and use tools"**  
→ `rig-core`. Start with `client.agent(...).preamble(...).tool(...).build()`.

**"I need to index documents for RAG (more than a few hundred files)"**  
→ `swiftide`. Its streaming pipeline handles large document sets efficiently.  
→ For small corpora, `rig`'s built-in `InMemoryVectorStore` + `FileLoader` is sufficient.

**"I need stateful multi-step workflows with persistence and conditional branching"**  
→ `graph-flow`. This is its only purpose — and it does it well.

**"I need to build or consume MCP servers"**  
→ `rmcp`. The official Rust MCP SDK. Nothing else comes close.

**"I need a web API around my agent"**  
→ `rig` + `axum` (Chapter 7). They compose naturally via `AppState`.

**"I need local LLM inference (no API key, privacy-sensitive)"**  
→ `kalosm`. Downloads and caches models; runs on CPU or GPU.

**"I need edge deployment at CDN nodes"**  
→ WASM (`wasm32-wasip2`) + Cloudflare Workers via the `worker` crate.

**"I need a complex multi-agent system with agent supervision"**  
→ `graph-flow` with rig agents as nodes (Chapter 15) for production.  
→ `autoagents` for experimental actor-model supervision (watch its roadmap first).

### The "which RAG crate" question

This comes up constantly. The short answer:

| Scenario | Recommendation |
|---|---|
| < 500 docs, no rebuild | `rig` built-in vector store |
| 500–50k docs, needs incremental updates | `swiftide` |
| Shared vector store across services | `rig` + Qdrant (`rig-qdrant`) or Redis |
| Need semantic chunking + metadata extraction | `swiftide` (`MetadataQAText` transformer) |

---

## 18.4 Hybrid Architectures: Rust + Java

Most organisations migrating to Rust won't rewrite everything overnight. The practical path is a hybrid: Rust handles LLM-intensive work, Java continues to own business logic, auth, and databases.

```
┌──────────────────────────────────┐
│  Java Spring Boot                │
│  Auth · Business Logic · DB      │
│  Spring Security · JPA · Kafka   │
└──────────────┬───────────────────┘
               │ REST / gRPC / MCP
┌──────────────▼───────────────────┐
│  Rust Axum Service               │
│  LLM · Embeddings · Agents       │
│  rig-core · swiftide · axum      │
└──────────────┬───────────────────┘
               │
┌──────────────▼───────────────────┐
│  LLM Providers / Vector Stores   │
│  OpenAI · Anthropic · Qdrant     │
└──────────────────────────────────┘
```

This boundary works well because:
- LLM calls are stateless HTTP — easy to extract
- Embedding and vector search are CPU/memory-bound — Rust excels here
- Business rules, user management, billing, audit logs stay in Java where your team already has expertise
- The interface is standard HTTP (or MCP if you use `rmcp`) — no language coupling

### MCP as the integration boundary

`rmcp` enables a cleaner split: your Rust service exposes tools via MCP, and any client (Java Spring AI, Claude Desktop, other Rust agents) consumes them. This inverts the dependency — Java doesn't import Rust code, it calls MCP tools over HTTP/STDIO.

```toml
# Java side: Spring AI MCP client connects to your Rust MCP server
# Rust side:
rmcp = "1.6"
```

The Spring AI MCP client speaks the same protocol as `rmcp` — this is the integration layer.

---

## 18.5 Framework Maturity Risks

Every framework in this book is pre-1.0. That's not a reason to avoid Rust for production agentic systems — it's a reason to manage the risk explicitly.

### Risk: breaking changes between minor versions

Pre-1.0 crates can introduce breaking API changes between minor versions — for example, changes to method signatures or removal of features. This is normal for active pre-1.0 projects.

**Mitigation:**
- Pin exact versions in `Cargo.lock` and commit it
- Check changelogs before upgrading: the rig changelog lists breaking changes clearly
- Run `cargo test` before and after any version bump
- Treat each upgrade as a mini-PR with its own review

```toml
# Cargo.toml — pin to exact minor version
rig-core  = "=0.37.0"
swiftide  = "=0.32.1"
graph-flow = "=0.5.1"
```

Using `=` (exact version) is more conservative than `"0.37"` (which allows patch updates). For production, exact pins are safer.

### Risk: project abandonment

graph-flow has 312 GitHub stars and 6.6k downloads. It could stall.

**Mitigation:**
- The `Task` + `Context` + `GraphBuilder` abstraction is thin — porting to a different orchestration layer is a week of work, not a month
- Keep your business logic out of graph-flow types — put it in plain structs that happen to implement `Task`
- Monitor the repo: if commits stop for 6 months, evaluate alternatives

### Risk: ecosystem fragmentation

New agent crates appear monthly. `langchain-rust` was the dominant framework in 2023 — its last release was October 2024. It's already deprecated.

**Mitigation:**
- Prefer crates with clear institutional backing (rmcp is maintained by the MCP team)
- Prefer crates used in production by multiple organisations
- Abstract LLM calls behind a trait so you can swap providers

```rust
// Your own abstraction layer — don't let rig types leak everywhere
trait AgentBackend: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String>;
}
```

### Risk: rig-core 1.0 API changes

rig doesn't have a 1.0 roadmap published. The pre-1.0 guarantee means the maintainers can break anything in a minor release.

**Mitigation:** The same as above — pin versions, test on upgrade, keep rig types at the edges of your application.

---

## 18.6 The Rust Agentic Ecosystem in 2026

### What's solidified

**MCP is the protocol layer.** rmcp's 9.7M downloads signals that MCP has won as the tool-integration standard for AI agents. Building your tools as MCP servers means they work with any client — Rust, Java, Python, Claude Desktop.

**rig-core is the default LLM layer.** For Rust developers who need LLM calls, tool calling, and structured output, rig-core is the starting point. It's not perfect, but it's actively maintained and has the widest provider coverage.

**Local inference is viable.** kalosm, candle (from Hugging Face), and llama.cpp Rust bindings mean you can run Llama 3.2 or Mistral 7B locally in pure Rust. As of 2026, model quality for everyday tasks (summarisation, classification, extraction) is competitive with GPT-3.5-level API calls. For privacy-sensitive workloads, local inference is a real option.

### What's still unsettled

**High-level orchestration.** graph-flow is functional but small. The LangGraph4j pattern (compile-time graph definition, streaming, time-travel debugging) doesn't have a mature Rust equivalent yet. This is the biggest gap relative to the Java ecosystem.

**Observability integration.** The tracing/OpenTelemetry stack is solid (Chapter 16), but rig doesn't emit traces by default — you instrument your own code. Spring AI's Micrometer integration is more turnkey.

**Evaluation frameworks.** Python has RAGAS, LangSmith, and others for LLM evaluation. Rust has nothing comparable yet. You'll need to roll your own evaluation harness or use Python evaluation tools against your Rust service's API.

### Where Rust wins clearly

| Scenario | Why Rust |
|---|---|
| Edge deployment | WASM, <15 MB binary, ~0 ms cold start |
| High-throughput embedding | Tokio parallelism, zero-copy, no GC pauses |
| Cost-sensitive scale-out | 10–30 MB idle memory vs 150–400 MB for Spring Boot |
| Privacy-sensitive inference | Local inference via kalosm, no API call |
| Serverless (cold start sensitive) | 5–50 ms cold start vs 3–8 s for JVM |

### Where Java still leads

| Scenario | Why Java |
|---|---|
| Complex business logic | Mature ecosystem, DI, Spring Security, JPA |
| Evaluation and observability | LangSmith, Micrometer, Spring Boot Actuator |
| Team familiarity | Most AI teams know Java; Rust has a steeper learning curve |
| RAG with complex pipelines | LangChain4j + Spring AI ETL pipeline is more mature |

---

## 18.7 Key Takeaways

- **rig / swiftide / graph-flow compose, not compete**: rig = LLM layer, swiftide = indexing layer, graph-flow = orchestration layer
- **rmcp is infrastructure**: 9.7M downloads; use it as the integration boundary between services
- **Decision rule**: agent → rig; indexing → swiftide; stateful workflow → graph-flow; MCP → rmcp; web → axum
- **Hybrid architecture**: Rust handles LLM/embedding/inference; Java keeps auth/business logic/DB; MCP or REST as the boundary
- **Pin exact versions** (`=0.37.0`) for pre-1.0 crates in production
- **graph-flow abandonment risk**: low migration cost because the abstraction is thin — keep business logic out of framework types
- **Local inference is viable** for privacy-sensitive and cost-sensitive workloads via kalosm / candle
- **Rust's clearest wins**: edge deployment, cold start, memory footprint, throughput
- **Java's clearest wins**: observability tooling, evaluation frameworks, team expertise, business logic ecosystems

---

## What's Next

Chapter 19 puts everything together in a capstone project: a full research agent built with rig, swiftide, and rmcp — from document indexing through tool calling and MCP server exposure, to a deployed Axum API.

---

*→ Java reference: Spring AI vs LangChain4j vs LangGraph4j comparison (Ch 20)*
