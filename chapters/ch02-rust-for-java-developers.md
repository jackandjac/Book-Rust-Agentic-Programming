# Chapter 2: Rust for Java Developers

> **Framework versions in this chapter:** No AI framework dependencies — pure Rust language.  
> **Rust edition:** 2024 (requires rustc 1.85.0+)  
> **Java reference:** Java 21 with virtual threads, records, sealed classes.

---

## What You'll Learn

- Rust's ownership model and why it exists — mapped directly to Java's GC model
- Borrowing and references: Rust's safe alternative to Java's shared object references
- Traits vs. Java interfaces: the similarities, the surprises, and the orphan rule
- `Result<T, E>` and `Option<T>`: explicit error handling without checked exceptions or null
- `async/await` with Tokio vs. Java's `CompletableFuture` and virtual threads
- `serde`: Rust's Jackson equivalent for JSON serialization

This chapter is intentionally focused. Rust is a large language; we cover the subset you'll need to read and write the AI agent code in this book. For a complete treatment, see [The Rust Programming Language](https://doc.rust-lang.org/book/) (free, official, excellent).

---

## 2.1 Variables and the Type System

Let's start with the familiar and build toward what's different.

In Java, you declare variables with `var` (Java 10+) or explicit types:

```java
// Java
var message = "hello";          // type inferred as String
String greeting = "hello";      // explicit type
final int count = 42;           // immutable
```

In Rust:

```rust
// Rust
let message = "hello";          // type inferred as &str
let greeting: String = String::from("hello"); // explicit type
let count = 42i32;              // immutable by default
let mut counter = 0;            // mutable — requires explicit mut
```

**Key difference: immutability is the default.** In Java, variables are mutable unless you add `final`. In Rust, they're immutable unless you add `mut`. This isn't just style — the compiler enforces it. Trying to assign to a `let` binding without `mut` is a compile error.

### Two kinds of strings

Rust has two string types, which surprises Java developers:

```rust
let s1: &str = "hello";           // string slice — borrowed, lives in program binary
let s2: String = String::from("hello"); // owned, heap-allocated, growable
```

Think of `&str` as a read-only view into some string data (like Java's `String` in some ways), and `String` as the owned, mutable version (more like `StringBuilder`). In practice:

- String literals in your code are `&str`
- Strings you build at runtime (LLM responses, user input) are `String`
- Functions that just read a string typically accept `&str`; functions that own or modify a string use `String`

We'll return to why this distinction exists when we cover ownership.

---

## 2.2 Ownership: Why Rust Has No Garbage Collector

This is the concept Java developers find hardest — and also the most important. Take your time here.

### The Problem Ownership Solves

Java manages memory with a garbage collector: the JVM tracks which objects are still reachable, and periodically frees the ones that aren't. This works well and you rarely think about it. The trade-off is runtime overhead (GC pauses, CPU cycles for tracking) and nondeterminism (you don't know exactly when memory is freed).

Rust takes a different approach: instead of tracking memory at runtime, it enforces rules at compile time that make memory management both automatic and deterministic.

### The Three Ownership Rules

From the official Rust book — these are the complete rules:

1. Each value in Rust has an **owner**.
2. There can only be **one owner at a time**.
3. When the owner **goes out of scope**, the value is dropped (memory freed).

This sounds simple. The implications are significant.

### Rule in action: Move semantics

```rust
fn main() {
    let s1 = String::from("hello");
    let s2 = s1;  // s1's ownership MOVES to s2

    println!("{s1}"); // COMPILE ERROR: s1 was moved, it no longer exists
}
```

Coming from Java, this is counterintuitive. In Java, `s2 = s1` makes both variables point to the same object. In Rust, there's only ever one owner — so `s2 = s1` transfers ownership. `s1` is gone.

The equivalent Java scenario would be passing an object to a method that "consumes" it — except in Java, you can still use the original reference afterward, which can cause bugs. Rust's compiler prevents that class of bug entirely.

### Copy types: the exception

Primitive types — integers, booleans, floats, chars — implement the `Copy` trait. For these, assignment copies the value rather than moving it:

```rust
fn main() {
    let x = 5;
    let y = x;  // x is COPIED, not moved

    println!("x={x}, y={y}"); // Both are valid — Copy types are fine
}
```

This mirrors Java's behavior for primitive types (`int`, `boolean`, etc.) vs. reference types (`String`, `Object`).

### Ownership and functions

Passing a value to a function moves it — the function becomes the owner:

```rust
fn main() {
    let s = String::from("hello");
    takes_ownership(s);    // s moves into the function

    // println!("{s}"); // COMPILE ERROR: s was moved into takes_ownership
}

fn takes_ownership(some_string: String) {
    println!("{some_string}");
} // some_string goes out of scope here — memory is freed automatically
```

This is the ownership system doing what the GC does in Java, but at compile time, with zero runtime overhead.

---

## 2.3 Borrowing: Sharing Without Transferring Ownership

Moving ownership everywhere would be impractical — you'd have to return every value from every function to use it again. Rust solves this with **borrowing**: temporarily lending a reference to a value without transferring ownership.

```rust
fn main() {
    let s = String::from("hello");
    let length = calculate_length(&s); // borrow s with &

    println!("'{s}' has length {length}"); // s still valid — we only borrowed it
}

fn calculate_length(s: &String) -> usize {
    s.len()
} // s goes out of scope, but since it's a reference (not owner), nothing is freed
```

The `&` creates a reference — a pointer to the value that doesn't own it. When the reference goes out of scope, ownership isn't affected.

### Mutable references

References are immutable by default. To mutate through a reference, use `&mut`:

```rust
fn main() {
    let mut s = String::from("hello");
    append_world(&mut s);
    println!("{s}"); // "hello, world"
}

fn append_world(s: &mut String) {
    s.push_str(", world");
}
```

### The borrowing rules (the borrow checker)

Rust enforces two rules about references at compile time:

1. You can have **any number of immutable references** (`&T`) to a value at the same time.
2. You can have **exactly one mutable reference** (`&mut T`) — and when you do, no immutable references can exist simultaneously.

This is the borrow checker. Its job is to prevent data races at compile time. In Java, you guard concurrent access with `synchronized` or `ReentrantLock` — and if you forget, the bug surfaces at runtime. In Rust, the compiler rejects code that would allow concurrent mutable access:

```rust
fn main() {
    let mut s = String::from("hello");

    let r1 = &s;      // immutable borrow — ok
    let r2 = &s;      // another immutable borrow — ok
    let r3 = &mut s;  // COMPILE ERROR: cannot borrow as mutable
                      // because it is also borrowed as immutable

    println!("{r1}, {r2}, {r3}");
}
```

This rule makes Rust's concurrency guarantees possible. When you spawn tasks that share data in Chapter 9, the borrow checker will guide you toward patterns that are safe by construction.

### Java mental model mapping

| Java concept | Rust equivalent |
|-------------|----------------|
| Object reference (`MyObj obj`) | Owned value (`let obj: MyObj`) |
| Passing object to method | Move (ownership transfer) or borrow (`&`) |
| Reading shared data across threads | Multiple `&T` references (safe) |
| `synchronized` write access | `&mut T` + single-writer rule |
| Garbage collection | Ownership drop at end of scope |
| `final` reference to mutable object | `let` binding to `mut` data |

---

## 2.4 Structs: Rust's Alternative to Java Classes

Rust doesn't have classes. It has **structs** for data, and `impl` blocks for methods.

```rust
// Rust
struct ChatMessage {
    role: String,
    content: String,
}

impl ChatMessage {
    // Constructor convention: an associated function named `new`
    // `Self` is an alias for the type this impl block belongs to (ChatMessage here)
    fn new(role: &str, content: &str) -> Self {
        Self {  // equivalent to writing `ChatMessage { ... }` — same type
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    // Method — note &self (immutable borrow of self)
    fn is_user_message(&self) -> bool {
        self.role == "user"
    }

    // Mutable method — note &mut self
    fn append(&mut self, text: &str) {
        self.content.push_str(text);
    }
}

fn main() {
    let mut msg = ChatMessage::new("user", "Hello");
    msg.append(", world!");
    println!("Is user: {}", msg.is_user_message());
}
```

The Java equivalent:

```java
// Java
public class ChatMessage {
    private final String role;
    private String content;

    public ChatMessage(String role, String content) {
        this.role = role;
        this.content = content;
    }

    public boolean isUserMessage() {
        return role.equals("user");
    }

    public void append(String text) {
        this.content += text;
    }
}
```

Key differences:
- No `new` keyword at call site — `ChatMessage::new(...)` is just a function
- No `private`/`public` fields — Rust uses modules for visibility (fields are private to the module by default)
- `&self` vs `&mut self` makes immutability explicit in each method signature

### Deriving common behavior

Rust has derive macros that auto-implement common traits — similar to what Lombok does in Java:

```rust
#[derive(Debug, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

// Now you can:
let msg = ChatMessage { role: "user".to_string(), content: "hi".to_string() };
println!("{msg:?}");        // Debug printing — like @ToString in Lombok
let msg2 = msg.clone();     // Clone — like implementing Cloneable
```

| Lombok annotation | Rust derive |
|-------------------|-------------|
| `@ToString` | `#[derive(Debug)]` + `{:?}` |
| `@EqualsAndHashCode` | `#[derive(PartialEq, Eq, Hash)]` |
| `@Clone` (manual) | `#[derive(Clone)]` |
| `@Getter`/`@Setter` | No equivalent — access fields directly or write methods |

We'll add `#[derive(Serialize, Deserialize)]` from `serde` in section 2.7 — that's the Jackson equivalent.

---

## 2.5 Traits: More Powerful Than Java Interfaces

A trait defines shared behavior. The syntax looks similar to a Java interface:

```rust
// Rust trait
pub trait Summarize {
    fn summarize(&self) -> String;
}

// Default implementation (like Java interface default methods)
pub trait Greet {
    fn greet(&self) -> String {
        String::from("Hello!")  // default — can be overridden
    }
}
```

Implementing a trait:

```rust
struct BlogPost {
    title: String,
    author: String,
    content: String,
}

impl Summarize for BlogPost {
    fn summarize(&self) -> String {
        format!("{}, by {}", self.title, self.author)
    }
}
```

So far this looks like Java. Here's where it diverges.

### Trait bounds: generic functions with constraints

In Java, you'd use an interface as a parameter type:

```java
// Java
public void display(Summarizable item) {
    System.out.println(item.summarize());
}
```

In Rust, you use trait bounds:

```rust
// Rust — two equivalent syntaxes
fn display(item: &impl Summarize) {
    println!("{}", item.summarize());
}

// Or with explicit generic syntax (more flexible):
fn display<T: Summarize>(item: &T) {
    println!("{}", item.summarize());
}
```

Multiple trait bounds (like implementing multiple Java interfaces):

```rust
use std::fmt::Display;

fn display_and_summarize<T: Summarize + Display>(item: &T) {
    println!("Display: {item}");
    println!("Summary: {}", item.summarize());
}
```

### The orphan rule: a key constraint

Rust enforces the **orphan rule**: you can only implement a trait on a type if **either the trait or the type** is defined in your crate. You can't implement someone else's trait on someone else's type.

This prevents conflicts — imagine two libraries both trying to implement `Display` for `String`. In Java, this isn't an issue because method dispatch goes through the object, not the interface. In Rust, traits are resolved statically, so conflicts must be impossible by construction.

**Practical implication:** You can implement `serde::Serialize` (from the `serde` crate) on your own `ChatMessage` struct (your type, external trait — allowed). You cannot implement `serde::Serialize` on `String` (both external — not allowed). In practice, `serde` derives handle this with macros.

### Traits used heavily in Rust AI code

These traits appear throughout the book — knowing them now will help:

| Trait | What it does | Java analogy |
|-------|-------------|-------------|
| `Debug` | Printable via `{:?}` | `toString()` for debugging |
| `Display` | Printable via `{}` | `toString()` for users |
| `Clone` | Explicit deep copy | `Cloneable` |
| `Send` | Safe to send across threads | No equivalent (enforced at compile time) |
| `Sync` | Safe to reference from multiple threads | No equivalent |
| `Future` | Async computation | `CompletableFuture` conceptually |
| `From`/`Into` | Type conversions | No direct equivalent |

`Send` and `Sync` are **marker traits** — they have no methods. The compiler uses them to enforce thread safety:

- A type is `Send` if it can be **moved** to another thread (ownership transfer across thread boundary)
- A type is `Sync` if references to it can be **shared** across threads simultaneously

The compiler auto-derives these for most types. The common case where it doesn't:

| Type | `Send`? | `Sync`? | Why |
|------|---------|---------|-----|
| `Rc<T>` | ❌ | ❌ | Reference counting is not atomic — two threads incrementing the count is a data race |
| `Arc<T>` | ✅ | ✅ | Atomic reference counting — safe across threads |
| `RefCell<T>` | ✅ | ❌ | Interior mutability without locks — reading from two threads simultaneously is unsafe |
| `Mutex<T>` | ✅ | ✅ | Locks protect the inner value — safe across threads |

**The practical rule for async agent code:** When you see `the trait Send is not implemented for Rc<...>`, replace `Rc` with `Arc`. When you see it for `RefCell<...>`, wrap the data in `Mutex<T>` instead. The compiler is telling you the type isn't safe to share across async tasks — the fix is to use the thread-safe version.

Java doesn't have this concept because all Java objects are heap-allocated and reference-counted by the GC, which is inherently thread-safe (the GC handles the reference counting). Rust makes the choice explicit at the type level.

---

## 2.6 Enums with Data: More Than Java's Enums

Before covering `Result` and `Option`, we need to understand Rust enums — because `Result` and `Option` are both enums.

In Java, enums are named constants with optional fields. In Rust, enum variants can hold different types of data:

```rust
// Java-style enum — Rust supports this too
enum Direction {
    North,
    South,
    East,
    West,
}

// Rust enum with data — no Java equivalent
enum LlmResponse {
    Text(String),                          // text response
    ToolCall { name: String, args: String }, // tool invocation
    Error(String),                         // failure
}
```

You handle enum variants with `match` — exhaustive pattern matching that the compiler enforces:

```rust
fn handle_response(response: LlmResponse) {
    match response {
        LlmResponse::Text(content) => println!("Text: {content}"),
        LlmResponse::ToolCall { name, args } => println!("Tool: {name}({args})"),
        LlmResponse::Error(msg) => eprintln!("Error: {msg}"),
        // Compiler error if you miss a variant — can't accidentally ignore a case
    }
}
```

You'll see this pattern throughout `rig-core` and `async-openai` APIs — responses come back as enums with variants for different message types. Understanding this pattern is essential for reading and writing Rust AI code.

---

## 2.7 Result and Option: Explicit Error Handling

> **Note:** `Result` and `Option` are both Rust enums — re-read §2.6 if you skipped it.

This is one of Rust's biggest departures from Java — and one of the biggest improvements for agent code.

### Option<T>: no null

```rust
enum Option<T> {
    Some(T),   // a value exists
    None,      // no value
}
```

Instead of returning `null` or `Optional<T>`, Rust functions return `Option<T>`. The compiler forces you to handle both cases:

```rust
fn find_user(id: u32) -> Option<String> {
    if id == 1 {
        Some(String::from("Alice"))
    } else {
        None
    }
}

fn main() {
    let user = find_user(42);

    // Must handle both cases — compiler won't let you ignore None
    match user {
        Some(name) => println!("Found: {name}"),
        None => println!("No user found"),
    }

    // Shorthand options:
    let user2 = find_user(1).unwrap_or_else(|| String::from("anonymous"));
    let user3 = find_user(1)?; // In functions returning Option — propagates None
}
```

Comparison to Java `Optional`:

```java
// Java — Optional is a wrapper you can accidentally unwrap unsafely
Optional<String> user = findUser(42);
String name = user.get(); // throws NoSuchElementException if empty — runtime error
String safe = user.orElse("anonymous"); // safe
```

```rust
// Rust — None is part of the type system; you can't accidentally unwrap
let name = find_user(42).unwrap(); // panics at runtime if None — use sparingly
let safe = find_user(42).unwrap_or("anonymous".to_string()); // safe
```

In agent code, `Option` appears constantly: the LLM might not return a tool call, a document might not have a summary, a config key might be absent. Handling these explicitly prevents the class of bugs where a missing value causes a NPE deep in a call stack.

### Result<T, E>: no unchecked exceptions

```rust
enum Result<T, E> {
    Ok(T),    // success, contains the value
    Err(E),   // failure, contains the error
}
```

A function that can fail returns `Result`. The caller must handle both cases. Here's reading a file — the Java way and the Rust way:

```java
// Java — exception is invisible in the signature
public String readFile(String path) {
    try {
        return Files.readString(Path.of(path));
    } catch (IOException e) {
        throw new RuntimeException(e); // or handle it
    }
}
```

```rust
// Rust — failure is visible in the return type
fn read_file(path: &str) -> Result<String, std::io::Error> {
    std::fs::read_to_string(path) // returns Result<String, io::Error>
}
```

### The `?` operator: concise error propagation

Handling `Result` with `match` is verbose. The `?` operator is the shorthand — it means "unwrap the `Ok` value, or return the `Err` to my caller":

```rust
use std::fs;

fn read_config(path: &str) -> Result<String, std::io::Error> {
    let contents = fs::read_to_string(path)?; // if Err, return it immediately
    Ok(contents.trim().to_string())
}
```

This is equivalent to:

```rust
fn read_config(path: &str) -> Result<String, std::io::Error> {
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return Err(e),
    };
    Ok(contents.trim().to_string())
}
```

The `?` operator is what you'll see in virtually every function in this book. It makes error propagation feel as natural as Java's exception propagation, but with the error path visible in the type signature.

### anyhow: the pragmatic error crate

All code examples in this book use the `anyhow` crate for error handling in applications. It provides `anyhow::Result<T>` — a `Result` that accepts *any* error type, so you don't have to specify the exact error type in every function:

```rust
use anyhow::Result; // anyhow::Result<T> = Result<T, anyhow::Error>

async fn call_llm(prompt: &str) -> Result<String> {
    let client = async_openai::Client::new();
    // ... LLM call ...
    // Any error type works with ? here
    Ok(response)
}
```

This mirrors how Java developers use `RuntimeException` for application code — wrap any error and propagate it. All examples in this book are applications, so `anyhow::Result` is always appropriate here. (The `thiserror` crate is for when you're writing a library and need callers to match on specific error variants — we won't need it in this book.)

---

## 2.8 serde: Rust's Jackson

`serde` is Rust's serialization/deserialization framework. In LangChain4j, you annotate a POJO with Jackson annotations and get JSON automatically. In Rust, you derive `Serialize`/`Deserialize` on a struct.

Add to `Cargo.toml`:

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Define a struct:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct ToolResult {
    tool_name: String,
    output: String,
    success: bool,
}
```

Serialize to JSON:

```rust
let result = ToolResult {
    tool_name: "weather".to_string(),
    output: "72°F, sunny".to_string(),
    success: true,
};

let json = serde_json::to_string(&result)?;
println!("{json}");
// {"tool_name":"weather","output":"72°F, sunny","success":true}
```

Deserialize from JSON:

```rust
let json = r#"{"tool_name":"weather","output":"72°F, sunny","success":true}"#;
let result: ToolResult = serde_json::from_str(json)?;
println!("{:?}", result);
```

| Jackson (Java) | serde (Rust) |
|---------------|-------------|
| `@JsonProperty("tool_name")` | `#[serde(rename = "tool_name")]` |
| `@JsonIgnore` | `#[serde(skip)]` |
| `ObjectMapper.writeValueAsString()` | `serde_json::to_string()` |
| `ObjectMapper.readValue()` | `serde_json::from_str()` |
| `@JsonInclude(NON_NULL)` | `#[serde(skip_serializing_if = "Option::is_none")]` |

LLM APIs return JSON. Tool calls send and receive JSON. `serde` is in every chapter of this book. The `#[derive(Serialize, Deserialize)]` annotation is as fundamental to Rust AI code as `@Data` is to Lombok users.

---

## 2.9 Async/Await: CompletableFuture Without the Boilerplate

LLM API calls are inherently I/O-bound — you send a request and wait for a response that may take 1–30 seconds. Doing this synchronously would block the entire thread. Java and Rust both have async models; they work similarly at the surface but differ in important ways.

### The Tokio runtime

Rust's async/await is part of the language, but the **runtime** is not in the standard library. Every Rust AI application uses Tokio. Add it to `Cargo.toml`:

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
```

The `#[tokio::main]` attribute transforms your `async fn main()` into a synchronous main that starts the Tokio runtime:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Running on Tokio");
    Ok(())
}
```

Think of `#[tokio::main]` as the equivalent of Spring Boot's `SpringApplication.run()` — it boots the machinery that makes async work.

### async fn and .await

Mark a function `async` to make it return a `Future`. Use `.await` to wait for a `Future` to complete without blocking the thread:

```rust
use anyhow::Result;

async fn fetch_joke() -> Result<String> {
    // Simulating an LLM call
    let response = call_llm("Tell me a short joke").await?;
    Ok(response)
}

#[tokio::main]
async fn main() -> Result<()> {
    let joke = fetch_joke().await?;
    println!("{joke}");
    Ok(())
}
```

Java `CompletableFuture` equivalent:

```java
// Java
CompletableFuture<String> future = fetchJoke();
String joke = future.join(); // blocks until complete
System.out.println(joke);
```

`.await` in Rust is non-blocking in the async context (it yields to the Tokio scheduler while waiting), while `.join()` in Java blocks the calling thread.

### tokio::spawn: running tasks concurrently

To run multiple async tasks concurrently — like calling multiple LLM tools in parallel — use `tokio::spawn`:

```rust
use tokio::task::JoinHandle;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Spawn two tasks — they run concurrently
    let handle1: JoinHandle<String> = tokio::spawn(async {
        call_weather_tool("London").await.unwrap()
    });

    let handle2: JoinHandle<String> = tokio::spawn(async {
        call_search_tool("Rust programming").await.unwrap()
    });

    // Wait for both to complete
    let weather = handle1.await?;
    let search = handle2.await?;

    println!("Weather: {weather}");
    println!("Search: {search}");
    Ok(())
}
```

Java equivalent with `CompletableFuture`:

```java
// Java
CompletableFuture<String> f1 = CompletableFuture.supplyAsync(() -> callWeatherTool("London"));
CompletableFuture<String> f2 = CompletableFuture.supplyAsync(() -> callSearchTool("Rust"));

String weather = f1.join();
String search  = f2.join();
```

Key differences:
- `tokio::spawn` returns `JoinHandle<T>` — `.await`ing it gives `Result<T, JoinError>`
- Tokio tasks are lightweight (64 bytes per task) — you can spawn thousands; Java threads are OS threads with ~1MB stack
- Java 21 virtual threads narrow this gap considerably, but Tokio's model is still more explicit about async boundaries

### tokio::join!: awaiting multiple futures without spawning

For cases where both futures are I/O-bound (like two LLM API calls), `join!` is simpler than `spawn`:

```rust
use tokio::join;

let (weather, search) = join!(
    call_weather_tool("London"),
    call_search_tool("Rust programming")
);
```

**`join!` vs `spawn` — when to use which:**

| | `tokio::join!` | `tokio::spawn` |
|-|---------------|----------------|
| **Runs on** | Current task (cooperative) | Separate worker thread (potentially parallel) |
| **Best for** | I/O-bound work (network calls, LLM API) | CPU-bound work or independent long-running tasks |
| **Overhead** | Nearly zero | Small allocation per task (~64 bytes) |
| **Error handling** | Results returned inline | `JoinHandle::await` returns `Result<T, JoinError>` |

In agent code, most tool calls are I/O-bound (HTTP calls to APIs), so `join!` is usually the right choice for running multiple tools in parallel. Use `spawn` when a task might run for a long time independently (a background indexing job) or when you need to cancel it.

### The async/await mental model

```
Java thread model:
[Thread 1] ──────────────── waiting for HTTP ──────────────── response
(thread is BLOCKED, consuming OS thread resources while waiting)

Tokio async model:
[Task 1] ────── yields ──── [Task 2 runs] ──── [Task 1 resumes]
(thread is NOT blocked; Tokio scheduler runs other tasks while Task 1 waits)
```

For AI agent code — where every LLM call is a network wait — this model means one OS thread can serve hundreds of concurrent agent sessions. This is why `async-openai` and `rig-core` are both async libraries built on Tokio.

---

## 2.10 Putting It Together: A Minimal Async Program

Here's a complete, runnable program that uses everything from this chapter — structs, traits, Result, serde, and async/await:

```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};

// Struct with serde derives
#[derive(Debug, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

impl Message {
    fn new(role: &str, content: &str) -> Self {
        Message {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    fn is_assistant(&self) -> bool {
        self.role == "assistant"
    }
}

// A trait for anything that can produce a response
trait Responder {
    fn respond(&self, input: &str) -> String;
}

// A simple echo responder (placeholder for LLM in Chapter 3)
struct EchoResponder;

impl Responder for EchoResponder {
    fn respond(&self, input: &str) -> String {
        format!("Echo: {input}")
    }
}

// Async function returning Result
async fn process_message(responder: &impl Responder, input: &str) -> Result<Message> {
    // In Chapter 3, this becomes a real LLM call
    let response_text = responder.respond(input);
    let message = Message::new("assistant", &response_text);
    Ok(message)
}

#[tokio::main]
async fn main() -> Result<()> {
    let responder = EchoResponder;

    let reply = process_message(&responder, "Hello, Rust!").await?;

    println!("Role: {}", reply.role);
    println!("Content: {}", reply.content);
    println!("Is assistant: {}", reply.is_assistant());

    // Serialize to JSON
    let json = serde_json::to_string_pretty(&reply)?;
    println!("\nAs JSON:\n{json}");

    Ok(())
}
```

Expected output:

```
Role: assistant
Content: Echo: Hello, Rust!
Is assistant: true

As JSON:
{
  "role": "assistant",
  "content": "Echo: Hello, Rust!"
}
```

This is the skeleton of every AI agent component in this book. In Chapter 3, `EchoResponder` becomes a real LLM client, and `process_message` becomes a streaming chat function.

To run this example from the companion repository:

```bash
cd code-examples/ch03-llm-basics  # we'll build this in Chapter 3
cargo run
```

---

## 2.11 Common Compile Errors and What They Mean

You will see these. Understanding them now saves hours later.

### "cannot borrow as mutable because it is also borrowed as immutable"

```
error[E0502]: cannot borrow `s` as mutable because it is also borrowed as immutable
```

**What it means:** You have an active immutable reference (`&s`) and are trying to create a mutable reference (`&mut s`) at the same time. The borrow checker won't allow it.

**Fix:** Ensure the immutable reference is no longer in scope before creating the mutable one, or restructure the code to avoid needing both simultaneously.

### "does not live long enough"

```
error[E0597]: `s` does not live long enough
```

**What it means:** A reference outlives the value it points to. You're storing a reference somewhere that will exist longer than the value being referenced.

**Fix:** Usually means you need to store an owned `String` instead of a borrowed `&str`, or restructure ownership so the owned value lives long enough.

### "the trait `Send` is not implemented for..."

```
error[E0277]: `Rc<RefCell<i32>>` cannot be sent between threads safely
```

**What it means:** You're trying to move a value into a `tokio::spawn` task that isn't thread-safe. `Rc` (reference-counted pointer) is not thread-safe; `Arc` (atomic reference-counted) is.

**Fix:** Replace `Rc<T>` with `Arc<T>`, and `RefCell<T>` with `Mutex<T>` when crossing async task boundaries.

### "expected `&str`, found `String`" (and vice versa)

**What it means:** String types don't coerce automatically in function calls.

**Fix:** Use `.as_str()` to go from `String` to `&str`, or `.to_string()` / `String::from()` to go the other way.

---

## Key Takeaways

- Rust's **ownership model** eliminates garbage collection by enforcing at compile time that each value has one owner and is freed when the owner goes out of scope
- **Borrowing** (`&T`, `&mut T`) lets you use values without transferring ownership — the borrow checker ensures no data races are possible
- **Traits** are more powerful than Java interfaces: they support generics, multiple bounds, and default implementations — but the orphan rule means you can't implement external traits on external types
- **`Result<T, E>`** and **`Option<T>`** make failure and absence explicit in function signatures — the `?` operator propagates errors concisely
- **`serde`** with `#[derive(Serialize, Deserialize)]` is the Jackson of Rust — expect it in every chapter
- **Tokio** is the async runtime; `async fn` + `.await` is conceptually similar to `CompletableFuture` but with the scheduler explicitly managed by Tokio rather than the JVM
- Compile errors are the borrow checker teaching you the rules — they prevent runtime bugs

---

## Further Reading

- [The Rust Programming Language, Ch. 4](https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html) — ownership in full depth
- [The Rust Programming Language, Ch. 10](https://doc.rust-lang.org/book/ch10-02-traits.html) — traits in full depth
- [Tokio Tutorial](https://tokio.rs/tokio/tutorial) — async/await and Tokio from first principles
- [serde.rs](https://serde.rs) — serde documentation with all derive attributes
- [anyhow crate](https://docs.rs/anyhow) — pragmatic error handling for applications
- [Rust async book](https://rust-lang.github.io/async-book/) — deep dive into Futures and async runtimes

---

*Next: Chapter 3 — LLM Basics in Rust: async-openai and Your First Streaming Chat*
