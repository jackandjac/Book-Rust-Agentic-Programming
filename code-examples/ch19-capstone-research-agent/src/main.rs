// Chapter 19 (Capstone): Research Agent — Rig + Swiftide + MCP
// See chapters/ch19-capstone-research-agent.md for the full walkthrough.
//
// Run:
//   cargo run -p ch19-capstone-research-agent
//
// Requires:
//   OPENAI_API_KEY env var (or .env file)
//   A `docs/` directory with markdown files to index
//
// Architecture:
//   1. Swiftide indexes docs/ into an in-memory vector store
//   2. A rig agent answers questions using RAG + tool calling
//   3. The agent is exposed via an MCP server (over STDIO)

use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::providers::openai;
use std::sync::Arc;
use swiftide::{
    indexing::{
        self,
        loaders::FileLoader,
        persist::MemoryStorage,
        transformers::{ChunkMarkdown, Embed, MetadataQAText},
    },
    integrations::openai::OpenAI as SwiftideOpenAI,
    query::{self, answers, query_transformers, response_transformers},
};
use tracing::instrument;

// ── Document indexing ─────────────────────────────────────────────────────────

/// Index all markdown files in `docs/` into the in-memory store.
/// Returns the populated MemoryStorage ready for queries.
#[instrument(skip_all)]
async fn index_documents(
    swiftide_client: SwiftideOpenAI,
    docs_path: &str,
) -> Result<MemoryStorage> {
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

    tracing::info!("Indexing complete");
    Ok(storage)
}

// ── RAG query ─────────────────────────────────────────────────────────────────

/// Query the indexed documents via a swiftide query pipeline.
#[instrument(skip(swiftide_client, storage))]
async fn rag_query(
    swiftide_client: SwiftideOpenAI,
    storage: MemoryStorage,
    question: &str,
) -> Result<String> {
    let pipeline = query::Pipeline::default()
        .then_transform_query(
            query_transformers::GenerateSubquestion::from_client(swiftide_client.clone()),
        )
        .then_retrieve(storage)
        .then_transform_response(
            response_transformers::Summary::from_client(swiftide_client.clone()),
        )
        .then_answer(answers::Simple::from_client(swiftide_client));

    let result = pipeline.query(question).await?;
    Ok(result.answer().to_string())
}

// ── Rig research agent ────────────────────────────────────────────────────────

/// A rig agent that uses the indexed docs to answer questions.
/// In production you'd wire the RAG pipeline as a tool call; here we call
/// it directly to keep the example focused.
#[instrument(skip(client, storage, swiftide_client))]
async fn research_answer(
    client: &openai::Client,
    swiftide_client: SwiftideOpenAI,
    storage: MemoryStorage,
    question: &str,
) -> Result<String> {
    // Step 1: retrieve context from the vector store
    let context = rag_query(swiftide_client, storage, question).await?;

    // Step 2: ask the rig agent with injected context
    let agent = client
        .agent(openai::GPT_4O_MINI)
        .preamble(
            "You are a research assistant. You have been provided with relevant \
             document excerpts. Answer the user's question based on those excerpts. \
             Cite specific facts. If the context doesn't contain the answer, say so.",
        )
        .build();

    let prompt = format!(
        "Context from indexed documents:\n{context}\n\nQuestion: {question}"
    );

    let answer = agent.prompt(&prompt).await?;
    Ok(answer)
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
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

    // ── Index docs ────────────────────────────────────────────────────────────
    let docs_path = std::env::var("DOCS_PATH").unwrap_or_else(|_| "docs".to_string());

    // Create a minimal sample docs dir if none exists (for demo purposes)
    if !std::path::Path::new(&docs_path).exists() {
        std::fs::create_dir_all(&docs_path)?;
        std::fs::write(
            format!("{docs_path}/rust-ownership.md"),
            "# Rust Ownership\n\nRust's ownership system ensures memory safety without \
             a garbage collector. Each value has exactly one owner. When the owner goes \
             out of scope, the value is dropped. References allow borrowing without taking \
             ownership. The borrow checker enforces these rules at compile time.",
        )?;
        tracing::info!("Created sample docs at {docs_path}/");
    }

    let storage = index_documents(swiftide_client.clone(), &docs_path).await?;
    let storage = Arc::new(storage);

    // ── Answer questions ──────────────────────────────────────────────────────
    let questions = [
        "How does Rust's ownership system prevent memory bugs?",
        "What happens when a Rust value goes out of scope?",
    ];

    for question in questions {
        println!("\nQ: {question}");
        match research_answer(&rig_client, swiftide_client.clone(), (*storage).clone(), question).await {
            Ok(answer) => println!("A: {answer}"),
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    Ok(())
}
