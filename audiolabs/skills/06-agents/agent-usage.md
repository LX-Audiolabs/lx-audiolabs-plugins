---
id: agent-usage
group: agents
summary: How agents consume AGAL.md, map, notes, skills without context bloat.
---

# Agent usage

**Summary:** Orientation layer for AI-assisted Rust audio work.  
Start at **`AGAL.md`**, then the structural map. Skills load on purpose — not a dump.

Product: **agentic-audiolab**, binary **`agal`**, folder **`audiolabs/`**.

## Read order (hot path)

1. **`audiolabs/AGAL.md`** — agal-owned entry: skills index + hot path
2. **`audiolabs/audiolabs.agent.md`** — structural map + **health** (`ok` / `degraded` / `blocked`)
3. If **blocked** — fix **error** findings first (`path` + `fix` on each line)
4. **`audiolabs/audiolabs.delta.md`** — what changed since last generate
5. **`audiolabs/notes/<focus>.md`** — one plugin or crate (e.g. `aether.md`)
6. **Skills** on demand from the pack list in `AGAL.md` (never the whole tree)
7. Escalate only if needed:
   - `agal --plugin <name> .` → `<name>.slice.json`
   - `audiolabs.json` for params_fields / full edges / **info** findings

## Workspace root vs agal folder

| File | Who owns it |
|------|-------------|
| **`AGENTS.md`** (repo root) | **you** — product rules, commit policy, team notes |
| **`audiolabs/AGAL.md`** | **agal** — regenerated; skills links + orientation |

Root `AGENTS.md` should **point** at `audiolabs/AGAL.md` and keep product rules.  
agal never overwrites root `AGENTS.md`.

## Skills (tool packs)

- Live under **`audiolabs/skills/`** after `agal skills sync` (never repo-root `*_SKILL.md`).
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
- **notes/** — intent, open work, decisions (edit below HUMAN fence)
- **agal.toml** — migrations, rules, `[[suppress]]`
- Tool packs: `agal skills sync` (default: **core**)

## Commands

```bash
agal .                          # regenerate map + AGAL.md + notes headers + html
agal --plugin aether .          # + one-hop slice
agal doctor                     # Clippy + clap-validator PATH check
agal skills sync                # core → audiolabs/skills/
agal skills sync --only policy  # opt-in style skills
agal skills sync --only ui/slint
agal skills sync --only all     # full skill pack (rare)
agal skills list
```
