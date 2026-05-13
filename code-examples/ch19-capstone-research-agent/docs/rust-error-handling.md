# Error Handling in Rust

Rust models errors as values, not exceptions. There are no try/catch blocks.

## Result<T, E>

The primary error type is `Result<T, E>`, an enum with two variants:

```rust
enum Result<T, E> {
    Ok(T),   // success — contains the value
    Err(E),  // failure — contains the error
}
```

Callers must explicitly handle both variants. Ignoring a `Result` produces a compiler warning.

## The ? Operator

The `?` operator propagates errors up the call stack, equivalent to Java's checked exception
re-throw. It can only be used in functions that return `Result` or `Option`.

```rust
use anyhow::Result;

fn read_config(path: &str) -> Result<Config> {
    let text = std::fs::read_to_string(path)?;  // returns Err if file missing
    let config: Config = toml::from_str(&text)?; // returns Err if parse fails
    Ok(config)
}
```

## anyhow for Application Code

The `anyhow` crate provides a type-erased `anyhow::Error` that can hold any error type.
Use it in application code where you want to propagate errors without specifying types:

```rust
use anyhow::{Context, Result};

fn load_model(name: &str) -> Result<Model> {
    let path = find_model_path(name)
        .context("model not found in search path")?;
    Model::from_file(&path)
        .with_context(|| format!("failed to load model from {path:?}"))
}
```

## thiserror for Library Code

When writing a library, define typed errors with `thiserror`:

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("rate limit exceeded: retry after {retry_after}s")]
    RateLimit { retry_after: u64 },
    #[error("context window exceeded: {tokens} tokens > {limit} limit")]
    ContextOverflow { tokens: usize, limit: usize },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

Typed errors let callers match on specific variants; `anyhow::Error` does not.
