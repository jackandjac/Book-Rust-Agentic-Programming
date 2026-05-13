# Chapter 5: Structured Output

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` (772k downloads)  
> `schemars = "1"` (JSON Schema generation — rig-core internally uses 1.0.4)  
> `serde = "1"`, `serde_json = "1"`, `anyhow = "1"`, `tokio = "1"`  
>
> **Java reference:** `BeanOutputConverter` / `StructuredOutputConverter` in Spring AI; typed return values from `@AiService` in LangChain4j

---

## What You'll Learn

- Why "just parse the JSON the LLM returns" is brittle — and the better approach
- How `schemars` generates a JSON Schema from your Rust structs at compile time
- How `rig-core`'s `Extractor<M, T>` extracts typed data from natural language
- The `#[schemars(required)]` pattern for `Option<T>` fields — and why it exists
- How to combine multiple extractors with rig's `pipeline::new()` and `try_parallel!`
- Build: a resume parser that extracts structured data from unstructured text

---

## 5.1 The Problem With Raw JSON Parsing

A common first approach to getting structured data from an LLM:

```rust
// Naive approach — don't do this
let response = agent.prompt("Extract the person's name and job title from: ...").await?;
let parsed: serde_json::Value = serde_json::from_str(&response)?;
let name = parsed["name"].as_str().unwrap_or("unknown");
```

This breaks in three common ways:

1. **The LLM wraps the JSON in prose.** Instead of `{"name": "..."}`, you get: `"Sure! Here's the structured data: {"name": "..."}"`. `serde_json::from_str` fails on the leading text.

2. **Field names vary.** You asked for `"name"` — the LLM returns `"full_name"` or `"person_name"`. The key doesn't exist; your code silently gets `None`.

3. **Types don't match.** You expect a number; the LLM returns `"42"` (a string). Deserialization fails.

The robust solution is to give the LLM the schema *upfront* — tell it exactly what JSON structure you need — and then retry if the output doesn't parse. This is what `rig-core`'s `Extractor` does.

---

## 5.2 Structured Output in Java

### Spring AI: `BeanOutputConverter`

```java
// Spring AI — structured output via BeanOutputConverter
record PersonInfo(String name, String jobTitle, int yearsExperience) {}

BeanOutputConverter<PersonInfo> converter = new BeanOutputConverter<>(PersonInfo.class);
String format = converter.getFormat(); // generates JSON schema instructions

Prompt prompt = new Prompt(
    "Extract person info from: " + text,
    OpenAiChatOptions.builder()
        .responseFormat(new ResponseFormat(ResponseFormat.Type.JSON_SCHEMA, converter.getJsonSchema()))
        .build()
);
ChatResponse response = openAiChatModel.call(prompt);
PersonInfo person = converter.convert(response.getResult().getOutput().getContent());
```

Spring AI uses OpenAI's `response_format: json_schema` when the provider supports it, and falls back to prompt-based schema injection for others.

### LangChain4j: `@AiService` Typed Returns

```java
// LangChain4j — return type defines the schema
interface ResumeParser {
    @UserMessage("Extract person info from the following text: {{text}}")
    PersonInfo parse(@V("text") String text);
}

ResumeParser parser = AiServices.builder(ResumeParser.class)
    .chatLanguageModel(model)
    .build();

PersonInfo info = parser.parse(resumeText); // typed return, no manual parsing
```

LangChain4j generates the JSON schema from the return type and handles the parse + retry loop internally.

In both cases: you define a data type → the framework generates the schema → the LLM fills it in → you get a typed object. `rig-core`'s `Extractor` follows the same pattern.

---

## 5.3 `JsonSchema` — Generating Schemas from Rust Types

The `schemars` crate provides the `JsonSchema` derive macro. Derive it alongside `Serialize` and `Deserialize` on any struct you want to extract:

```toml
# Cargo.toml
[dependencies]
rig-core = "0.37"
schemars = "1"
serde = { version = "1", features = ["derive"] }
```

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct PersonInfo {
    /// The person's full name
    name: String,
    /// Their current job title
    job_title: String,
    /// Years of professional experience (0 if unknown)
    years_experience: u32,
}
```

Doc comments (`///`) become the `"description"` fields in the generated JSON Schema. The schema rig sends to the LLM looks like:

```json
{
  "type": "object",
  "properties": {
    "name": { "type": "string", "description": "The person's full name" },
    "job_title": { "type": "string", "description": "Their current job title" },
    "years_experience": { "type": "integer", "minimum": 0, "description": "Years of professional experience" }
  },
  "required": ["name", "job_title", "years_experience"]
}
```

The LLM sees this schema and knows exactly what keys, types, and constraints to use. Descriptions guide the LLM's extraction — treat them like parameter documentation.

### The `Option<T>` + `#[schemars(required)]` Pattern

When a field might not be present in the source text, you face a tension:

- `String` — required in schema, the LLM must provide it. But what if the source doesn't mention it? The LLM invents a value.
- `Option<String>` — optional in schema by default. The LLM may omit the key, giving you `None`. But you might want the key to always appear (as `null`) so you can distinguish "not mentioned" from "parse error."

`#[schemars(required)]` resolves this: on an `Option<T>` field, it generates the schema as if the field were type `T` (non-nullable, required). Combined with the `Option<T>` Rust type, the effect is: the LLM sees a required `String` field and must produce a value — but your prompt instructs it to use `null` when data is missing, and serde's `Option<T>` deserializes `null` to `None`:

> **Note:** The attribute changes the schema generation, not serde deserialization. The LLM still receives a required-field schema; your preamble is what instructs it to return `null` for missing data. The `Option<T>` type then correctly deserializes `null` → `None`. This matches rig's official examples, which use this pattern for nullable extraction fields.

```rust
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct ResumeInfo {
    /// Candidate's full name
    #[schemars(required)]
    name: Option<String>,

    /// Current or most recent job title
    #[schemars(required)]
    job_title: Option<String>,

    /// Years of experience — null if not mentioned
    #[schemars(required)]
    years_experience: Option<u32>,

    /// List of technical skills mentioned
    skills: Vec<String>,  // always present, may be empty
}
```

With this pattern:
- `name: Some("Alice")` — name found in text
- `name: None` — text didn't mention a name; the LLM correctly returned `null`
- The key `"name"` is always in the JSON — you get consistent deserialization

> **Java parallel:** This is similar to using `Optional<String>` as a return type in LangChain4j's `@AiService` typed interface — the framework's schema generation marks the field as nullable while keeping it in the schema.

---

## 5.4 The `Extractor` — Typed LLM Extraction

`rig-core`'s `Extractor<M, T>` wraps a completion model and a target type. Given natural-language text, it returns a typed `T`. It handles schema injection, parse failures, and retries internally.

### Building an Extractor

```rust
use rig::providers::openai;

let client = openai::Client::from_env()?;

// Type parameter T must impl: JsonSchema + Deserialize + Serialize + Send + Sync
let extractor = client
    .extractor::<PersonInfo>(openai::GPT_4O_MINI)
    .preamble("Extract structured person information from the provided text. \
               If a field is not mentioned, use null.")
    .retries(2)  // retry up to 2 times if JSON parsing fails
    .build();
```

The `extractor::<T>()` call is generic over the target type — the same pattern as LangChain4j's `AiServices.builder(MyInterface.class)`.

### Extracting Data

```rust
let text = "Hi, I'm Jane Smith, a data scientist with 6 years of experience.";

let person: PersonInfo = extractor.extract(text).await?;
println!("{}", serde_json::to_string_pretty(&person)?);
```

Output:
```json
{
  "name": "Jane Smith",
  "job_title": "data scientist",
  "years_experience": 6
}
```

### Extracting With Usage Metadata

To track token usage for cost monitoring:

```rust
use rig::extractor::ExtractionResponse;

let response: ExtractionResponse<PersonInfo> = extractor.extract_with_usage(text).await?;
println!("{}", serde_json::to_string_pretty(&response.data)?);
println!("Tokens used: {}", response.usage.total_tokens);
```

`ExtractionResponse<T>` wraps your data alongside `Usage` (prompt tokens, completion tokens, total). This is the hook for cost tracking in production — covered further in Chapter 18.

---

## 5.5 Extracting Enums

`JsonSchema` works on enums too, making classification tasks clean:

```rust
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
/// The overall sentiment of a piece of text
enum Sentiment {
    Positive,
    Negative,
    Neutral,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct SentimentResult {
    /// The sentiment classification
    sentiment: Sentiment,
    /// Confidence score from 0.0 (uncertain) to 1.0 (certain)
    confidence: f64,
    /// Brief explanation of the classification
    reasoning: String,
}

let extractor = openai::Client::from_env()?
    .extractor::<SentimentResult>(openai::GPT_4O_MINI)
    .build();

let result = extractor
    .extract("The new product is absolutely fantastic! Best I've ever used.")
    .await?;

println!("{:?}", result.sentiment);  // Positive
println!("{:.2}", result.confidence); // e.g. 0.95
```

The enum variants become a JSON Schema `enum` constraint — the LLM is restricted to exactly those string values.

> **Java parallel:** Equivalent to using a Java `enum` as a field in a LangChain4j `@AiService` return type, or as the target type of a Spring AI `BeanOutputConverter`.

---

## 5.6 Complex Nested Structures

Nested structs compose naturally:

```rust
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct WorkExperience {
    /// Company or organization name
    company: String,
    /// Job title held at this company
    title: String,
    /// Year started (e.g. 2019)
    start_year: Option<u32>,
    /// Year ended — null if current position
    #[schemars(required)]
    end_year: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct Resume {
    /// Full name of the candidate
    #[schemars(required)]
    name: Option<String>,
    /// Contact email address
    #[schemars(required)]
    email: Option<String>,
    /// Professional summary or objective statement
    #[schemars(required)]
    summary: Option<String>,
    /// List of technical and professional skills
    skills: Vec<String>,
    /// Work history, most recent first
    experience: Vec<WorkExperience>,
}
```

The schema generation handles nested types recursively — `Vec<WorkExperience>` becomes a JSON array of objects, each with the `WorkExperience` schema. No manual schema assembly required.

---

## 5.7 How the Extractor Works

The extractor uses a **tool-call mechanism**, not prompt injection. Internally, it creates a synthetic `SubmitTool` whose parameters are defined by your target type's JSON Schema. It then sends your text to the LLM alongside this tool definition, asking the model to call the tool with the extracted data. The tool call arguments — which the LLM is constrained to produce as valid JSON matching the schema — are deserialized into `T`.

This approach:
- **Works across all providers** — any provider that supports tool calling (Anthropic, Gemini, Ollama, etc.) can use the extractor, regardless of whether they support OpenAI's native `response_format: json_schema` parameter
- **Is schema-enforced at the protocol level** — the LLM produces structured arguments for the tool call, not free-form JSON in prose
- **Retries on deserialization failure** — if the tool arguments fail to deserialize into `T`, the extractor retries up to the `.retries(n)` count

The three error variants you may encounter:

```rust
pub enum ExtractionError {
    NoData,                              // LLM returned empty content
    DeserializationError(serde_json::Error),  // JSON parse failed after retries
    CompletionError(CompletionError),     // Upstream API error
}
```

Handling them:

```rust
use rig::extractor::ExtractionError;

match extractor.extract(text).await {
    Ok(data) => process(data),
    Err(ExtractionError::DeserializationError(e)) => {
        // The LLM returned something, but it wasn't valid JSON matching your schema.
        // Check: is the preamble clear? Is the schema simple enough?
        eprintln!("Parse failed: {e}. Consider simplifying the target struct.");
    }
    Err(ExtractionError::NoData) => {
        eprintln!("LLM returned no content — check your prompt.");
    }
    Err(ExtractionError::CompletionError(e)) => {
        eprintln!("API error: {e}");
    }
}
```

---

## 5.8 Parallel Extraction with Pipelines

Sometimes you want to extract multiple different schema types from the same input simultaneously — for example, extracting names, topics, and sentiment from a batch of text in one pass.

`rig-core`'s pipeline system provides `try_parallel!` for this pattern. The macro runs multiple operations concurrently and collects their results into a tuple.

### The Pipeline Primitives

```rust
use rig::pipeline::{self, TryOp, agent_ops};
use rig::try_parallel;
```

| Primitive | Purpose |
|-----------|---------|
| `pipeline::new()` | Create a new pipeline |
| `.chain(op)` | Append an operation |
| `.map_ok(f)` | Transform a successful result |
| `try_parallel!(op1, op2, ...)` | Run ops concurrently, fail if any fails |
| `agent_ops::extract(extractor)` | Wrap an `Extractor` as a pipeline op |
| `.try_batch_call(n, inputs)` | Run pipeline on multiple inputs with concurrency `n` |

### Parallel Multi-Extraction

Here's a complete parallel extraction example using the rig pipeline API (adapted from `rig-core/examples/multi_extract.rs`):

```rust
use anyhow::Result;
use rig::client::ProviderClient;
use rig::pipeline::{self, TryOp, agent_ops};
use rig::providers::openai;
use rig::try_parallel;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct Names {
    names: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct Topics {
    topics: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct Sentiment {
    sentiment: f64,
    confidence: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let client = openai::Client::from_env()?;

    let names_extractor = client
        .extractor::<Names>(openai::GPT_4O_MINI)
        .preamble("Extract names from the given text.")
        .retries(2)
        .build();
    let topics_extractor = client
        .extractor::<Topics>(openai::GPT_4O_MINI)
        .preamble("Extract topics from the given text.")
        .retries(2)
        .build();
    let sentiment_extractor = client
        .extractor::<Sentiment>(openai::GPT_4O_MINI)
        .preamble("Extract sentiment and confidence from the given text.")
        .retries(2)
        .build();

    let chain = pipeline::new()
        .chain(try_parallel!(
            agent_ops::extract(names_extractor),
            agent_ops::extract(topics_extractor),
            agent_ops::extract(sentiment_extractor),
        ))
        .map_ok(|(names, topics, sentiment)| {
            format!(
                "Names: {}\nTopics: {}\nSentiment: {:.2} (confidence: {:.2})",
                names.names.join(", "),
                topics.topics.join(", "),
                sentiment.sentiment,
                sentiment.confidence,
            )
        });

    let inputs = vec![
        "Alice and Bob are debating quantum computing's impact on cryptography.",
        "I love my dog, but I hate rainy days.",
    ];

    // Process all inputs with up to 4 concurrent API calls
    let responses = chain.try_batch_call(4, inputs).await?;

    for (i, response) in responses.iter().enumerate() {
        println!("Input {}: {response}\n", i + 1);
    }

    Ok(())
}
```

Each input text is sent to three extractors concurrently — names, topics, and sentiment — all running in parallel via Tokio. The pipeline collects the three results into a tuple, then `map_ok` formats the final output.

> **Java parallel:** This mirrors Spring AI's parallel advisor chains or LangGraph4j's parallel node fanout — multiple operations applied to the same input concurrently, with results joined afterward.

### Pipelines as Composable Units

Pipelines are values — you can build them once and reuse:

```rust
// Build the pipeline once
let analysis_pipeline = pipeline::new()
    .chain(try_parallel!(
        agent_ops::extract(names_extractor),
        agent_ops::extract(sentiment_extractor),
    ))
    .map_ok(|(names, sentiment)| AnalysisResult { names, sentiment });

// Reuse for many inputs
let results = analysis_pipeline.try_batch_call(8, text_inputs).await?;
```

The batch concurrency parameter (`8` above) controls how many inputs are in-flight simultaneously. Tune it against your API rate limits.

---

## 5.9 Hands-On: Resume Parser

The complete runnable example — extracts structured data from free-form resume text.

```rust
// code-examples/ch05-structured-output/src/main.rs
use anyhow::Result;
use rig::client::ProviderClient;
use rig::providers::openai;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct WorkExperience {
    /// Employer name
    company: String,
    /// Role or job title
    title: String,
    /// Year started
    start_year: Option<u32>,
    /// Year ended — null if current role
    #[schemars(required)]
    end_year: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct Resume {
    /// Candidate's full name — null if not found
    #[schemars(required)]
    name: Option<String>,
    /// Email address — null if not mentioned
    #[schemars(required)]
    email: Option<String>,
    /// Technical and professional skills listed
    skills: Vec<String>,
    /// Work history, most recent first
    experience: Vec<WorkExperience>,
    /// Highest education level mentioned — null if not mentioned
    #[schemars(required)]
    education: Option<String>,
}

const RESUME_TEXT: &str = r#"
Sarah Chen | sarah.chen@example.com

Experienced software engineer with 8 years in distributed systems.

Skills: Rust, Go, Kubernetes, PostgreSQL, Redis, Apache Kafka

Experience:
- Staff Engineer, DataFlow Inc. (2022–present)
  Leading distributed ingestion pipeline team
- Senior Software Engineer, CloudBase (2019–2022)
  Built autoscaling microservices in Go

Education: B.S. Computer Science, UC Berkeley (2015)
"#;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let extractor = openai::Client::from_env()?
        .extractor::<Resume>(openai::GPT_4O_MINI)
        .preamble(
            "Extract structured resume data from the provided text. \
             Use null for any field that is not mentioned. \
             For experience, extract all positions listed."
        )
        .retries(2)
        .build();

    println!("Parsing resume...\n");
    let resume = extractor.extract(RESUME_TEXT).await?;

    println!("Name:      {:?}", resume.name);
    println!("Email:     {:?}", resume.email);
    println!("Education: {:?}", resume.education);
    println!("\nSkills ({}):", resume.skills.len());
    for skill in &resume.skills {
        println!("  - {skill}");
    }
    println!("\nExperience ({} positions):", resume.experience.len());
    for job in &resume.experience {
        println!(
            "  {} at {} ({:?}–{:?})",
            job.title, job.company, job.start_year, job.end_year
        );
    }

    Ok(())
}
```

Expected output:

```
Parsing resume...

Name:      Some("Sarah Chen")
Email:     Some("sarah.chen@example.com")
Education: Some("B.S. Computer Science, UC Berkeley (2015)")

Skills (6):
  - Rust
  - Go
  - Kubernetes
  - PostgreSQL
  - Redis
  - Apache Kafka

Experience (2 positions):
  Staff Engineer at DataFlow Inc. (Some(2022)–None)
  Senior Software Engineer at CloudBase (Some(2019)–Some(2022))
```

### Running the Example

```bash
cd code-examples
export OPENAI_API_KEY=sk-...
cargo run -p ch05-structured-output
```

---

## 5.10 Schema Design Tips

**Keep schemas flat where possible.** Deeply nested structures are harder for LLMs to fill in correctly. If a nested struct has more than 3–4 fields, consider whether it could be a flat list of typed structs instead.

**Use doc comments for every field.** They become schema descriptions — the LLM reads them. "The person's age in years" is much better than an undescribed `age: u32`. Think of it as writing a contract with the model.

**Use `Option<T>` + `#[schemars(required)]` for uncertain fields.** This tells the LLM "you must include this key, but it's okay to say null if you don't have the data." Without `required`, the LLM may omit the key entirely, making it impossible to distinguish "not present" from "parsing failed."

**Use `Vec<T>` for lists that might be empty.** `Vec<String>` is a better choice than `Option<Vec<String>>` for lists — an empty `[]` is cleaner to work with than `None`.

**Add a `preamble` with extraction instructions.** Don't rely solely on the schema — tell the extractor what to do with missing data, how to handle ambiguity, and what the source text represents. The preamble is your instruction layer; the schema is the output contract.

**Set `.retries(2)` for production.** One retry is usually enough to recover from a malformed JSON response. Two retries handles the rare cases where the LLM needs extra context. More than 3 retries usually indicates a schema that's too complex for the model.

---

## Key Takeaways

- Naive JSON parsing from LLM output is brittle — schema injection and typed extraction are more reliable.
- `#[derive(JsonSchema)]` from `schemars` generates the schema automatically from your struct definition; doc comments become field descriptions.
- `rig-core`'s `Extractor<M, T>` handles schema injection, parsing, and retries. Build one per target type with `.extractor::<T>(model).preamble(...).retries(n).build()`.
- `#[schemars(required)]` on `Option<T>` fields — the key is required in JSON, but the value may be `null`. Use for fields that might not appear in the source text.
- Pipelines (`pipeline::new()`, `try_parallel!`, `agent_ops::extract`) enable parallel extraction of multiple schemas from the same input in one pass.
- `.extract_with_usage()` returns token counts alongside the extracted data — use for cost tracking.

---

## Further Reading

- [rig-core extractor docs](https://docs.rs/rig-core/latest/rig/extractor/index.html) — `Extractor`, `ExtractorBuilder`, `ExtractionError`
- [schemars docs](https://graham.cool/schemars/) — full attribute reference for `JsonSchema` derive
- [schemars on docs.rs](https://docs.rs/schemars/1.0.4/schemars/) — API reference
- [Spring AI Structured Output](https://docs.spring.io/spring-ai/reference/api/structured-output-converter.html) — Java reference: `BeanOutputConverter`
- [LangChain4j AiServices typed returns](https://docs.langchain4j.dev/tutorials/ai-services) — Java reference: typed extraction via interface

---

*Next: Chapter 6 — RAG: Retrieval-Augmented Generation with Swiftide*
