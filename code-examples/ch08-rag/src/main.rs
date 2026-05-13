// Chapter 8: RAG — Retrieval-Augmented Generation
// See chapters/ch08-rag.md for the full explanation.
//
// Run: cargo run -p ch08-rag
// Requires: OPENAI_API_KEY env var (or .env file)
//
// This example builds a simple documentation Q&A bot using rig's built-in
// in-memory vector store. Documents are embedded at startup; queries retrieve
// the most relevant chunks before generation.

use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::embeddings::EmbeddingsBuilder;
use rig::providers::openai;
use rig::vector_store::in_memory_store::InMemoryVectorStore;
use rig_derive::Embed;
use serde::{Deserialize, Serialize};

// ── Document type ─────────────────────────────────────────────────────────────

/// A documentation snippet with metadata.
///
/// `#[derive(Embed)]` from rig-derive generates the `Embed` implementation.
/// The `#[embed]` annotation marks the field whose text is sent to the
/// embedding model — only `content` is embedded here, not `title` or `source`.
#[derive(Clone, Debug, Deserialize, Embed, Serialize)]
struct DocChunk {
    title: String,
    source: String,
    #[embed]
    content: String,
}

// ── Sample corpus ─────────────────────────────────────────────────────────────

/// Small in-process corpus — in a real application, these would be loaded from
/// files, a database, or a document loader.
fn sample_corpus() -> Vec<DocChunk> {
    vec![
        DocChunk {
            title: "Ownership".into(),
            source: "rust-book/ch04".into(),
            content: "Ownership is Rust's most unique feature. It enables Rust to make \
                      memory safety guarantees without a garbage collector. Each value has \
                      an owner; there can only be one owner at a time; when the owner goes \
                      out of scope, the value is dropped."
                .into(),
        },
        DocChunk {
            title: "Borrowing".into(),
            source: "rust-book/ch04".into(),
            content: "References allow you to refer to a value without taking ownership. \
                      At any given time, you can have either one mutable reference or any \
                      number of immutable references. References must always be valid."
                .into(),
        },
        DocChunk {
            title: "The Result type".into(),
            source: "rust-book/ch09".into(),
            content: "Result<T, E> is used for recoverable errors. The Ok(T) variant \
                      contains a success value; Err(E) contains an error value. The ? \
                      operator propagates errors automatically, returning early from the \
                      function if the Result is Err."
                .into(),
        },
        DocChunk {
            title: "Traits".into(),
            source: "rust-book/ch10".into(),
            content: "A trait defines functionality a type must provide. Traits are \
                      similar to interfaces in Java, but more powerful: they can have \
                      default implementations, and can be used as bounds on generic \
                      type parameters."
                .into(),
        },
        DocChunk {
            title: "Async/Await".into(),
            source: "rust-book/ch17".into(),
            content: "Async functions return a Future. Calling .await on a Future \
                      suspends the current task until the Future resolves. Tokio is the \
                      most popular async runtime. Unlike Java's CompletableFuture, Rust \
                      Futures are lazy — they do nothing until awaited."
                .into(),
        },
        DocChunk {
            title: "Cargo workspaces".into(),
            source: "rust-book/ch14".into(),
            content: "A Cargo workspace is a set of packages that share a Cargo.lock \
                      and output directory. All packages in a workspace must declare \
                      shared dependencies consistently. Workspaces are equivalent to \
                      Maven multi-module projects."
                .into(),
        },
    ]
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let client = openai::Client::from_env()?;

    // Step 1: Create the embedding model.
    // TEXT_EMBEDDING_3_SMALL is fast and cost-effective for retrieval tasks.
    let embedding_model = client.embedding_model(openai::TEXT_EMBEDDING_3_SMALL);

    // Step 2: Embed all documents.
    // EmbeddingsBuilder batches the requests; .build() sends them in one call.
    // Returns Vec<(DocChunk, OneOrMany<Embedding>)>.
    println!("Embedding {} documents...", sample_corpus().len());
    let embeddings = EmbeddingsBuilder::new(embedding_model.clone())
        .documents(sample_corpus())?
        .build()
        .await?;

    // Step 3: Build the in-memory vector store and create a searchable index.
    // InMemoryVectorStore::from_documents consumes the (doc, embeddings) tuples.
    // .index(model) wraps the store with the embedding model so it can embed
    // query strings at search time.
    let store = InMemoryVectorStore::from_documents(embeddings);
    let index = store.index(embedding_model);

    // Step 4: Build the RAG agent.
    // .dynamic_context(n, index) retrieves the top-n most similar chunks
    // and injects them into the context before each generation call.
    let rag_agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(
            "You are a helpful Rust documentation assistant. \
             Answer questions about Rust using only the provided context. \
             If the context does not contain enough information to answer, say so.",
        )
        .dynamic_context(2, index)  // retrieve top-2 chunks per query
        .build();

    // Step 5: Ask questions — the agent retrieves relevant chunks automatically.
    let questions = [
        "How does Rust prevent dangling references?",
        "What is the difference between ownership and borrowing?",
        "How do I handle errors in Rust?",
    ];

    for question in &questions {
        println!("\n─── Question ───────────────────────────────────────");
        println!("Q: {question}");
        let answer = rag_agent.prompt(question).await?;
        println!("A: {answer}");
    }

    Ok(())
}
