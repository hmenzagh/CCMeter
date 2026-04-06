# Changelog

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
