---
source: global
copied_by: template
date: 2026-07-30
adapted: false
reason: "universal realtime safety rules for all audio plugins"
---

# DSP Realtime Rules

**Summary:** Hard constraints für Audio-Thread-Code.
Gilt für jedes `process()` / jeden Audio-Callback.
Ziel: deterministische Laufzeit — keine Xruns, keine Dropouts, keine Crashes im Host.

## 1. Kein Alloc im Audio-Callback

- Kein `Vec::push`/`reserve`, `Box::new`, `String`, `format!`, `to_string()`, `clone()` auf Collections
- Versteckte Allocs zählen auch: `HashMap`-Wachstum, `collect()`, Lazy-Init beim ersten Call (`OnceLock`, `lazy_static`), geboxte Closures
- Alles in `prepare()`/`reset()` vorallokieren: Buffer auf Max-Blocksize, max. Voices, Delay-Lines
- Wachstum nur per Double-Buffer-Swap vom GUI-/Main-Thread aus — nie in-place im Callback
- Fixed-Capacity-Alternativen: `arrayvec`, statische Arrays, eigene Pools

## 2. Keine Locks, keine I/O, keine Syscalls im Audio-Thread

- Kein `Mutex`/`RwLock::lock()` in `process()` — blockiert → Priority Inversion → Dropouts
- GUI↔Audio nur lock-free: Atomics (`AtomicF32`, `AtomicBool`) für Parameter, SPSC-Ringbuffer (`rtrb`, `ringbuf`) für Events/Messages
- Kein File-I/O, kein `println!`/`eprintln!`/Logging, kein `thread::spawn`, kein Netzwerk
- Kein blockierendes Warten: `Condvar`, `channel.recv()`, `join()`
- Einzige erlaubte Lock-Form: `try_lock()` mit Fallback (z.B. alte Werte weiterverwenden)

## 3. Keine Panics, kein unwrap

- `unwrap()`/`expect()`/`panic!` im Callback = Abort mitten im Host-Prozess → verboten
- Indexing (`buf[i]`) nur wenn Bounds vorher bewiesen; sonst `get()` mit Fallback
- Kein `catch_unwind` im Hot Path — Code so designen, dass er nicht panicken kann
- Ungültige Zustände (SR/Blocksize in `prepare()`): sauber ablehnen, nicht durchreichen

## 4. Numerik & Struktur

- Denormal-Schutz in Feedback-Pfaden (FTZ/DAZ oder DC-Offset)
- Parameter-Smoothing vor dem Sample-Loop; Parameter einmal pro Block samplen
- Keine trig-Funktionen / `powf` im inneren Loop, wenn Koeffizienten pro Block reichen
- Erste-Call-Effekte vermeiden: jede Lazy-Init gehört in `prepare()`, nicht in `process()`

## Verifizieren

- Debug/Test mit zählendem `#[global_allocator]`: `process()` fahren, `alloc_count == 0` asserten
- Review-Checkliste: jede Zeile im Callback gegen §1–3 prüfen
- Unter Last testen (kleine Blocksize, hohe CPU) — Funktion allein beweist keine RT-Safety

## See also

- [audio-thread-boundary.md](./audio-thread-boundary.md) — was wo läuft, Ownership über Threads
- [dsp-correctness.md](./dsp-correctness.md) — Filter-/Analyse-Korrektheit
