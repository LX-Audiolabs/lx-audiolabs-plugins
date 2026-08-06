---
source: global
copied_by: template
date: 2026-07-30
adapted: false
reason: "thread boundary rules: what runs where, messaging, ownership of shared state"
---

# Audio Thread Boundary

**Summary:** Wer was wo ausführt — Boundary zwischen Audio-, UI- und Main-Thread.
Mechanik der Lock-free-Kommunikation: siehe [dsp-realtime.md](./dsp-realtime.md) §2.

## 1. Was darf in DSP (Audio-Thread)

- Nur: Parameter samplen, smoothen, Samples rechnen, State fortschreiben
- Liest: Atomics (Parameter), SPSC-Queue (Note-/UI-Events), vorallokierte Buffer
- Schreibt: Ausgabe-Buffer, Atomics für Metering-Werte (Peak/RMS), SPSC-Queue Richtung UI (Metering, Status)
- Alles andere ist verdächtig — siehe dsp-realtime §1–3

## 2. Was nur in UI / Main-Thread

- Rendering, Widgets, Event-Handling, Preset-/File-I/O, Plugin-Scan, Netzwerk
- Parameter-Gesten: `begin_gesture` / `set_parameter` / `end_gesture` — Audio-Thread liest nur das Ergebnis
- Schwere Vorbereitung: FFT-Pläne, Samples laden, Lookup-Tabellen bauen → dann per Swap an den Audio-Thread übergeben
- Metering-Darstellung: UI pollt die Metering-Atomics im eigenen Timer (~30 Hz), nie umgekehrt

## 3. Messaging

- **UI → Audio:** Atomics für kontinuierliche Parameter (Cutoff, Gain); SPSC-Ringbuffer für diskrete Events (Notes, Program-Change)
- **Audio → UI:** SPSC-Ringbuffer für Events (Voice-Status), Atomics/Triple-Buffer für Metering
- **Große Objekte** (IR, Wavetables, Presets): außerhalb bauen, dann Pointer-Swap (`Arc` + atomic swap / Double-Buffer); Audio-Seite droppt das alte Objekt nie im Callback — Rückgabe-Queue an den Main-Thread
- Reihenfolge: Event-Queue am Blockanfang einmal entleeren, dann Atomics samplen, dann Sample-Loop

## 4. Ownership über Shared State

- Audio-Thread **besitzt** den DSP-State exklusiv (Filter-State, Voices, Delay-Lines) — niemand sonst schreibt darein
- Shared State nur über zwei Formen: Atomics (POD) oder Queues (Nachrichten) — nie `&mut` über Threadgrenzen, nie `Mutex` auf dem Audiopfad
- Ownership-Wechsel nur per Move/Swap: Bauen und Droppen auf Main/UI, Rechnen auf Audio
- `unsafe`/`Send`-Overrides an der Boundary: einzeln begründen und kommentieren, sonst Tabu

## Checkliste (Review)

- [ ] Jeder shared Wert ist entweder Atomic oder fließt durch eine Queue
- [ ] Kein `&mut` auf DSP-State von außerhalb des Audio-Threads
- [ ] Große Objekte: gebaut + gedroppt außerhalb, nur geswappt auf Audio
- [ ] UI liest Metering per Timer-Poll, Audio pushed nie synchron in die UI
