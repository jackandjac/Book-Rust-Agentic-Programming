// Chapter 9: Swiftide — Streaming Indexing Pipelines
// See chapters/ch09-swiftide.md for the full explanation.
//
// Run: cargo run -p ch09-swiftide
// Requires: OPENAI_API_KEY env var (or .env file)
//
// This example indexes a small collection of Markdown documents from the
// docs/ directory, then queries the resulting index with a search term.
// All processing happens in-memory — no external database required.

use anyhow::Result;
use swiftide::{
    indexing::{
        self,
        loaders::FileLoader,
        persist::MemoryStorage,
        transformers::{ChunkMarkdown, Embed, MetadataQAText},
    },
    integrations::openai::OpenAI,
    query::{self, answers, query_transformers, response_transformers},
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    // ── Step 1: Build the OpenAI integration ─────────────────────────────────
    //
    // Unlike rig's typed constants (openai::GPT_4O_MINI), swiftide uses raw
    // string model names. The OpenAI struct acts as both an embedder and an
    // LLM for the pipeline transformers.
    let openai_client = OpenAI::builder()
        .default_embed_model("text-embedding-3-small")
        .default_prompt_model("gpt-4o-mini")
        .build()?;

    // ── Step 2: Shared in-memory storage ─────────────────────────────────────
    //
    // MemoryStorage stores chunks and their embeddings in process memory.
    // It implements both the `Persist` (write) and `Retrieve` (read) traits,
    // so the same instance can be used in both the indexing and query pipelines.
    let storage = MemoryStorage::default();

    // ── Step 3: Build and run the indexing pipeline ───────────────────────────
    //
    // Swiftide's pipeline is a streaming chain:
    //   FileLoader → chunk → enrich → embed → persist
    //
    // Each stage processes nodes as they arrive; the pipeline is lazy until
    // `.run()` is called.
    println!("Indexing docs/...");

    indexing::Pipeline::from_loader(
        FileLoader::new("docs").with_extensions(&["md"]),
    )
    // Split documents into chunks of 10–2048 characters.
    // Chunks smaller than 10 chars are discarded (usually headings or blanks).
    .then_chunk(ChunkMarkdown::from_chunk_range(10..2048))
    // Ask the LLM to generate Q&A pairs for each chunk and store them as
    // metadata. This enriches chunks with synthetic questions that improve
    // retrieval recall — the "hypothetical document" technique.
    .then(MetadataQAText::new(openai_client.clone()))
    // Embed all chunks in batches of 10. Batching amortises API latency.
    .then_in_batch(Embed::new(openai_client.clone()).with_batch_size(10))
    // Persist the embedded chunks to the in-memory store.
    .then_store_with(storage.clone())
    .run()
    .await?;

    println!("Indexing complete.\n");

    // ── Step 4: Build and run the query pipeline ──────────────────────────────
    //
    // A swiftide query pipeline mirrors the indexing pipeline:
    //   raw query → transform → retrieve → synthesise answer → return
    let pipeline = query::Pipeline::default()
        // (Optional) Expand the user query with a generated sub-question to
        // improve recall. Here we keep it simple and skip expansion.
        .then_transform_query(query_transformers::GenerateSubquestion::from_client(
            openai_client.clone(),
        ))
        // Embed the (possibly expanded) query and retrieve similar chunks from
        // the store. The number of results is configurable via `.top_k()`.
        .then_retrieve(storage.clone())
        // Concatenate the retrieved chunks into a context string.
        .then_transform_response(response_transformers::Summary::from_client(
            openai_client.clone(),
        ))
        // Generate a final answer using the retrieved context.
        .then_answer(answers::Simple::from_client(openai_client.clone()));

    // Run a question through the query pipeline.
    let question = "How does Rust prevent use-after-free bugs?";
    println!("Question: {question}");

    let result = pipeline.query(question).await?;
    println!("Answer:\n{}\n", result.answer());

    Ok(())
}
