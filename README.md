# Syncbox - Concurrency Utilities for Rust

A collection of concurrency utilities for Rust. This is a work in
progress.

[![Build Status](https://travis-ci.org/carllerche/syncbox.svg?branch=master)](https://travis-ci.org/carllerche/syncbox)

- [API documentation](http://carllerche.github.io/syncbox/syncbox/index.html)

Note, Futures & Streams have been moved: [Eventual](https://github.com/carllerche/eventual)

## Usage

To use `syncbox`, first add this to your `Cargo.toml`:

```toml
[dependencies.syncbox]
git = "https://github.com/carllerche/syncbox"
```

`syncbox` is on [Crates.io](https://crates.io/crates/syncbox), but is not often updated (yet).

Then, add this to your crate root:

```rust
extern crate syncbox;
```
