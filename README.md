# CodeFlow

CodeFlow turns source code into readable control-flow diagrams.

It works in two stages:
1. Extract source code into a `.graph` file (JSON payload).
2. Render that graph into a `.png` flowchart.

Supported parser backends:
- Python (`--lang python`)
- Octave/MATLAB-like syntax (`--lang octave`)
- Rust (`--lang rust`)

## Highlights

- Unified CLI (`codeflow`) for `extract`, `render`, and `build`.
- Orthogonal edge routing with collision checks.
- Optional strict layout diagnostics (`--strict-report`) for debugging.
- Optional transparent output and separate layers.
- Render output now prints file stats (dimensions and file size).

## Build

```bash
cargo build --release
```

## Quick Start

Run extract + render in one command:

```bash
cargo run --release --bin codeflow -- build /path/to/source.py --lang python
```

Or run steps separately:

```bash
cargo run --release --bin codeflow -- extract /path/to/source.py --lang python -o source.graph
cargo run --release --bin codeflow -- render source.graph -o source_flowchart.png
```

## CLI Reference

Show top-level help:

```bash
cargo run --release --bin codeflow -- --help
```

### `extract`

```bash
cargo run --release --bin codeflow -- extract --help
```

Core options:
- `--lang python|octave|rust`
- `<INPUT_FILE>`
- `-o, --output <PATH>` (default: input path with `.graph` extension)
- `--function <NAME>` (Python/Rust backends; default is module-level extraction)
- `--no-compact`, `--compact-max-stmts <N>` (Python backend controls)

### `render`

```bash
cargo run --release --bin codeflow -- render --help
```

Core options:
- `<INPUT_GRAPH>`
- `-o, --output <PNG>`
- `--strict-report` (debugging/validation report)
- `--hide-labels`
- `--transparent-bg`
- `--separate-layers`

### `build`

```bash
cargo run --release --bin codeflow -- build --help
```

Runs extract + render in one step.

## Output

- `.graph`: graph JSON representation (`nodes`, `edges`, `start`, `end`, metadata).
- `.png`: rendered flowchart.
- Renderer prints:
  - output dimensions (px)
  - output file size (bytes + human-readable)

Graph payload format details are documented in [GRAPH_FORMAT.md](./GRAPH_FORMAT.md).

## License

MIT. See [LICENSE](./LICENSE).
