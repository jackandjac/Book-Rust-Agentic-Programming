# Chapter 8: RAG — Retrieval-Augmented Generation

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` · `rig-derive = "0.1"` (for `#[derive(Embed)]`)  
> `tokio = "1"` · `anyhow = "1"` · `dotenvy = "0.15"`
>
> **Java reference:** LangChain4j `EmbeddingStoreIngestor` + `EmbeddingStoreContentRetriever`; Spring AI `VectorStore` + `QuestionAnswerAdvisor`

---

## What You'll Learn

- Why LLMs hallucinate and how RAG addresses it
- The `Embed` trait and `#[derive(Embed)]` — marking which fields to vectorize
- `EmbeddingsBuilder`: batching embedding API calls efficiently
- `InMemoryVectorStore` — rig's built-in vector store, no external database required
- `AgentBuilder::dynamic_context(n, index)` — attaching retrieval to an agent
- Chunking strategies: when to split documents and how
- What external vector stores look like (`rig-qdrant`)
- Build: a documentation Q&A bot

---

## 8.1 Why RAG?

LLMs are trained on a fixed corpus. They don't know about:
- Your internal documentation
- Data created after their training cutoff
- Proprietary knowledge specific to your domain

Without RAG, a question about your private codebase gets either a hallucinated answer or "I don't have access to that." With RAG, the application retrieves relevant chunks from your knowledge base and injects them into the prompt before generation — grounding the model in facts it was never trained on.

```
Without RAG:                    With RAG:
                                ┌──────────────────────┐
User query                      │ 1. Embed query       │
    │                           │ 2. Search vector DB  │
    ▼                           │ 3. Retrieve top-N    │
  LLM                           │    chunks            │
    │                           │ 4. Inject as context │
    ▼                           │ 5. Generate answer   │
  Answer (may hallucinate)      └──────────────────────┘
                                  Answer (grounded)
```

> **Java parallel:** This is exactly what LangChain4j's `EmbeddingStoreContentRetriever` combined with `AiServices` does, or Spring AI's `QuestionAnswerAdvisor`. Rig implements the same pipeline at the type level — the retrieval step is wired into `Agent` via `.dynamic_context()`.

---

## 8.2 The `Embed` Trait — Marking What to Vectorize

Every type you want to store in a vector store must implement the `Embed` trait:

```rust
// From rig::embeddings
pub trait Embed {
    fn embed(&self, embedder: &mut TextEmbedder) -> Result<(), EmbedError>;
}
```

The `embed` method pushes text strings into a `TextEmbedder`. Rig collects these strings and sends them to the embedding API.

### Manual Implementation

```rust
use rig::embeddings::{EmbedError, TextEmbedder};

struct DocChunk {
    title: String,
    source: String,
    content: String,
}

impl rig::Embed for DocChunk {
    fn embed(&self, embedder: &mut TextEmbedder) -> Result<(), EmbedError> {
        embedder.embed(self.content.clone());
        Ok(())
    }
}
```

Only `content` is embedded — `title` and `source` are metadata stored alongside the embedding but not used for similarity search.

### Derived Implementation with `#[derive(Embed)]`

For simple cases, use the `Embed` derive macro from `rig-derive`:

```rust
use rig_derive::Embed;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Embed, Serialize)]
struct DocChunk {
    title: String,
    source: String,
    #[embed]   // only this field is embedded
    content: String,
}
```

The `#[embed]` annotation marks exactly which field(s) the macro should embed. Multiple `#[embed]` fields are all sent to the embedding model — useful when you want to embed both a title and body for better retrieval recall.

`Serialize` is required because rig needs to serialize documents when storing them. `Clone` is required because `InMemoryVectorStore` may need to clone documents during indexing.

> **Java parallel:** In LangChain4j, you implement `TextSegment` content — the text that gets embedded is the `TextSegment.text()` value. Rig's `#[embed]` field annotation is equivalent to which text you'd put into `TextSegment.from(text)`.

---

## 8.3 Embedding Documents with `EmbeddingsBuilder`

`EmbeddingsBuilder` batches embedding requests for efficiency — instead of one API call per document, it sends all documents in as few calls as the model allows.

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::embeddings::EmbeddingsBuilder;
use rig::providers::openai;

let client = openai::Client::from_env();

// Create the embedding model.
// TEXT_EMBEDDING_3_SMALL: 1536 dimensions, fast, cost-effective for retrieval.
// TEXT_EMBEDDING_3_LARGE: 3072 dimensions, higher quality for precision tasks.
// TEXT_EMBEDDING_ADA_002: legacy, 1536 dimensions, still widely used.
let embedding_model = client.embedding_model(openai::TEXT_EMBEDDING_3_SMALL);

// Build embeddings for a batch of documents.
// .documents() returns Result — it validates that documents implement Embed.
// .build().await sends the API request and returns the embedded documents.
let embeddings = EmbeddingsBuilder::new(embedding_model.clone())
    .documents(my_docs)?   // my_docs: Vec<DocChunk>
    .build()
    .await?;

// embeddings: Vec<(DocChunk, OneOrMany<Embedding>)>
// Each tuple contains the original document and its embedding vector(s).
```

### Required Imports

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::embeddings::EmbeddingsBuilder;
use rig::providers::openai;
use rig_derive::Embed;  // for #[derive(Embed)]
```

`ProviderClient` is required for `openai::Client::from_env()`. `CompletionClient` is required for `.agent()` later. Note that `.embedding_model()` is provided by `EmbeddingsClient`, which is implemented by `openai::Client` — you do not need to import `EmbeddingsClient` explicitly; the method is in scope via the concrete client type.

> **Java parallel:** `EmbeddingsBuilder` maps to LangChain4j's `EmbeddingModel.embedAll(List<TextSegment>)` — batch embedding with a single API call. Spring AI's `EmbeddingModel.embedForResponse(List<String>)` is equivalent.

---

## 8.4 Building the Vector Store

Once you have embeddings, create the in-memory vector store:

```rust
use rig::vector_store::in_memory_store::InMemoryVectorStore;

// from_documents consumes Vec<(DocChunk, OneOrMany<Embedding>)>
// and builds an internal HashMap of id → (document, embedding).
let store = InMemoryVectorStore::from_documents(embeddings);

// .index(model) wraps the store with the embedding model.
// At query time, the index embeds the query string using this model
// and performs cosine similarity search against stored embeddings.
let index = store.index(embedding_model);
```

`InMemoryVectorStore::from_documents` assigns auto-generated IDs. If you need stable IDs (for retrieval by ID later), use `from_documents_with_id_f`:

```rust
let store = InMemoryVectorStore::from_documents_with_id_f(
    embeddings,
    |doc| doc.source.clone(),   // derive ID from source field
);
```

### Manual Search

You can query the index directly without going through an agent:

```rust
use rig::vector_store::{VectorStoreIndex, request::VectorSearchRequest};

let results = index
    .top_n::<DocChunk>(
        VectorSearchRequest::builder()
            .query("how does ownership work")
            .samples(3)          // return top-3 results
            .build(),
    )
    .await?;

// results: Vec<(f64, String, DocChunk)>
// Each tuple: (similarity_score, document_id, document)
for (score, id, doc) in results {
    println!("[{score:.3}] {id}: {}", doc.title);
}
```

> **Java parallel:** This is LangChain4j's `EmbeddingStore.findRelevant(embedding, maxResults)` or Spring AI's `VectorStore.similaritySearch(SearchRequest)`. The rig call is fully typed — the returned `DocChunk` is your concrete type, not a generic `TextSegment`.

---

## 8.5 Wiring RAG into an Agent

The key method is `AgentBuilder::dynamic_context(n, index)`:

```rust
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;

let rag_agent = client
    .agent(openai::GPT_4O_MINI)
    .preamble(
        "You are a documentation assistant. \
         Answer questions using only the provided context. \
         If the context does not contain the answer, say so explicitly.",
    )
    .dynamic_context(2, index)   // retrieve top-2 chunks per query
    .build();

// Each call to .prompt() or .chat() automatically:
//   1. Embeds the query string
//   2. Retrieves the top-2 most similar DocChunks from the index
//   3. Serializes those chunks into the agent's context
//   4. Sends the enriched prompt to the LLM
let answer = rag_agent.prompt("How does ownership work in Rust?").await?;
println!("{answer}");
```

The `n` in `dynamic_context(n, index)` is the number of chunks to retrieve per query. More chunks = more context = better recall but higher token cost. Start with 2–5 and tune based on your document size and budget.

### Multiple Vector Stores

You can attach multiple vector stores for different knowledge domains:

```rust
let agent = client
    .agent(openai::GPT_4O)
    .dynamic_context(2, rust_docs_index)
    .dynamic_context(2, company_policy_index)
    .dynamic_context(1, product_catalog_index)
    .build();
```

Each store is queried independently; all retrieved chunks are injected into the context. The total retrieved chunks in this example: up to 5 (2 + 2 + 1).

---

## 8.6 Chunking Strategies

Embedding entire documents produces poor retrieval. A 50-page PDF embedded as one vector averages out all its topics — the resulting vector matches nothing well. Instead, split documents into chunks before embedding.

### Chunk Size Guidelines

| Content type | Recommended chunk size | Notes |
|---|---|---|
| API documentation | 200–400 tokens | One concept per chunk |
| Prose/narrative | 400–800 tokens | Sentence-boundary splitting |
| Code | One function/class | Preserve syntactic units |
| FAQ | One Q&A pair | Natural semantic unit |

### Manual Chunking in Rust

Rig doesn't include a text splitter in `rig-core`. For simple cases, split on paragraph boundaries:

```rust
fn chunk_by_paragraph(text: &str, source: &str) -> Vec<DocChunk> {
    text.split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .enumerate()
        .map(|(i, para)| DocChunk {
            title: format!("chunk-{i}"),
            source: source.to_string(),
            content: para.trim().to_string(),
        })
        .collect()
}
```

For token-aware splitting (needed when chunks might exceed embedding model context limits), the `tiktoken-rs` crate provides OpenAI's tokenizer:

```rust
// tiktoken-rs = "0.5"  (add to Cargo.toml)
use tiktoken_rs::cl100k_base;

fn chunk_by_tokens(text: &str, max_tokens: usize) -> Vec<String> {
    let bpe = cl100k_base().unwrap();
    let tokens = bpe.encode_with_special_tokens(text);
    tokens
        .chunks(max_tokens)
        .map(|chunk| bpe.decode(chunk.to_vec()).unwrap())
        .collect()
}
```

Chapter 9 covers Swiftide, which provides a full streaming document processing pipeline including semantic-aware chunking.

### Loading from Files

Rig provides basic file loaders in `rig::loaders`:

```rust
use rig::loaders::FileLoader;
use std::path::Path;

// Load all .md files from a directory — one String per file
let docs: Vec<String> = FileLoader::with_glob("docs/**/*.md")?
    .read()
    .try_collect()
    .await?;
```

For PDFs, enable the `pdf` feature in `rig-core`:

```toml
# Cargo.toml
rig-core = { version = "0.37", features = ["pdf"] }
```

```rust
use rig::loaders::PdfFileLoader;

// Splits the PDF into pages — each page is a separate String
let pages: Vec<String> = PdfFileLoader::with_glob("manuals/**/*.pdf")?
    .read_with_path()
    .try_collect()
    .await?;
```

There is no built-in web loader in `rig-core`. For web content, use `reqwest` + `scraper` to fetch and parse HTML, then pass the text to your chunking function.

---

## 8.7 External Vector Stores

`InMemoryVectorStore` is ideal for prototypes and small corpora (thousands of documents). For production use with millions of documents, you need a dedicated vector database.

### `rig-qdrant` — Qdrant Integration

The official rig integration with [Qdrant](https://qdrant.tech/) is provided by the `rig-qdrant` crate (version 0.2.6, 24k downloads):

```toml
# Cargo.toml
rig-qdrant = "0.2"
qdrant-client = "1.18"
```

```rust
use qdrant_client::Qdrant;
use rig_qdrant::QdrantVectorStore;

let qdrant = Qdrant::from_url("http://localhost:6334").build()?;

// Create a Qdrant-backed vector store
let store = QdrantVectorStore::new(
    qdrant,
    embedding_model.clone(),
    "my_collection",     // Qdrant collection name
);

// Attach to agent — same API as InMemoryVectorStore
let agent = client
    .agent(openai::GPT_4O_MINI)
    .dynamic_context(3, store)
    .build();
```

The API is identical to `InMemoryVectorStore` from the agent's perspective — `dynamic_context` accepts any type implementing `VectorStoreIndexDyn`. This is the power of rig's trait-based design: swap the store without changing the agent code.

> Note: Qdrant insertion (indexing documents) uses `rig-qdrant`'s own API — consult the `rig-qdrant` docs for the `insert` or `upsert` methods.

### Other Community Stores

The rig ecosystem also has integrations for:
- **MongoDB Atlas Vector Search** — `rig-mongodb`
- **LanceDB** — `rig-lancedb`
- **SQLite-VSS** — via `rig-sqlite`

All implement the same `VectorStoreIndexDyn` trait, so the agent-side code stays unchanged regardless of which backend you use.

> **Java parallel:** In Spring AI, you swap vector stores by changing the `VectorStore` bean — `SimpleVectorStore` (in-memory) → `QdrantVectorStore` → `RedisVectorStore`. The application code using `QuestionAnswerAdvisor` doesn't change. LangChain4j's `EmbeddingStore` interface provides the same abstraction.

---

## 8.8 Hands-On: Documentation Q&A Bot

The complete runnable example:

```rust
// code-examples/ch08-rag/src/main.rs
use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::embeddings::EmbeddingsBuilder;
use rig::providers::openai;
use rig::vector_store::in_memory_store::InMemoryVectorStore;
use rig_derive::Embed;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Embed, Serialize)]
struct DocChunk {
    title: String,
    source: String,
    #[embed]
    content: String,
}

fn sample_corpus() -> Vec<DocChunk> {
    vec![
        DocChunk {
            title: "Ownership".into(),
            source: "rust-book/ch04".into(),
            content: "Ownership is Rust's most unique feature. Each value has an owner; \
                      there can only be one owner at a time; when the owner goes out of \
                      scope, the value is dropped.".into(),
        },
        DocChunk {
            title: "Borrowing".into(),
            source: "rust-book/ch04".into(),
            content: "References allow you to refer to a value without taking ownership. \
                      At any given time, you can have either one mutable reference or any \
                      number of immutable references. References must always be valid.".into(),
        },
        DocChunk {
            title: "Result".into(),
            source: "rust-book/ch09".into(),
            content: "Result<T, E> is used for recoverable errors. Ok(T) contains a \
                      success value; Err(E) contains an error. The ? operator propagates \
                      errors automatically.".into(),
        },
        DocChunk {
            title: "Traits".into(),
            source: "rust-book/ch10".into(),
            content: "A trait defines functionality a type must provide. Traits are \
                      similar to Java interfaces but more powerful: they support default \
                      implementations and can be used as generic bounds.".into(),
        },
        DocChunk {
            title: "Async/Await".into(),
            source: "rust-book/ch17".into(),
            content: "Async functions return a Future. .await suspends the current task \
                      until the Future resolves. Rust Futures are lazy — they do nothing \
                      until awaited, unlike Java's CompletableFuture.".into(),
        },
    ]
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = openai::Client::from_env();
    let embedding_model = client.embedding_model(openai::TEXT_EMBEDDING_3_SMALL);

    println!("Embedding {} documents...", sample_corpus().len());
    let embeddings = EmbeddingsBuilder::new(embedding_model.clone())
        .documents(sample_corpus())?
        .build()
        .await?;

    let store = InMemoryVectorStore::from_documents(embeddings);
    let index = store.index(embedding_model);

    let rag_agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(
            "You are a Rust documentation assistant. Answer using only the provided \
             context. If the context does not contain the answer, say so.",
        )
        .dynamic_context(2, index)
        .build();

    let questions = [
        "How does Rust prevent dangling references?",
        "How do I handle errors in Rust?",
        "What makes Rust Futures different from Java's CompletableFuture?",
    ];

    for question in &questions {
        println!("\nQ: {question}");
        let answer = rag_agent.prompt(question).await?;
        println!("A: {answer}");
    }

    Ok(())
}
```

### Running the Example

```bash
cd code-examples
export OPENAI_API_KEY=sk-...
cargo run -p ch08-rag
```

Expected output (abbreviated):
```
Embedding 5 documents...

Q: How does Rust prevent dangling references?
A: Rust's borrowing rules prevent dangling references by ensuring that references
   must always be valid. At any given time, you can have either one mutable
   reference or any number of immutable references...

Q: How do I handle errors in Rust?
A: Use Result<T, E> for recoverable errors. The Ok(T) variant contains a success
   value; Err(E) contains an error value. The ? operator propagates errors
   automatically...
```

---

## 8.9 RAG Quality Considerations

Getting RAG to work is easy. Getting it to work *well* requires attention to:

### Retrieval Quality

| Problem | Cause | Fix |
|---|---|---|
| Wrong chunks retrieved | Chunks too large | Smaller chunks (200–400 tokens) |
| Relevant chunks missed | Chunks too small | Larger chunks with overlap |
| Irrelevant answers | `n` too low | Increase `dynamic_context(n, ...)` |
| Context too long | `n` too high | Decrease n; use better chunking |

### Embedding Model Choice

- `TEXT_EMBEDDING_3_SMALL` (1536 dims): fast, cheap, good for most retrieval tasks
- `TEXT_EMBEDDING_3_LARGE` (3072 dims): higher quality, 6× the cost — use when precision matters
- `TEXT_EMBEDDING_ADA_002`: legacy, still reliable, lower cost than LARGE

The embedding model used at **indexing time** and **query time** must be the same. Mixing models produces incorrect similarity scores.

### Chunking Overlap

Real chunking strategies use overlap — each chunk shares some content with the adjacent chunk. This prevents relevant sentences from being split across chunk boundaries:

```rust
fn chunk_with_overlap(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let step = chunk_size - overlap;
    (0..)
        .map(|i| i * step)
        .take_while(|&start| start < words.len())
        .map(|start| {
            words[start..usize::min(start + chunk_size, words.len())].join(" ")
        })
        .collect()
}
```

A 20% overlap is a common starting point.

---

## Key Takeaways

- RAG grounds LLM responses in your documents by retrieving relevant chunks before generation — preventing hallucination on private or recent data.
- `#[derive(Embed)]` from `rig-derive` generates the `Embed` implementation; `#[embed]` marks which field(s) to vectorize.
- `EmbeddingsBuilder::new(model).documents(docs)?.build().await?` — batch-embed documents; returns `Vec<(T, OneOrMany<Embedding>)>`.
- `InMemoryVectorStore::from_documents(embeddings).index(model)` — build an in-memory searchable index.
- `agent.dynamic_context(n, index)` — wire retrieval into the agent; top-n chunks are retrieved and injected automatically on every `.prompt()` call.
- The embedding model at indexing and query time must match.
- For production: swap `InMemoryVectorStore` for `rig-qdrant` or another community store — the agent-side code doesn't change.
- Chunk size matters: 200–400 tokens per chunk with ~20% overlap is a good starting point for most text.

---

## Further Reading

- [rig-core vector_store docs](https://docs.rs/rig-core/latest/rig/vector_store/) — `InMemoryVectorStore`, `VectorStoreIndex`, `VectorSearchRequest`
- [rig-core embeddings docs](https://docs.rs/rig-core/latest/rig/embeddings/) — `EmbeddingsBuilder`, `Embed` trait
- [rig-qdrant](https://crates.io/crates/rig-qdrant) — Qdrant integration (v0.2.6)
- [Qdrant documentation](https://qdrant.tech/documentation/) — Vector database setup and configuration
- [LangChain4j RAG tutorial](https://docs.langchain4j.dev/tutorials/rag) — Java reference: `EmbeddingStoreIngestor`, `EmbeddingStoreContentRetriever`
- [Spring AI VectorStore](https://docs.spring.io/spring-ai/reference/api/vectordbs.html) — Java reference: `VectorStore`, `QuestionAnswerAdvisor`

---

*Next: Chapter 9 — Swiftide: Streaming Indexing Pipelines*
