# claude-agent-rust-sdk-macros

Procedural macros of [`claude-agent-rust-sdk`](https://crates.io/crates/claude-agent-rust-sdk),
the Rust SDK for building agents on top of the Claude CLI.

Do not depend on this crate directly. It is used through the `macros` feature
of `claude-agent-rust-sdk`, which re-exports the `#[tool]` attribute and
requires this crate at its own exact version:

```toml
[dependencies]
claude-agent-rust-sdk = { version = "0.1", features = ["macros"] }
```

The attribute is documented with the library:
<https://docs.rs/claude-agent-rust-sdk>.

## License

Licensed under either of the
[MIT license](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/LICENSE-MIT)
or the
[Apache License, Version 2.0](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/LICENSE-APACHE),
at your option.
