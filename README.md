# shp2geojson

High-performance ESRI Shapefile (.shp) to GeoJSON converter. Handles everything from single files to terabyte-scale batch workloads with streaming I/O, parallel workers, resumable jobs, and a live TUI.

## Install

### Pre-built binaries (recommended)

Download from the [latest release](https://github.com/s19835/shp2geojson/releases/latest):

| Platform | Artifact | Reprojection |
|----------|----------|:---:|
| macOS (Apple Silicon) | `shp2geojson-aarch64-apple-darwin.tar.gz` | ✅ |
| Linux x86\_64 | `shp2geojson-x86_64-unknown-linux-gnu.tar.gz` | ✅ |
| Linux ARM64 | `shp2geojson-aarch64-unknown-linux-gnu.tar.gz` | — |
| Windows x86\_64 | `shp2geojson-x86_64-pc-windows-msvc.zip` | — |

> Linux ARM64 and Windows binaries are lite builds (no reprojection). Use Docker for full support on those platforms.

### Package managers

```bash
# macOS (Homebrew)
brew tap s19835/tap && brew install shp2geojson

# Windows (Scoop)
scoop bucket add s19835 https://github.com/s19835/scoop-bucket
scoop install shp2geojson

# Docker (all platforms, full reprojection support)
docker pull ghcr.io/s19835/shp2geojson:latest
```

### cargo install (from crates.io)

Requires **Rust 1.88+** and system dependencies for reprojection support.

**Linux / WSL (Ubuntu/Debian):**
```bash
# Install system dependencies first
sudo apt install pkg-config libproj-dev cmake

# Full install (with reprojection)
cargo install shp2geojson

# Or skip system deps — install without reprojection
cargo install shp2geojson --no-default-features
```

**macOS:**
```bash
brew install proj pkgconf cmake
cargo install shp2geojson
```

**Windows (native):**
```bash
# No system deps needed — lite build
cargo install shp2geojson --no-default-features
```

> **Note:** `cargo install shp2geojson` (with default features) requires `libproj`, `pkg-config`, and `cmake` on the system. If you just want to convert shapefiles without CRS reprojection, `--no-default-features` works out of the box on any platform with no extra installs.

### Prerequisites

- **Rust 1.88+** (run `rustup update stable` if you get version errors)
- **libproj 9.2+**, **pkg-config**, **cmake** — only needed for reprojection (`--features reproject`)

## Quick start

```bash
# Convert all shapefiles in a directory
shp2geojson --input ./shapefiles --output ./geojson

# Preview what would be converted (no files written)
shp2geojson --input ./shapefiles --dry-run

# Convert with 8 parallel workers, overwriting existing output
shp2geojson --input ./data --output ./out --jobs 8 --overwrite

# Output as GeoJSONL (one feature per line, for streaming pipelines)
shp2geojson --input ./data --output ./out --geojsonl

# Resume a previous run (skips already-completed files)
shp2geojson --input ./data --output ./out --resume
```

## CLI flags

| Flag | Description |
|------|-------------|
| `--input PATH` | Source folder (scanned recursively for `.shp` files) |
| `--output PATH` | Output root directory (mirrors source structure) |
| `--jobs N` | Parallel worker threads (default: CPU count) |
| `--resume` | Skip files completed in a prior run |
| `--dry-run` | Discover and validate shapefiles without converting |
| `--output-format human\|json` | Output format for progress/events (default: `human`) |
| `--reproject` | Reproject to WGS84 when `.prj` is present (default) |
| `--no-reproject` | Pass coordinates through unchanged |
| `--geojsonl` | Write GeoJSONL instead of GeoJSON FeatureCollection |
| `--overwrite` | Overwrite existing output files |
| `--log PATH` | Error log path (default: `{output}/conversion_errors.log`) |
| `--config PATH` | Config file path (default: `.shp2geojson.toml`) |
| `--completions SHELL` | Generate shell completions (`bash`, `zsh`, `fish`) |

## Interactive commands

When running in a terminal, type commands while conversion is in progress:

| Command | Action |
|---------|--------|
| `/status` | Show current progress (done/failed/pending/workers) |
| `/pause` | Pause all workers after their current file |
| `/resume` | Resume paused workers |
| `/workers N` | Scale workers up or down |
| `/skip FILE` | Skip a pending file |
| `/log` | Show last 20 lines of the error log |
| `/dry-run` | List remaining pending files |
| `/quit` | Save checkpoint and exit after in-flight files finish |
| `/help` | Show available commands |

## Configuration file

Create `.shp2geojson.toml` in your working directory (or specify with `--config`). CLI flags always override config values.

```toml
[conversion]
reproject = true
output_format = "geojson"   # "geojson" or "geojsonl"
overwrite = false
jobs = 4

[output]
log_file = "errors.log"

[hooks]
on_file_complete = "echo 'Done: {{file}} -> {{output}} ({{features}} features)'"
on_file_failed = "echo 'FAILED: {{file}} — {{reason}}'"
on_batch_done = "curl -X POST https://hooks.example.com/done -d '{{summary_json}}'"
on_pause = "echo 'Run paused'"
```

### Hook template variables

- **on_file_complete**: `{{file}}`, `{{output}}`, `{{features}}`, `{{duration_ms}}`
- **on_file_failed**: `{{file}}`, `{{reason}}`
- **on_batch_done**: `{{converted}}`, `{{failed}}`, `{{elapsed_s}}`, `{{gb_processed}}`, `{{summary_json}}`

All template values are shell-quoted to prevent injection.

## JSON event output

Use `--output-format json` for machine-readable newline-delimited JSON on stdout:

```bash
shp2geojson --input ./data --output ./out --output-format json 2>/dev/null
```

Events: `start`, `file_done`, `file_failed`, `file_skipped`, `batch_done`, `paused`, `resumed`, `workers_changed`, `file_skipped_by_user`.

## Reprojection

When a `.prj` sidecar is present and `--reproject` is active (the default), geometries are automatically reprojected to WGS84 (EPSG:4326) as required by GeoJSON RFC 7946. Without a `.prj` file, coordinates pass through unchanged with a warning logged.

Disable with `--no-reproject` if your data is already in WGS84 or you want raw coordinates.

## Checkpointing and resume

Each completed file is recorded in `.shp2geojson_state.json` inside the output directory. Use `--resume` on subsequent runs to skip already-converted files. Failed files are retried on resume. Ctrl+C saves the checkpoint before exiting.

## Shell completions

```bash
# Bash
shp2geojson --completions bash > /usr/local/etc/bash_completion.d/shp2geojson

# Zsh
shp2geojson --completions zsh > ~/.zfunc/_shp2geojson

# Fish
shp2geojson --completions fish > ~/.config/fish/completions/shp2geojson.fish
```

## Logging

Diagnostic messages are controlled via the `RUST_LOG` environment variable:

```bash
RUST_LOG=debug shp2geojson --input ./data --output ./out
RUST_LOG=shp2geojson=info shp2geojson --input ./data --output ./out
```

## License

MIT
