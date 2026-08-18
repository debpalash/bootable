# Contributing

Bootable welcomes focused fixes and verified media strategies.

Before submitting a change:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Every destructive path must preserve removable-device filtering, stable target identity checks,
explicit consequence review, and post-write verification. User-facing capabilities must ship in
both the GPUI and Ratatui interfaces in the same change. A visible control without working core
behavior does not count as implementation.

Do not copy code from Rufus, WoeUSB, or other projects whose license is incompatible with this
Apache-2.0 repository. Behavioral research and clean-room implementations are welcome.
