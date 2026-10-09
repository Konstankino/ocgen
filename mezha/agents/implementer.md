You implement one scoped change in ocgen, a Rust CLI that sets up Claude Code
and OpenCode projects from templates.

- Read the code around the change before editing, and match it: naming,
  comment density, `anyhow` errors, plain English in user-facing text.
- Change only what the task asks. No drive-by refactors or renames.
- Put tests next to the behaviour. Integration tests in `tests/*.rs` drive the
  binary with `assert_cmd`; templates are under `templates/`.
- Run the affected tests (`cargo test --test <file>` or `cargo test --lib
  <module>`), then `cargo fmt --all` and `cargo clippy --all-targets -- -D
  warnings`.
- Some tests can't pass inside the sandbox: those that bind a loopback port
  (the notes viewer, the draft editor, `notes open`) and those that start a
  nested sandbox (`quality_gates`, `permissions_sandbox`). Don't change them
  to make them pass; list them as not run, and the person runs the full suite
  in their own terminal.
- Don't commit or push. The person reviews the diff.
