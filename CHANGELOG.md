# Changelog

## [Unreleased]

### Fixed
- **Token & cost accuracy on proxied API calls** — when Claude Code is pointed at a corporate Bedrock gateway or any third-party LLM proxy that strips the `requestId` request header, CCMeter previously fell back to a `line_uuid`-based dedup that never fires (each JSONL line carries a unique `uuid`), so a streaming response with N content-block lines got summed N times. Observed inflation on a real proxy user: **~2.6× across 28k events / 511 sessions**. Parser now uses `message.id` (`msg_…`) as the canonical dedup key when `requestId` is absent — both id-spaces are globally unique per Anthropic API call, so this restores the deltaize-by-canonical-stream guarantee for proxy users. Direct-API users (where `requestId` is present) are unaffected.

## [2.0.0] - 2026-04-13

### Added
- **Rate limit tracking view** — real-time usage monitoring for Claude OAuth accounts, with separate input/output token tracking, hit and rate history, and animated blinking status indicators.
- **`t` keyboard shortcut** to toggle the rate limit tracking view (backtick `` ` `` still works as an alias). Easier to reach on non-US keyboard layouts.
- **Async discovery refresh** with non-blocking state management for smoother UI updates.
- **Versioned cache schema** with automatic invalidation. `~/.config/ccmeter/history.json` carries a `schema_version`; mismatches trigger a clean rebuild on next launch so accuracy fixes propagate without manual intervention.
- **One-time cache-state banner** at the top of the dashboard:
  - "Cache rebuilt" (warning color) when a schema migration occurs.
  - "Cache was unreadable" (error color) when the on-disk file couldn't be read or parsed, with a hint to delete it if the issue persists.
  - Both dismiss on any keypress.
- **`CCMETER_FORCE_BANNER` env var** for testing the banners after migration has already happened. Set to `recovered` for the corruption banner, anything else for the migration banner.

### Fixed
- **Token & cost accuracy** — Claude Code logs the same API response in multiple places (streaming chunks, sub-agent mirrors, `/compact` retries). CCMeter now dedupes by `requestId` (Anthropic's billing unit), eliminating the 2–3× over-counting previously observed on days with heavy sub-agent activity. Totals now match what Anthropic actually billed.
- **Multi-minute timeline accuracy** — long streaming responses (extended thinking + large outputs) now correctly distribute their tokens across the minutes they actually spanned, instead of collapsing onto the final completion minute. `active_minutes` clustering, the minute-level heatmap, and rate-limit forecasts are all more accurate.
- **Code metrics on partial-overlap streams** — when a non-canonical stream carries a unique `Edit`/`Write` block, its `lines_suggested` / `lines_added` / `lines_deleted` are preserved via zero-billing ghost markers, avoiding silent under-counting of code activity. Ghosts are deduped across multiple mirror files by timestamp so a 3-file (canonical + 2 mirrors) layout doesn't double-count line metrics.
- **Ghost events no longer leak into model breakdowns** — zero-billing markers now carry an empty `model` field, so they fall through to `ModelId::Other` and are filtered out of model-share aggregations instead of producing phantom slices.
- **User-side patch dedup** — patches replayed into sub-agent transcripts (Edit/Write acceptances) are now deduped by line `uuid`, fixing inflated `lines_added` and skewed efficiency scores on sub-agent–heavy days.
- **Cost fallback includes `cache_creation`** — token-based cost estimation (used when raw `costUSD` is absent, i.e. Pro plans) now bills `cache_creation` at `input_price × 1.25` instead of ignoring it. Closes a 5–15 % under-estimate on cache-heavy sessions.
- **Usage preservation** — usage data is now kept only for non-expired credentials, and asymmetric merges preserve enriched hit data across refreshes.
- **Label clipping** — prevent label clipping and overlapping in rate tracking charts.

### Changed
- **Modularize rate tracking UI** into 13 focused component modules for better maintainability.
- **Restructure data models and handlers** for usage tracking, with new OAuth and rate limits modules.
- **Improve discovery configuration and error handling**.
- **Landing page replaced by rate tracking view** as the primary experience on launch.
- **Dashboard styling** updated to align with new rate tracking components.

## [1.4.1] - 2026-04-09

### Added
- Update notification banner: checks GitHub releases on startup and displays a rainbow-animated alert when a newer version is available

## [1.4.0] - 2026-04-09

### Added
- ASCII art logo on the loading screen
- Extract star animation into dedicated module

### Fixed
- KPI values and card costs now reflect the actual time window for 1H and 12H filters (sub-day minute-level filtering)
- Fix 1H and 12H filters ignoring data from the previous day when the time window crosses midnight — KPIs, cards, and model stats now correctly include yesterday's data
- Fix panic on non-ASCII characters (accented letters, emoji, CJK) in project rename and search inputs — cursor now navigates by character boundaries instead of raw bytes
- Fix `truncate_str` panic when truncation falls in the middle of a multi-byte UTF-8 character
- Fix search bar and rename modal cursor position being offset when text contains multi-byte characters
- Fix ←/→ project navigation order not matching the displayed card order — navigation now follows the visual sort (starred first, then by cost) instead of alphabetical

### Changed
- KPI "Avg/day" label switches to "Total tokens" in sub-day views (1H, 12H, Today) for clarity
- Track `cache_read`, `cache_creation`, `lines_added`, and `lines_deleted` in the compact event index

## [1.3.2] - 2026-04-06

### Changed
- Distribute leftover pixels to leading columns instead of discarding them; only trailing columns shrink, maximizing space usage and preserving visual alignment

## [1.3.1] - 2026-04-06

### Fixed
- Uniformly reduce heatmap cell sizes when the panel is too narrow

## [1.3.0] - 2026-04-06

### Added
- Persist user preferences (settings saved across sessions)
- Expanded heatmap setting with tabbed settings UI

### Changed
- Extract Settings into standalone persistent module

## [1.2.1] - 2026-04-06

### Changed
- Replace `Vec<Event>` with compact `EventIndex` for reduced memory usage
- Add `x86_64-apple-darwin` build target

## [1.2.0] - 2026-04-05

### Added
- Weekly days×hours heatmap view

### Fixed
- Constrain heatmap time ranges to the selected render range
- Always use 2-char cells in the intraday view to prevent cells from packing together on narrow panels

### Changed
- Derive weekly view from the render range

## [1.1.0] - 2026-04-05

### Added
- Async loading screen displayed during startup

### Documentation
- Homebrew installation instructions

## [1.0.0] - 2026-04-05

- Initial release
