# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**shp2geojson** is a high-performance Rust CLI tool for converting ESRI Shapefiles (.shp) to GeoJSON. Designed for scale — from single files to terabyte-sized batch workloads — with streaming I/O, parallel processing, resumable jobs, and an interactive TUI.

The project specification lives in `shp2geojson-guidelines.md`. An interactive CLI mockup is in `shp2geojson-mockup.html`.

## Build & Development Commands

```bash
cargo build                  # Debug build
cargo build --release        # Release build
cargo run -- --input ./data --output ./geojson   # Run with args
cargo test                   # Run all tests
cargo test <test_name>       # Run a single test
cargo clippy                 # Lint
cargo fmt                    # Format code
cargo fmt -- --check         # Check formatting without modifying
```

## Architecture

Four-stage pipeline: **Discovery → Work Queue → Parallel Workers → Output + Logging**

1. **Discovery** — Recursively walks `--input`, validates sidecar files (`.dbf`, `.shx` required; `.prj`, `.cpg` optional)
2. **Work Queue** — Thread-safe channel-based job queue with lifecycle states (Pending → In Progress → Done | Failed)
3. **Parallel Workers** — `--jobs N` workers (default: CPU count) stream `.shp`+`.dbf` record-by-record, optionally reproject via `.prj`, write GeoJSON incrementally
4. **Output & Logging** — GeoJSON/GeoJSONL to mirrored directory structure, TUI progress on stderr, `conversion_errors.log` at output root

### Key Design Decisions

- **Streaming I/O** — Never load full files into RAM; read/write record-by-record
- **Atomic checkpointing** — `.shp2geojson_state.json` written via temp file + rename after each job; enables `--resume`
- **Stderr for UI, stdout for data** — Progress/TUI renders to stderr; JSON events and results go to stdout
- **Hook system** — Shell commands in `.shp2geojson.toml` fire on lifecycle events (`on_file_complete`, `on_file_failed`, `on_batch_done`) with template variable substitution

### Planned Crate Dependencies

| Concern | Crate |
|---------|-------|
| Shapefile parsing | `shapefile` |
| GeoJSON serialization | `geojson` |
| CRS reprojection | `proj` (bindings to libproj) |
| Parallelism | `rayon` + `crossbeam` |
| Progress UI | `indicatif` |
| CLI args | `clap` (derive macros) |
| Config file | `toml` + `serde` |
| Structured logging | `tracing` + `tracing-subscriber` |
| Interactive TUI | `crossterm` |

### Shapefile Gotchas

- Multi-file format: each dataset requires `.shp` + `.dbf` + `.shx` together
- No encoding standard — `.cpg` is optional; without it non-ASCII may be garbled
- GeoJSON output is 3–5× larger than shapefile input (verbose text format)
- RFC 7946 mandates WGS84 (EPSG:4326) for GeoJSON — reprojection needed when `.prj` specifies another CRS
- `libproj` must be available on host system for reprojection
