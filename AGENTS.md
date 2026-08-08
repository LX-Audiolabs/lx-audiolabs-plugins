# AGENTS.md — LX Audiolabs

Product / team rules for this workspace. **Orientation graph + skills** are owned by agal.

## Orientation (agal)

Read **`agal/AGAL.md`** first for map, health, skills index, and hot path.  
Structural detail: `agal/agal.agent.md`.  
Regenerate: `agal .` · skills: `agal skills sync` · doctor: `agal doctor`.

Do **not** dump `agal/` into context. Load one note / one skill on demand.

## github commits & push

Commits: `user.name=lxndrbe` · `user.email=ardvinnamoon@gmail.com`  
GitHub auth: `github.user=lxndrbe`

## UI direction (one-liner)

Slint 1.17.1 + **aura-editor** (femtovg/OpenGL default); plugins on aura-editor.  
Detail when needed: `agal skills sync --only ui/slint` → `agal/skills/04-ui/slint.md`.

## Build

```bash
cargo truce install --clap -p <pluginname>
```
