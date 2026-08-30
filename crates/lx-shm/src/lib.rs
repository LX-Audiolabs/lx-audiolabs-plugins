//! Cross-process / cross-DLL publish/subscribe hub backed by named shared memory.
//!
//! Each `.clap` / `.vst3` / `.dll` is a separate cdylib, so a process-global
//! `OnceLock` in a statically-linked rlib is NOT shared between plugins. Two
//! different plugin files each get their own copy and therefore cannot talk
//! through a plain global.
//!
//! Solution: a single named shared-memory segment (`CreateFileMapping` on
//! Windows, `shm_open`+`mmap` on macOS via the `shared_memory` crate). All
//! plugin instances in the host process map the SAME segment.
//!
//! ## Architecture
//!
//! Two registries live in every segment:
//!
//! **Publisher slots** — each producer claims one and writes its payload
//! plus a `target` name (which consumer to send to; empty = broadcast).
//!
//! **Consumer slots** — each consumer claims one and publishes its instance
//! `name`, so producers can list available targets.
//!
//! The generic [`Hub<S>`] is shared by the spectrum/relay channel
//! ([`RelayHub`] = `Hub<SpectrumSlot>`) and the CV channel
//! ([`CvHub`] = `Hub<CvSlot>`). Each channel lives in its own OS segment so
//! CV publishers never appear in the relay channel and vice versa.
//! CV payload is nine named floats (`CV_LOCK`…`CV_RAND`); never audio-rate.
//!
//! ## Concurrency model
//!
//! Each slot uses a seqlock: the writer bumps `seq` to odd, writes the payload
//! via raw pointers, then bumps to even. Readers copy the payload and retry if
//! `seq` changed or was odd during the copy. Payload fields live in `UnsafeCell`;
//! byte access uses `copy_nonoverlapping` for safety. All cross-thread access is
//! guarded by atomic operations or the seqlock.
//!
//! **Audio-thread safe:** Reads are allocation-free and never block.
//!
//! ## Liveness tracking
//!
//! Each write stamps `heartbeat_ms` (wall-clock millis). Readers skip slots whose
//! heartbeat is older than `STALE_MS`, so a removed plugin's entry disappears
//! automatically after timeout. Slots are claimed via compare-and-swap (CAS), so
//! two instances never hold the same slot. A slot held by a dead instance (stale
//! heartbeat) is reclaimable by any new claimant.
//!
//! ## Error handling
//!
//! Invalid slot indices (>= `MAX_SLOTS` or `MAX_CONSUMERS`) are silently ignored by
//! write/touch functions. `claim_*_slot()` returns `None` when all slots are full
//! or taken by stale instances. `read_*()` returns empty results when the hub
//! cannot be mapped. Seqlock reads retry up to 4 times if the writer interferes;
//! if all 4 retries fail, the read is dropped (partial data is not returned).

use std::cell::UnsafeCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use shared_memory::{Shmem, ShmemConf};

/// Number of spectrum bins per payload frame.
pub const SPECTRUM_BINS: usize = 1024;
/// Number of CV channels per payload frame.
///
/// Layout (block/step rate, never audio-rate):
/// `lock`, `gate`, `pitch`, `bus_a`, `bus_b`, `eoc`, `env`, `lfo`, `rand`.
pub const CV_CHANNELS: usize = 9;
/// CV index: register freeze amount (0…1).
pub const CV_LOCK: usize = 0;
/// CV index: pulse/level gate (not MIDI gate).
pub const CV_GATE: usize = 1;
/// CV index: 1V/oct-style or MIDI-float pitch for FX/glide.
pub const CV_PITCH: usize = 2;
/// CV index: cross-track layer bus A.
pub const CV_BUS_A: usize = 3;
/// CV index: cross-track layer bus B.
pub const CV_BUS_B: usize = 4;
/// CV index: end-of-cycle trigger (e.g. Nimbus freeze).
pub const CV_EOC: usize = 5;
/// CV index: envelope follower / Trace env.
pub const CV_ENV: usize = 6;
/// CV index: LFO.
pub const CV_LFO: usize = 7;
/// CV index: random / S&H.
pub const CV_RAND: usize = 8;
/// Maximum number of publisher slots.
pub const MAX_SLOTS: usize = 16;
/// Maximum number of consumer instances advertising a name.
pub const MAX_CONSUMERS: usize = 16;
/// Maximum label/name length in bytes (UTF-8).
pub const MAX_NAME_LEN: usize = 32;
/// A slot is considered dead if no heartbeat arrived within this window.
pub const STALE_MS: u64 = 500;

/// Per-band energy (dB) for dynamic-EQ triggering.
pub const EQ_BANDS: usize = 5;

// ----------------------------------------------------------------------------
// Segment identity helpers
// ----------------------------------------------------------------------------

const fn magic_from_suffix(suffix: &[u8]) -> u32 {
    assert!(
        suffix.len() == 4,
        "MAGIC_SUFFIX must be exactly four ASCII bytes"
    );
    ((suffix[0] as u32) << 24)
        | ((suffix[1] as u32) << 16)
        | ((suffix[2] as u32) << 8)
        | (suffix[3] as u32)
}

// ----------------------------------------------------------------------------
// Slot trait and concrete slot layouts
// ----------------------------------------------------------------------------

/// Common interface for publisher slot layouts stored in [`HubShared<S>`].
///
/// Each slot type owns its OS segment id, its magic suffix, and its version.
/// The trait also exposes the shared administrative fields so the generic
/// [`Hub<S>`] can implement claim/release/heartbeat logic once.
///
/// # Safety
///
/// Implementations must be `#[repr(C)]`, must place the administrative fields
/// (`seq`, `claimed`, `generation`, `heartbeat_ms`, `name_len`, `active`, `name`,
/// `target_len`, `target`) at the same offsets, and must be `Sync` because the
/// backing memory is shared across threads and processes.
pub unsafe trait Slot: Send + Sync + Sized + 'static {
    /// OS-global name for this channel's segment.
    const OS_ID: &'static str;
    /// Four ASCII bytes used to compute the segment magic.
    const MAGIC_SUFFIX: &'static str;
    /// Segment layout version written by the creator.
    const VERSION: u32;

    fn seq(&self) -> &AtomicU32;
    fn claimed(&self) -> &AtomicU32;
    fn generation(&self) -> &AtomicU32;
    fn heartbeat_ms(&self) -> &AtomicU64;
    fn active(&self) -> &AtomicU32;
    fn name_len(&self) -> &UnsafeCell<u32>;
    fn name(&self) -> &UnsafeCell<[u8; MAX_NAME_LEN]>;
    fn target_len(&self) -> &UnsafeCell<u32>;
    fn target(&self) -> &UnsafeCell<[u8; MAX_NAME_LEN]>;
}

/// One spectrum publisher's data. `#[repr(C)]` so the byte layout is identical across DLLs.
#[repr(C)]
pub struct SpectrumSlot {
    /// Seqlock counter: even = stable, odd = write in progress.
    seq: AtomicU32,
    /// Auto-slot ownership: 0 = free, 1 = claimed. CAS-guarded; a slot whose
    /// owner died (stale heartbeat) can be reclaimed.
    claimed: AtomicU32,
    /// Bumped on every successful claim (fresh or stale-reclaim). The holder
    /// caches the value it got back from `claim_slot`; `write`/`touch` check
    /// it still matches before touching the payload, so an evicted owner
    /// (reclaimed out from under it after a stale heartbeat) finds out and
    /// stops writing instead of corrupting whoever took the slot.
    generation: AtomicU32,
    /// Wall-clock millis of the last write (liveness).
    heartbeat_ms: AtomicU64,
    /// Payload (seqlock-protected, accessed via raw pointers):
    name_len: UnsafeCell<u32>,
    /// Set to 1 atomically *after* the payload is fully written. Readers check
    /// this inside the seqlock so they never observe `active=1` with stale data.
    active: AtomicU32,
    name: UnsafeCell<[u8; MAX_NAME_LEN]>,
    /// Target consumer instance name; empty = broadcast to every consumer.
    target_len: UnsafeCell<u32>,
    target: UnsafeCell<[u8; MAX_NAME_LEN]>,
    bins: UnsafeCell<[f32; SPECTRUM_BINS]>,
    /// Per-band energy (dB) for dynamic-EQ triggering: Low Shelf, Peak 1–3, High Shelf.
    band_energy: UnsafeCell<[f32; EQ_BANDS]>,
}

// SAFETY: all cross-thread access goes through atomics (seq/heartbeat) and the
// seqlock-guarded raw-pointer payload; we never hand out `&` to the payload.
unsafe impl Sync for SpectrumSlot {}

unsafe impl Slot for SpectrumSlot {
    /// OS-global name for the relay segment. The `_vN` suffix is bumped whenever the
    /// slot layout or claim protocol changes, so an old plugin (different layout)
    /// maps a *separate* segment instead of colliding with the new one.
    /// `_v2`: added the `claimed` flag for atomic auto-slot assignment.
    /// `_v3`: publisher slots gained a `target` name; added the consumer-name registry.
    /// `_v4`: publisher slots gained a `band_energy` [f32; 5] for dynamic-EQ triggering.
    /// `_v5`: publisher slots gained a `generation` counter — a stale-reclaimed
    /// slot's original (evicted) owner can now detect it lost the slot and stop
    /// writing, instead of racing the new owner forever (see `write`/`touch`).
    /// `_v6`: `active` changed from `UnsafeCell<u32>` to `AtomicU32` and is now
    /// written *after* payload data with `Release` ordering, fixing a cross-process
    /// data race where readers saw `active=1` before the seqlock-protected payload
    /// was actually visible.
    /// Seqlock writers now heal abandoned odd `seq` values and reset `seq` on claim
    /// so a crash mid-write cannot brick a slot permanently.
    /// `_v7`: creator zero-fills the whole hub; `try_claim` reclaims any non-zero
    /// `claimed` with a stale heartbeat (not only `claimed==1`). Uninitialized or
    /// corrupted consumer slots previously stuck forever → Relay saw "no Lucent".
    const OS_ID: &str = "lxaudiolabs_lucent_relay_v7";
    /// "LXRD" — marks a fully-initialized relay segment.
    const MAGIC_SUFFIX: &str = "LXRD";
    const VERSION: u32 = 7;

    fn seq(&self) -> &AtomicU32 {
        &self.seq
    }
    fn claimed(&self) -> &AtomicU32 {
        &self.claimed
    }
    fn generation(&self) -> &AtomicU32 {
        &self.generation
    }
    fn heartbeat_ms(&self) -> &AtomicU64 {
        &self.heartbeat_ms
    }
    fn active(&self) -> &AtomicU32 {
        &self.active
    }
    fn name_len(&self) -> &UnsafeCell<u32> {
        &self.name_len
    }
    fn name(&self) -> &UnsafeCell<[u8; MAX_NAME_LEN]> {
        &self.name
    }
    fn target_len(&self) -> &UnsafeCell<u32> {
        &self.target_len
    }
    fn target(&self) -> &UnsafeCell<[u8; MAX_NAME_LEN]> {
        &self.target
    }
}

/// One CV publisher's data. `#[repr(C)]` so the byte layout is identical across DLLs.
///
/// Admin field order matches [`SpectrumSlot`] (see [`Slot`] safety contract).
/// Payload is [`CV_CHANNELS`] floats — see `CV_LOCK`…`CV_RAND`.
#[repr(C)]
pub struct CvSlot {
    /// Seqlock counter: even = stable, odd = write in progress.
    seq: AtomicU32,
    /// Auto-slot ownership: 0 = free, 1 = claimed. CAS-guarded.
    claimed: AtomicU32,
    /// Generation counter for stale-reclaim detection.
    generation: AtomicU32,
    /// Wall-clock millis of the last write (liveness).
    heartbeat_ms: AtomicU64,
    /// Payload (seqlock-protected, accessed via raw pointers):
    name_len: UnsafeCell<u32>,
    /// Set to 1 atomically *after* the payload is fully written.
    active: AtomicU32,
    name: UnsafeCell<[u8; MAX_NAME_LEN]>,
    /// Target consumer instance name; empty = broadcast.
    target_len: UnsafeCell<u32>,
    target: UnsafeCell<[u8; MAX_NAME_LEN]>,
    values: UnsafeCell<[f32; CV_CHANNELS]>,
}

// SAFETY: see SpectrumSlot. All cross-thread access is via atomics and the
// seqlock-guarded raw-pointer payload; we never hand out `&` to the payload.
unsafe impl Sync for CvSlot {}

unsafe impl Slot for CvSlot {
    const OS_ID: &str = "lxaudiolabs_cv_v1";
    const MAGIC_SUFFIX: &str = "CVRD";
    const VERSION: u32 = 1;

    fn seq(&self) -> &AtomicU32 {
        &self.seq
    }
    fn claimed(&self) -> &AtomicU32 {
        &self.claimed
    }
    fn generation(&self) -> &AtomicU32 {
        &self.generation
    }
    fn heartbeat_ms(&self) -> &AtomicU64 {
        &self.heartbeat_ms
    }
    fn active(&self) -> &AtomicU32 {
        &self.active
    }
    fn name_len(&self) -> &UnsafeCell<u32> {
        &self.name_len
    }
    fn name(&self) -> &UnsafeCell<[u8; MAX_NAME_LEN]> {
        &self.name
    }
    fn target_len(&self) -> &UnsafeCell<u32> {
        &self.target_len
    }
    fn target(&self) -> &UnsafeCell<[u8; MAX_NAME_LEN]> {
        &self.target
    }
}

/// One consumer instance advertising its name so publishers can target it.
#[repr(C)]
struct ConsumerSlot {
    seq: AtomicU32,
    claimed: AtomicU32,
    heartbeat_ms: AtomicU64,
    name_len: UnsafeCell<u32>,
    name: UnsafeCell<[u8; MAX_NAME_LEN]>,
}

// SAFETY: see SpectrumSlot.
unsafe impl Sync for ConsumerSlot {}

#[repr(C)]
struct HubShared<S: Slot> {
    magic: AtomicU32,
    version: AtomicU32,
    slots: [S; MAX_SLOTS],
    consumers: [ConsumerSlot; MAX_CONSUMERS],
}

// Compile-time layout guarantees so the segment is byte-compatible everywhere.
const _: () = {
    assert!(CV_CHANNELS == 9);
    assert!(CV_RAND == CV_CHANNELS - 1);
    assert!(core::mem::align_of::<SpectrumSlot>() == 8);
    assert!(core::mem::align_of::<ConsumerSlot>() == 8);
    assert!(core::mem::align_of::<CvSlot>() == 8);
    assert!(
        core::mem::size_of::<HubShared<SpectrumSlot>>()
            == 8 + MAX_SLOTS * core::mem::size_of::<SpectrumSlot>()
                + MAX_CONSUMERS * core::mem::size_of::<ConsumerSlot>()
    );
    assert!(
        core::mem::size_of::<HubShared<CvSlot>>()
            == 8 + MAX_SLOTS * core::mem::size_of::<CvSlot>()
                + MAX_CONSUMERS * core::mem::size_of::<ConsumerSlot>()
    );
};

/// Get wall-clock time in milliseconds since `UNIX_EPOCH`.
///
/// Returns `SystemTime::now()` as milliseconds, consistent across all plugins
/// in the process. Used for heartbeat tracking and slot liveness checks.
///
/// # Returns
///
/// Milliseconds since `UNIX_EPOCH`. If system time is unavailable, returns 0
/// (a very old heartbeat that will be treated as stale).
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

/// Format a consumer instance's display name.
///
/// If the provided name is empty or whitespace, returns a default name like
/// "Hub 1", "Hub 2", etc. (slot+1). Otherwise returns the name as-is.
#[must_use]
pub fn display_name(name: &str, slot: u8) -> String {
    if name.trim().is_empty() {
        format!("Hub {}", slot + 1)
    } else {
        name.to_string()
    }
}

/// Copy a UTF-8 name into a fixed slot buffer, returning the written length.
///
/// # Safety
///
/// `buf` must point to at least `MAX_NAME_LEN` writable bytes. The caller must
/// ensure no other thread reads the buffer until this function returns.
unsafe fn write_name_bytes(buf: *mut u8, name: &str) -> u32 {
    let bytes = name.as_bytes();
    let len = bytes.len().min(MAX_NAME_LEN);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);
        for i in len..MAX_NAME_LEN {
            *buf.add(i) = 0;
        }
    }
    u32::try_from(len).expect("len is clamped to MAX_NAME_LEN <= u32::MAX")
}

/// Seqlock write start — returns the stable even `seq` before bumping to odd.
/// Heals abandoned odd sequences left in shared memory (e.g. after a crash
/// mid-write); otherwise readers treat the slot as permanently busy.
fn seqlock_begin(seq: &AtomicU32) -> u32 {
    loop {
        let cur = seq.load(Ordering::Acquire);
        if cur & 1 == 0 {
            seq.store(cur.wrapping_add(1), Ordering::Release);
            fence(Ordering::Release);
            return cur;
        }
        let _ = seq.compare_exchange(
            cur,
            cur.wrapping_add(1),
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
    }
}

#[inline]
fn seqlock_end(seq: &AtomicU32, seq0: u32) {
    fence(Ordering::Release);
    seq.store(seq0.wrapping_add(2), Ordering::Release);
}

/// Generic CAS-based slot claim shared by both registries.
/// `claimed`/`heartbeat` are the slot's atomics. Returns whether the claim won.
fn try_claim(claimed: &AtomicU32, heartbeat: &AtomicU64, now_ms: u64) -> bool {
    let mut c = claimed.load(Ordering::Acquire);
    if c != 0 {
        let hb = heartbeat.load(Ordering::Acquire);
        let stale = hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS;
        if stale {
            let _ = claimed.compare_exchange(c, 0, Ordering::AcqRel, Ordering::Relaxed);
            c = claimed.load(Ordering::Acquire);
        }
    }
    if c == 0
        && claimed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        heartbeat.store(now_ms, Ordering::Release);
        true
    } else {
        false
    }
}

/// Same CAS claim as `try_claim`, plus a generation bump — used only by the
/// publisher registry (publisher slots have a `generation` counter, consumer
/// slots don't need one). Returns the new generation on a won claim, `None` if
/// the slot is live and held.
fn try_claim_gen(
    claimed: &AtomicU32,
    heartbeat: &AtomicU64,
    generation: &AtomicU32,
    now_ms: u64,
) -> Option<u32> {
    let mut c = claimed.load(Ordering::Acquire);
    if c != 0 {
        let hb = heartbeat.load(Ordering::Acquire);
        let stale = hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS;
        if stale {
            let _ = claimed.compare_exchange(c, 0, Ordering::AcqRel, Ordering::Relaxed);
            c = claimed.load(Ordering::Acquire);
        }
    }
    if c == 0
        && claimed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        heartbeat.store(now_ms, Ordering::Release);
        Some(generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1))
    } else {
        None
    }
}

/// Generic shared-memory hub for a publisher slot type `S` plus a fixed
/// consumer-name registry.
///
/// `RelayHub` and `CvHub` are type aliases for `Hub<SpectrumSlot>` and
/// `Hub<CvSlot>`; each maps its own OS segment id and magic.
pub struct Hub<S: Slot> {
    _shmem: Shmem,
    shared: *const HubShared<S>,
}

// SAFETY: `shared` points into the shared mapping; all access is via atomics +
// seqlock-guarded raw pointers (see Slot). The mapping outlives the handle.
unsafe impl<S: Slot> Send for Hub<S> {}
unsafe impl<S: Slot> Sync for Hub<S> {}

impl<S: Slot> Hub<S> {
    fn open_or_create() -> Option<Hub<S>> {
        let size = core::mem::size_of::<HubShared<S>>();
        let magic = magic_from_suffix(S::MAGIC_SUFFIX.as_bytes());

        let (shmem, is_creator) = map_segment::<S>(size)?;

        // SAFETY: `shmem.as_ptr()` comes from the OS allocator and is page-aligned,
        // which satisfies `HubShared<S>`'s 8-byte alignment requirement.
        #[allow(clippy::cast_ptr_alignment)]
        let shared = shmem.as_ptr().cast::<HubShared<S>>();

        if is_creator {
            unsafe {
                let p = shared.cast::<u8>();
                std::ptr::write_bytes(p, 0, size);
                (*shared).version.store(S::VERSION, Ordering::Release);
                (*shared).magic.store(magic, Ordering::Release);
            }
        } else {
            let mut spins = 0u32;
            while unsafe { (*shared).magic.load(Ordering::Acquire) } != magic {
                std::thread::yield_now();
                spins += 1;
                if spins > 1_000_000 {
                    return None;
                }
            }
        }

        Some(Hub {
            _shmem: shmem,
            shared,
        })
    }

    // ---- Publisher registry (writer = producer, reader = consumer) -----------

    /// Claim a free publisher slot for this instance.
    ///
    /// Scans the publisher registry for the first unclaimed slot (or a stale slot
    /// that can be reclaimed). Uses compare-and-swap atomics to ensure only one
    /// instance claims any given slot. Call this once at plugin initialization.
    ///
    /// # Arguments
    ///
    /// * `now_ms` - Current wall-clock time in milliseconds (from [`now_ms()`])
    ///
    /// # Returns
    ///
    /// - `Some((slot_index, generation))` if a slot was claimed (index
    ///   `0..MAX_SLOTS`). Store both — `generation` must be passed to every
    ///   `write`/`touch` call so the hub can tell if this claim is still
    ///   valid (see `write`).
    /// - `None` if all slots are occupied by live instances
    ///
    /// # Panics
    ///
    /// Never panics in practice: the slot index is bounded by `MAX_SLOTS`, which
    /// fits in `u8`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some((slot, generation)) = hub.claim_slot(lx_shm::now_ms()) {
    ///     // Store slot + generation, use both to publish data
    /// }
    /// ```
    #[must_use]
    pub fn claim_slot(&self, now_ms: u64) -> Option<(u8, u32)> {
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };
            if let Some(generation) =
                try_claim_gen(s.claimed(), s.heartbeat_ms(), s.generation(), now_ms)
            {
                s.seq().store(0, Ordering::Release);
                return Some((
                    u8::try_from(idx).expect("idx < MAX_SLOTS <= u8::MAX"),
                    generation,
                ));
            }
        }
        None
    }

    /// Release a previously claimed publisher slot.
    ///
    /// Call this on plugin teardown to free the slot for other instances.
    /// Marks the slot as unclaimed and sets heartbeat to 0 (treated as immediately
    /// stale by readers).
    ///
    /// # Arguments
    ///
    /// * `slot` - The slot index returned by [`claim_slot()`]
    ///
    /// # Behavior on invalid slot
    ///
    /// If `slot >= MAX_SLOTS`, this call is silently ignored (no-op).
    pub fn release_slot(&self, slot: u8) {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return;
        }
        let s = unsafe { &(*self.shared).slots[idx] };
        s.heartbeat_ms().store(0, Ordering::Release);
        s.claimed().store(0, Ordering::Release);
    }

    /// Raw atomic snapshot of a slot: (claimed, generation, seq, `heartbeat_ms`).
    /// For debugging — no seqlock, just the admin fields.
    #[must_use]
    pub fn slot_raw_state(&self, slot: u8) -> Option<(u32, u32, u32, u64)> {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return None;
        }
        let s = unsafe { &(*self.shared).slots[idx] };
        Some((
            s.claimed().load(Ordering::Acquire),
            s.generation().load(Ordering::Acquire),
            s.seq().load(Ordering::Acquire),
            s.heartbeat_ms().load(Ordering::Acquire),
        ))
    }

    // ---- Consumer registry (writer = consumer, reader = publisher) -----------

    /// Claim a consumer-name registry slot.
    ///
    /// Call this at plugin initialization to advertise your instance's name to
    /// publishers. Publishers read the consumer registry to find available targets.
    ///
    /// # Arguments
    ///
    /// * `now_ms` - Current wall-clock time (initializes heartbeat)
    ///
    /// # Returns
    ///
    /// - `Some(slot_index)` if a free slot was claimed (index `0..MAX_CONSUMERS`)
    /// - `None` if all consumer slots are occupied
    ///
    /// # Panics
    ///
    /// Never panics in practice: the slot index is bounded by `MAX_CONSUMERS`,
    /// which fits in `u8`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(slot) = hub.claim_consumer_slot(lx_shm::now_ms()) {
    ///     hub.write_consumer_name(slot, "My Analyzer", lx_shm::now_ms());
    /// }
    /// ```
    #[must_use]
    pub fn claim_consumer_slot(&self, now_ms: u64) -> Option<u8> {
        for idx in 0..MAX_CONSUMERS {
            let s = unsafe { &(*self.shared).consumers[idx] };
            if try_claim(&s.claimed, &s.heartbeat_ms, now_ms) {
                s.seq.store(0, Ordering::Release);
                return Some(u8::try_from(idx).expect("idx < MAX_CONSUMERS <= u8::MAX"));
            }
        }
        None
    }

    /// Release a previously claimed consumer-name slot.
    ///
    /// Call this on plugin teardown to free your name entry for other instances.
    /// Marks the slot as unclaimed and sets heartbeat to 0 (immediately stale).
    ///
    /// # Arguments
    ///
    /// * `slot` - The slot index returned by [`claim_consumer_slot()`]
    ///
    /// # Behavior on invalid slot
    ///
    /// If `slot >= MAX_CONSUMERS`, this call is silently ignored (no-op).
    pub fn release_consumer_slot(&self, slot: u8) {
        let idx = slot as usize;
        if idx >= MAX_CONSUMERS {
            return;
        }
        let s = unsafe { &(*self.shared).consumers[idx] };
        s.heartbeat_ms.store(0, Ordering::Release);
        s.claimed.store(0, Ordering::Release);
    }

    /// True if this consumer slot is claimed and its heartbeat is still fresh.
    #[must_use]
    pub fn consumer_slot_live(&self, slot: u8, now_ms: u64) -> bool {
        let idx = slot as usize;
        if idx >= MAX_CONSUMERS {
            return false;
        }
        let s = unsafe { &(*self.shared).consumers[idx] };
        if s.claimed.load(Ordering::Acquire) == 0 {
            return false;
        }
        let hb = s.heartbeat_ms.load(Ordering::Acquire);
        hb != 0 && now_ms.wrapping_sub(hb) <= STALE_MS
    }

    /// Publish this consumer's name and refresh its heartbeat.
    ///
    /// Call this on a regular interval (e.g., every 100ms) to keep your name
    /// visible to publishers. Publishers read this registry to build target lists.
    ///
    /// # Arguments
    ///
    /// * `slot` - The consumer slot index (from [`claim_consumer_slot()`])
    /// * `name` - The consumer's display name (max `MAX_NAME_LEN` bytes)
    /// * `now_ms` - Current wall-clock time (updates heartbeat)
    ///
    /// # Behavior on invalid slot
    ///
    /// If `slot >= MAX_CONSUMERS`, this call is silently ignored (no-op).
    ///
    /// # Example
    ///
    /// ```ignore
    /// // In a regular update loop:
    /// if let Some(slot) = my_consumer_slot {
    ///     hub.write_consumer_name(slot, "My Analyzer", lx_shm::now_ms());
    /// }
    /// ```
    pub fn write_consumer_name(&self, slot: u8, name: &str, now_ms: u64) {
        let idx = slot as usize;
        if idx >= MAX_CONSUMERS {
            return;
        }
        let s = unsafe { &(*self.shared).consumers[idx] };

        // Self-heal: a writer with a valid slot index owns this entry even if
        // `claimed` was never set (adopted atomics after reload) or was cleared.
        s.claimed.store(1, Ordering::Release);

        let seq0 = seqlock_begin(&s.seq);
        unsafe {
            *s.name_len.get() = write_name_bytes(s.name.get().cast::<u8>(), name);
        }
        seqlock_end(&s.seq, seq0);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
    }

    /// Read one consumer slot's name if it is live and non-empty.
    /// Returns the name length. Allocation-free — safe on the audio thread.
    fn read_consumer_slot(
        &self,
        idx: usize,
        now_ms: u64,
        out: &mut [u8; MAX_NAME_LEN],
    ) -> Option<usize> {
        let s = unsafe { &(*self.shared).consumers[idx] };
        let hb = s.heartbeat_ms.load(Ordering::Acquire);
        if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
            return None;
        }
        for _ in 0..4 {
            let seq1 = s.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                continue;
            }
            let name_len = unsafe {
                std::ptr::copy_nonoverlapping(
                    s.name.get().cast::<u8>(),
                    out.as_mut_ptr(),
                    MAX_NAME_LEN,
                );
                (*s.name_len.get() as usize).min(MAX_NAME_LEN)
            };
            fence(Ordering::Acquire);
            if seq1 == s.seq.load(Ordering::Acquire) {
                return if name_len == 0 { None } else { Some(name_len) };
            }
        }
        None
    }

    /// Check if a live consumer with the given name exists.
    #[must_use]
    pub fn consumer_exists(&self, name: &str, now_ms: u64) -> bool {
        let mut buf = [0u8; MAX_NAME_LEN];
        for idx in 0..MAX_CONSUMERS {
            if let Some(n) = self.read_consumer_slot(idx, now_ms, &mut buf)
                && &buf[..n] == name.as_bytes()
            {
                return true;
            }
        }
        false
    }

    /// Get the name of the single live consumer, if exactly one exists.
    pub fn single_consumer_name(&self, now_ms: u64, out: &mut [u8; MAX_NAME_LEN]) -> Option<usize> {
        let mut found: Option<usize> = None;
        let mut scratch = [0u8; MAX_NAME_LEN];
        for idx in 0..MAX_CONSUMERS {
            if let Some(n) = self.read_consumer_slot(idx, now_ms, &mut scratch) {
                if found.is_some() {
                    return None;
                }
                out[..n].copy_from_slice(&scratch[..n]);
                found = Some(n);
            }
        }
        found
    }

    /// List all live consumer names for UI dropdowns or routing decisions.
    ///
    /// Returns the display names of all live, non-empty consumer instances.
    /// Results are deduplicated; each unique name appears at most once.
    ///
    /// # Arguments
    ///
    /// * `now_ms` - Current wall-clock time (stale check)
    ///
    /// # Returns
    ///
    /// A vector of unique consumer names. Empty vector if no consumers are live.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let targets = hub.read_consumers(lx_shm::now_ms());
    /// if targets.is_empty() {
    ///     println!("No consumers available");
    /// } else {
    ///     for target in targets {
    ///         println!("Available target: {}", target);
    ///     }
    /// }
    /// ```
    #[must_use]
    pub fn read_consumers(&self, now_ms: u64) -> Vec<String> {
        let mut out = Vec::new();
        self.read_consumers_into(now_ms, &mut out);
        out
    }

    /// [`read_consumers`] variant that reuses a caller-owned buffer (keeps the
    /// `String` capacities across calls). For hot paths that resolve relay
    /// targets repeatedly.
    pub fn read_consumers_into(&self, now_ms: u64, out: &mut Vec<String>) {
        let mut n = 0;
        for idx in 0..MAX_CONSUMERS {
            let s = unsafe { &(*self.shared).consumers[idx] };

            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
                continue;
            }

            for _ in 0..4 {
                let seq1 = s.seq.load(Ordering::Acquire);
                if seq1 & 1 != 0 {
                    continue;
                }
                let mut name_buf = [0u8; MAX_NAME_LEN];
                let name_len = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.name.get().cast::<u8>(),
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (*s.name_len.get() as usize).min(MAX_NAME_LEN)
                };
                fence(Ordering::Acquire);
                let seq2 = s.seq.load(Ordering::Acquire);
                if seq1 == seq2 {
                    let name = String::from_utf8_lossy(&name_buf[..name_len]);
                    if !name.is_empty() && !out[..n].iter().any(|c| c == &name) {
                        if let Some(slot) = out.get_mut(n) {
                            slot.clear();
                            slot.push_str(&name);
                        } else {
                            out.push(name.into_owned());
                        }
                        n += 1;
                    }
                    break;
                }
            }
        }
        out.truncate(n);
    }
}

/// Spectrum/relay hub: `Hub<SpectrumSlot>` mapped to `lxaudiolabs_lucent_relay_v7`.
pub type RelayHub = Hub<SpectrumSlot>;

/// CV hub: `Hub<CvSlot>` mapped to `lxaudiolabs_cv_v1`.
pub type CvHub = Hub<CvSlot>;

impl RelayHub {
    /// Publish spectrum bins and metadata to this publisher slot.
    ///
    /// Updates the slot's payload atomically using seqlock synchronization. All
    /// fields are written together (bins, band energy, labels, target).
    ///
    /// The payload is protected by a seqlock: readers see either the old or new
    /// data, never a partially-written state.
    ///
    /// # Arguments
    ///
    /// * `slot` - The publisher slot index (from [`claim_slot()`])
    /// * `generation` - The generation returned alongside `slot` by
    ///   [`claim_slot()`]. Checked against the slot's current generation
    ///   before writing — if another instance reclaimed this slot (this
    ///   one's heartbeat went stale), the generations no longer match and
    ///   the write is skipped instead of corrupting the new owner's data.
    /// * `label` - A short name for this publisher (max `MAX_NAME_LEN` bytes)
    /// * `target` - Name of the consumer to send to:
    ///   - Empty string: broadcast to every consumer
    ///   - Non-empty: only the consumer with matching `display_name` receives it
    /// * `bins` - Spectrum data (up to `SPECTRUM_BINS` f32 values, typically dB)
    /// * `band_energy` - Per-band energy levels (up to `EQ_BANDS` f32 values, dB)
    /// * `now_ms` - Current wall-clock time in milliseconds (updates heartbeat)
    ///
    /// # Returns
    ///
    /// `true` if the write happened. `false` if `slot >= MAX_SLOTS` or this
    /// instance no longer owns the slot (`generation` mismatch) — the caller
    /// should treat `false` as "I was evicted" and clear its cached slot so
    /// it reclaims a fresh one on the next call to [`claim_slot()`].
    ///
    /// # Array truncation
    ///
    /// If `bins` or `band_energy` are shorter than expected, the rest is zero-filled
    /// (spectrum bins are filled with -90.0 dB, band energy with -90.0 dB).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn write(
        &self,
        slot: u8,
        generation: u32,
        label: &str,
        target: &str,
        bins: &[f32],
        band_energy: &[f32],
        now_ms: u64,
    ) -> bool {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return false;
        }
        let s = unsafe { &(*self.shared).slots[idx] };
        if s.generation.load(Ordering::Acquire) != generation {
            return false;
        }

        let seq0 = seqlock_begin(&s.seq);

        unsafe {
            *s.name_len.get() = write_name_bytes(s.name.get().cast::<u8>(), label);
            *s.target_len.get() = write_name_bytes(s.target.get().cast::<u8>(), target);

            let bins_ptr = s.bins.get().cast::<f32>();
            let n = bins.len().min(SPECTRUM_BINS);
            std::ptr::copy_nonoverlapping(bins.as_ptr(), bins_ptr, n);
            for i in n..SPECTRUM_BINS {
                *bins_ptr.add(i) = -90.0;
            }

            let be_ptr = s.band_energy.get().cast::<f32>();
            let m = band_energy.len().min(EQ_BANDS);
            std::ptr::copy_nonoverlapping(band_energy.as_ptr(), be_ptr, m);
            for i in m..EQ_BANDS {
                *be_ptr.add(i) = -90.0;
            }
        }

        s.active.store(1, Ordering::Release);

        seqlock_end(&s.seq, seq0);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
        true
    }

    /// Update metadata and heartbeat WITHOUT writing spectrum data.
    ///
    /// Useful for keeping a publisher alive when audio is not actively being
    /// published (e.g., when transport is stopped). Updates label, target, and
    /// heartbeat but leaves bins and `band_energy` untouched, so consumers continue
    /// seeing stale but valid spectrum data.
    ///
    /// # Arguments
    ///
    /// * `slot` - The publisher slot index
    /// * `generation` - The generation from [`claim_slot()`] — same
    ///   ownership check as [`write()`]. Critical here specifically:
    ///   without it, an evicted owner's `touch()` would keep refreshing the
    ///   slot's heartbeat, so it would never look stale and the two owners
    ///   would fight over the payload forever instead of the evicted one
    ///   backing off.
    /// * `label` - A short name for this publisher
    /// * `target` - Target consumer name (empty = broadcast)
    /// * `now_ms` - Current wall-clock time (refreshes heartbeat)
    ///
    /// # Returns
    ///
    /// `true` if the touch happened, `false` if `slot >= MAX_SLOTS` or this
    /// instance was evicted (`generation` mismatch) — same caller contract
    /// as [`write()`].
    #[must_use]
    pub fn touch(&self, slot: u8, generation: u32, label: &str, target: &str, now_ms: u64) -> bool {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return false;
        }
        let s = unsafe { &(*self.shared).slots[idx] };
        if s.generation.load(Ordering::Acquire) != generation {
            return false;
        }

        let seq0 = seqlock_begin(&s.seq);

        unsafe {
            *s.name_len.get() = write_name_bytes(s.name.get().cast::<u8>(), label);
            *s.target_len.get() = write_name_bytes(s.target.get().cast::<u8>(), target);
        }

        s.active.store(1, Ordering::Release);

        seqlock_end(&s.seq, seq0);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
        true
    }

    /// Read spectrum data from all publishers targeting this consumer.
    ///
    /// Returns a list of (`publisher_slot`, `publisher_label`, `spectrum_bins`) tuples
    /// for all live publishers whose target is empty (broadcast) or matches `my_name`.
    /// Stale publishers (no heartbeat within `STALE_MS` milliseconds) are skipped.
    ///
    /// Audio-thread safe: allocation-free for an empty result; allocates only for
    /// publishers found.
    ///
    /// # Arguments
    ///
    /// * `my_name` - This consumer's display name (use [`display_name()`])
    /// * `now_ms` - Current wall-clock time (used for stale checks)
    ///
    /// # Returns
    ///
    /// A vector of `(publisher_slot, publisher_name, spectrum_bins)` tuples.
    ///
    /// Returns empty vector if no matching publishers are live.
    ///
    /// # Retry behavior
    ///
    /// Each slot is read up to 16 times if the writer interferes (seqlock conflict).
    /// If all retries fail, that slot is silently skipped.
    #[must_use]
    pub fn read_active(
        &self,
        my_name: &str,
        now_ms: u64,
    ) -> Vec<(u8, String, [f32; SPECTRUM_BINS])> {
        let mut out = Vec::new();
        let n = self.read_active_into(my_name, now_ms, &mut out);
        out.truncate(n);
        out
    }

    /// [`read_active`] variant that reuses a caller-owned buffer.
    ///
    /// Returns the live feed count `n`. Entries beyond `n` are left intact so
    /// their `String` capacity is not dropped when relays go offline and come
    /// back (audio-thread FFT hop: no steady-state heap after warmup).
    /// Callers must use only `&out[..n]`.
    ///
    /// Spectrum bins are fixed `[f32; SPECTRUM_BINS]` (no per-hop `Vec` growth).
    /// Prefer pre-filling `out` with `MAX_SLOTS` slots (`String::with_capacity(MAX_NAME_LEN)`)
    /// so the first hop never `push`es either.
    ///
    /// # Panics
    ///
    /// Never panics in practice: publisher slot indices are bounded by `MAX_SLOTS`,
    /// which fits in `u8`.
    pub fn read_active_into(
        &self,
        my_name: &str,
        now_ms: u64,
        out: &mut Vec<(u8, String, [f32; SPECTRUM_BINS])>,
    ) -> usize {
        let mut n = 0;
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };

            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
                continue;
            }

            for _ in 0..16 {
                let seq1 = s.seq.load(Ordering::Acquire);
                if seq1 & 1 != 0 {
                    continue;
                }
                if s.active.load(Ordering::Acquire) == 0 {
                    break;
                }

                let mut name_buf = [0u8; MAX_NAME_LEN];
                let mut target_buf = [0u8; MAX_NAME_LEN];
                let mut bins = [0.0f32; SPECTRUM_BINS];
                let (name_len, target_len) = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.bins.get().cast::<f32>(),
                        bins.as_mut_ptr(),
                        SPECTRUM_BINS,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.name.get().cast::<u8>(),
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.target.get().cast::<u8>(),
                        target_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (
                        (*s.name_len.get() as usize).min(MAX_NAME_LEN),
                        (*s.target_len.get() as usize).min(MAX_NAME_LEN),
                    )
                };

                fence(Ordering::Acquire);
                let seq2 = s.seq.load(Ordering::Acquire);
                if seq1 == seq2 {
                    let target = std::str::from_utf8(&target_buf[..target_len]).unwrap_or("");
                    if target.is_empty() || target == my_name {
                        let name = std::str::from_utf8(&name_buf[..name_len]).unwrap_or("");
                        let slot = u8::try_from(idx).expect("idx < MAX_SLOTS <= u8::MAX");
                        if let Some(entry) = out.get_mut(n) {
                            entry.0 = slot;
                            entry.1.clear();
                            entry.1.push_str(name);
                            entry.2 = bins;
                        } else {
                            let mut s = String::with_capacity(MAX_NAME_LEN.max(name.len()));
                            s.push_str(name);
                            out.push((slot, s, bins));
                        }
                        n += 1;
                    }
                    break;
                }
            }
        }
        n
    }

    /// Diagnostic dump of all publisher slots.
    ///
    /// # Panics
    /// Never in practice — the internal integer conversions use compile-time
    /// constants (`MAX_SLOTS`, `STALE_MS`) that always fit their target types.
    #[must_use]
    pub fn diagnose_publishers(
        &self,
        my_name: &str,
        now_ms: u64,
    ) -> Vec<(u8, bool, i64, String, String, bool)> {
        let mut out = Vec::with_capacity(MAX_SLOTS);
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };
            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            let age = if hb == 0 {
                i64::MAX
            } else {
                #[allow(clippy::cast_possible_wrap)]
                let v = now_ms.wrapping_sub(hb) as i64;
                v
            };
            let stale = hb == 0 || age > i64::try_from(STALE_MS).expect("STALE_MS fits i64");

            let mut label = String::new();
            let mut target = String::new();
            let mut matches = false;
            let mut raw_active = false;

            if !stale {
                for _ in 0..16 {
                    let seq1 = s.seq.load(Ordering::Acquire);
                    if seq1 & 1 != 0 {
                        continue;
                    }
                    if s.active.load(Ordering::Acquire) == 0 {
                        break;
                    }
                    raw_active = true;
                    let mut name_buf = [0u8; MAX_NAME_LEN];
                    let mut target_buf = [0u8; MAX_NAME_LEN];
                    let (name_len, target_len) = unsafe {
                        std::ptr::copy_nonoverlapping(
                            s.name.get().cast::<u8>(),
                            name_buf.as_mut_ptr(),
                            MAX_NAME_LEN,
                        );
                        std::ptr::copy_nonoverlapping(
                            s.target.get().cast::<u8>(),
                            target_buf.as_mut_ptr(),
                            MAX_NAME_LEN,
                        );
                        (
                            (*s.name_len.get() as usize).min(MAX_NAME_LEN),
                            (*s.target_len.get() as usize).min(MAX_NAME_LEN),
                        )
                    };
                    fence(Ordering::Acquire);
                    if seq1 == s.seq.load(Ordering::Acquire) {
                        label = String::from_utf8_lossy(&name_buf[..name_len]).into_owned();
                        target = String::from_utf8_lossy(&target_buf[..target_len]).into_owned();
                        matches = target.is_empty() || target == my_name;
                        break;
                    }
                }
            }

            out.push((
                u8::try_from(idx).expect("idx < MAX_SLOTS <= u8::MAX"),
                raw_active,
                age,
                label,
                target,
                matches,
            ));
        }
        out
    }

    /// Read band energy levels from a specific publisher slot.
    ///
    /// Reads the per-band energy (dB) array from the publisher slot. Typical usage
    /// is to get dynamic-EQ trigger levels from a linked publisher.
    ///
    /// Audio-thread safe: no allocation, lock-free.
    ///
    /// # Arguments
    ///
    /// * `slot` - The publisher slot index
    /// * `now_ms` - Current wall-clock time (stale check)
    ///
    /// # Returns
    ///
    /// - `Some([f32; EQ_BANDS])` if the slot is live and readable
    /// - `None` if the slot is stale, invalid, or seqlock retries exhausted
    ///
    /// # Band layout
    ///
    /// The returned array typically contains: [Low Shelf, Peak 1, Peak 2, Peak 3, High Shelf]
    #[must_use]
    pub fn read_band_energy(&self, slot: u8, now_ms: u64) -> Option<[f32; EQ_BANDS]> {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return None;
        }
        let s = unsafe { &(*self.shared).slots[idx] };

        let hb = s.heartbeat_ms.load(Ordering::Acquire);
        if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
            return None;
        }

        for _ in 0..16 {
            let seq1 = s.seq.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                continue;
            }
            if s.active.load(Ordering::Acquire) == 0 {
                return None;
            }
            let mut energy = [0.0f32; EQ_BANDS];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    s.band_energy.get().cast::<f32>(),
                    energy.as_mut_ptr(),
                    EQ_BANDS,
                );
            }
            fence(Ordering::Acquire);
            let seq2 = s.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                return Some(energy);
            }
        }
        None
    }

    /// Find a publisher by name and read its band energy.
    ///
    /// Scans all publisher slots for one matching the given name, then reads its
    /// band energy array. Convenience method combining name lookup + energy read.
    ///
    /// Audio-thread safe: no allocation, lock-free.
    ///
    /// # Arguments
    ///
    /// * `name` - Publisher name to search for (matched against slot label)
    /// * `now_ms` - Current wall-clock time (stale check)
    ///
    /// # Returns
    ///
    /// - `Some((slot_index, band_energy))` if a live matching publisher is found
    /// - `None` if no live publisher matches the name
    ///
    /// # Panics
    ///
    /// Never panics in practice: the slot index is bounded by `MAX_SLOTS`, which
    /// fits in `u8`.
    #[must_use]
    pub fn find_band_energy(&self, name: &str, now_ms: u64) -> Option<(u8, [f32; EQ_BANDS])> {
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };

            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
                continue;
            }

            for _ in 0..16 {
                let seq1 = s.seq.load(Ordering::Acquire);
                if seq1 & 1 != 0 {
                    continue;
                }
                if s.active.load(Ordering::Acquire) == 0 {
                    break;
                }
                let mut name_buf = [0u8; MAX_NAME_LEN];
                let name_len = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.name.get().cast::<u8>(),
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (*s.name_len.get() as usize).min(MAX_NAME_LEN)
                };
                fence(Ordering::Acquire);
                if seq1 == s.seq.load(Ordering::Acquire) {
                    let slot_name = String::from_utf8_lossy(&name_buf[..name_len]);
                    if slot_name == name {
                        let slot = u8::try_from(idx).expect("idx < MAX_SLOTS <= u8::MAX");
                        return self.read_band_energy(slot, now_ms).map(|e| (slot, e));
                    }
                }
                break;
            }
        }
        None
    }
}

impl CvHub {
    /// Publish CV values and metadata to this publisher slot.
    ///
    /// # Arguments
    ///
    /// * `slot` - The publisher slot index (from [`claim_slot()`])
    /// * `generation` - The generation returned alongside `slot` by [`claim_slot()`]
    /// * `label` - A short name for this publisher (max `MAX_NAME_LEN` bytes)
    /// * `target` - Target consumer name (empty = broadcast)
    /// * `values` - CV channel values (`[f32; CV_CHANNELS]`)
    /// * `now_ms` - Current wall-clock time (updates heartbeat)
    ///
    /// # Returns
    ///
    /// `true` if the write happened, `false` if the slot is invalid or the
    /// generation no longer matches.
    #[must_use]
    pub fn write_cv(
        &self,
        slot: u8,
        generation: u32,
        label: &str,
        target: &str,
        values: &[f32; CV_CHANNELS],
        now_ms: u64,
    ) -> bool {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return false;
        }
        let s = unsafe { &(*self.shared).slots[idx] };
        if s.generation.load(Ordering::Acquire) != generation {
            return false;
        }

        let seq0 = seqlock_begin(&s.seq);

        unsafe {
            *s.name_len.get() = write_name_bytes(s.name.get().cast::<u8>(), label);
            *s.target_len.get() = write_name_bytes(s.target.get().cast::<u8>(), target);
            std::ptr::copy_nonoverlapping(
                values.as_ptr(),
                s.values.get().cast::<f32>(),
                CV_CHANNELS,
            );
        }

        s.active.store(1, Ordering::Release);

        seqlock_end(&s.seq, seq0);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
        true
    }

    /// Read CV values from all live publishers targeting this consumer.
    #[must_use]
    pub fn read_active_cv(
        &self,
        my_name: &str,
        now_ms: u64,
    ) -> Vec<(u8, String, [f32; CV_CHANNELS])> {
        self.read_active_cv_with_target(my_name, now_ms)
            .into_iter()
            .map(|(slot, name, _target, values)| (slot, name, values))
            .collect()
    }

    /// Read CV values from all live publishers targeting this consumer,
    /// including each publisher's `target` name.
    ///
    /// Returns `(slot_index, publisher_name, target_name, values)` for every
    /// live publisher whose target is empty (broadcast) or matches `my_name`.
    /// The returned `target_name` is empty for broadcasts.
    ///
    /// # Panics
    /// Never in practice — `idx < MAX_SLOTS` (16) always fits `u8`.
    #[must_use]
    pub fn read_active_cv_with_target(
        &self,
        my_name: &str,
        now_ms: u64,
    ) -> Vec<(u8, String, String, [f32; CV_CHANNELS])> {
        let mut out = Vec::new();
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };

            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
                continue;
            }

            for _ in 0..16 {
                let seq1 = s.seq.load(Ordering::Acquire);
                if seq1 & 1 != 0 {
                    continue;
                }
                if s.active.load(Ordering::Acquire) == 0 {
                    break;
                }

                let mut name_buf = [0u8; MAX_NAME_LEN];
                let mut target_buf = [0u8; MAX_NAME_LEN];
                let mut values = [0.0f32; CV_CHANNELS];
                let (name_len, target_len) = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.values.get().cast::<f32>(),
                        values.as_mut_ptr(),
                        CV_CHANNELS,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.name.get().cast::<u8>(),
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.target.get().cast::<u8>(),
                        target_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (
                        (*s.name_len.get() as usize).min(MAX_NAME_LEN),
                        (*s.target_len.get() as usize).min(MAX_NAME_LEN),
                    )
                };

                fence(Ordering::Acquire);
                let seq2 = s.seq.load(Ordering::Acquire);
                if seq1 == seq2 {
                    let target = std::str::from_utf8(&target_buf[..target_len]).unwrap_or("");
                    if target.is_empty() || target == my_name {
                        let name = String::from_utf8_lossy(&name_buf[..name_len]);
                        let slot = u8::try_from(idx).expect("idx < MAX_SLOTS <= u8::MAX");
                        out.push((slot, name.into_owned(), target.to_string(), values));
                    }
                    break;
                }
            }
        }
        out
    }

    /// Find a CV publisher by name and read its values.
    ///
    /// # Panics
    /// Never in practice — `idx < MAX_SLOTS` (16) always fits `u8`.
    #[must_use]
    pub fn find_cv(&self, name: &str, now_ms: u64) -> Option<(u8, [f32; CV_CHANNELS])> {
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };

            let hb = s.heartbeat_ms.load(Ordering::Acquire);
            if hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS {
                continue;
            }

            for _ in 0..16 {
                let seq1 = s.seq.load(Ordering::Acquire);
                if seq1 & 1 != 0 {
                    continue;
                }
                if s.active.load(Ordering::Acquire) == 0 {
                    break;
                }

                let mut name_buf = [0u8; MAX_NAME_LEN];
                let mut values = [0.0f32; CV_CHANNELS];
                let name_len = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.values.get().cast::<f32>(),
                        values.as_mut_ptr(),
                        CV_CHANNELS,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.name.get().cast::<u8>(),
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (*s.name_len.get() as usize).min(MAX_NAME_LEN)
                };

                fence(Ordering::Acquire);
                if seq1 == s.seq.load(Ordering::Acquire) {
                    let slot_name = String::from_utf8_lossy(&name_buf[..name_len]);
                    if slot_name == name {
                        let slot = u8::try_from(idx).expect("idx < MAX_SLOTS <= u8::MAX");
                        return Some((slot, values));
                    }
                }
                break;
            }
        }
        None
    }
}

/// Create or open the OS segment, retrying through the Windows leftover-file
/// and lost-create-race cases.
///
/// `shared_memory` on Win32 `create_new`s a file under `%TEMP%\shared_memory-rs`.
/// If `CreateFileMapping` then fails, that file is left behind; the next
/// `create()` returns `MappingIdExists` and `open()` can also fail (AV lock,
/// `ERROR_ALREADY_EXISTS` treated as error). `yield_now` × 10k is not enough
/// on GitHub `windows-latest` — `cv_hub_isolation` then saw `relay_hub() == None`.
fn map_segment<S: Slot>(size: usize) -> Option<(Shmem, bool)> {
    const ATTEMPTS: u32 = 50;
    const PAUSE: Duration = Duration::from_millis(2);

    for attempt in 0..ATTEMPTS {
        if let Ok(m) = ShmemConf::new().os_id(S::OS_ID).size(size).create() {
            return Some((m, true));
        }
        if let Ok(m) = ShmemConf::new().os_id(S::OS_ID).size(size).open() {
            return Some((m, false));
        }
        // Only when *both* failed: leftover backing file from a previous
        // process that created the file then died before mapping it.
        #[cfg(windows)]
        {
            let mut path = std::env::temp_dir();
            path.push("shared_memory-rs");
            path.push(S::OS_ID.trim_start_matches('/'));
            let _ = std::fs::remove_file(&path);
        }
        if attempt + 1 < ATTEMPTS {
            std::thread::sleep(PAUSE);
        }
    }
    None
}

/// Get the process-global relay hub.
///
/// Returns a reference to the shared-memory hub, creating it if this is the
/// first call. The hub is initialized once and never dropped.
#[must_use]
pub fn relay_hub() -> Option<&'static RelayHub> {
    static HUB: OnceLock<RelayHub> = OnceLock::new();
    once_hub(&HUB)
}

/// Get the process-global CV hub.
///
/// Returns a reference to the CV shared-memory hub, creating it if this is the
/// first call. The hub is initialized once and never dropped.
#[must_use]
pub fn cv_hub() -> Option<&'static CvHub> {
    static HUB: OnceLock<CvHub> = OnceLock::new();
    once_hub(&HUB)
}

/// Cache only a live hub. A transient `open_or_create` failure must not poison
/// every later caller in this process (`OnceLock<Option<_>>` stored `None`).
fn once_hub<S: Slot>(lock: &'static OnceLock<Hub<S>>) -> Option<&'static Hub<S>> {
    if let Some(h) = lock.get() {
        return Some(h);
    }
    let hub = Hub::<S>::open_or_create()?;
    Some(lock.get_or_init(|| hub))
}

/// Resolve which consumer name a relay publisher should target.
#[must_use]
pub fn resolve_relay_target(hub: &RelayHub, selected: &str, now_ms: u64) -> Option<String> {
    if selected.is_empty() {
        return Some(String::new());
    }
    resolve_from_consumers(selected, &hub.read_consumers(now_ms))
}

/// Pure [`resolve_relay_target`] against an already-fetched consumer list.
#[must_use]
pub fn resolve_from_consumers(selected: &str, consumers: &[String]) -> Option<String> {
    if selected.is_empty() {
        return Some(String::new());
    }
    if consumers.iter().any(|c| c == selected) {
        return Some(selected.to_string());
    }
    if consumers.len() == 1 {
        return Some(consumers[0].clone());
    }
    // ponytail: broadcast beats silent drop when the saved target is stale
    Some(String::new())
}

#[cfg(test)]
mod resolve_target_tests {
    use super::*;

    // NOTE: no hub-based "stale target broadcasts" test here — the hub is
    // process-global shared memory, so a parallel test (or a running plugin)
    // with a live consumer heartbeat makes "no consumers" racy. The resolve
    // logic is covered deterministically by `resolve_from_consumers_pure_cases`.

    #[test]
    fn resolve_from_consumers_pure_cases() {
        let none: Vec<String> = Vec::new();
        assert_eq!(resolve_from_consumers("", &none).as_deref(), Some(""));
        // stale target, no consumers → broadcast (publish must not silently stop)
        assert_eq!(resolve_from_consumers("Ghost", &none).as_deref(), Some(""));

        let one = vec!["Lucent 1".to_string()];
        assert_eq!(
            resolve_from_consumers("Lucent 1", &one).as_deref(),
            Some("Lucent 1")
        );
        // stale target, exactly one consumer → auto-target it
        assert_eq!(
            resolve_from_consumers("Ghost", &one).as_deref(),
            Some("Lucent 1")
        );

        let two = vec!["A".to_string(), "B".to_string()];
        assert_eq!(resolve_from_consumers("B", &two).as_deref(), Some("B"));
        // stale target, multiple consumers → broadcast
        assert_eq!(resolve_from_consumers("Ghost", &two).as_deref(), Some(""));
    }

    #[test]
    fn consumer_heartbeat_is_discoverable() {
        let hub = RelayHub::open_or_create().expect("hub");
        let now = now_ms();
        assert!(
            now > 0,
            "now_ms() returned 0 — heartbeats would be invisible"
        );
        let slot = hub.claim_consumer_slot(now).expect("free consumer slot");
        hub.write_consumer_name(slot, "Test Lucent", now);
        assert!(
            hub.consumer_exists("Test Lucent", now),
            "consumer_exists missed write to slot {slot}"
        );
        let found = hub.read_consumers(now);
        assert!(
            found.iter().any(|n| n == "Test Lucent"),
            "read_consumers missed registered consumer: {found:?}"
        );
        hub.release_consumer_slot(slot);
    }
}

#[cfg(test)]
mod cv_tests {
    use super::*;
    use std::sync::Mutex;

    // CV tests all mutate the same process-global segment; serialize them so
    // generation counters and slot scans stay deterministic.
    static CV_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn release_all_cv_slots(hub: &CvHub) {
        for i in 0..u8::try_from(MAX_SLOTS).expect("MAX_SLOTS <= u8::MAX") {
            hub.release_slot(i);
        }
    }

    // Exact f32 compares: values round-trip byte-identical through the shm ring.
    #[allow(clippy::float_cmp)]
    #[test]
    fn cv_roundtrip() {
        let _guard = CV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let hub = cv_hub().expect("cv hub");
        let now = now_ms();
        release_all_cv_slots(hub);

        let (slot, generation) = hub.claim_slot(now).expect("claim cv slot");
        let mut values = [0.0f32; CV_CHANNELS];
        values[CV_LOCK] = 0.0;
        values[CV_GATE] = 0.1;
        values[CV_PITCH] = 0.2;
        values[CV_BUS_A] = 0.3;
        values[CV_BUS_B] = 0.4;
        values[CV_EOC] = 0.5;
        values[CV_ENV] = 0.6;
        values[CV_LFO] = 0.7;
        values[CV_RAND] = 0.8;
        assert!(
            hub.write_cv(slot, generation, "cv-pub", "cv-consumer", &values, now),
            "write_cv failed"
        );

        let (found_slot, found_values) = hub.find_cv("cv-pub", now).expect("find_cv");
        assert_eq!(found_slot, slot);
        assert_eq!(found_values, values);

        let active = hub.read_active_cv("cv-consumer", now);
        assert!(
            active
                .iter()
                .any(|(s, n, v)| *s == slot && n == "cv-pub" && *v == values),
            "read_active_cv missed publisher: {active:?}"
        );

        hub.release_slot(slot);
    }

    #[test]
    fn cv_hub_isolation() {
        let _guard = CV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cv = cv_hub().expect("cv hub");
        let relay = relay_hub().expect("relay hub");
        let now = now_ms();
        release_all_cv_slots(cv);

        let (slot, generation) = cv.claim_slot(now).expect("claim cv slot");
        let values = [0.5f32; CV_CHANNELS];
        assert!(cv.write_cv(slot, generation, "cv-only", "", &values, now));

        let relay_active = relay.read_active("", now);
        assert!(
            !relay_active.iter().any(|(_, name, _)| name == "cv-only"),
            "CV publisher leaked into relay hub: {relay_active:?}"
        );

        cv.release_slot(slot);
    }

    #[test]
    fn cv_stale_reclaim() {
        let _guard = CV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let hub = cv_hub().expect("cv hub");
        let now = now_ms();
        release_all_cv_slots(hub);

        let (slot, gen1) = hub.claim_slot(now).expect("claim cv slot");
        hub.release_slot(slot);
        let (slot2, gen2) = hub.claim_slot(now).expect("reclaim cv slot");

        assert_eq!(slot, slot2, "reclaim should return the same freed slot");
        assert!(
            gen2 > gen1,
            "generation must bump after stale reclaim: {gen1} -> {gen2}"
        );

        hub.release_slot(slot2);
    }
}
