# Contributing

Bugs, ideas and patches are welcome. Open an issue for anything bigger than a few lines so we can agree on the shape before you write it.

## Setting up

```
cargo build
cargo test
T0_ROOT=/tmp/t0-dev cargo run
```

`T0_ROOT` points t0 at a throwaway folder, so you never test against your own threads. `cargo clippy` should come back clean.

## What a good change looks like

- One thing per pull request. A fix and a refactor are two pull requests.
- Keep the file layout: `store.rs` touches the disk, `command.rs` changes threads, `keys.rs` maps keys, `ui.rs` draws. The README's Layout section says where everything lives.
- Every command that changes a thread records how to undo it. If you add one, add its `Step` too, and a case to the test in `command.rs`.
- A new key goes in three places: `keys.rs`, the `KEYS` table in `ui.rs`, and the table in the README.
- A new line of grammar goes in `grammar.rs` and in `GRAMMAR` there, which is what `--help` prints.
- `thread.md` is hand-edited by people. Whatever you write, the parser has to read back, and a line it does not understand has to survive as notes.

## Style

Match the file you are in. Short doc comments that say why, not what. No new dependencies without an issue first.

## Commits

One line, lowercase, no trailing period, says what changed. `git log` is the changelog.

## The gif

`docs/record.py` renders `docs/t0.gif` from a seeded tree. Re-run it after a visible change to the app. It needs `brew install agg`.
