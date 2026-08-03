# Changelog — LX Audiolabs Slint Edition

**August 2026** — Kompletter UI-Neubau auf Slint 1.17.1 + baseview 0.3.

---

## Alle Plugins

- **Neue UI-Engine:** Slint 1.17.1 mit `lx-slint-editor` statt Vizia.
  - Standard-Backend jetzt FemtoVG/OpenGL — Plugins nur noch halb so groß wie mit Skia.
  - Alternativ: Skia (GPU) oder Software-Renderer inklusive Screenshot-Funktion.
- **baseview 0.3 direkt von crates.io** — kein eigener Fork mehr nötig.
- **Soft-3D Design** mit software-sicheren Knöpfen und konsistentem Look.
- **Clipboard-Support:** Ctrl+V für Vault-Pfade in allen Plugins.
- **Peak-dB-Anzeige** jetzt per Klick auf den Wert zurücksetzbar.
- **Canvas-Größe stabil bei DPI-Änderung** — kein Versatz mehr in Bitwig.
- **Linux CLAP:** Offizielle Linux-Builds via Zig-Cross-Compilation.

---

## Equilibrium 1.7.0

- PRE-MASTER jetzt mit 2-Sekunden-Peak-Hold.
- Peak-Reset per Klick.
- UI-Politur: Analyse-Balken, Farbabstimmung.
- Vault-Pfad mit Clipboard-Paste.

## Meridian 1.10.0

- **Tilt-EQ jetzt −2 bis +2 dB** (vorher −1 bis +1).
- **Spectrum mit Smooth-Toggle** (geglättet / ungeglättet).
- Auto-Loud LUFS-Metering.
- Peak-Hold-Reset per Klick auf die Anzeige.
- Vault-Persistenz für Presets.
- Meter-Skalierung, Cut-Slope-Farben (amber), Knob-Defaults überarbeitet.

## Aether 1.3.0

- Vault-Pfad mit Clipboard-Paste.
- TextInput-Fokus im Vault-Overlay korrigiert.

## Lucent 1.1.0 (noch keine BETA)

- **FFT-Hover-Cursor:** vertikaler Balken + Hz/kHz an der Maus (log 20 Hz…20 kHz).
- Resonance/Masking-Panels stabil: gleiche Breite, kleine ON/OFF, kein Layout-Jump.
- Volle Feature-Parität zur Vizia-Version (1.0.0-Linie).
- **SPAN-äquivalente Spektrum-Anzeige:** Smooth-Toggle, Range −78 dB.
- Session-SNAP für max-hold Resonance/Masking.
- RT-Stack-Port aus dev: SHM-into-APIs, Masking, Resolve-Cache.

## Lucent Relay 1.1.0 (noch keine BETA)

- SemVer-Align mit Lucent Slint-Edition (1.0.0 war noch Vizia-Linie).
- All-off-Maske korrigiert.
- Feature-Parität zur Vizia-Version.

## Aurum 0.2.6 (noch garkeine BETA)

- Peak-Hold-Reset.
- Clipper-Waveforms gefüllt.
- LIMIT/COLOR Layout-Politur.

---
