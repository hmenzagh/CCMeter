<p align="center">
  <img src="assets/logo.svg" width="280" alt="CCMeter logo" />
</p>

<p align="center">
  <strong>A terminal dashboard for Claude Code usage analytics</strong><br/>
  Track tokens, costs, code generation, and efficiency, all from your terminal.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust" alt="Rust 1.85+" />
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-blue" alt="Platform" />
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License" />
</p>

---

<p align="center">
  <img src="assets/dashboard.png" alt="CCMeter dashboard view" />
</p>

## Quick start

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/hmenzagh/CCMeter/releases/latest/download/ccmeter-installer.sh | sh
ccmeter          # launch the dashboard
# press `.` for the settings panel, `q` to quit
```

## Overview

CCMeter reads your local Claude Code session data and renders an interactive TUI dashboard. Data refreshes every 5 minutes (manual reload with `r`).

**Metrics & analytics**
- **Cost tracking** — per-model USD breakdown (Opus, Sonnet, Haiku) via built-in pricing tables
- **Token analytics** — input, output, and prompt cache usage over time
- **Code metrics** — lines suggested, accepted, added, and deleted, with acceptance rate
- **Active time estimation** — approximates how long you actually spent working on each project from session activity
- **Efficiency score** — tokens per line of code changed (tok/ln, lower is better); each card has a quartile gauge (green → yellow → red) comparing it to other projects
- **KPI banner** — total cost, current streak, active days, avg tokens/day, and efficiency score at a glance

**Visualizations**
- **Heatmaps** — four GitHub-style contribution grids (input, output, lines changed, acceptance rate) with trend sparklines; minute-level granularity on 1h / 12h / Today filters
- **Project cards** — scrollable grid with per-project sparklines colored by model usage
- **Per-project detail** — dedicated charts, model distribution, cost sparklines, and estimated active time
- **Time filters** — 1h, 12h, Today, Last week, Last month, All

**Project handling & performance**
- **Auto-discovery & grouping** — finds Claude projects and groups them by git repository
- **Multi-source roots** — switch between Claude config directories with `Shift+Tab`
- **Persistent cache** — historical metrics cached locally for near-instant startup; only new sessions get parsed
- **Responsive layout** — heatmaps and card grids adapt to terminal size

## Installation

### Install script (recommended)

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/hmenzagh/CCMeter/releases/latest/download/ccmeter-installer.sh | sh
```

### From source

```bash
git clone https://github.com/hmenzagh/CCMeter.git
cd CCMeter
cargo install --path .     # installs `ccmeter` to ~/.cargo/bin (make sure it's in $PATH)
```

Or build without installing — the binary will be at `target/release/ccmeter`:

```bash
cargo build --release
```

**Requirements (from source):** Rust 1.85+ and Cargo.

## Usage

```bash
ccmeter
```

### Keybindings

| Key | Action |
|-----|--------|
| `Tab` | Cycle time filter |
| `Shift+Tab` | Switch source root |
| `j` / `k` or `Up` / `Down` | Scroll projects |
| `h` / `l` or `Left` / `Right` | Navigate between projects |
| `Esc` | Deselect project |
| `.` | Open settings panel |
| `r` | Reload data |
| `q` / `Ctrl+C` | Quit |

### Settings panel

Press `.` to open the settings panel, where you can:

- **Rename** projects with custom display names
- **Merge** multiple projects into a single group
- **Split** sources out of auto-detected groups
- **Star** favorites (animated rainbow border)
- **Hide** projects from the dashboard

## How it works

CCMeter discovers Claude Code sessions by scanning your home directory for any folder whose name contains `claude` and that has a `projects/` subdirectory with session logs (so `~/.claude/projects`, `~/.config/claude/projects`, and other Claude-compatible CLIs are all picked up automatically). It parses JSONL session files in parallel using [rayon](https://github.com/rayon-rs/rayon), extracts token counts and model identifiers, and computes costs from built-in pricing tables.

```
Session JSONL → parallel parse → daily aggregates → cached history → TUI render
```

### Cache

Parsed metrics are persisted to `~/.config/ccmeter/history.json`. On subsequent launches, only new or modified session files are parsed, everything else is served from cache, making startup near-instant even with thousands of sessions.

### Per-project view

Use `h`/`l` or arrow keys to select a project card. The dashboard switches to a detail view showing:

- Cost and token charts scoped to that project (daily or minute-level depending on time filter)
- Model distribution bar with per-model cost breakdown
- Active time estimate, sessions count, lines added/deleted, and efficiency gauge
- Heatmaps filtered to the selected project only

Press `Esc` to go back to the global overview.

<p align="center">
  <img src="assets/project.png" alt="CCMeter per-project view" />
</p>

## Configuration

User overrides are stored at `~/.config/ccmeter/overrides.json` and can be edited through the settings panel or manually.

## Tech stack

| Crate | Role |
|-------|------|
| [ratatui](https://ratatui.rs) | Terminal UI framework |
| [crossterm](https://github.com/crossterm-rs/crossterm) | Terminal event handling |
| [clap](https://github.com/clap-rs/clap) | CLI argument parsing |
| [rayon](https://github.com/rayon-rs/rayon) | Parallel JSONL parsing |
| [chrono](https://github.com/chronotope/chrono) | Date/time handling |
| [serde](https://serde.rs) / [serde_json](https://github.com/serde-rs/json) | Serialization & JSONL parsing |
| [dirs](https://github.com/dirs-dev/dirs-rs) | Cross-platform home/config paths |

## Project structure

```
src/
├── main.rs               # Entry point & event loop
├── app.rs                # Core application state
├── config/
│   ├── mod.rs
│   ├── discovery.rs      # Project auto-discovery
│   └── overrides.rs      # User configuration & merges
├── data/
│   ├── mod.rs
│   ├── parser.rs         # JSONL session parsing
│   ├── cache.rs          # Persistent metric cache
│   ├── tokens.rs         # Daily token aggregation
│   └── models.rs         # Model pricing tables
└── ui/
    ├── mod.rs
    ├── dashboard.rs      # Main layout
    ├── heatmap.rs        # Heatmap rendering
    ├── theme.rs          # Color theme
    ├── time_filter.rs    # Time range logic
    ├── settings_view.rs  # Settings panel
    └── cards/
        ├── mod.rs
        ├── data.rs       # Card data aggregation
        └── render.rs     # Card rendering
```

## License

MIT
