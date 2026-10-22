# Contributing to seqviz-rs

Bug reports, documentation fixes and code contributions are welcome. For a new file format or a larger change to browser behaviour, start with an [issue](https://github.com/Georicl/seqviz-rs/issues) describing how it should work.

## Report a problem

Open an [issue](https://github.com/Georicl/seqviz-rs/issues) with:

- The output of `sq --version` and how you installed it: Homebrew, a release archive or source.
- Your OS and architecture, terminal application, and whether you use SSH or tmux.
- The command and key or mouse actions that reproduce the problem, with the expected and actual result.
- The file format and compression, a small synthetic example when possible, and any error text. The `F2` page shows file and index information; include any error text from the footer. Screenshots help with display problems.

For slow loading, include the file size and whether this is the first open or a repeat using an existing cache.

## Build and check

Use Rust stable with the `rustfmt` and `clippy` components, a C compiler, and Python 3 with `venv`. Run these commands from the repository root; they match the checks run by CI on Linux and macOS.

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked

python3 -m venv /tmp/seqviz-qa
/tmp/seqviz-qa/bin/pip install pyte==0.8.2
/tmp/seqviz-qa/bin/python .github/scripts/terminal_smoke.py
/tmp/seqviz-qa/bin/python .github/scripts/vcf_terminal_smoke.py
```

The two terminal checks exercise the release binary in a pseudoterminal (PTY), including navigation, selection, resizing and clipboard behaviour. They use temporary input files and send clipboard output through OSC 52 inside the test PTY. Set `SQ_BIN` if you need to test a different binary; otherwise it uses `target/release/sq`.

To browse the bundled examples manually:

```sh
cargo run --release --locked -- test/test.fa test/alignments.bam test/variants.vcf
```

## Where to start

| Area | Files |
| --- | --- |
| CLI parsing, startup and application shell | `src/main.rs`, `src/app/` |
| Shared input, cache and task services | `src/core/source.rs`, `src/core/cache.rs`, `src/core/tasks/` |
| Sequence readers and persistent indexes | `src/modules/sequence/data/` |
| VCF parsing, indexing and domain values | `src/modules/vcf/reader.rs`, `src/modules/vcf/data.rs`, `src/modules/vcf/model.rs` |
| Module registration, messages and sessions | `src/modules/registry.rs`, `src/modules/protocol.rs`, `src/modules/session.rs` |
| Document interface and format-specific pages | `src/modules/view.rs`, `src/modules/sequence/view/`, `src/modules/vcf/view/` |
| Reusable terminal components | `src/widgets/` |
| Clipboard backends | `src/core/clipboard/`, `src/app/terminal.rs` |
| Configuration, header attributes and text output | `src/app/config.rs`, `src/modules/sequence/config.rs`, `src/modules/sequence/palette.rs`, `src/modules/sequence/headers.rs`, `src/modules/sequence/output.rs` |

Keep dependencies flowing from the application assembly points to domain modules and shared services. `app` owns global input, tabs and terminal output; `core` owns file, cache, task and clipboard mechanisms; `widgets` renders reusable UI; each module owns its parsing, domain values, queries, interaction and colours. New format-specific fields and components belong in their module. Register a module through `registry`, route its work through `session` and expose its page through `DocumentView`.

Unit tests live beside the code. `tests/` contains integration tests, and `test/` holds the shared FASTA, SAM, BAM and VCF fixtures.

For a behaviour fix, add a regression test that fails before the fix and passes afterwards. Keep new fixtures small and synthetic. For terminal interactions, cover the affected state or layout in `src/app/tests.rs`, `src/modules/sequence/view/tests.rs` or `src/modules/vcf/view/tests.rs`; extend the PTY check when reproducing the problem requires real terminal events. Source files must remain read-only, and copying must remain disabled throughout SAM/BAM views.

## Send a pull request

Target `main` and keep each PR focused on one change. Describe the problem, the resulting behaviour and the checks you ran. Include reproduction steps or before/after examples when they help a reviewer understand the change.

Changes to `main` go through pull requests. Both the Linux and macOS checks must pass before merging.
