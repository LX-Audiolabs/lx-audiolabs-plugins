---
source: global
copied_by: template
date: 2026-07-29
adapted: false
reason: "clap plugin format rules and conventions"
---

# CLAP Format

**Summary:** Rules for building CLAP plugins.
Covers factory registration, plugin description, and threading.
Read this when targeting CLAP hosts.

## Key Rules

- Use `clap_plugin_factory` registration
- Declare `plugin_features` correctly
- Handle host callbacks in the main thread only

## See Also

- [registry/frameworks.toml](../../registry/frameworks.toml) → `formats.clap`
