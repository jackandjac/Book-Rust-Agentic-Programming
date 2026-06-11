# Chapter 19: Capstone — Building a Research Agent

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` · `swiftide = "0.32"` · `rmcp = "1.6"` · `axum = "0.8"` · `tokio = "1"`
>
> **Java reference:** LangChain4j + Spring AI + Spring Boot end-to-end (Chapter 21 of Java book)

---

This chapter builds a complete research agent from scratch to deployment. We combine everything from Parts II–V:

- **swiftide** indexes a document corpus into a vector store
- **rig** answers questions with RAG-augmented prompts
- **rmcp** (§19.5) shows how to expose the agent as an MCP server
- **axum** (§19.7) wraps the whole thing in an HTTP API

The runnable example (`cargo run -p ch19-capstone-research-agent`) demonstrates the core indexing and querying flow. Sections 19.5–19.7 show the MCP and HTTP wrapping patterns as focused snippets building on the same foundation.

---

## 19.1 Project Structure

```
ch19-capstone-research-agent/
├── Cargo.toml
└── src/
    └── main.rs      # indexing + RAG + agent + main
```

For a production project you'd split these into modules. The single-file approach here keeps the narrative linear.

```toml
[package]
name = "ch19-capstone-research-agent"
version = "0.1.0"
edition = "2024"
description = "Chapter 19 (Capstone): Building a Research Agent"

[dependencies]
tokio    = { workspace = true }
anyhow   = { workspace = true }
serde    = { workspace = true }
serde_json = { workspace = true }
tracing  = { workspace = true }
tracing-subscriber = { workspace = true }
dotenvy  = { workspace = true }
rig-core = { workspace = true }
swiftide = { workspace = true }
rmcp     = { workspace = true }
```

---

## 19.2 Phase 1: Document Indexing with Swiftide

The first job is getting documents into a vector store. We use swiftide's streaming pipeline:

```rust
use swiftide::{
    indexing::{
        self,
        loaders::FileLoader,
        persist::MemoryStorage,
        transformers::{ChunkMarkdown, Embed, MetadataQAText},
    },
    integrations::openai::OpenAI as SwiftideOpenAI,
};

async fn index_documents(
    swiftide_client: SwiftideOpenAI,
    docs_path: &str,
) -> anyhow::Result<MemoryStorage> {
    let storage = MemoryStorage::default();

    indexing::Pipeline::from_loader(
        FileLoader::new(docs_path).with_extensions(&["md"]),
    )
    .then_chunk(ChunkMarkdown::from_chunk_range(100..2048))
    .then(MetadataQAText::new(swiftide_client.clone()))
    .then_in_batch(Embed::new(swiftide_client).with_batch_size(10))
    .then_store_with(storage.clone())
    .run()
    .await?;

    Ok(storage)
}
```

Each step of the pipeline:
- `FileLoader` — discovers `*.md` files in `docs/`
- `ChunkMarkdown` — splits each file into chunks of 100–2048 characters, respecting heading boundaries
- `MetadataQAText` — calls the LLM to generate hypothetical questions for each chunk (improves retrieval recall)
- `Embed` — generates embeddings in batches of 10 (fewer round trips to the API)
- `MemoryStorage` — holds the indexed nodes in memory

For production, replace `MemoryStorage` with `swiftide::integrations::qdrant::Qdrant` or `redis::RedisStorage` — the pipeline code is identical.

### Java comparison

```java
// LangChain4j equivalent
EmbeddingStoreIngestor.builder()
    .documentTransformer(new DocumentByParagraphSplitter(2048, 100))
    .embeddingModel(embeddingModel)
    .embeddingStore(embeddingStore)
    .build()
    .ingest(documents);
```

The swiftide pipeline is more composable (add/remove stages without changing the rest) and streams documents lazily — useful for large corpora where loading everything into memory first would be prohibitive.

---

## 19.3 Phase 2: RAG Query Pipeline

Once indexed, swiftide's query pipeline handles retrieval and synthesis:

```rust
use swiftide::query::{
    self, answers, query_transformers, response_transformers,
};

async fn rag_query(
    swiftide_client: SwiftideOpenAI,
    storage: MemoryStorage,
    question: &str,
) -> anyhow::Result<String> {
    let pipeline = query::Pipeline::default()
        .then_transform_query(
            query_transformers::GenerateSubquestion::from_client(
                swiftide_client.clone(),
            ),
        )
        .then_retrieve(storage)
        .then_transform_response(
            response_transformers::Summary::from_client(swiftide_client.clone()),
        )
        .then_answer(answers::Simple::from_client(swiftide_client));

    let result = pipeline.query(question).await?;
    Ok(result.answer().to_string())
}
```

Query pipeline stages:
1. `GenerateSubquestion` — decomposes complex questions into sub-queries for better recall
2. `then_retrieve` — finds the most relevant chunks via cosine similarity
3. `Summary` — condenses multiple retrieved chunks into a coherent context passage
4. `Simple` — passes the context + question to the LLM for a final answer

---

## 19.4 Phase 3: Rig Agent with Context Injection

The rig agent receives the retrieved context and generates the final answer. The simplest integration pattern passes context directly in the prompt:

```rust
use rig::{client::{CompletionClient, ProviderClient}, completion::Prompt, providers::openai};

async fn research_answer(
    client: &openai::Client,
    swiftide_client: SwiftideOpenAI,
    storage: MemoryStorage,
    question: &str,
) -> anyhow::Result<String> {
    // Retrieve context
    let context = rag_query(swiftide_client, storage, question).await?;

    // Ask rig agent with injected context
    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(
            "You are a research assistant. Answer questions based on the provided \
             document context. Cite specific facts. If the context doesn't contain \
             the answer, say so.",
        )
        .build();

    let prompt = format!(
        "Context from indexed documents:\n{context}\n\nQuestion: {question}"
    );

    Ok(agent.prompt(&prompt).await?)
}
```

### A note on rig + swiftide integration

rig and swiftide don't share types — swiftide's query result is a plain `String`; rig's agent takes a `&str`. They compose at the string boundary, which is low-tech but robust: no crate version coupling.

A more sophisticated integration would register `rag_query` as a rig tool (Chapter 4 pattern), so the agent can decide *when* to retrieve vs. answer from its own knowledge. The context-injection approach above is simpler and works well when you always want retrieval.

---

## 19.5 Phase 4: MCP Server Exposure

Wrapping the research agent as an MCP server lets any MCP client — Claude Desktop, another Rust service, a Spring AI application — use it without knowing it's Rust:

```rust
use rmcp::{
    ServerHandler,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool,
    transport::stdio,
    ServiceExt,
};
use serde::Deserialize;

#[derive(Debug, schemars::JsonSchema, Deserialize)]
struct ResearchParams {
    question: String,
    #[serde(default = "default_docs_path")]
    docs_path: String,
}

fn default_docs_path() -> String {
    "docs".to_string()
}

#[derive(Clone)]
struct ResearchServer {
    client: std::sync::Arc<openai::Client>,
}

#[rmcp::tool_router(server_handler)]
impl ResearchServer {
    #[tool(description = "Answer a research question using indexed documents.")]
    async fn research(
        &self,
        rmcp::Parameters(ResearchParams { question, docs_path }): rmcp::Parameters<ResearchParams>,
    ) -> String {
        // In a real server you'd share a pre-indexed storage;
        // for clarity, we index on each call here
        let swiftide_client = match SwiftideOpenAI::builder()
            .default_embed_model("text-embedding-3-small")
            .default_prompt_model("gpt-4o-mini")
            .build()
        {
            Ok(c) => c,
            Err(e) => return format!("Error building client: {e}"),
        };

        match index_documents(swiftide_client.clone(), &docs_path).await {
            Ok(storage) => {
                match research_answer(&self.client, swiftide_client, storage, &question).await {
                    Ok(answer) => answer,
                    Err(e) => format!("Error: {e}"),
                }
            }
            Err(e) => format!("Indexing error: {e}"),
        }
    }
}
```

The MCP server entry point:

```rust
let server = ResearchServer {
    client: std::sync::Arc::new(openai::Client::from_env()),
};
let service = server.serve(stdio()).await?;
service.waiting().await?;
```

When run with `cargo run`, this process speaks MCP over STDIO. Any MCP client can call `research` with a question and get a sourced answer from your document corpus.

---

## 19.6 Wiring It All Together

The full `main()` function runs in two modes based on an environment variable:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ch14_research_agent=debug".parse()?)
                .add_directive("info".parse()?),
        )
        .init();

    let rig_client = openai::Client::from_env();
    let swiftide_client = SwiftideOpenAI::builder()
        .default_embed_model("text-embedding-3-small")
        .default_prompt_model("gpt-4o-mini")
        .build()?;

    let docs_path = std::env::var("DOCS_PATH").unwrap_or_else(|_| "docs".to_string());
    let storage = index_documents(swiftide_client.clone(), &docs_path).await?;

    let questions = [
        "How does Rust's ownership system prevent memory bugs?",
        "What happens when a Rust value goes out of scope?",
    ];

    for question in questions {
        println!("\nQ: {question}");
        match research_answer(
            &rig_client,
            swiftide_client.clone(),
            storage.clone(),
            question,
        )
        .await
        {
            Ok(answer) => println!("A: {answer}"),
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    Ok(())
}
```

Run it:

```bash
cd code-examples
export OPENAI_API_KEY="sk-..."
export DOCS_PATH="./my-docs"
RUST_LOG=info cargo run -p ch19-capstone-research-agent
```

Expected output:

```
Q: How does Rust's ownership system prevent memory bugs?
A: Rust's ownership system prevents memory bugs by enforcing that each value
   has exactly one owner at compile time. When the owner goes out of scope,
   the value is automatically dropped, eliminating use-after-free bugs. The
   borrow checker ensures that references are always valid, preventing
   dangling pointers without a runtime garbage collector.

Q: What happens when a Rust value goes out of scope?
A: When a Rust value goes out of scope, Rust automatically calls its Drop
   implementation, freeing any owned memory. This is deterministic — it
   happens at a known point in the program, not non-deterministically like
   Java's GC.
```

---

## 19.7 Production Hardening

The capstone example above omits several things you'd add before shipping:

### Pre-index at startup, query at runtime

```rust
// Don't re-index on every query — index once, query many times
let storage = Arc::new(index_documents(swiftide_client.clone(), &docs_path).await?);

// Share across requests
let state = AppState { rig_client, swiftide_client, storage };
```

### Incremental indexing

```rust
// Track last-indexed timestamp; only ingest new/changed files
// swiftide doesn't have built-in change detection; use file mtimes:
let new_files: Vec<PathBuf> = find_files_modified_after(&docs_path, last_indexed)?;
if !new_files.is_empty() {
    index_subset(swiftide_client.clone(), new_files, storage.clone()).await?;
}
```

### Persistent vector store

```toml
# Replace MemoryStorage with Qdrant
swiftide-integrations = { version = "0.32", features = ["qdrant"] }
```

```rust
use swiftide::integrations::qdrant::Qdrant;
let storage = Qdrant::builder()
    .collection_name("research-docs")
    .build()?;
```

Data survives restarts. Multiple instances share the same index.

### Structured answers with citations

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ResearchAnswer {
    answer: String,
    confidence: f32,  // 0.0–1.0
    sources: Vec<String>,
}

// Use rig's Extractor instead of plain agent.prompt()
let extractor = client
    .extractor::<ResearchAnswer>(openai::GPT_4O_MINI)
    .build();
let result = extractor.extract(&prompt).await?;
```

---

## 19.8 Key Takeaways

- **Indexing phase**: swiftide `Pipeline` → `FileLoader` → `ChunkMarkdown` → `MetadataQAText` → `Embed` → `MemoryStorage`
- **Query phase**: swiftide `query::Pipeline` → `GenerateSubquestion` → `then_retrieve` → `Summary` → `Simple::answer()`
- **Agent phase**: rig agent with context injected via formatted prompt; or use rig `Extractor<M, T>` for structured answers with citations
- **MCP exposure**: wrap the agent in `#[tool_router(server_handler)]` + `server.serve(stdio())` — any MCP client can now call it
- **Pre-index at startup, share via `Arc`** — never re-index on each request
- **For persistence**: swap `MemoryStorage` for Qdrant or Redis — the pipeline code is identical
- **For production answers**: use `Extractor<ResearchAnswer>` with a `sources: Vec<String>` field

---

## What's Next

Chapter 20 builds the second capstone: a stateful multi-agent pipeline that processes a research brief through distinct agent roles (research, synthesis, review), with human approval gates and PostgreSQL-backed session persistence.

---

*→ Java reference: LangChain4j + Spring AI + Spring Boot end-to-end research agent (Ch 21)*
