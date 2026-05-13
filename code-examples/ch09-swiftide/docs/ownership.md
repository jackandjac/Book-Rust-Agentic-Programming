# Ownership

Ownership is Rust's most unique feature. It enables Rust to make memory safety
guarantees without a garbage collector.

## Rules

1. Each value in Rust has an *owner*.
2. There can only be one owner at a time.
3. When the owner goes out of scope, the value is dropped.

## Move semantics

When you assign a value to another variable, the original variable is *moved*:

```rust
let s1 = String::from("hello");
let s2 = s1; // s1 is moved — it can no longer be used
```

## Clone

To keep both variables alive, use `.clone()`:

```rust
let s1 = String::from("hello");
let s2 = s1.clone();
println!("{s1} {s2}"); // both valid
```

## The stack vs the heap

Types that live entirely on the stack (integers, booleans, `f64`) implement the
`Copy` trait and are copied automatically. Heap-allocated types (like `String`)
are moved unless explicitly cloned.
