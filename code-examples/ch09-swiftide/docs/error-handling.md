# Error Handling in Rust

Rust uses the `Result<T, E>` type for recoverable errors instead of exceptions.

## Result and the ? operator

```rust
use std::fs;
use anyhow::Result;

fn read_config(path: &str) -> Result<String> {
    let content = fs::read_to_string(path)?;  // ? propagates the error
    Ok(content)
}
```

The `?` operator is equivalent to:
```rust
match fs::read_to_string(path) {
    Ok(v) => v,
    Err(e) => return Err(e.into()),
}
```

## anyhow for application code

`anyhow::Result<T>` is a type alias for `Result<T, anyhow::Error>`. It accepts
any error that implements `std::error::Error`, so you rarely need to think about
error type conversions in application code.

## thiserror for library code

When building a library, define your own error type with `thiserror`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MyError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
```

## Option<T>

`Option<T>` handles the absence of a value — Rust's replacement for `null`:

```rust
fn find_user(id: u64) -> Option<User> { /* ... */ }

if let Some(user) = find_user(42) {
    println!("Found: {}", user.name);
}
```

## Comparison with Java

| Java | Rust |
|------|------|
| Checked exceptions | `Result<T, E>` |
| `try/catch` | `match` on `Result` |
| `throws IOException` | `-> Result<T, io::Error>` |
| `Optional<T>` | `Option<T>` |
| `null` | `None` |
