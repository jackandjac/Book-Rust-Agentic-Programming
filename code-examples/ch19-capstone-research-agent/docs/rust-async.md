# Async Programming in Rust

Rust's async system provides non-blocking I/O without threads, using zero-cost futures.

## Futures

A `Future` is a value that represents a computation that may not have completed yet.
Unlike Java's `CompletableFuture`, Rust futures are lazy — they do nothing until polled.

```rust
async fn fetch_data(url: &str) -> Result<String, reqwest::Error> {
    let body = reqwest::get(url).await?.text().await?;
    Ok(body)
}
```

## The `.await` Operator

`.await` suspends the current task until the future resolves. The thread is not blocked —
the runtime can schedule other tasks while waiting. This is equivalent to Java's
`CompletableFuture.thenCompose` but with direct sequential syntax.

## Tokio Runtime

Tokio is the dominant async runtime for Rust. It provides:

- A multi-threaded work-stealing scheduler
- Async I/O (TCP, UDP, files, timers)
- Channels (`mpsc`, `oneshot`, `broadcast`) for task communication
- `tokio::spawn` for background tasks

```rust
#[tokio::main]
async fn main() {
    let handle = tokio::spawn(async {
        // runs concurrently
        42
    });
    let result = handle.await.unwrap();
}
```

## Comparison to Java Virtual Threads (Java 21+)

| Feature | Rust async | Java virtual threads |
|---------|-----------|---------------------|
| Overhead per task | ~100 bytes | ~1 KB |
| Scheduling | Cooperative (explicit `.await`) | Preemptive |
| Blocking call safety | Must use `spawn_blocking` | Transparent |
| Zero-cost abstraction | Yes | No (carrier thread pinning) |
