# shp2geojson — Project Guidelines

> A high-performance Rust CLI for converting Shapefiles to GeoJSON at any scale — from single files to terabyte-sized batch workloads.

---

## Table of Contents

1. [Project Overview](#1-project-overview)
2. [Architecture](#2-architecture)
3. [CLI Interface](#3-cli-interface)
4. [Slash Commands (Interactive Mode)](#4-slash-commands-interactive-mode)
5. [Hooks System](#5-hooks-system)
6. [Configuration File](#6-configuration-file)
7. [Output Formats](#7-output-formats)
8. [Checkpointing & Resume](#8-checkpointing--resume)
9. [Progress Display](#9-progress-display)
10. [Error Logging](#10-error-logging)
11. [Crate Decisions](#11-crate-decisions)
12. [Known Drawbacks & Gotchas](#12-known-drawbacks--gotchas)
13. [Open Design Questions](#13-open-design-questions)

---

## 1. Project Overview

`shp2geojson` is a production-grade Rust CLI tool that converts ESRI Shapefiles (`.shp`) to GeoJSON. It is designed for both quick single-file conversions and large-scale batch processing of terabyte-sized datasets.

**Core goals:**
- Memory-efficient streaming — no full file loads into RAM
- Parallel multi-core processing with a configurable worker count
- Resumable jobs — interrupted runs can be continued where they left off
- Scriptable and CI/CD-friendly structured output
- Rich interactive TUI for human use
- Project-level configuration via a versioned config file

---

## 2. Architecture

The tool operates as a four-stage pipeline:

```
Discovery → Work Queue → Parallel Workers → Output + Logging
```

### 2.1 Discovery Phase

Recursively walks the `--input` folder. For each `.shp` found, validates that required sidecar files exist:

| File | Required | Purpose |
|------|----------|---------|
| `.shp` | ✅ | Geometry |
| `.dbf` | ✅ | Attributes |
| `.shx` | ✅ | Spatial index |
| `.prj` | ⚠️ optional | CRS definition |
| `.cpg` | ⚠️ optional | Character encoding |

Missing `.dbf` or `.shx` causes the job to be marked **Invalid** and logged — it never blocks other jobs.

### 2.2 Work Queue

A thread-safe channel-based job queue. Each job has a lifecycle:

```
Pending → In Progress → Done | Failed
```

The queue state is written atomically to `.shp2geojson_state.json` after each completed job, enabling resume.

### 2.3 Parallel Workers

`--jobs N` workers (default: logical CPU count). Each worker:

1. Claims a job from the queue
2. Reads `.shp` + `.dbf` record-by-record (streaming)
3. Optionally reprojects geometry to WGS84 using `.prj`
4. Writes GeoJSON incrementally to the output path
5. Mirrors the source folder structure under the output root
6. Fires any configured hooks on completion or failure

### 2.4 Output & Logging

- GeoJSON (or GeoJSONL) files written to the mirrored output directory
- A live progress TUI rendered to stderr (so stdout remains clean for piping)
- A persistent `conversion_errors.log` written to the output root

---

## 3. CLI Interface

### Basic Usage

```bash
# Single folder conversion
shp2geojson --input ./data --output ./geojson

# Specify worker count
shp2geojson --input ./data --output ./geojson --jobs 16

# Resume an interrupted run
shp2geojson --input ./data --output ./geojson --resume

# Validate without converting
shp2geojson --input ./data --dry-run

# CI/CD mode — machine-readable JSON output
shp2geojson --input ./data --output ./geojson --output-format json
```

### Full Option Reference

| Flag | Default | Description |
|------|---------|-------------|
| `--input <PATH>` | required | Source folder (scanned recursively) |
| `--output <PATH>` | required | Output root (mirrors source structure) |
| `--jobs <N>` | num CPUs | Parallel worker count |
| `--resume` | false | Skip already-converted files |
| `--dry-run` | false | Discover + validate, no writes |
| `--output-format <json\|human>` | human | Output mode |
| `--reproject` / `--no-reproject` | true | Auto-reproject to WGS84 |
| `--geojsonl` | false | Write newline-delimited GeoJSONL |
| `--overwrite` | false | Overwrite existing output files |
| `--log <PATH>` | `{output}/conversion_errors.log` | Error log path |
| `--config <PATH>` | `.shp2geojson.toml` | Explicit config file path |

---

## 4. Slash Commands (Interactive Mode)

When running in the default interactive (human) mode, slash commands are available while the job runs — inspired by Claude Code's `/clear`, `/model`, `/hooks` command system.

| Command | Description |
|---------|-------------|
| `/status` | Print live worker status and throughput stats |
| `/pause` | Gracefully pause all workers and checkpoint state |
| `/resume` | Resume paused workers |
| `/workers <N>` | Dynamically increase or decrease worker count |
| `/skip <file>` | Skip a specific pending file |
| `/log` | Tail the error log inline |
| `/dry-run` | Preview remaining pending files without converting |
| `/quit` | Checkpoint and exit cleanly |

---

## 5. Hooks System

Inspired by Claude Code's hooks for CI/CD automation. Hooks are shell commands configured in `.shp2geojson.toml` that fire on lifecycle events. They receive template variables at runtime.

### Available Events

| Event | Trigger | Template Variables |
|-------|---------|-------------------|
| `on_file_complete` | After each successful conversion | `{{file}}`, `{{output}}`, `{{features}}`, `{{duration_ms}}` |
| `on_file_failed` | After each failed conversion | `{{file}}`, `{{reason}}` |
| `on_batch_done` | After all files are processed | `{{summary_json}}`, `{{converted}}`, `{{failed}}` |
| `on_pause` | When `/pause` is called | — |

### Hook Examples

```toml
[hooks]
# Upload each output to S3 immediately
on_file_complete = "aws s3 cp {{output}} s3://my-bucket/geojson/ --quiet"

# Slack alert on failure
on_file_failed = "curl -s https://hooks.slack.com/T.../B... -d '{\"text\": \"❌ Failed: {{file}} — {{reason}}\"}'"

# Trigger downstream pipeline when batch is done
on_batch_done = "python3 ingest.py --summary '{{summary_json}}'"
```

Hooks are executed via `std::process::Command`. A hook failure is logged but never stops the conversion pipeline.

---

## 6. Configuration File

`.shp2geojson.toml` is the project-level config — version-controllable, shareable across a team. Inspired by Claude Code's `CLAUDE.md` as a persistent project-level instruction store.

CLI flags always override the config file.

### Full Reference

```toml
[conversion]
reproject        = true       # Auto-reproject to WGS84 (default: true)
output_format    = "geojson"  # "geojson" or "geojsonl"
overwrite        = false      # Overwrite existing outputs (default: false)
jobs             = 0          # 0 = auto (num logical CPUs)

[output]
mirror_structure = true       # Mirror source folder structure in output
log_file         = "./logs/conversion_errors.log"

[hooks]
on_file_complete = ""
on_file_failed   = ""
on_batch_done    = ""
```

---

## 7. Output Formats

### 7.1 Human Mode (default)

Rich terminal UI with live progress bars, worker status, and ETA. Written to stderr so stdout remains pipeable.

### 7.2 JSON Mode (`--output-format json`)

Newline-delimited JSON events emitted to stdout — one event per line. Designed for CI/CD pipelines, log aggregators, and downstream scripting.

```json
{"event":"start","total_files":2847,"timestamp":"2026-03-08T14:00:00Z"}
{"event":"file_done","file":"counties.shp","output":"out/counties.geojson","duration_ms":1240,"features":9821}
{"event":"file_failed","file":"broken.shp","reason":"missing .dbf sidecar"}
{"event":"batch_done","converted":2841,"failed":6,"elapsed_s":847,"gb_processed":3.2}
```

**Example pipeline usage:**

```bash
# Watch only failures in real time
shp2geojson --input ./data --output ./out --output-format json \
  | jq 'select(.event == "file_failed")'

# Count converted files
shp2geojson ... --output-format json \
  | jq 'select(.event == "batch_done") | .converted'
```

### 7.3 GeoJSONL (`--geojsonl`)

Writes one GeoJSON Feature per line instead of a `FeatureCollection` wrapper. Better for big-data tools (BigQuery, Spark, tippecanoe) that prefer streaming input.

```json
{"type":"Feature","geometry":{...},"properties":{...}}
{"type":"Feature","geometry":{...},"properties":{...}}
```

---

## 8. Checkpointing & Resume

Inspired by Claude Code's automatic checkpoint system. A `.shp2geojson_state.json` file is written atomically to the output root after every completed job.

### State File Format

```json
{
  "run_id": "a3f9c1d2",
  "started": "2026-03-08T14:00:00Z",
  "input_root": "/data/shapefiles",
  "output_root": "/data/geojson",
  "done": ["counties/counties_2024.shp", "hydro/watersheds.shp"],
  "failed": ["broken/corrupt.shp"],
  "pending": ["roads/motorways.shp"]
}
```

### Resume Behavior

- `--resume` reads the state file and skips all files listed in `done`
- Files listed in `failed` are retried unless `--skip-failed` is also passed
- Partial output files from interrupted jobs are deleted before re-attempting
- If no state file exists, `--resume` starts a fresh run

---

## 9. Progress Display

```
shp2geojson  v0.1.0   run_id: a3f9c1d2

  [████████████░░░░░░░░]  1,240 / 2,847 files   3.2 GB processed   ETA 00:14:32

  Worker 1  counties_2024.shp          [████████░░]  63%   1.2 MB/s
  Worker 2  watersheds_global.shp      [██░░░░░░░░]  18%   0.8 MB/s
  Worker 3  motorways_europe.shp       [█████░░░░░]  49%   2.1 MB/s
  Worker 4  (idle)

  ✓ converted  1,234    ✗ failed  6    ⚠ skipped  0
```

- Rendered to **stderr** (stdout stays clean)
- Uses `indicatif` for progress bars, `crossterm` for the dynamic worker lines
- Refreshes at ~10 Hz, degrades gracefully in non-TTY environments (CI logs)

---

## 10. Error Logging

`conversion_errors.log` is written to the output root (configurable with `--log`).

### Format

```
[2026-03-08 14:22:01] FAILED   input/broken.shp         — missing .dbf sidecar
[2026-03-08 14:23:45] FAILED   input/corrupt.shp         — unexpected EOF at record 4821
[2026-03-08 14:25:10] INVALID  input/no_prj.shp          — no .prj found, CRS unknown, passthrough
[2026-03-08 14:26:03] SKIPPED  input/already_done.shp    — output exists, --resume active
```

### Severity Levels

| Level | Meaning |
|-------|---------|
| `FAILED` | Conversion attempted and errored |
| `INVALID` | Sidecar validation failed, never attempted |
| `SKIPPED` | Skipped due to `--resume` or existing output |
| `WARN` | Completed but with caveats (e.g. unknown CRS, passthrough) |

---

## 11. Crate Decisions

| Concern | Crate | Rationale |
|---------|-------|-----------|
| Shapefile parsing | `shapefile` | Pure Rust, streaming record API |
| GeoJSON serialization | `geojson` | Standard, serde-compatible |
| CRS reprojection | `proj` | Bindings to PROJ — industry standard |
| Parallelism | `rayon` + `crossbeam` | Work-stealing, channel-based queue |
| Progress UI | `indicatif` | Multi-bar support, ETA calculation |
| CLI args | `clap` | Derive macros, auto --help generation |
| Config file | `toml` + `serde` | Simple, human-readable config format |
| State / resume | `serde_json` | Atomic writes via temp file + rename |
| Hook execution | `std::process::Command` | No extra dep needed |
| Structured logging | `tracing` + `tracing-subscriber` | Async-safe, structured, filterable |
| Interactive TUI | `crossterm` | Cross-platform, raw mode input |

---

## 12. Known Drawbacks & Gotchas

### Shapefile Format
- **Multi-file format** — each dataset is `.shp` + `.dbf` + `.shx`. Missing any of these means the file cannot be converted.
- **No official encoding standard** — `.cpg` is optional. Without it, non-ASCII attribute values may be garbled.

### GeoJSON Format
- **Size explosion** — GeoJSON is verbose text. Expect 3–5× size increase over the source shapefile. A 200 GB dataset may produce 700 GB–1 TB of GeoJSON.
- **WGS84 required by spec** — RFC 7946 mandates EPSG:4326. Files without a `.prj` are passed through with a warning.
- **No streaming standard** — Standard GeoJSON requires wrapping `FeatureCollection`. Use `--geojsonl` for big-data streaming compatibility.

### Scale
- **Output disk space** is the primary constraint at TB scale, not RAM.
- **File system overhead** — thousands of small shapefiles can cause significant inode/directory traversal cost. `--dry-run` helps estimate this before committing.
- **PROJ dependency** — reprojection requires `libproj` on the host system. Static linking is possible but increases binary size.

---

## 13. Open Design Questions

The following decisions are not yet finalized and require confirmation before implementation:

1. **CRS handling default** — auto-reproject to WGS84 when `.prj` is present, or warn and passthrough? Reprojection is safer for spec compliance; passthrough is faster and lossless.

2. **Default output format** — standard GeoJSON (`FeatureCollection`) or GeoJSONL? Standard is more universally compatible; GeoJSONL is better for streaming and big-data tools.

3. **Overwrite behavior** — silently overwrite existing output files, or require explicit `--overwrite` flag? Fail-safe default is recommended.

4. **Resume completeness check** — mark a file as "done" based on output file existence, or checksum the source `.shp` against what was converted? Checksum is safer but slower at scale.
