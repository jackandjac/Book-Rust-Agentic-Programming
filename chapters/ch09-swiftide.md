# Chapter 9: Swiftide — Streaming Indexing Pipelines

> **Framework versions in this chapter:**  
> `swiftide = "0.32"` (81k downloads) · `tokio = "1"` · `anyhow = "1"` · `dotenvy = "0.15"`
>
> **Java reference:** `EmbeddingStoreIngestor` (LangChain4j), `DocumentReader → DocumentTransformer → VectorStore` pipeline (Spring AI ETL)

In Chapter 8 you built a RAG system the direct way: embed a list of documents, store them in an in-memory vector store, and wire the index to a rig agent. That approach works well for tens of documents. When you need to index thousands of files from a GitHub repository, a documentation site, or a knowledge base — and keep the index fresh — you need a different tool.

Enter **Swiftide**: a streaming-first indexing pipeline library for Rust. Where Chapter 8's code was imperative ("embed these six documents"), Swiftide is declarative ("files go in, embedded chunks come out, here's how each stage transforms them"). The pipeline runs as a stream — nodes flow through stages concurrently, and each stage can operate in parallel — so it scales to large corpora without loading everything into memory at once.

---

## 9.1 Swiftide vs Rig: Different Jobs

Before writing any code, it's worth being explicit about what swiftide is and what it isn't.

| Concern | rig-core | swiftide |
|---------|---------|---------|
| LLM completion | ✅ Core feature | ❌ Delegates to integrations |
| Tool calling | ✅ `#[rig_tool]`, `Agent` | ❌ No concept |
| Streaming indexing | 🔶 Manual, one-off | ✅ Core feature |
| Chunking strategies | 🔶 Manual | ✅ Built-in (`ChunkMarkdown`, `ChunkCode`, ...) |
| Metadata enrichment | 🔶 Manual | ✅ `MetadataQAText`, `MetadataKeywords`, ... |
| Query pipeline | 🔶 `dynamic_context` only | ✅ Full query-transform-retrieve-answer chain |
| Production observability | 🔶 Via `tracing` | ✅ `tracing` + experimental `langfuse` |

**Rule of thumb:** use swiftide to build and maintain the index; use rig to query it and generate answers. The two libraries complement rather than replace each other.

### Java equivalent

In LangChain4j, the equivalent of swiftide's indexing pipeline is `EmbeddingStoreIngestor`:

```java
// LangChain4j
EmbeddingStoreIngestor ingestor = EmbeddingStoreIngestor.builder()
    .documentSplitter(DocumentSplitters.recursive(300, 30))
    .embeddingModel(embeddingModel)
    .embeddingStore(store)
    .build();

ingestor.ingest(documents);
```

Spring AI's equivalent is its ETL pipeline: `DocumentReader → List<DocumentTransformer> → VectorStore`. Swiftide is the Rust answer to both, with streaming concurrency baked in from the start.

---

## 9.2 Pipeline Architecture

A Swiftide indexing pipeline has three conceptual zones:

```
┌─────────────┐    ┌────────────────────────────────┐    ┌──────────────┐
│   LOAD      │───▶│            TRANSFORM           │───▶│   PERSIST    │
│             │    │                                │    │              │
│ FileLoader  │    │ chunk → enrich → embed         │    │ MemoryStorage│
│ GitLoader   │    │                                │    │ Qdrant       │
│ S3Loader    │    │ (each stage is a stream node)  │    │ Redis        │
└─────────────┘    └────────────────────────────────┘    └──────────────┘
```

Every stage implements the `Transformer` or `BatchableTransformer` trait. The pipeline connects them with Tokio channels under the hood: each stage reads from its input channel and writes to its output channel. This means:

- **Memory efficiency**: only a bounded buffer of nodes lives in memory at any time.
- **Concurrency**: stages run in parallel on the Tokio thread pool.
- **Backpressure**: if a downstream stage is slow (e.g., an embedding API rate limit), upstream stages pause naturally.

The indexing pipeline builder method chain mirrors the stages:

```rust
indexing::Pipeline::from_loader(loader)   // LOAD
    .then_chunk(chunker)                  // split into chunks
    .then(transformer)                    // enrich each chunk
    .then_in_batch(batch_transformer)     // embed in batches
    .then_store_with(storage)             // PERSIST
    .run()
    .await?
```

---

## 9.3 Minimal Working Example

The quickest way to understand swiftide is to see it run. This example indexes three Markdown files from a local directory and stores the embedded chunks in memory.

### Cargo.toml

```toml
[dependencies]
swiftide = { version = "0.32", features = ["openai"] }
tokio   = { version = "1", features = ["full"] }
anyhow  = "1"
dotenvy = "0.15"
```

Note the `features = ["openai"]` flag — swiftide's integrations are gated behind feature flags so you only pull in the dependencies you need.

### The pipeline

```rust
use anyhow::Result;
use swiftide::{
    indexing::{
        self,
        loaders::FileLoader,
        persist::MemoryStorage,
        transformers::{ChunkMarkdown, Embed},
    },
    integrations::openai::OpenAI,
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let openai_client = OpenAI::builder()
        .default_embed_model("text-embedding-3-small")
        .default_prompt_model("gpt-4o-mini")
        .build()?;

    let storage = MemoryStorage::default();

    indexing::Pipeline::from_loader(
        FileLoader::new("docs").with_extensions(&["md"]),
    )
    .then_chunk(ChunkMarkdown::from_chunk_range(10..2048))
    .then_in_batch(Embed::new(openai_client.clone()).with_batch_size(10))
    .then_store_with(storage.clone())
    .run()
    .await?;

    println!("Done — {} chunks indexed", storage.len());
    Ok(())
}
```

Compare the same task in LangChain4j:

```java
// LangChain4j equivalent
List<Document> docs = FileSystemDocumentLoader.loadDocuments("docs");
DocumentSplitter splitter = DocumentSplitters.recursive(2048, 100);
EmbeddingStoreIngestor.builder()
    .documentSplitter(splitter)
    .embeddingModel(embeddingModel)
    .embeddingStore(store)
    .build()
    .ingest(docs);
```

The structural similarity is clear. Key difference: the LangChain4j version is sequential (each document is split, then all splits are embedded, then all are stored); swiftide's version is streaming and concurrent throughout.

---

## 9.4 Chunking Strategies

Chunking is the most consequential parameter in any RAG system. Chunks too large overwhelm the LLM context; chunks too small lose semantic coherence.

### `ChunkMarkdown`

```rust
// Minimum 10 chars (skip headings/blanks), maximum 2048 chars
.then_chunk(ChunkMarkdown::from_chunk_range(10..2048))
```

`ChunkMarkdown` understands Markdown structure — it tries to break at heading boundaries and paragraph boundaries before resorting to character-count splitting. This produces more semantically coherent chunks than a naive character splitter.

### `ChunkCode`

For source code, swiftide provides `ChunkCode` which uses tree-sitter to parse the AST and chunk at function/class boundaries:

```rust
use swiftide::indexing::transformers::ChunkCode;

.then_chunk(ChunkCode::try_for_language("rust")?)
```

This produces chunks that map to natural code units — a function, an `impl` block, a test — rather than arbitrary character windows.

### `ChunkText` (plain text)

For plain text without structure, `ChunkText` falls back to character-count splitting with overlap:

```rust
use swiftide::indexing::transformers::ChunkText;

// 512-char chunks with 64-char overlap
.then_chunk(ChunkText::from_chunk_range(512..512).with_overlap(64))
```

### Java comparison

LangChain4j's `DocumentSplitters.recursive(chunkSize, overlap)` is closest to `ChunkText`. Spring AI offers similar `TokenTextSplitter` and `CharacterTextSplitter`. Neither has a Markdown-aware or AST-aware equivalent in the standard library; that awareness is one of swiftide's differentiators.

---

## 9.5 Metadata Enrichment

Plain chunks often lack enough context for precise retrieval. The query "How does Rust prevent dangling pointers?" might match a chunk about ownership perfectly — but only if the chunk contains those exact words. In practice, documentation is written for readers, not search engines.

Swiftide's metadata transformers solve this with **synthetic enrichment**: they call an LLM to generate additional metadata — questions the chunk answers, keywords it covers, a one-sentence summary — and store those in the node's metadata map. At query time, the search runs against this richer representation.

### `MetadataQAText`

```rust
use swiftide::indexing::transformers::MetadataQAText;

.then(MetadataQAText::new(openai_client.clone()))
```

For each chunk, `MetadataQAText` calls the LLM and asks: *"What questions does this text answer?"* The generated questions are stored as metadata and are included in the embedding — dramatically improving recall for conversational queries.

### `MetadataKeywords`

```rust
use swiftide::indexing::transformers::MetadataKeywords;

.then(MetadataKeywords::from_client(openai_client.clone()))
```

Generates a list of keywords from the chunk. Useful when you know users will search with technical terms.

### `MetadataTitle`

```rust
use swiftide::indexing::transformers::MetadataTitle;

// Derives a short title for the chunk from its content
.then(MetadataTitle::from_client(openai_client.clone()))
```

Enrichment transformers call the LLM once per chunk, so they add latency and token cost. The usual trade-off: skip enrichment for high-volume indexing of well-structured content; add it for sparse, narrative, or conversational corpora where retrieval recall is paramount.

---

## 9.6 Storage Backends

### In-memory (development)

```rust
use swiftide::indexing::persist::MemoryStorage;

let storage = MemoryStorage::default();
// ...pipeline...
.then_store_with(storage.clone())

// Later, in the query pipeline:
.then_retrieve(storage.clone())
```

`MemoryStorage` is the fastest option and requires no external service. It disappears when the process exits, making it ideal for development, prototyping, and testing.

### Qdrant (production)

Swiftide has a built-in Qdrant integration:

```toml
swiftide = { version = "0.32", features = ["openai", "qdrant"] }
qdrant-client = "1"
```

```rust
use swiftide::integrations::qdrant::Qdrant;

let qdrant = Qdrant::builder()
    .collection_name("rust-docs")
    .vector_size(1536)  // text-embedding-3-small output dimension
    .build()?;

// In the indexing pipeline:
.then_store_with(qdrant.clone())

// In the query pipeline:
.then_retrieve(qdrant.clone())
```

The Qdrant integration creates the collection automatically if it does not exist, respecting the `vector_size` you specify. For `text-embedding-3-small` the correct size is **1536**; for `text-embedding-3-large` it is **3072**.

### Redis

```toml
swiftide = { version = "0.32", features = ["openai", "redis"] }
```

```rust
use swiftide::integrations::redis::Redis;

let redis = Redis::try_from_url("redis://localhost:6379", "rust-docs")?;
.then_store_with(redis.clone())
```

Redis suits use-cases where you need fast random-access retrieval alongside caching and session storage — sharing the same Redis instance for multiple concerns.

---

## 9.7 The Query Pipeline

Swiftide's query pipeline mirrors its indexing pipeline, but in reverse: raw question → transform → retrieve → synthesise → answer.

```rust
use swiftide::query::{self, answers, query_transformers, response_transformers};

let pipeline = query::Pipeline::default()
    // Expand the query to improve recall
    .then_transform_query(
        query_transformers::GenerateSubquestion::from_client(openai_client.clone()),
    )
    // Retrieve similar chunks from storage
    .then_retrieve(storage.clone())
    // Summarise the retrieved chunks into a coherent context
    .then_transform_response(
        response_transformers::Summary::from_client(openai_client.clone()),
    )
    // Generate the final answer
    .then_answer(answers::Simple::from_client(openai_client.clone()));

let result = pipeline.query("How does Rust prevent use-after-free?").await?;
println!("{}", result.answer());
```

### Query transformers

| Transformer | Effect |
|-------------|--------|
| `GenerateSubquestion` | Generates one sub-question from the original query to improve recall |
| `Embed` | Embeds the (transformed) query for vector search |

### Response transformers

| Transformer | Effect |
|-------------|--------|
| `Summary` | Asks LLM to summarise retrieved chunks into a single context paragraph |

### Answers

| Answer | Effect |
|--------|--------|
| `Simple` | Sends the context to the LLM with the original question, returns the response |

For a comparison: rig's `dynamic_context(n, index)` is roughly equivalent to `then_retrieve(storage)` + `then_answer(Simple)` in a single step. Swiftide's pipeline gives you explicit control over each step, at the cost of more configuration.

---

## 9.8 Hands-On: Indexing a Markdown Knowledge Base

The complete example in `code-examples/ch09-swiftide/` indexes three Markdown files from the `docs/` directory, enriches each chunk with synthetic Q&A metadata, and answers a question about the corpus.

```
ch09-swiftide/
├── Cargo.toml
├── docs/
│   ├── ownership.md
│   ├── async.md
│   └── error-handling.md
└── src/
    └── main.rs
```

### Running it

```bash
cd code-examples
export OPENAI_API_KEY="sk-..."
cargo run -p ch09-swiftide
```

Expected output (truncated):

```
Indexing docs/...
Indexing complete.

Question: How does Rust prevent use-after-free bugs?
Answer:
Rust prevents use-after-free bugs through its ownership system. Each value
has exactly one owner; when the owner goes out of scope, the value is dropped.
References are checked at compile time: you can have either one mutable
reference or any number of immutable references — never both simultaneously.
This eliminates the class of bugs where a reference outlives the value it
points to.
```

### Step-by-step walkthrough

**Step 1 — Build the OpenAI integration:**

```rust
let openai_client = OpenAI::builder()
    .default_embed_model("text-embedding-3-small")
    .default_prompt_model("gpt-4o-mini")
    .build()?;
```

Note the raw string model names — swiftide does not use typed constants like rig's `openai::GPT_4O_MINI`. The builder accepts any model name; an invalid name will fail at runtime when the first API call is made.

**Step 2 — Shared storage:**

```rust
let storage = MemoryStorage::default();
```

The `storage` handle is `Clone`, so you pass a `.clone()` to each pipeline that needs it. Under the hood, all clones share the same `Arc<RwLock<Vec<Node>>>` — data written by the indexing pipeline is immediately visible to the query pipeline.

**Step 3 — Indexing pipeline:**

```rust
indexing::Pipeline::from_loader(
    FileLoader::new("docs").with_extensions(&["md"]),
)
.then_chunk(ChunkMarkdown::from_chunk_range(10..2048))
.then(MetadataQAText::new(openai_client.clone()))
.then_in_batch(Embed::new(openai_client.clone()).with_batch_size(10))
.then_store_with(storage.clone())
.run()
.await?;
```

The stages run concurrently: while one chunk is being enriched by `MetadataQAText`, the previous chunk may already be in the batch embedder, and the chunk before that may already be persisted.

**Step 4 — Query pipeline:**

```rust
let pipeline = query::Pipeline::default()
    .then_transform_query(
        query_transformers::GenerateSubquestion::from_client(openai_client.clone()),
    )
    .then_retrieve(storage.clone())
    .then_transform_response(
        response_transformers::Summary::from_client(openai_client.clone()),
    )
    .then_answer(answers::Simple::from_client(openai_client.clone()));

let result = pipeline.query("How does Rust prevent use-after-free bugs?").await?;
```

The query is embedded (inside `then_retrieve`), compared against stored embeddings, and the top-k most similar chunks are retrieved. `Summary` condenses them; `Simple` generates the final answer.

---

## 9.9 Observability

Swiftide uses the `tracing` crate for structured logging, consistent with the rest of the Rust ecosystem. Add a subscriber to see pipeline progress:

```rust
tracing_subscriber::fmt()
    .with_env_filter("swiftide=debug,info")
    .init();
```

With `swiftide=debug` you see each chunk as it passes through each stage — useful for diagnosing slow transformers or embedding errors.

### Langfuse (experimental)

Swiftide has an experimental integration with [Langfuse](https://langfuse.com), an open-source LLM observability platform:

```toml
swiftide = { version = "0.32", features = ["openai", "langfuse"] }
```

```rust
use swiftide::integrations::langfuse::Langfuse;

let langfuse = Langfuse::default(); // reads LANGFUSE_* env vars
indexing::Pipeline::from_loader(loader)
    // ...
    .with_observability(langfuse)
    .run()
    .await?;
```

This records spans for each pipeline stage in your Langfuse dashboard. The integration is marked experimental in Swiftide 0.32 — the API may change in future versions.

There is currently no native OpenTelemetry exporter in swiftide. If you need OTel traces, the `tracing-opentelemetry` crate can bridge `tracing` spans to an OTel collector, but pipeline-level spans (one per stage, not one per chunk) require manual instrumentation.

---

## 9.10 Key Takeaways

- **Swiftide complements rig** — use swiftide for building and maintaining the index at scale; use rig for agent-driven retrieval and generation.
- **The pipeline is a stream** — nodes flow through stages concurrently with backpressure; memory usage is bounded even for large corpora.
- **`ChunkMarkdown` and `ChunkCode`** split at semantic boundaries, not just character counts — a significant quality improvement over naive splitters.
- **Metadata enrichment** (`MetadataQAText`, `MetadataKeywords`) improves recall by indexing synthetic questions and keywords alongside raw text.
- **Model names are raw strings** in swiftide (e.g., `"text-embedding-3-small"`), unlike rig's typed constants. Invalid names fail at runtime.
- **`MemoryStorage`** lives at `swiftide::indexing::persist::MemoryStorage` — not in `integrations`.
- **`features = ["openai"]`** must be enabled explicitly — swiftide's integrations are all feature-gated.
- **Observability**: `tracing` for structured logs; `langfuse` feature for experimental dashboard integration; no native OTel.

---

## What's Next

This chapter focused on building the index. Chapter 10 turns to memory: how agents maintain context across multiple turns and across process restarts. Memory in rig ranges from simple sliding-window conversation history to vector-backed long-term recall — and combining them with a Swiftide index gives you a full-featured, persistent AI assistant.

---

*→ Java reference: "DocumentTransformer / EmbeddingStore ingestion pipeline" (LangChain4j); Spring AI ETL pipeline (`DocumentReader`, `DocumentTransformer`, `VectorStore`)*
