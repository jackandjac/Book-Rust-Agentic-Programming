# Rust Ownership System

Rust's ownership system is the primary mechanism for memory safety without a garbage collector.

## Rules

Every value in Rust has a single owner. When the owner goes out of scope, the value is dropped
and its memory is freed. This is deterministic — it happens at a known point, not
non-deterministically like Java's garbage collector.

```rust
{
    let s = String::from("hello"); // s owns the String
}   // s goes out of scope; String is dropped here
```

## Borrowing

Instead of transferring ownership, you can borrow a value with references:

- `&T` — immutable borrow; many readers allowed simultaneously
- `&mut T` — mutable borrow; exactly one writer, no other borrows allowed

```rust
fn print_len(s: &str) {
    println!("{}", s.len()); // borrows s, does not own it
}
```

## Why This Prevents Memory Bugs

- **Use-after-free**: impossible — owner controls lifetime; no dangling pointers
- **Double-free**: impossible — exactly one owner, drops exactly once
- **Data races**: impossible — borrow checker enforces aliasing XOR mutation at compile time

The borrow checker enforces all these rules at compile time. No runtime checks needed.
