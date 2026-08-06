---
id: agent-usage
group: agents
summary: How agents consume AGAL.md, map, notes, skills without context bloat.
triggers: AGAL, hot path, orientation, skills sync, context budget, disclosure
verify: L3→L2→L1→L0; 1 note · ≤1 skill · errors first; no skills/ dump
---

# Agent usage

**Summary:** Orientation layer for AI-assisted Rust audio work.  
Start at **`AGAL.md`**, then the structural map. Skills load on purpose — not a dump.

Product: **agentic-audiolab**, binary **`agal`**, folder **`agal/`**.

## Disclosure (read layers)

Progressive disclosure — open the **next** layer only if the current one is not enough:

| Layer | Artifact | Open when |
|-------|----------|-----------|
| **L3** entry | `agal/AGAL.md` | always first — budget, loadouts, skills index |
| **L2** map | `agal.agent.md` (+ `delta`) | structure / health / what changed |
| **L1** focus | `notes/<focus>.md` (scan **`[ATOM]`** first) | work on one plugin/crate |
| **L0** raw | `*.slice.json` / `agal.json` | map + note still insufficient |

Skills are a **side loadout** (≤1 file; match `triggers`), not a layer.  
HTML / Cheatsheet = humans. Do **not** skip to L0 by default.

## Read order (hot path)

1. **L3** **`agal/AGAL.md`** — entry: skills index + budget / loadouts / disclosure
2. **L2** **`agal/agal.agent.md`** — structural map + **health** (`ok` / `degraded` / `blocked`)
3. If **blocked** — fix error findings first (`path` + `fix` on each line)
4. **L2** **`agal/agal.delta.md`** — what changed since last generate
5. **L1** **`agal/notes/<focus>.md`** — **one** note; atoms → open → intent
6. **loadout** — skills on demand from `AGAL.md` (never the whole tree; **≤1** skill file)
7. **L0** escalate only if needed:
   - `agal --plugin <name> .` → `<name>.slice.json`
   - `agal.json` for params_fields / full edges / **info** findings

## Context budget (per turn)

| Cap | Default |
|-----|---------|
| focus notes | **1** |
| skill files loaded | **1** (or one loadout row) |
| findings | **errors first**, then warns; skip info until escalate |
| JSON / slice | only after map + note are not enough |

## Equipped vs catalog

- **Equipped (on disk)** — top of `AGAL.md` after generate/sync: group names + file count.
- **Catalog** — full pack list + per-skill index lower in `AGAL.md`.
- If equipped is empty → `agal skills sync` before loading skills.

## Focus strip (L3)

When health is **degraded** / **blocked**, `AGAL.md` lists top error/warn findings (path + fix).  
Fix those first. Full list remains in `agal.agent.md`.

## Task loadouts / presets

| Task | Command | Load in context | Verify |
|------|---------|-----------------|--------|
| DSP / process / realtime | `--preset dsp-fix` | `00-core/*` | `cargo clippy --workspace --all-targets -- -D warnings` |
| Policy / terse edits | `--preset policy-edit` | caveman **or** ponytail | — |
| Slint UI | `--preset slint-ui` | slint (+ core) | — |
| CLAP ship / formats | `--preset clap-ship` | clap | after build: `clap-validator validate path/to/plugin.clap` |
| Agent orientation playbook | `--preset agent-playbook` | agent-usage | — |
| Full pack | `--only all` | rare | — |

```bash
agal skills sync --preset slint-ui
agal skills sync --only slint-ui    # same expansion
agal skills list                    # shows presets
```

## Stack layers

| Layer | Tool | Role |
|-------|------|------|
| **structure** | **agal** | map, health, notes, curated skills |
| **Rust lint** | Clippy | `agal doctor` + CI — not run by generate |
| **CLAP binary** | clap-validator | same |
| **symbols / call graph** | optional (codegraph, codebase-memory, graphify, …) | not agal; see `agal doctor` |

`agal doctor` PATH-checks Clippy + clap-validator **and** optional symbol binaries (found / not on PATH). Symbol tools never become generate findings.

## Skill frontmatter (optional)

```yaml
id: dsp-realtime
summary: …
triggers: process, realtime, audio callback
verify: review process() for alloc/lock
```

After sync, `AGAL.md` skill index shows `triggers` / `verify` when set — helps pick the right pack without opening every file.

## Notes atoms

**Graph atoms (auto)** — regenerated inside each note's AUTO block (max 12):

- kind / frameworks / migration
- formats, roles, has_process / has_editor
- semantic edges (`uses_ui`, `ipc_peer`, …), top deps / dependents
- error + warn findings (with `fix` when present)

At **L1**, scan the ```text [ATOM] …``` block first, then the human body.

**Human atoms** — below the HUMAN marker for durable decisions/lessons:

```text
[ATOM] type=decision|lesson|constraint | detail=…
```

**Workspace memory** — `notes/_workspace.md`: cross-plugin durable notes. Created once; **never** overwritten by `agal .`. Keep ~80 lines.

## Workspace root vs agal folder

| File | Who owns it |
|------|-------------|
| **`AGENTS.md`** (repo root) | **you** — product rules, commit policy, team notes |
| **`agal/AGAL.md`** | **agal** — regenerated; skills links + orientation |

Root `AGENTS.md` should **point** at `agal/AGAL.md` and keep product rules.  
agal never overwrites root `AGENTS.md`.

## Skills (tool packs)

- Live under **`agal/skills/`** after `agal skills sync` (never repo-root `*_SKILL.md`).
- Numbered groups: `00-core`, `01-policy`, `02-frameworks`, `03-formats`, `04-ui`, `05-migrations`, `06-agents`.
- Selectors: `core`, `ui/slint`, `04-ui/slint`, mixes, `all`.
- Without `--force`, existing files are **skipped** (local adaptations kept).
- Load **one** skill when the task needs it — not the whole tree.
- Live index is regenerated into **`AGAL.md`** on every `agal .` and `agal skills sync`.

## Do not

- Load every skill on every turn
- Open full JSON by default
- Treat completed migrations as ongoing work (agent map is quiet when legacy=0)
- Treat HTML as agent input (human map)
- Assume Clippy / clap-validator ran — use `agal doctor` + your CI
- Dump specialized guidance into root AGENTS.md when it belongs in a skill pack

## Humans

- **Cheatsheet.md** — all CLI commands in this folder
- **HTML** — overview graph, focus 1-hop, findings
- **notes/** — intent, open work, decisions, optional atoms (edit below HUMAN fence)
- **agal.toml** — migrations, rules, `[[suppress]]`
- Tool packs: `agal skills sync` (default: **core**)

## Commands

```bash
agal .                          # regenerate map + AGAL.md + notes headers + html
agal --plugin aether .          # + one-hop slice
agal doctor                     # Clippy + clap-validator PATH check
agal skills sync                # core → agal/skills/
agal skills sync --only policy  # opt-in style skills
agal skills sync --only ui/slint
agal skills sync --only all     # full skill pack (rare)
agal skills list
```
