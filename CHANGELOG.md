# Changelog — LX Audiolabs Slint Edition

Format: newest release first. Shared UI / host notes under **Alle Plugins**,
then per-plugin sections. SemVer: **patch** = fix, **minor** = feature (API/DSP
unchanged), **major** = breaking.

**August 2026** — Kompletter UI-Neubau auf Slint 1.17.1 + baseview 0.3.

---

## 2026-08-08 — AURA product cutover (host smoke)

Branch: **`cutover/meridian-aura`** → merged 2026-08-09. Framework path deps →
sibling `AURA` repo. Shared stack: `PluginLogic` / derive params / CLAP export /
`aura-editor` on `aura-baseview`. Product DSP/UI stay in `lx-dsp`,
`lx-analysis`, `lx-ui-slint`.

### Status

| Plugin | AURA code | First host smoke / release build | CLAP id (stable) |
|--------|-----------|----------------------------------|------------------|
| **Aether** | done | **green** (first smoke + release) | `be.lxndr.aether` |
| **Mensor** | done | **green** (release) | `[removed]` (renamed from *aurum* — too close to *AURA*) |
| **Equilibrium** | done | **green** (first smoke + release) | `be.lxndr.equilibrium` |
| **Lucent** | done | **green** (release; rustfft AVX ICE fixed) | `be.lxndr.lucent` (keep) |
| **Lucent Relay** | done | **green** (release) | `be.lxndr.*` (keep) |
| **Meridian** | done | **green** (Bitwig + REAPER + release) | `be.lxndr.meridian` |

### Tooling notes

- rustc **1.96.0** pin (`rust-toolchain.toml`); 1.97 ICE/lld on this machine
- clap-validator: **always `-j 1` on Windows** (parallel ACCESS_VIOLATION flakes).
  Lucent Relay `param-fuzz-*` can crash under parallel jobs; serial full suite
  is green. Ship gate: `.\validate-clap.ps1` (forces `-j 1`). Unrelated to
  Lucent Relay `_flush_sentinel` (that only fixes param-set flush tests).
- Install: **one** CLAP path only (`%LOCALAPPDATA%\Programs\Common\CLAP\…`)
- Release: `codegen-units = 16` in root `Cargo.toml` avoids a rustc ICE in
  `rustfft` AVX code (was `1`); LTO stays enabled

### Version

Cutover complete — no catalog release tag yet; next tags happen on the
post-cutover release train. AURA framework version during cutover: **0.5.0**.

---

## 2026-08-04 — Bundled UI font (Linux footer clip fix)

### Alle Plugins (shared)

- **Bundled UI fonts** (OFL via `truce-font`): **Noto Sans** (Regular + Bold)
  als Default, **JetBrains Mono** (Regular + Bold) alternativ registriert.
  Bisher kein Font mitgeliefert → Slint nahm den OS-Default (Segoe UI /
  DejaVu Sans …) und Textmetriken drifteten pro Plattform. Auf Linux
  überlief der 110-px-Footer (`LxShellFooter`) bei Meridian/Equilibrium
  um ein paar px → Value-Texte unten abgeschnitten. Jetzt identische
  Metriken auf jedem OS.
- Verdrahtung: `lx-slint-build` materialisiert alle vier TTFs nach
  `OUT_DIR`, `lx-ui-slint/ui/lx.slint` importiert die Faces, jedes Plugin
  setzt `default-font-family: Lx.font-family` am Root-Window. Umschalter
  ist eine Konstante im Theme (`lx-theme.slint`): `"Noto Sans"` ↔
  `"JetBrains Mono"`. Bold bleibt in-family (kein Systemfont-Fallback).

### Version bumps

| Plugin        | From   | To     | Kind  | Notes                                |
|---------------|--------|--------|-------|--------------------------------------|
| Meridian      | 1.11.0 | 1.11.1 | patch | Linux: Footer-Clip durch Font-Metrik |
| Equilibrium   | 1.8.0  | 1.8.1  | patch | Linux: Footer-Clip durch Font-Metrik |
| Aether        | 1.4.0  | 1.4.1  | patch | Bundled font (Konsistenz)            |
| Lucent        | 1.2.0  | 1.2.1  | patch | Bundled font (Konsistenz)            |
| Lucent Relay  | 1.1.1  | 1.1.2  | patch | Bundled font (Konsistenz)            |
| Mensor        | 0.4.1  | 0.4.2  | patch | Bundled font (Konsistenz)            |

---

## 2026-08-04 — Mensor 0.4.1: Clipper Defaults 0 dB + SHAPE/COLOR Layout

- **Clipper Ceil M / Ceil S:** Range jetzt −6…**0 dB**, Default **0 dB** (vorher
  −1 dB → Eingangssignal clippte sofort). RESET-Button setzt ebenfalls 0 dB;
  Rechtsklick-Reset der Ceil-Knobs = 0 dB, Soft-Knobs = 0 %.
- **SHAPE kompakter:** M/S-Toggle bottom-aligned in der Knobzeile, Wave-Pits
  80 → 68 px; M/S-EQ-Reihen gequetscht (Spacing 8→4 px, Bänder 10→6 px) →
  kein Scrollbalken mehr im SHAPE-Tab.
- **SWEETEN LPF:** Range 18–35 kHz → **18–20 kHz** (FFT zeigt nur bis 20 kHz —
  voller Knobweg jetzt sichtbar wirksam), Default 20 kHz, Anzeige in kHz.
  Rechtsklick-Reset: HPF = 10 Hz (unten), LPF = 20 kHz (oben).
- **Footer DELIVERY:** LUFS/dBTP/LRA/PLR mit fester Breite (64 px, wie IN LVL)
  → Anzeige springt nicht mehr bei ein- vs. zweistelligen Werten.

---

## 2026-08-04 — Aurum: Rename `aurum-slint` → `aurum` + UI-Fixes

### Aurum 0.4.0 (noch garkeine BETA)

- **Rename:** Crate/Ordner `aurum-slint` → `aurum`; CLAP heißt jetzt "Aurum"
  (`bundle_id = "aurum"`, fourcc `Aurm`). Das alte Vizia-Aurum ist damit
  endgültig ersetzt. Workspace/CI/Build-Skripte/README nachgezogen.
- **SHAPE:** Clipper Soft M / Soft S Defaults 50 % → **0 %** (kein Clipping
  im Default); RESET-Button setzt ebenfalls 0 %.
- **M/S EQ:** Gain-Slider → kleine Knobs, Shelf/Q-Buttons (A/B/C) rechts
  daneben; Spektren-Pits flacher (72/88/104 px) → SIDE-Reihe ohne Scrollen
  erreichbar.
- **COLOR / SWEETEN:** EQ-Curve deckt wieder den vollen Pit ab (X-Achse =
  Spektrum-Achse), HPF-Rampe sitzt korrekt am linken Rand.
- **LIMIT:** True Peak Limiter jetzt rechts neben dem M/S MB Limiter statt
  darunter → kein Scrollen.
- **Header:** `MONO` → `MID` (Reihenfolge MID | SIDE), Param-Name "Mid".

### lx-ui-slint (shared)

- `LxEqCurve`: `fit: ImageFit.fill` auf dem Curve-Path — Koordinaten sind in
  Rust vorgemappt; Default `contain` letterboxte die Curve (sichtbar als
  X-Achsen-Versatz in Aurum SWEETEN). Betrifft auch Meridian/Aether (gleiche
  Overlay-Logik, jetzt überall exakt).

---

## 2026-08-04 — UI Zoom + shared SVG brand + OpenGL soft-fail

### Alle Plugins (shared)

- **UI Zoom (product scale):** fixed steps **75% · 100% (default) · 125%**.
  Layout stays at design coordinates; FemtoVG content scale =
  `host_scale × ui_zoom`. Host frame = `design × ui_zoom`. Knobs, peak meters,
  spectrum paths scale together (no per-control reflow).
- **SVG logo** (`lx-ui-slint` / `LxBrandLogo`) replaces text wordmark — same
  brand mark in every plugin header.
- **Logo click** opens the zoom menu (Rectangle overlay, not `PopupWindow` —
  DAW hit-tests).
- **Global preference:** `%APPDATA%\LX Audiolabs\ui-zoom` (Windows) /
  `~/.config/lx-audiolabs/ui-zoom` (Linux). First open / missing file → **100%**.
  Only exact steps 75/100/125 are loaded.
- Host HiDPI (`set_scale_factor`) remains separate from product zoom.
- Implementation: `lx-slint-editor::UiZoom`, `LxShellHeader` zoom props.
- **OpenGL / FemtoVG soft-fail** (`lx-slint-baseview` + `LxSlintEditor`):
  missing GL context, `make_current` failure, or FemtoVG renderer init
  (Result or panic) **no longer takes down the host**. Editor stays closed;
  audio still runs. Log line:
  `LX UI: OpenGL 3.2 Core unavailable or FemtoVG init failed …`.
  Targets old Linux/mac embeds (e.g. REAPER + weak GLX). Does not add a
  software/Skia fallback — still require GL 3.2+ for a working UI.

### Version bumps

| Plugin        | From   | To     | Kind  | Notes                                      |
|---------------|--------|--------|-------|--------------------------------------------|
| Meridian      | 1.10.0 | 1.11.0 | minor | Zoom menu + SVG logo                       |
| Equilibrium   | 1.7.0  | 1.8.0  | minor | Zoom menu + SVG logo                       |
| Aether        | 1.3.0  | 1.4.0  | minor | Zoom menu + SVG logo                       |
| Lucent        | 1.1.0  | 1.2.0  | minor | Zoom menu + SVG logo                       |
| Aurum         | 0.2.6  | 0.3.0  | minor | Zoom menu + SVG logo (0.x feature bump)    |
| Lucent Relay  | 1.1.0  | 1.1.1  | patch | SVG logo only; stays 100% (compact UI)     |

### Meridian 1.11.0

- UI Zoom 75/100/125 via logo menu; shared SVG brand.

### Equilibrium 1.8.0

- UI Zoom 75/100/125 via logo menu; shared SVG brand.

### Aether 1.4.0

- UI Zoom 75/100/125 via logo menu; shared SVG brand.

### Lucent 1.2.0 (noch keine BETA)

- UI Zoom 75/100/125 via logo menu; shared SVG brand.

### Lucent Relay 1.1.1 (noch keine BETA)

- Shared SVG brand (`LxBrandLogo`); no zoom menu (window stays design size).

### Aurum 0.3.0 (noch garkeine BETA)

- UI Zoom 75/100/125 via logo menu; shared SVG brand.

---

## Earlier — Alle Plugins (Slint stack)

### INFO — Systemanforderung UI (OpenGL)

- Standard-Renderer: **FemtoVG** → braucht **OpenGL 3.2 Core** (oder neuer).
  OpenGL ES 3.0+ mit funktionierendem Treiber ist vergleichbar.
- Sehr alte GPUs/Treiber, reines GL 2.x oder kaputte GLX-Embeds in der DAW
  (z. B. altes Linux + REAPER) sind **nicht unterstützt** — Editor kann crashen
  oder nicht öffnen. Die frühere **Vizia + Skia**-UI war hier weicher.
- **wgpu** würde dieselben alten Rechner nicht retten. Nur der Editor hängt an
  OpenGL; die Audio-Engine nicht.

- **Neue UI-Engine:** Slint 1.17.1 mit `lx-slint-editor` statt Vizia.
  - Standard-Backend jetzt FemtoVG/OpenGL — Plugins nur noch halb so groß wie mit Skia.
  - Alternativ: Skia (GPU) oder Software-Renderer inklusive Screenshot-Funktion.
- **baseview 0.3 direkt von crates.io** — kein eigener Fork mehr nötig.
- **Soft-3D Design** mit software-sicheren Knöpfen und konsistentem Look.
- **Clipboard-Support:** Ctrl+V für Vault-Pfade in allen Plugins.
- **Correlation-Bar** unter den Peakmetern ersetzt den alten Correlation-Dot.
- **Canvas-Größe stabil bei DPI-Änderung** — kein Versatz mehr in Bitwig.
- **Linux CLAP:** Offizielle Linux-Builds via Zig-Cross-Compilation.
- **Linux UI-Stabilität (lx-slint-baseview):** Kein pro-Frame Geometry-Rebuild
  und kein Host-`request_resize`-Kampf (Bitwig wuchs dagegen an) → weniger
  Knob/Slider-Ruckeln und weniger abgeschnittene Footer. Content-Scale bleibt
  Host-`set_scale` (nicht Xft.dpi).

---

## Earlier — per plugin (pre–UI-Zoom)

### Equilibrium 1.7.0

- PRE-MASTER jetzt mit 2-Sekunden-Peak-Hold.
- Peak-Reset per Klick.
- UI-Politur: Analyse-Balken, Farbabstimmung.
- Vault-Pfad mit Clipboard-Paste.

### Meridian 1.10.0

- **Tilt-EQ jetzt −2 bis +2 dB** (vorher −1 bis +1).
- **Spectrum mit Smooth-Toggle** (geglättet / ungeglättet).
- Auto-Loud LUFS-Metering.
- Peak-Hold-Reset per Klick auf die Anzeige.
- Vault-Persistenz für Presets.
- Meter-Skalierung, Cut-Slope-Farben (amber), Knob-Defaults überarbeitet.

### Aether 1.3.0

- Vault-Pfad mit Clipboard-Paste.
- TextInput-Fokus im Vault-Overlay korrigiert.

### Lucent 1.1.0 (noch keine BETA)

- **FFT-Hover-Cursor:** vertikaler Balken + Hz/kHz an der Maus (log 20 Hz…20 kHz).
- Resonance/Masking-Panels stabil: gleiche Breite, kleine ON/OFF, kein Layout-Jump.
- Volle Feature-Parität zur Vizia-Version (1.0.0-Linie).
- **SPAN-äquivalente Spektrum-Anzeige:** Smooth-Toggle, Range −78 dB.
- Session-SNAP für max-hold Resonance/Masking.
- RT-Stack-Port aus dev: SHM-into-APIs, Masking, Resolve-Cache.

### Lucent Relay 1.1.0 (noch keine BETA)

- SemVer-Align mit Lucent Slint-Edition (1.0.0 war noch Vizia-Linie).
- All-off-Maske korrigiert.
- Feature-Parität zur Vizia-Version.

### Aurum 0.2.6 (noch garkeine BETA)

- Peak-Hold-Reset.
- Clipper-Waveforms gefüllt.
- LIMIT/COLOR Layout-Politur.

---
