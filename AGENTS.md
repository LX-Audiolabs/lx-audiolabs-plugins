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

## Slint
When writing, editing, or debugging `.slint` files, ALWAYS consult `SLINT_SKILL.md` first — it covers language rules, layout/sizing, common compile errors, truce-slint interop, and widget reference. Slint version: **1.17.1**, software renderer only.

## truce
building with the truce framework is `cargo truce install --clap -p <pluginname>` this already is release.

## github commits & push
Commits always as user.name="lxndrbe" & user.email="ardvinnamoon@gmail.com"
Github AUTH always as github.user "lxndrbe"

## UI direction (2026-07)
- Path: **Slint** via **`truce-slint`** (software renderer + wgpu present). Build: **`lx-slint-build`** (`@truce` widgets, Slint **1.17.1** matching truce-slint).
- Runtime: `truce_slint::SlintEditor` (`PluginContext` / `SyncFn`). No local FemtoVG bridge; if GPU later: Slint **`renderer-skia`**.
- Layout: **`crates/`** = libs (`lx-slint-build`, `lx-dsp`, `lx-analysis`, `lx-vault`, `lx-shm`, `lx-ui-slint`); **`plugins/`** = products.
- **UI shell:** `LxShellHeader` + `LxShellBody` / Left / Main / Right; reuse `LxKnob`, `LxLineEdit`, `LxButton`, etc.
- **Slint-native redesign track:** Lucent + Aurum (full UI rebuild, not Vizia clone) — candidates to drop from `lx-audiolabs-dev` once shipped.
- Also in tree: Lucent Relay, Equilibrium, Aether, Meridian (Slint ports).
- Dev repo Vizia: keep remaining plugins; Lucent/Aurum can leave when Slint variants are release-ready.
