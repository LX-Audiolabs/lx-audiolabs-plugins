---
source: global
copied_by: template
date: 2026-07-29
adapted: false
reason: "slint ui stack rules for audio plugins"
---

# Slint UI

**Summary:** UI patterns for Slint-based plugin editors.
Covers component hierarchy, data flow, and editor hosting.
Read this when using Slint for plugin GUIs.

## Key Rules

- Export `component` blocks from `.slint` files
- Keep editor logic separate from `process()`
- Use callbacks for parameter changes

## See Also

- [registry/frameworks.toml](../../registry/frameworks.toml) → `ui.slint`
