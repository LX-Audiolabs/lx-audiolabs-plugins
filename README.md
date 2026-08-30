# LX Audiolabs — plugins

[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Slint](https://img.shields.io/badge/Slint-1.17.1-2379F4.svg)](https://slint.dev)
[![AURA](https://img.shields.io/badge/framework-AURA-6E40C9.svg)](https://github.com/LX-Audiolabs/aura)
[![agal](https://img.shields.io/badge/powered%20by-agal-00ADD8.svg)](https://github.com/LX-Audiolabs/agal)

Official public product catalog for **LX Audiolabs**, built on **[AURA](https://github.com/LX-Audiolabs/aura)** (Slint UI + CLAP-first). Release CLAPs are built on **GitHub Actions**.

| | |
|--|--|
| **License** | [GPL-3.0-or-later](./LICENSE) |
| **Framework** | [AURA](https://github.com/LX-Audiolabs/aura) via crates.io (`lx-aura-*` 0.12) |
| **Ship format** | **CLAP** (scripts + release zips) |
| **Other formats** | VST3 / LV2 via AURA features — **self-build**, not packaged here |

---

## Status

| Plugin | Version | Role | Ship |
|--------|---------|------|------|
| **Aether** | 1.x | EQ + crossfeed | yes |
| **Meridian** | 1.x | Channel strip | yes |
| **Equilibrium** | 1.x | Pre-master spectral balancer | yes |
| **Lucent** | 1.x | Spectrum analyzer (standalone / hybrid / relay) | yes |
| **Lucent Relay** | 1.x | Relay publisher for Lucent | yes |

These five plugins are the current **shipping set**.

---

## Layout

```
crates/                 # product libraries
  lx-ui-slint/          # design system (Lx* components)
  lx-editor-utils/      # editor helpers (dirty, meters, snap, …)
  lx-vault/             # preset paths / last-preset config
  lx-analysis/          # product *Shared analysis types
plugins/                # products
  aether/ meridian/ equilibrium/
  lucent/ lucent-relay/
```

Runtime GUI: **aura-editor** + **aura-baseview** (default FemtoVG / OpenGL).

---

## Requirements

### Framework (AURA)

Ship deps come from **crates.io** (`lx-aura`, `lx-aura-editor`, `lx-aura-dsp`,
`lx-aura-build`, `lx-aura-test`, … — workspace pins **0.12.0**). Lib names stay
`aura::` / `aura_test::` / …

`cargo-aura` / preview / host are **not** on crates.io yet — install from the
AURA git tree when you need the CLI locally:

```bash
cargo install --git https://github.com/LX-Audiolabs/aura --locked cargo-aura
# or path: cargo install --path ../AURA/tools/cargo-aura --locked
```

Docs, ship matrix, and format wrappers: **[AURA README](https://github.com/LX-Audiolabs/aura)**.

### System (UI)

Default editor path is **FemtoVG** → needs a working **OpenGL 3.2 Core** (or newer) context. Pure GL 2.x / broken host embeds are unsupported; the UI may fail to open. **Audio processing does not need OpenGL** — only the editor window does.

---

## Plugins (detail)

| Crate | What |
|-------|------|
| `aether` | EQ + crossfeed |
| `meridian` | Channel strip |
| `equilibrium` | Pre-master spectral balancer |
| `lucent` | Spectrum analyzer (standalone / hybrid / relay) |
| `lucent-relay` | Relay publisher |

---

## Build (CLAP — supported ship path)

```powershell
# Workspace check
cargo check --workspace

# Install CLAP into the host search path (release)
cargo aura install --clap --release -plug meridian

# All shipping plugins
cargo aura install --clap --release -plug aether meridian equilibrium lucent lucent-relay

# Artifacts are produced under target/release/ (or target/<target-triplet>/release/
# for cross builds). GitHub Actions builds and packages the release CLAPs; local
# packaging scripts are no longer shipped in this repository.
```

### CLAP validate 

use https://github.com/free-audio/clap-validator

---

## VST3 / LV2 (self-build)

**Release packaging and default scripts ship CLAP only.**  
VST3 and LV2 are supported by **AURA** on its [ship matrix](https://github.com/LX-Audiolabs/aura#clap-first-formats); this catalog does not produce VST3/LV2 zips.

If you need them, build yourself with AURA’s CLI (plugin crates already declare `vst3` / `lv2` features):

```powershell
# Requires AURA + cargo-aura (see above)
cargo aura install --vst3 --release -plug meridian   # Windows / macOS
cargo aura install --lv2  --release -plug meridian   # Linux
```

Details, host notes, and support matrix: **[github.com/LX-Audiolabs/aura](https://github.com/LX-Audiolabs/aura)**.

---

## License

Copyright © 2026 LX Audiolabs  

GPL-3.0-or-later — see [LICENSE](./LICENSE).  
Plugins link **AURA** (also GPL-3.0-or-later); distributing a plugin binary means GPL obligations for that combined work.
