# seqviz-rs

**Browse biological sequences directly in your terminal.**

`seqviz-rs` provides the `sq` command for browsing nucleotide and protein FASTA, FASTQ, SAM and BAM files. Open one file or several mixed-format files, move between records, inspect sequence and quality information, and copy the content you need. Source files remain unchanged.

The `dev` branch contains the first browser implementation. The interface is in English and targets macOS and Linux, including SSH terminals. It runs directly from the command line or through a blocking Yazi opener.

## Quick start

From this checkout, run the included FASTA example in a terminal:

```sh
cargo run --release --locked -- test/test.fa
```

Open FASTA and BAM in separate tabs:

```sh
cargo run --release --locked -- test/test.fa test/alignments.bam
```

Press `Tab` to switch files, `?` for help, and `q` to exit.

To install the command from this checkout:

```sh
cargo install --path . --locked
sq --version
```

Building requires Rust and Cargo, plus a C compiler for the native dependencies. SQLite is bundled through `rusqlite`; gzip decoding uses zlib through `libz-sys`. The repository's checks use the stable Rust toolchain.

After installation:

```sh
sq genome.fa
sq reads.fastq.gz
sq proteins.faa alignments.bam
sq -- "-sample.fa"
```

The browser starts when both stdin and stdout are terminals. Redirecting stdout retains the plain, uncompressed FASTA printing mode; this mode does not open the interactive browser or decode the other formats.

## Supported files

| Format | Browsing features |
| --- | --- |
| Nucleotide FASTA | Full header, sequence, length, GC and nucleotide colours |
| Protein FASTA | Full header, sequence, length in amino acids and residue-category colours |
| FASTQ | Wrapped sequence and quality fields, quality-coloured bases, quality overview and an optional aligned quality-character row |
| SAM / BAM | One entry per alignment, with SEQ, QUAL, reference, position, FLAG, MAPQ, CIGAR and optional fields |
| gzip / BGZF | Compressed text and BAM reading with persistent seek checkpoints |

FASTA extensions include `.fa`, `.fasta`, `.fna`, `.fas`, `.fsa`, `.faa` and `.pep`. FASTQ uses `.fq` or `.fastq`. File contents are checked during opening and parsing.

FASTQ quality uses Phred+33. Recognised Illumina, Nanopore, PacBio and SRA headers expose the fields present in the input; unrecognised headers remain available as text. The current record shows mean, minimum and maximum quality, and the Q30 percentage.

SAM/BAM records retain their stored sequence orientation. Alignments with the same QNAME remain separate, and missing sequence or quality is shown explicitly.

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

Sequence lines wrap to the available terminal width. Positions are 1-based and displayed in a separate gutter. Resizing preserves the sequence position and selection coordinates. Each open file retains its own record, viewport and selection when switching tabs.

The information panel has four pages: **Help**, **File**, **Current record** and **Statistics**. The current-record page includes the full header and parsed properties.

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

Mouse-wheel scrolling acts on the pane under the pointer. Sequence dragging can cross display lines; dragging at a viewport edge scrolls the selection. Inside the information panel, Esc closes the panel and cancels background work. Inside an input prompt, Esc closes that prompt.

## Copying

The focused pane determines what `y` copies:

| Location | Clipboard content |
| --- | --- |
| FASTA record list | Full header and complete sequence, wrapped at 60 characters |
| FASTQ record list | Full header, sequence, original separator line and quality |
| Sequence pane | Selected sequence characters, without display line breaks, positions or quality characters |
| Information panel | Selected text, or the current page when no text is selected |

**All copying is disabled for SAM/BAM**, including the information panel.

Complete-record copying reads the whole selected record before sending it to the clipboard. Local copying uses `pbcopy` on macOS or `wl-copy`, `xclip` or `xsel` on Linux. SSH copying sends an OSC 52 clipboard request to the terminal.

## Large files and statistics

The first viewport is read before a complete scan. A UI thread handles drawing and input, while one background worker handles reading, indexing, search and statistics. They communicate through Rust message channels. The worker prioritises the active file and checks new requests between bounded scan steps.

Record metadata and sequence intervals are stored in a paged SQLite index. Sequence content stays in the source file; the UI receives a bounded window of sequence and a page of record names.

| Input storage | Positioning method |
| --- | --- |
| Uncompressed | Source byte offsets and indexed sequence intervals |
| BGZF | Compression-block checkpoints |
| Ordinary gzip | Persistent DEFLATE positions, bit offsets, 32 KiB dictionaries and checksum state |

Partial and completed indexes can be reused. Source identity checks invalidate stale indexes after a file changes. Ordinary gzip reading does not create a complete uncompressed copy.

Open **Statistics** with `F4` to request file-wide results. Unknown totals are marked as partial while scanning. The displayed metrics are:

- Record count, total length, minimum and maximum length, and mean length.
- N50 for FASTA and FASTQ.
- GC as `(G + C) / all sequence characters`, including ambiguous characters and gaps, displayed to two decimal places. Protein GC is not applicable.

SAM/BAM statistics count every alignment, including secondary and supplementary records, without deduplication. Sequence metrics exclude records with missing SEQ; N50 is not displayed.

Recoverable SAM errors are counted and reported with their positions. An unrecoverable structure stops scanning that file while keeping already indexed content available. Other file tabs remain usable.

## Configuration

Configuration is optional. The built-in theme works without any files.

For custom colours, place [config/config.toml](config/config.toml) and [config/content.toml](config/content.toml) in `$XDG_CONFIG_HOME/seqviz-rs/`, or `~/.config/seqviz-rs/` when XDG_CONFIG_HOME is unset:

```text
seqviz-rs/
├── config.toml
└── content.toml
```

The main configuration controls the interface colours and references a separate content palette:

```toml
[display]
show_quality = true

[theme]
content = "content.toml"
background = "#181b23"
foreground = "#e0e5ef"
```

Content palette paths are relative to the main configuration file. The content palette defines nucleotide and amino-acid categories, plus inclusive minimum Phred-score thresholds. Omitted fields retain their defaults. There is one configurable theme, with no in-app theme switching.

Reading caches live in `$XDG_CACHE_HOME/seqviz-rs/`, falling back to `~/.cache/seqviz-rs/`.

## Yazi integration

Install `sq` on PATH and merge the matching opener example into your Yazi configuration:

| Yazi configuration syntax | Example |
| --- | --- |
| Yazi 25.4: `name` rules and shell arguments | [contrib/yazi.toml](contrib/yazi.toml) |
| Yazi 26.9: `url` rules and `%s` placeholders | [contrib/yazi-26.toml](contrib/yazi-26.toml) |

The opener uses `block = true`: pressing Enter on a supported file opens `sq` in the same terminal, and quitting returns to Yazi. The integration uses an opener rather than a preview plugin.

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

The interface uses **Ratatui** and **Crossterm**. Persistent indexes use **rusqlite/SQLite**; compressed input uses **zlib** through `libz-sys`. Configuration uses **Serde + TOML**. Background work uses `std::thread`, `mpsc` channels and atomic cancellation flags.

## Development and validation

Run the Rust checks from the repository root:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Tests cover record boundaries, wrapped FASTQ, compressed seeking, cache reuse and invalidation, background scheduling and UI state. The SAM/BAM fixtures were generated with samtools; running the tests does not require samtools.

For interactive acceptance checks, [scripts/terminal_smoke.py](scripts/terminal_smoke.py) runs the release binary in a pseudo-terminal. It checks navigation, search, position jumps, resizing, quality display, information pages, SAM/BAM copy restrictions and terminal restoration. Its Python dependency is separate from the application:

```sh
python3 -m venv /tmp/seqviz-qa
/tmp/seqviz-qa/bin/pip install pyte==0.8.2
cargo build --release --locked
/tmp/seqviz-qa/bin/python scripts/terminal_smoke.py
```

[GitHub Actions](.github/workflows/check.yml) defines these checks for Linux and macOS. [docs/performance.md](docs/performance.md) records generated-data measurements and reproduction commands; [examples/measure.rs](examples/measure.rs) measures data-layer startup, scanning and indexed reads.

The [product manual](docs/product-manual.md) records the v1 design. VCF, editing, pasting, source-file writes, reference pileups, runtime file addition and session restoration are reserved for later work.
