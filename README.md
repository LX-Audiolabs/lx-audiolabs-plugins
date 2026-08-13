# LX Audiolabs — plugins

[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Slint](https://img.shields.io/badge/Slint-1.17.1-2379F4.svg)](https://slint.dev)
[![AURA](https://img.shields.io/badge/framework-AURA-6E40C9.svg)](https://github.com/LX-Audiolabs/aura)
[![agal](https://img.shields.io/badge/powered%20by-agal-00ADD8.svg)](https://github.com/LX-Audiolabs/agal)

Official public product catalog for **LX Audiolabs**, built on **[AURA](https://github.com/LX-Audiolabs/aura)** (Slint UI + CLAP-first). Release CLAPs are built on **GitHub Actions**.

| | |
|--|--|
| **License** | [GPL-3.0-or-later](./LICENSE) |
| **Framework** | [AURA](https://github.com/LX-Audiolabs/aura) (path sibling or git) |
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

This catalog depends on AURA crates. Local layout used by LX:

```text
parent/
  aura/                      # https://github.com/LX-Audiolabs/aura
  lx-audiolabs-plugins/      # this repo
```

Root `Cargo.toml` uses path deps: `../AURA/...` (folder name may be `aura` or `AURA` — match the path or adjust).

Also install the CLI from the AURA tree:

```bash
cargo install --path ../AURA/tools/cargo-aura --locked
# or clone AURA first, then:
# cargo install --path path/to/aura/tools/cargo-aura --locked
export AURA_PATH="/path/to/aura"   # PowerShell: $env:AURA_PATH = "…"
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

Shared brand UI: `crates/lx-ui-slint` (`LxKnob`, spectrum, shell, …).

```slint
import {
    Lx, LxShellHeader, LxKnob, LxSpectrum, LxGoniometer, LxLedPeakMeter,
} from "../../../crates/lx-ui-slint/ui/lx.slint";
```

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

### CLAP validate (ship gate)

```powershell
cargo install clap-validator   # once
cargo aura install --clap --release -plug lucent-relay
.\validate-clap.ps1 -Plugins lucent-relay
.\validate-clap.ps1            # all installed LX CLAPs found on disk
```

**Windows:** always use **serial** validation (`-j 1`). Parallel jobs can flake with `0xc0000005`. `validate-clap.ps1` forces `-j 1`.

### Linux CLAPs (from Windows)

Cross-compile via Zig. The repository no longer hard-codes Windows linker
wrappers in `.cargo/config.toml`; set the linker via env vars or `cargo-zigbuild`:

```powershell
winget install zig.zig --source winget
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu

$env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = "zig cc"
cargo build --release --package aether --target x86_64-unknown-linux-gnu
```

Then copy `target/x86_64-unknown-linux-gnu/release/libaether.so` to your CLAP
search path (e.g. `~/.clap/`) or bundle it manually.

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
Plugins link **AURA** (also GPL-3.0-or-later); distributing a plugin binary means GPL obligations for that combined work. Selling with source is fine; closed-only ships are not.
