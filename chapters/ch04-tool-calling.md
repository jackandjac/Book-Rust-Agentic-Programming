# Chapter 4: Tool Calling

> **Framework versions in this chapter:**  
> `rig-core = "0.37"` (772k downloads — bumped from 0.36; all Ch4 APIs unchanged)  
> `rig-derive = "0.1"` (proc-macro crate — ships separately from `rig-core`)  
> `async-openai = "0.38"` (4.8M downloads, updated May 11 2026)  
> `tokio = "1"`, `serde = "1"`, `anyhow = "1"`, `dotenvy = "0.15"`  
>
> **Java reference:** "Tool Calling and Function Execution" in LangChain4j (`@Tool` annotation)

---

## What You'll Learn

- How the OpenAI tool-calling protocol works at the wire level — essential for debugging in production
- How to implement the raw two-round-trip dispatch loop with `async-openai`
- How `rig-core`'s `Tool` trait replaces the manual boilerplate
- How the `#[rig_tool]` macro (from `rig-derive`) generates a `Tool` implementation from a function
- Error handling in tools: what `ToolError` is and when to use custom error types
- Build: a multi-tool weather and temperature converter agent

---

## 4.1 What Tool Calling Actually Is

If you've used LangChain4j's `@Tool` annotation, you know the result: methods annotated with `@Tool` become callable by the LLM. The framework handles the plumbing — schema generation, routing the LLM's tool request to the right method, feeding the result back.

What you may not have thought about is what's happening underneath. Tool calling is a two-round-trip protocol:

**Round 1 — The LLM decides to use a tool:**
1. You send the LLM a message, along with a list of tool definitions (JSON schemas)
2. The LLM responds with a `tool_calls` array instead of natural-language `content`
3. This is not a final answer — it's the model asking your code to run something

**Round 2 — Your code runs the tool and reports back:**
4. You execute the requested function locally
5. You send a new API call with the conversation history PLUS a "tool" role message containing the result
6. NOW the LLM generates its final natural-language response

This two-trip structure is the same regardless of language or framework. In Java, LangChain4j does both trips invisibly. In Rust, we'll first do it manually (so you understand it), then see how `rig-core` automates it.

---

## 4.2 Tool Calling in Java: The `@Tool` Annotation

Here's the LangChain4j approach you already know:

```java
// Java — LangChain4j @Tool
public class WeatherTools {

    @Tool("Get the current weather for a city")
    public String getWeather(
        @P("The city name, e.g. 'London'") String city
    ) {
        // Real implementation would call a weather API
        return "The weather in " + city + " is 15°C and partly cloudy.";
    }
}

// Wire it up:
ChatLanguageModel model = OpenAiChatModel.builder()
    .apiKey(System.getenv("OPENAI_API_KEY"))
    .modelName("gpt-4o")
    .build();

WeatherTools tools = new WeatherTools();
Assistant assistant = AiServices.builder(Assistant.class)
    .chatLanguageModel(model)
    .tools(tools)
    .build();

String response = assistant.chat("What's the weather in Paris?");
```

The `@Tool` annotation generates the JSON schema from the method signature and docstring. LangChain4j's `AiServices` handles both API round-trips internally — you never see them.

The Rust path to the same result goes through understanding the protocol first, then reaching the same level of abstraction.

---

## 4.3 The Raw Protocol: async-openai

Before reaching for `rig-core`, let's see what the LLM's tool-calling protocol actually looks like. This is the foundation every framework is built on.

### 4.3.1 Defining a Tool Schema

A tool definition is a JSON object. In async-openai, it's built with typed structs:

```rust
use async_openai::types::{
    ChatCompletionTool, ChatCompletionToolType, FunctionObject,
};
use serde_json::json;

fn weather_tool() -> ChatCompletionTool {
    ChatCompletionTool {
        r#type: ChatCompletionToolType::Function,
        function: FunctionObject {
            name: "get_weather".to_string(),
            description: Some("Get the current weather for a city".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string",
                        "description": "The city name, e.g. 'London'"
                    }
                },
                "required": ["city"]
            })),
            strict: Some(false),
        },
    }
}
```

`serde_json::json!` builds the parameters as a raw JSON Schema value — OpenAI's API requires a JSON Schema object here, not a typed struct.

### 4.3.2 The Two-Round-Trip Dispatch Loop

Here's the complete manual tool-calling loop. This is what all frameworks hide:

```rust
use async_openai::{
    Client,
    types::{
        ChatCompletionRequestAssistantMessageArgs,
        ChatCompletionRequestMessage,
        ChatCompletionRequestToolMessageArgs,
        ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
        FinishReason,
    },
};
use anyhow::{anyhow, Result};

// Pure Rust implementation — no LLM framework involved
fn get_weather(city: &str) -> String {
    format!("The weather in {city} is 15°C and partly cloudy.")
}

/// Demonstrates a single tool-call exchange.
/// Note: handles one round of tool calls only — for multi-step chains,
/// wrap the dispatch in a loop until finish_reason is Stop.
async fn run_with_tools(question: &str) -> Result<String> {
    let client = Client::new(); // reads OPENAI_API_KEY from env

    let tools = vec![weather_tool()];

    // --- Round 1: Ask the LLM ---
    let mut messages: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestUserMessageArgs::default()
            .content(question)
            .build()?
            .into(),
    ];

    let request = CreateChatCompletionRequestArgs::default()
        .model("gpt-4o")
        .messages(messages.clone())
        .tools(tools.clone())
        .build()?;

    let response = client.chat().create(request).await?;
    let choice = response.choices.into_iter().next()
        .ok_or_else(|| anyhow!("No choices returned"))?;

    // Did the LLM want to call a tool?
    if choice.finish_reason == Some(FinishReason::ToolCalls) {
        let tool_calls = choice.message.tool_calls.unwrap_or_default();

        // Add the assistant's tool-call message to conversation history
        messages.push(
            ChatCompletionRequestAssistantMessageArgs::default()
                .tool_calls(tool_calls.clone())
                .build()?
                .into(),
        );

        // --- Execute each requested tool ---
        for tool_call in &tool_calls {
            let result = match tool_call.function.name.as_str() {
                "get_weather" => {
                    let args: serde_json::Value =
                        serde_json::from_str(&tool_call.function.arguments)?;
                    let city = args["city"]
                        .as_str()
                        .ok_or_else(|| anyhow!("Missing 'city' argument"))?;
                    get_weather(city)
                }
                other => format!("Unknown tool: {other}"),
            };

            // Add the tool result to the message history
            messages.push(
                ChatCompletionRequestToolMessageArgs::default()
                    .tool_call_id(tool_call.id.clone())
                    .content(result)
                    .build()?
                    .into(),
            );
        }

        // --- Round 2: Ask the LLM again with tool results ---
        let request2 = CreateChatCompletionRequestArgs::default()
            .model("gpt-4o")
            .messages(messages)
            .build()?;

        let response2 = client.chat().create(request2).await?;
        let final_choice = response2.choices.into_iter().next()
            .ok_or_else(|| anyhow!("No choices in round 2"))?;

        Ok(final_choice.message.content.unwrap_or_default())
    } else {
        // LLM answered directly without using any tool
        Ok(choice.message.content.unwrap_or_default())
    }
}
```

Read through this carefully — everything else in this chapter is an abstraction over this pattern.

A few things to notice:

1. **The `match` statement is your routing table.** `match tool_call.function.name.as_str()` dispatches to the right Rust function. With multiple tools, this `match` grows. Frameworks replace this manual routing with a registration system.

2. **Arguments arrive as a JSON string.** `tool_call.function.arguments` is a raw string, not a struct. You deserialize it yourself. If the LLM hallucinates an argument name or type, you get a deserialization error here.

3. **Tool results go into the message history.** The second API call includes: user message → assistant tool_calls message → tool result message. Token count grows with each tool call.

4. **`finish_reason == ToolCalls` signals a tool request.** If you try to read `choice.message.content` when finish_reason is ToolCalls, it will be `None`.

5. **This example handles one round of tool calls.** If the LLM's second response also requests a tool (multi-step reasoning), you'd need a `loop` wrapping the dispatch. `rig-core` handles this for you.

> **Why this matters:** When something goes wrong in production — an LLM passes the wrong argument type, or your tool errors — you need to understand this protocol to debug it. The framework logs you see in LangChain4j's `INFO` output are structured versions of these two round-trips.

---

## 4.4 The `Tool` Trait in rig-core

`rig-core` replaces the manual dispatch loop with a trait system. Implement `Tool` for a struct, and `rig-core`'s agent handles both API round-trips, routing, and chaining automatically.

Here is the actual `Tool` trait from `rig-core::tool`:

```rust
// From rig-core source (simplified for clarity)
pub trait Tool: Send + Sync {
    const NAME: &'static str;

    type Error: std::error::Error + Send + Sync + 'static;
    type Args: for<'a> Deserialize<'a> + Send + Sync;
    type Output: Serialize + Send + Sync;

    async fn definition(&self, prompt: String) -> ToolDefinition;
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error>;
}
```

Note that `definition()` is `async fn`. In most implementations it returns immediately, but the trait allows async resolution (e.g., fetching a schema from a remote source).

Each associated type serves a clear purpose:

| Associated type | Purpose |
|----------------|---------|
| `Error` | Any type implementing `std::error::Error + Send + Sync` |
| `Args` | Deserializes from the JSON string the LLM sends |
| `Output` | Serializes to the string sent back to the LLM |

### Implementing `Tool` Manually

Here's a complete Tool implementation from the rig-core examples directory (adapted):

```rust
use rig::{
    completion::{Prompt, ToolDefinition},
    providers::openai,
    tool::Tool,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

// The argument struct — derived from whatever JSON the LLM sends
#[derive(Deserialize)]
struct OperationArgs {
    x: i32,
    y: i32,
}

// A typed error for this tool — thiserror is idiomatic for library/tool code
#[derive(Debug, thiserror::Error)]
#[error("math error")]
struct MathError;

// The tool struct — can be zero-sized if stateless
#[derive(Deserialize, Serialize)]
struct Add;

impl Tool for Add {
    const NAME: &'static str = "add";

    type Error = MathError;
    type Args = OperationArgs;
    type Output = i32;  // returned as serialized JSON to the LLM

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "add".to_string(),
            description: "Add x and y together".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number", "description": "The first number to add" },
                    "y": { "type": "number", "description": "The second number to add" }
                },
                "required": ["x", "y"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(args.x + args.y)
    }
}
```

Registering with an agent:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let agent = openai::Client::from_env()
        .agent(openai::GPT_4O)
        .preamble("You are a calculator. Use the tools before answering.")
        .tool(Add)
        .max_tokens(1024)
        .build();

    let response = agent.prompt("Calculate 2 - 5.").await?;
    println!("{response}");

    Ok(())
}
```

`.tool(Add)` registers the tool. `.prompt()` handles the full dispatch loop — both round-trips, routing, and multi-step chains.

> **Error types in tool code:** The rig-core source uses `thiserror` for tool error types (as shown above with `MathError`). Chapter 2 said "we won't need `thiserror` in this book" — that was imprecise. The accurate rule: **use `thiserror` when defining a Tool's `Error` associated type** (gives callers a typed, matchable error); **use `anyhow` in main functions and higher-level application code** (simpler, good enough). The `#[rig_tool]` macro (next section) accepts `anyhow::Result` and handles the conversion automatically.

---

## 4.5 The `#[rig_tool]` Macro

Writing `definition()` and the Args struct by hand is tedious. The `rig-derive` crate provides the `#[rig_tool]` attribute macro, which generates the full `Tool` trait implementation from a function's signature.

> **Important:** `#[rig_tool]` comes from the **`rig-derive`** crate, which is a separate package from `rig-core`. You need both in `Cargo.toml`.

```toml
[dependencies]
rig-core = "0.37"
rig-derive = "0.1"
```

### Using the Macro

```rust
use rig_derive::rig_tool;

#[rig_tool(
    description = "Get the current weather for a named city",
    params(city = "The city name, e.g. 'London' or 'Tokyo'")
)]
fn get_weather(city: String) -> Result<String, ToolError> {
    Ok(format!("The weather in {city} is 15°C and partly cloudy."))
}
```

The macro attributes:

| Attribute | Purpose |
|-----------|---------|
| `description` | The tool's overall description sent to the LLM |
| `params(arg = "desc", ...)` | Per-parameter descriptions — become JSON Schema `"description"` fields |
| `name` | Optional custom tool name (default: the function name) |
| `required(arg1, arg2)` | Mark which parameters are required in the schema |

The macro generates:
1. A `struct` named after your function (e.g., `GetWeather`)
2. An `Args` struct from the function parameters, with each `params()` entry as the schema description
3. `definition()` implementation from `description` and the derived Args schema
4. A `call()` implementation wrapping your function body

### Java vs Rust: Side-by-Side

```java
// Java — LangChain4j
@Tool("Get the current weather for a city")
public String getWeather(
    @P("The city name, e.g. 'London'") String city
) {
    return "15°C and partly cloudy in " + city;
}
```

```rust
// Rust — rig-derive
#[rig_tool(
    description = "Get the current weather for a city",
    params(city = "The city name, e.g. 'London'")
)]
async fn get_weather(city: String) -> Result<String, ToolError> {
    Ok(format!("15°C and partly cloudy in {city}"))
}
```

The concepts map directly: annotation → attribute macro, `@P` → `params()` entry, return type → `Result<String, ToolError>`. Errors are explicit in the return type. Use `async fn` only when the tool body itself makes I/O calls; pure-computation tools use `fn`.

---

## 4.6 Multiple Tools

Real agents use multiple tools. Register them with chained `.tool()` calls:

```rust
use rig_derive::rig_tool;
use rig::tool::ToolError;
use rig::providers::openai;

#[rig_tool(
    description = "Get the current weather for a named city",
    params(city = "The city name, e.g. 'London'"),
    required(city)
)]
fn get_weather(city: String) -> Result<String, ToolError> {
    // Stub — replace with an HTTP call to a weather API
    match city.to_lowercase().as_str() {
        "london" => Ok("London: 12°C, overcast".to_string()),
        "paris"  => Ok("Paris: 18°C, sunny".to_string()),
        "tokyo"  => Ok("Tokyo: 22°C, humid".to_string()),
        other    => Ok(format!("{other}: 20°C, clear")),
    }
}

#[rig_tool(
    description = "Convert temperature between Celsius (C), Fahrenheit (F), and Kelvin (K)",
    params(
        value = "The numeric value to convert",
        from  = "Source unit: C, F, or K",
        to    = "Target unit: C, F, or K"
    ),
    required(value, from, to)
)]
fn convert_temperature(
    value: f64,
    from: String,
    to: String,
) -> Result<String, ToolError> {
    let celsius = match from.to_uppercase().as_str() {
        "C" => value,
        "F" => (value - 32.0) * 5.0 / 9.0,
        "K" => value - 273.15,
        other => return Err(ToolError::ToolCallError(
            format!("Unknown source unit '{other}'. Use C, F, or K.").into()
        )),
    };

    let result = match to.to_uppercase().as_str() {
        "C" => celsius,
        "F" => celsius * 9.0 / 5.0 + 32.0,
        "K" => celsius + 273.15,
        other => return Err(ToolError::ToolCallError(
            format!("Unknown target unit '{other}'. Use C, F, or K.").into()
        )),
    };

    Ok(format!("{value}°{from} = {result:.1}°{to}"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let agent = openai::Client::from_env()
        .agent(openai::GPT_4O)
        .preamble(
            "You are a helpful assistant with weather data and a temperature converter. \
             Use the tools when relevant. Be concise."
        )
        .tool(get_weather)
        .tool(convert_temperature)
        .build();

    let response = agent
        .prompt("What's the weather in Tokyo, and what is that in Fahrenheit?")
        .await?;

    println!("{response}");
    Ok(())
}
```

When this runs, the LLM calls `get_weather` (getting Celsius), then calls `convert_temperature` — two tool calls, all managed by rig's dispatch loop. Your code sees a single `.prompt().await?`.

> **Tool call chaining:** Rig's agent loop runs until the LLM stops requesting tools (`finish_reason == Stop`). Multi-step chains where the LLM calls several tools before answering work automatically. The LangChain4j parallel: `AiServices` similarly loops until the model is satisfied.

---

## 4.7 Error Handling in Tools

### Tool Errors with `ToolError`

The `rig::tool::ToolError` type is rig's error for the tool execution layer. When your tool returns `Err(...)`, rig serializes the error message and sends it back to the LLM as the tool result content. The LLM can then decide to retry with corrected arguments, apologize, or try a different approach.

```rust
use rig::tool::ToolError;

#[rig_tool(
    description = "Look up a stock price by ticker symbol",
    params(ticker = "Stock ticker symbol, e.g. 'AAPL' or 'GOOGL'"),
    required(ticker)
)]
fn get_stock_price(ticker: String) -> Result<String, ToolError> {
    // Validate — tickers are 1-5 uppercase ASCII letters
    let ticker = ticker.trim().to_uppercase();
    if ticker.is_empty()
        || ticker.len() > 5
        || !ticker.chars().all(|c| c.is_ascii_alphabetic())
    {
        return Err(ToolError::ToolCallError(format!(
            "Invalid ticker '{}'. Expected 1-5 letters (e.g. 'AAPL')", ticker
        ).into()));
    }

    // Proceed with validated, normalized input
    Ok(format!("{ticker}: $142.50 (+1.2%)"))
}
```

The validation error goes back to the LLM as:
```
Tool 'get_stock_price' returned: Invalid ticker 'AAPL.'. Expected 1-5 letters (e.g. 'AAPL')
```

This gives the model the information it needs to retry with `"AAPL"`.

> **Java parallel:** This is the same discipline as validating `@Tool` parameters with `@NotNull`, `@Pattern`, etc. in LangChain4j. In Rust, the compiler already handles type safety (no `null` for `String`), but range and format validation remains your responsibility.

### Using Custom Error Types with Manual `Tool` Impl

When implementing `Tool` manually (not with the macro), use `thiserror` for the `Error` associated type. It gives callers typed, matchable errors:

```rust
#[derive(Debug, thiserror::Error)]
enum WeatherError {
    #[error("city '{0}' not found")]
    CityNotFound(String),
    #[error("API request failed: {0}")]
    ApiError(String),
}

impl Tool for WeatherApiTool {
    type Error = WeatherError;
    // ...
    async fn call(&self, args: WeatherArgs) -> Result<WeatherOutput, WeatherError> {
        // rig converts this to ToolError via its From impl
    }
}
```

> **The two error layers:** `thiserror` in the tool definition gives you rich typed errors inside your tool logic. Rig converts them to `ToolError` (a simpler type) before sending to the LLM. This mirrors how LangChain4j catches `RuntimeException` from tool methods and handles them as tool failures.

---

## 4.8 Stateful Tools

So far, tools have been stateless. Real tools often need state: an HTTP client, a database connection pool, an API key. When implementing `Tool` manually, state lives in the struct:

```rust
// Illustrative — adds reqwest to your Cargo.toml if used
struct WeatherApiTool {
    api_key: String,
    http_client: reqwest::Client,  // reused across calls — efficient connection pooling
}

impl WeatherApiTool {
    fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            http_client: reqwest::Client::new(),
        }
    }
}
```

Register stateful tools the same way:

```rust
let weather_tool = WeatherApiTool::new(std::env::var("WEATHER_API_KEY")?);

let agent = openai::Client::from_env()
    .agent(openai::GPT_4O)
    .tool(weather_tool)
    .build();
```

The tool holds the HTTP client, initialized once. Multiple tool invocations within the same agent session reuse it — the correct, efficient pattern.

> **Java parallel:** Equivalent to `@Component`-annotated LangChain4j tools injected as Spring beans, where the bean holds an injected `RestTemplate` or `WebClient`.

---

## 4.9 Hands-On: Weather and Temperature Converter Agent

The complete runnable example in `code-examples/ch04-tool-calling/src/main.rs`:

```rust
// code-examples/ch04-tool-calling/src/main.rs
use anyhow::Result;
use rig::providers::openai;
use rig::tool::ToolError;
use rig_derive::rig_tool;

#[rig_tool(
    description = "Get the current weather for a named city",
    params(city = "The city name, e.g. 'London' or 'New York'"),
    required(city)
)]
fn get_weather(city: String) -> Result<String, ToolError> {
    // Stub: replace with a real weather API call
    match city.to_lowercase().as_str() {
        "london" => Ok("London: 12°C, overcast".to_string()),
        "paris"  => Ok("Paris: 18°C, sunny".to_string()),
        "tokyo"  => Ok("Tokyo: 22°C, humid".to_string()),
        other    => Ok(format!("{other}: 20°C, clear skies")),
    }
}

#[rig_tool(
    description = "Convert temperature between Celsius (C), Fahrenheit (F), and Kelvin (K)",
    params(
        value = "The numeric temperature to convert",
        from  = "Source unit: C, F, or K",
        to    = "Target unit: C, F, or K"
    ),
    required(value, from, to)
)]
fn convert_temperature(
    value: f64,
    from: String,
    to: String,
) -> Result<String, ToolError> {
    let celsius = match from.to_uppercase().as_str() {
        "C" => value,
        "F" => (value - 32.0) * 5.0 / 9.0,
        "K" => value - 273.15,
        other => return Err(ToolError::ToolCallError(
            format!("Unknown source unit '{other}'. Use C, F, or K.").into()
        )),
    };

    let result = match to.to_uppercase().as_str() {
        "C" => celsius,
        "F" => celsius * 9.0 / 5.0 + 32.0,
        "K" => celsius + 273.15,
        other => return Err(ToolError::ToolCallError(
            format!("Unknown target unit '{other}'. Use C, F, or K.").into()
        )),
    };

    Ok(format!("{value}°{from} = {result:.1}°{to}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let agent = openai::Client::from_env()
        .agent(openai::GPT_4O)
        .preamble(
            "You are a helpful assistant with weather data and a temperature converter. \
             Use the tools when the user asks about weather or temperatures. Be concise.",
        )
        .tool(get_weather)
        .tool(convert_temperature)
        .build();

    let questions = [
        "What is the weather in London?",
        "What's the weather in Tokyo, and what is that in Fahrenheit?",
        "Convert 100°C to Kelvin.",
    ];

    for question in &questions {
        println!("\n> {question}");
        let response = agent.prompt(question).await?;
        println!("{response}");
    }

    Ok(())
}
```

### Running the Example

```bash
cd code-examples
export OPENAI_API_KEY=sk-...
cargo run -p ch04-tool-calling
```

Expected output (approximate — LLM phrasing varies):

```
> What is the weather in London?
The current weather in London is 12°C and overcast.

> What's the weather in Tokyo, and what is that in Fahrenheit?
Tokyo is currently 22°C, which is 71.6°F. It's also humid there.

> Convert 100°C to Kelvin.
100°C equals 373.2 K.
```

The second question triggers two tool calls — `get_weather` then `convert_temperature` — all within a single `.prompt()` invocation.

---

## 4.10 What Rig Handles vs. What You Handle

```
| Concern                                        | Rig | You |
|------------------------------------------------|-----|-----|
| Tool JSON schema generation                    | ✅  |     |
| Sending schemas to the LLM                     | ✅  |     |
| Detecting finish_reason == tool_calls          | ✅  |     |
| Routing to the right tool by name              | ✅  |     |
| Deserializing the LLM's argument JSON          | ✅  |     |
| Executing the tool function                    | ✅  |     |
| Adding tool result to conversation history     | ✅  |     |
| Sending the second LLM request                 | ✅  |     |
| Multi-turn tool chains (N tool calls in a row) | ✅  |     |
| The tool's actual logic                        |     | ✅  |
| Input validation inside the tool              |     | ✅  |
| Error messages (what goes back to the LLM)     |     | ✅  |
| External API calls inside the tool             |     | ✅  |
```

Rig handles the protocol; you handle the domain logic. This is the same division as LangChain4j's `AiServices` + `@Tool`.

---

## 4.11 Tool Calling vs. Function Calling: Terminology

You'll encounter both terms in documentation:

- **Function calling** — OpenAI's original term (GPT-4 era). Still appears in older blog posts and some `async-openai` type names.
- **Tool calling** — The current preferred term, used by OpenAI since late 2023 and adopted by all providers (Anthropic, Google, Mistral). A "tool" is more general than a "function" — it can be a code interpreter, file reader, or any capability.
- **LangChain4j's `@Tool`** — Uses the newer "tool" framing.
- **`#[rig_tool]`** — Same naming, Rust world.

In production logs, you may see `function_call` and `tool_calls` both appear depending on the provider and API version. They're the same underlying protocol.

---

## Key Takeaways

- Tool calling is a two-round-trip protocol: user message → LLM requests tool → execute → ask again with result → LLM answers. Every framework wraps this loop.
- `async-openai` exposes the raw protocol — understanding it lets you debug tool failures in any framework.
- `rig-core`'s `Tool` trait requires `definition()` (the JSON schema) and `call()` (the execution). `definition()` is `async fn`.
- `#[rig_tool]` from the `rig-derive` crate generates the `Tool` implementation from a function signature. Parameter descriptions go in `params(arg = "desc")`, not doc comments.
- Tool functions return `Result<T, ToolError>`. Use `thiserror` for custom `Tool::Error` types in manual implementations; the macro returns `ToolError` directly.
- `openai::Client::from_env()` returns `Result` — always unwrap with `?`.
- Stateful tools hold state (HTTP clients, API keys) in struct fields — owned by the tool, initialized once.

---

## Further Reading

- [rig-core tool module docs](https://docs.rs/rig-core/latest/rig/tool/index.html) — `Tool` trait, `ToolDefinition`, `ToolError`
- [rig-derive docs](https://docs.rs/rig-derive) — `#[rig_tool]` attribute macro parameters
- [rig-core agent_with_tools example](https://github.com/0xPlaygrounds/rig/blob/main/examples/agent_with_tools.rs) — the canonical manual `Tool` implementation
- [OpenAI Tool Calling guide](https://platform.openai.com/docs/guides/function-calling) — the underlying protocol
- [LangChain4j Tool Calling](https://docs.langchain4j.dev/tutorials/tools/) — Java reference for comparison

---

*Next: Chapter 5 — Structured Output: JSON from LLMs with Serde and Rig Extractors*
