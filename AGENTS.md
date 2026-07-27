# AGENTS.md — LX Audiolabs

## Caveman
Talk terse. Drop articles/filler/pleasantries. Fragments OK. Technical terms exact.
`/caveman lite|full|ultra|wenyan`. Stop: "stop caveman". Normal prose for security warnings, irreversible actions, confusion. Code/commits/PRs normal.

## Ponytail — Lazy Senior Dev
Before code, climb ladder: 1. YAGNI? 2. Already in codebase? 3. stdlib? 4. platform? 5. installed dep? 6. one-liner? 7. write minimum.
Bug fix = root cause, not symptom. Trace every caller.
No abstractions, no new deps, no boilerplate. Delete > add. Boring > clever. Fewest files. Question complexity. Mark simplifications `ponytail:`.
Not lazy: input validation, error handling preventing data loss, security, accessibility, explicit requests. Non-trivial logic → ONE assert/test.

## graphify
When `graphify-out/graph.json` exists, query/path/explain first before raw grep or source reads. `/graphify` to build/update.

## audio-graph
When `audio-graph/audio-graph.json` exists, read it before working on plugins or crates. It contains the detected framework (truce, nih-plug, clack, JUCE), plugin-vs-crate kind, editor migration status (`from` legacy vs `to` migrated), internal dependency edges, and AST-level audio symbols (`PluginLogic`, `process`, `editor`, `Params` structs, `truce::plugin!` macro calls). Use it to avoid framework-confused suggestions or partial migrations that improve one plugin while breaking others. Regenerate with `lx-audio-graph .` (global install), or keep current with `lx-audio-graph --watch .` / `lx-audio-graph install-hook .`.

## truce
When writing, editing, or debugging truce plugin code (Params, PluginLogic, process, state, editor), ALWAYS consult `TRUCE_SKILL.md` — covers param macros, lifecycle, threading rules, common bugs, shared crates, and build workflow.
Building: `cargo truce install --clap -p <pluginname>` (release build).

## Slint
When writing, editing, or debugging `.slint` files, ALWAYS consult `SLINT_SKILL.md` first — project context, backend choices, baseview versions, LX-specific interop. For `.slint` language questions (syntax, layout, gotchas), also reference [slint-ui/ai-plugins](https://github.com/slint-ui/ai-plugins/blob/master/skills/slint/SKILL.md). Slint version: **1.17.1**, multi-backend via slint-baseview.

## github commits & push
Commits always as user.name="lxndrbe" & user.email="ardvinnamoon@gmail.com"
Github AUTH always as github.user "lxndrbe"

## UI direction (2026-07)
- Path: **Slint** via **slint-baseview** (our fork, baseview 0.3 next, 3 backends: femtovg/OpenGL, wgpu, wgpu-vulkan).
- Editor adapter: **lx-slint-editor** (`truce::Editor` on slint-baseview), replacing truce-slint.
- Runtime: `slint_baseview::SlintWindow` via `lx_slint_editor::LxSlintEditor`. No local FemtoVG bridge; wgpu + Vulkan paths avoid OpenGL crash vectors.
- Layout: **`crates/`** = libs (`lx-slint-build`, `lx-slint-editor`, `lx-dsp`, `lx-analysis`, `lx-vault`, `lx-shm`, `lx-ui-slint`); **`plugins/`** = products.
- Plugins still on truce-slint (truce-dev, baseview-truce 0.1.1). Migration to lx-slint-editor pending.
