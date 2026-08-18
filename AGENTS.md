# Repository instructions

- Use the normal repository and shell tools for inspection, editing, builds, and tests.
- Preserve the safety gates around removable-media writes and never select a fixed disk implicitly.
- Treat GUI/TUI parity as an invariant: every user-facing feature, state, label, section order, and
  error/retry behavior must be implemented in both interfaces in the same change. Rendering may
  adapt to terminal constraints, but capability and information hierarchy must not diverge.
