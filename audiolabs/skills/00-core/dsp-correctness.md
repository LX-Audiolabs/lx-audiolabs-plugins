---
source: global
copied_by: template
date: 2026-07-30
adapted: false
reason: "universal DSP correctness rules: filters, IIR/FIR state, FFT/metering analysis"
---

# DSP Correctness Rules

**Summary:** Korrektheits-Regeln für DSP-Bausteine.
Ergänzt [dsp-realtime.md](./dsp-realtime.md) (RT-Safety) und
[filter-biquad.md](./filter-biquad.md) (Biquad-Stabilität).

## 1. Filter: Samplerate & State explizit

- Jeder Filter dokumentiert: gültige SR-Range, SR-Abhängigkeit der Koeffizienten, State-Layout
- Koeffizienten sind Funktion der Samplerate — bei SR-Wechsel (`prepare()`) neu berechnen, nie weiterverwenden
- State (Delay-Elemente, `z1/z2`) klar benennen und typisieren; kein impliziter globaler State
- Ein Filter-Struct pro Kanal (oder State pro Kanal) — State-Sharing zwischen Kanälen ist ein Bug

## 2. IIR/FIR: Init, Denormal, Reset explizit

- **Init:** State startet bei 0 (oder dokumentiertem DC-Wert) — kein uninitialisierter Speicher, kein f32::NAN
- **Denormal:** IIR mit Feedback braucht Denormal-Schutz (FTZ/DAZ-Flag oder `+ 1e-20`-Offset im Feedback-Pfad)
- **Reset:** explizite `reset()`-Methode, die State löscht ohne Koeffizienten anzufassen; wird bei Transport-Stop/Seek und `reset()` des Hosts aufgerufen
- Klickfreiheit: nach Reset/SR-Wechsel Koeffizienten smoothen oder State crossfaden, nicht hart umschalten
- FIR: Länge/ taps fest oder in `prepare()` gesetzt; Latenz = (taps-1)/2 Samples dokumentieren

## 3. FFT, Metering, Analyse: Fenster, Latenz, Auflösung dokumentieren

- **Fenster:** Typ (Hann/Blackman/…), Größe N, Hop/Overlap (z.B. 50 %) explizit am Code/API vermerken
- **Latenz:** Analyse-Latenz in Samples (Fenstergröße + Hop + Pufferung) angeben — entscheidet über Delay-Compensation
- **Auflösung:** Frequenzauflösung = SR/N Hz; Zeitauflösung = Hop/SR s — beides dokumentieren
- Metering: Ballistik (Attack/Release in ms oder dB/s), Peak vs. RMS vs. LUFS klar unterscheiden
- Analyse-Threads: FFT nie im Audio-Callback (→ dsp-realtime §2); Audio-Thread schreibt nur in Ringbuffer

## Checkliste (Review)

- [ ] SR-Wechsel führt zu neu berechneten Koeffizienten
- [ ] `reset()` löscht State, lässt Koeffizienten
- [ ] Feedback-Pfade denormal-sicher
- [ ] Fenster/Latenz/Auflösung an jeder Analyse dokumentiert
- [ ] Kein FFT/Metering-Compute im Audio-Thread
