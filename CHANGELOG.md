# Changelog

## [2.0.0-beta.1] - 2026-04-09

### Added
- Rate limit tracking view with real-time usage monitoring for Claude OAuth accounts

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
