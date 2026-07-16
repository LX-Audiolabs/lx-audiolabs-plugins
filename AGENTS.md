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
- New path: **Slint** via `truce-slint`, **Truce standard widgets** (`@truce`), **software renderer** (`renderer-software`). Goal: no special femtovg/truce forks.
- Prototypes live in sibling repo: `C:\Users\lxndr\Documents\LX-AudioLabs\lx-audiolabs-slint`
- That workspace **path-deps only** shared crates from this vizia tree (`shared-dsp`, `shared-analysis`, `shared-vault`, `shm-hub`) — no DSP copy.
- **When rebuilding a plugin in Slint, use shared-ui-slint.** Header = `LxShellHeader` + custom actions as children; body = `LxShellBody` + `LxShellLeft` / `LxShellMain` / `LxShellRight`. No old `LxShell`.
- Status: **Aether Slint** done. **Meridian Slint** done.
- This repo (`lx-audiolabs-dev`) remains production Vizia plugins until a cutover decision.
