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

## truce
building with the truce framework is `cargo truce install --clap -p <pluginname>` this already is release.

## github commits & push
Commits always as user.name="lxndrbe" & user.email="ardvinnamoon@gmail.com"
Github AUTH always as github.user "lxndrbe"

## UI direction (2026-07)
- Full Vizia rebuild = hard to maintain (truce-vizia + vizia-audio + femtovg forks).
- New path: **Slint** + **FemtoVG OpenGL** via local **`lx-slint-editor`** (baseview + GL), **not** `truce-slint` (software + wgpu).
- Build: **`lx-slint-build`** materializes `@truce` widgets + JetBrains Mono (slint-build 1.17).
- Runtime: `lx_slint_editor::SlintEditor` — same setup API as truce-slint (`PluginContext` / `SyncFn`).
- Prototypes live in sibling repo: `C:\Users\lxndr\Documents\LX-AudioLabs\lx-audiolabs-slint`
- Layout: **`crates/`** = all libs (`lx-slint-editor`, `lx-slint-build`, `lx-dsp`, `lx-analysis`, `lx-vault`, `lx-shm`, `lx-ui-slint`); **`plugins/`** = products only.
- Shared audio libs live under `crates/` in this repo (path-deps); keep in sync with vizia tree when DSP changes — no divergent fork intent.
- **When rebuilding a plugin in Slint, use `crates/lx-ui-slint`.** Header = `LxShellHeader` (branding left only) + custom actions as children; body = `LxShellBody` + `LxShellLeft` / `LxShellMain` / `LxShellRight`. No old `LxShell`. Reuse `LxKnob`, `LxLineEdit`, `LxButton`, etc. — don't hand-roll plugin-specific styling.
- Status: **Aether/Meridian** on `lx-slint-editor` (FemtoVG-GL). **Aurum** still Vizia in `lx-audiolabs-dev` until bridge proven in DAW.
- This repo (`lx-audiolabs-dev`) remains production Vizia plugins until a cutover decision.
