# Async/Await in Rust

Rust's async model lets you write concurrent code that looks synchronous.

## Futures are lazy

Unlike Java's `CompletableFuture`, a Rust `Future` does nothing until polled.
You typically poll a future by calling `.await` on it inside an async function.

```rust
async fn fetch_data() -> String {
    // This body runs only when the future is awaited.
    "data".to_string()
}

#[tokio::main]
async fn main() {
    let result = fetch_data().await;
    println!("{result}");
}
```

## Tokio

Tokio is the most popular async runtime for Rust. It provides:
- A multi-threaded scheduler
- Async I/O primitives (TCP, UDP, files)
- Timers and channels
- `tokio::spawn` for spawning concurrent tasks

## Channels

Tokio channels are how async tasks communicate:

```rust
let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(32);
tokio::spawn(async move { tx.send("hello".to_string()).await.unwrap(); });
while let Some(msg) = rx.recv().await {
    println!("{msg}");
}
```

## Comparison with Java

| Java | Rust |
|------|------|
| `CompletableFuture<T>` | `impl Future<Output = T>` |
| Eager (executes immediately) | Lazy (executes on `.await`) |
| `thenApply` | `.map` on streams, or sequential `.await` |
| Virtual threads (Java 21) | `tokio::spawn` |
