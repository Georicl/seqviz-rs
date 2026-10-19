# seqviz-rs

**Browse biological sequences without leaving your terminal.**

`seqviz-rs` ships the `sq` command for browsing nucleotide and protein FASTA, FASTQ, SAM, and BAM files. Open one file or several files in mixed formats, move between records, inspect sequence and quality data, and copy what you need — the source files are never modified.

This branch contains the first browser implementation. The interface is in English and targets macOS and Linux, including SSH sessions. Run it straight from the command line or through a blocking Yazi opener.

## Quick start

Run the bundled FASTA example from this checkout:

```sh
cargo run --release --locked -- test/test.fa
```

Open FASTA and BAM in separate tabs:

```sh
cargo run --release --locked -- test/test.fa test/alignments.bam
```

Press `Tab` to switch files, `?` for help, and `q` to quit.

To install the command from this checkout:

```sh
cargo install --path . --locked
sq --version
```

Building needs Rust and Cargo, plus a C compiler for the native dependencies: SQLite is bundled through `rusqlite`, and gzip decoding uses zlib through `libz-sys`. The repository checks use the stable Rust toolchain.

Once installed:

```sh
sq genome.fa
sq reads.fastq.gz
sq proteins.faa alignments.bam
sq -- "-sample.fa"
```

The browser opens only when both stdin and stdout are terminals. If you redirect stdout, `sq` falls back to printing plain, uncompressed FASTA; in that mode it neither starts the interactive browser nor decodes the other formats.

## Supported files

| Format | Browsing features |
| --- | --- |
| Nucleotide FASTA | Full header, sequence, length, GC and nucleotide colours |
| Protein FASTA | Full header, sequence, length in amino acids and residue-category colours |
| FASTQ | Wrapped sequence and quality fields, quality-coloured bases, quality overview and an optional aligned quality-character row |
| SAM / BAM | One entry per alignment, with SEQ, QUAL, reference, position, FLAG, MAPQ, CIGAR and optional fields |
| gzip / BGZF | Compressed text and BAM reading with persistent seek checkpoints |

FASTA extensions include `.fa`, `.fasta`, `.fna`, `.fas`, `.fsa`, `.faa`, and `.pep`; FASTQ uses `.fq` or `.fastq`. The extension selects the expected format, and the file contents are then verified while the file is opened and parsed.

FASTQ quality is Phred+33. Headers from Illumina, Nanopore, PacBio, and SRA are recognised, and the fields they carry are exposed; unrecognised headers remain available as plain text. For the current record, the browser shows mean, minimum, and maximum quality together with the Q30 percentage.

SAM/BAM records keep the sequence in its stored orientation. Alignments that share a QNAME stay separate rather than being merged, and missing sequence or quality is shown explicitly.

## Browser layout

```text
┌─ File tabs ───────────────────────────────────────────┐
│ Record list       │ Header and record properties      │
│                   │ Quality overview, when available  │
│                   │ Position  Sequence                │
│                   │           Quality characters      │
├───────────────────────────────────────────────────────┤
│ Reading status and keyboard shortcuts                 │
└───────────────────────────────────────────────────────┘
```

Sequence lines wrap to the terminal width. Positions are 1-based and shown in a separate gutter. Resizing keeps both the sequence position and the selection coordinates. Every open file keeps its own record, viewport, and selection while you switch tabs.

The information panel has four pages: **Help**, **File**, **Current record**, and **Statistics**. The current-record page shows the full header along with its parsed properties.

## Controls

| Action | Key or mouse operation |
| --- | --- |
| Move through records or sequence rows | Up / Down or `j` / `k` |
| Move the sequence cursor horizontally | Left / Right or `h` / `l` |
| Focus record list / sequence | `Ctrl+h` / `Ctrl+l`, or click the pane |
| Focus sequence from the record list | Enter or Right |
| Previous / next record | `[` / `]` |
| Page through the focused pane | Page Up / Page Down |
| First / last record or sequence position | Home / End |
| Search record names by partial match | `/`, enter a name, then Enter |
| Next name match | `n` |
| Jump within the current sequence | `:`, enter a 1-based position, then Enter |
| Next / previous file | Tab / Shift+Tab |
| Close the current file | `x` |
| Help / file / current record / statistics | F1 or `?` / F2 / F3 / F4 |
| Change information page | Tab / Shift+Tab or Left / Right |
| Select sequence | Mouse drag, or `v` followed by cursor movement |
| Copy | `y` |
| Show / hide quality characters | `Q` |
| Cancel work and pause background indexing | Esc in the browser |
| Return from an information panel | `q` |
| Quit the browser | `q` in the browser, or Ctrl+C |

The mouse wheel scrolls the pane under the pointer. A sequence drag can cross display lines, and dragging past a viewport edge scrolls the selection. Inside the information panel, Esc closes the panel and cancels background work; inside an input prompt, Esc closes that prompt.

## Copying

`y` copies from whichever pane has focus:

| Location | Clipboard content |
| --- | --- |
| FASTA record list | Full header and complete sequence, wrapped at 60 characters |
| FASTQ record list | Full header, sequence, original separator line and quality |
| Sequence pane | Selected sequence characters, without display line breaks, positions or quality characters |
| Information panel | Selected text, or the current page when no text is selected |

**Copying is disabled entirely for SAM/BAM**, including in the information panel.

Copying a whole record reads the record in full before sending it to the clipboard. Locally, copying uses `pbcopy` on macOS and `wl-copy`, `xclip`, or `xsel` on Linux. Over SSH, it sends an OSC 52 clipboard request to the terminal.

## Large files and statistics

The first viewport is read before the full scan begins. One UI thread handles drawing and input, while a single background worker handles reading, indexing, search, and statistics; the two communicate through Rust message channels. The worker prioritises the active file and checks for new requests between bounded scan steps.

Record metadata and sequence intervals live in a paged SQLite index, while sequence content stays in the source file. The UI receives only a bounded window of sequence and a page of record names.

| Input storage | Positioning method |
| --- | --- |
| Uncompressed | Source byte offsets and indexed sequence intervals |
| BGZF | Compression-block checkpoints |
| Ordinary gzip | Persistent DEFLATE positions, bit offsets, 32 KiB dictionaries and checksum state |

Partial and completed indexes can both be reused. When a file changes, a source-identity check invalidates the stale index. Reading an ordinary gzip file never creates a full uncompressed copy.

Open **Statistics** with `F4` to request file-wide results. Totals that are still unknown during the scan are marked as partial. The displayed metrics are:

- Record count, total length, minimum and maximum length, and mean length.
- N50 for FASTA and FASTQ.
- GC as `(G + C) / all sequence characters`, including ambiguous characters and gaps, displayed to two decimal places. Protein GC is not applicable.

SAM/BAM statistics count every alignment, including secondary and supplementary records, and do not deduplicate. Sequence metrics skip records with no SEQ, and N50 is not shown.

Recoverable SAM errors are counted and reported with their positions. If the structure is unrecoverable, scanning that file stops but the content already indexed remains available, and the other tabs keep working.

## Configuration

Configuration is optional: the built-in theme works without any files.

To customise colours, put [config/config.toml](config/config.toml) and [config/content.toml](config/content.toml) in `$XDG_CONFIG_HOME/seqviz-rs/`, or in `~/.config/seqviz-rs/` when `XDG_CONFIG_HOME` is unset:

```text
seqviz-rs/
├── config.toml
└── content.toml
```

The main configuration sets the interface colours and points to a separate content palette:

```toml
[display]
show_quality = true

[theme]
content = "content.toml"
background = "#181b23"
foreground = "#e0e5ef"
```

Content-palette paths are relative to the main configuration file. The palette defines nucleotide and amino-acid categories plus inclusive minimum Phred-score thresholds; omitted fields keep their defaults. There is a single configurable theme and no in-app theme switching.

Reading caches live in `$XDG_CACHE_HOME/seqviz-rs/`, falling back to `~/.cache/seqviz-rs/`.

## Yazi integration

Install `sq` on your PATH, then merge the matching opener example into your Yazi configuration:

| Yazi configuration syntax | Example |
| --- | --- |
| Yazi 25.4: `name` rules and shell arguments | [contrib/yazi.toml](contrib/yazi.toml) |
| Yazi 26.9: `url` rules and `%s` placeholders | [contrib/yazi-26.toml](contrib/yazi-26.toml) |

The opener sets `block = true`, so pressing Enter on a supported file opens `sq` in the same terminal and quitting returns you to Yazi. The integration is an opener, not a preview plugin.

## Code structure

| Module | Responsibility |
| --- | --- |
| [src/main.rs](src/main.rs) | CLI entry point and terminal/printing mode selection |
| [src/ui/](src/ui/) | Browser state, input handling, layout and rendering |
| [src/worker.rs](src/worker.rs) | Background scheduling, cancellation and UI messages |
| [src/data.rs](src/data.rs), [src/data/](src/data/) | Format parsing, SQLite record/interval indexes, range reads, statistics and record copying |
| [src/source.rs](src/source.rs) | Seekable plain, gzip and BGZF input |
| [src/headers.rs](src/headers.rs) | FASTQ header metadata parsing |
| [src/config.rs](src/config.rs) | Main theme and content-palette loading |
| [src/clipboard.rs](src/clipboard.rs) | System clipboard and OSC 52 output |
| [src/fa_reader.rs](src/fa_reader.rs), [src/display.rs](src/display.rs) | Streaming FASTA printing |

The interface is built with **Ratatui** and **Crossterm**. Persistent indexes use **rusqlite/SQLite**, compressed input uses **zlib** through `libz-sys`, and configuration uses **Serde + TOML**. Background work uses `std::thread`, `mpsc` channels, and atomic cancellation flags.

## Development and validation

Run the Rust checks from the repository root:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Tests cover record boundaries, wrapped FASTQ, compressed seeking, cache reuse and invalidation, background scheduling, and UI state. The SAM/BAM fixtures were generated with samtools, but running the tests does not require samtools.

For interactive acceptance checks, [scripts/terminal_smoke.py](scripts/terminal_smoke.py) runs the release binary in a pseudo-terminal. It exercises navigation, search, position jumps, resizing, quality display, information pages, SAM/BAM copy restrictions, and terminal restoration. Its Python dependency is separate from the application:

```sh
python3 -m venv /tmp/seqviz-qa
/tmp/seqviz-qa/bin/pip install pyte==0.8.2
cargo build --release --locked
/tmp/seqviz-qa/bin/python scripts/terminal_smoke.py
```

[GitHub Actions](.github/workflows/check.yml) defines these checks for Linux and macOS. [docs/performance.md](docs/performance.md) records measurements on generated data along with the commands to reproduce them; [examples/measure.rs](examples/measure.rs) measures data-layer startup, scanning, and indexed reads.

The [product manual](docs/product-manual.md) records the v1 design. VCF, editing, pasting, source-file writes, reference pileups, runtime file addition, and session restoration are reserved for later work.
