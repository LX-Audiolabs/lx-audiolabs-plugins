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
//! Two registries live in the segment:
//!
//! **Publisher slots** — each producer claims one and writes its payload
//! plus a `target` name (which consumer to send to; empty = broadcast).
//!
//! **Consumer slots** — each consumer claims one and publishes its instance
//! `name`, so producers can list available targets.
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
//! Invalid slot indices (>= MAX_SLOTS or MAX_CONSUMERS) are silently ignored by
//! write/touch functions. `claim_*_slot()` returns `None` when all slots are full
//! or taken by stale instances. `read_*()` returns empty results when the hub
//! cannot be mapped. Seqlock reads retry up to 4 times if the writer interferes;
//! if all 4 retries fail, the read is dropped (partial data is not returned).

use std::cell::UnsafeCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
use std::time::{SystemTime, UNIX_EPOCH};

use shared_memory::{Shmem, ShmemConf};

/// Number of spectrum bins per payload frame.
pub const SPECTRUM_BINS: usize = 1024;
/// Maximum number of publisher slots.
pub const MAX_SLOTS: usize = 16;
/// Maximum number of consumer instances advertising a name.
pub const MAX_CONSUMERS: usize = 16;
/// Maximum label/name length in bytes (UTF-8).
pub const MAX_NAME_LEN: usize = 32;
/// A slot is considered dead if no heartbeat arrived within this window.
pub const STALE_MS: u64 = 500;

/// OS-global name for the segment. The `_vN` suffix is bumped whenever the
/// slot layout or claim protocol changes, so an old plugin (different layout)
/// maps a *separate* segment instead of colliding with the new one.
/// `_v2`: added the `claimed` flag for atomic auto-slot assignment.
/// `_v3`: publisher slots gained a `target` name; added the consumer-name registry.
/// `_v4`: publisher slots gained `band_energy` [f32; 5] for dynamic-EQ triggering.
/// `_v5`: publisher slots gained a `generation` counter — a stale-reclaimed
/// slot's original (evicted) owner can now detect it lost the slot and stop
/// writing, instead of racing the new owner forever (see `write`/`touch`).
/// `_v6`: `active` changed from `UnsafeCell<u32>` to `AtomicU32` and is now
/// written *after* payload data with `Release` ordering, fixing a cross-process
/// data race where readers saw `active=1` before the seqlock-protected payload
/// was actually visible.
/// Seqlock writers now heal abandoned odd `seq` values and reset `seq` on claim
/// so a crash mid-write cannot brick a slot permanently.
const SHM_OS_ID: &str = "lxaudiolabs_lucent_relay_v6";
/// "LXRD" — marks a fully-initialized segment.
const MAGIC: u32 = 0x4C58_5244;
const VERSION: u32 = 6;
/// Number of EQ bands for band-energy reporting.
pub const EQ_BANDS: usize = 5;

/// One publisher's data. `#[repr(C)]` so the byte layout is identical across DLLs.
#[repr(C)]
struct PublisherSlot {
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
unsafe impl Sync for PublisherSlot {}

/// One consumer instance advertising its name so publishers can target it.
#[repr(C)]
struct ConsumerSlot {
    seq: AtomicU32,
    claimed: AtomicU32,
    heartbeat_ms: AtomicU64,
    name_len: UnsafeCell<u32>,
    name: UnsafeCell<[u8; MAX_NAME_LEN]>,
}

// SAFETY: see PublisherSlot.
unsafe impl Sync for ConsumerSlot {}

#[repr(C)]
struct HubShared {
    magic: AtomicU32,
    version: AtomicU32,
    slots: [PublisherSlot; MAX_SLOTS],
    consumers: [ConsumerSlot; MAX_CONSUMERS],
}

// Compile-time layout guarantees so the segment is byte-compatible everywhere.
const _: () = {
    assert!(core::mem::align_of::<PublisherSlot>() == 8);
    assert!(core::mem::align_of::<ConsumerSlot>() == 8);
    assert!(
        core::mem::size_of::<HubShared>()
            == 8 + MAX_SLOTS * core::mem::size_of::<PublisherSlot>()
                + MAX_CONSUMERS * core::mem::size_of::<ConsumerSlot>()
    );
};

/// Get wall-clock time in milliseconds since UNIX_EPOCH.
///
/// Returns `SystemTime::now()` as milliseconds, consistent across all plugins
/// in the process. Used for heartbeat tracking and slot liveness checks.
///
/// # Returns
///
/// Milliseconds since UNIX_EPOCH. If system time is unavailable, returns 0
/// (a very old heartbeat that will be treated as stale).
///
/// # Example
///
/// ```ignore
/// let now_ms = lx_shm::now_ms();
/// hub.write(slot, "my-label", "target-name", &bins, &band_energy, now_ms);
/// ```
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Format a consumer instance's display name.
///
/// If the provided name is empty or whitespace, returns a default name like
/// "Hub 1", "Hub 2", etc. (slot+1). Otherwise returns the name as-is.
///
/// This allows unnamed consumer instances to still be discoverable and targetable
/// by producers (who read the display name from the consumer registry).
///
/// # Arguments
///
/// * `name` - The consumer's chosen name (may be empty or whitespace)
/// * `slot` - The consumer's slot index (0-based)
///
/// # Example
///
/// ```ignore
/// let name = lx_shm::display_name("My Analyzer", 0);  // "My Analyzer"
/// let name = lx_shm::display_name("", 0);              // "Hub 1"
/// ```
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
    len as u32
}

/// Handle to the cross-process shared memory hub.
///
/// Provides access to publisher and consumer slot registries for multi-instance
/// cross-DLL communication. The hub persists for the process lifetime and is
/// lazily initialized on first access via [`relay_hub()`].
///
/// This type is thread-safe and audio-thread safe (all reads are lock-free and
/// allocation-free via seqlock synchronization).
///
/// # Thread safety
///
/// - `Send + Sync`: safe to share across threads
/// - Read operations never block or allocate
/// - Write operations are atomic
/// - Seqlock retries handle concurrent writes (up to 4 retries)
///
/// # Usage
///
/// Get the hub via [`relay_hub()`], then use it to claim slots, publish data,
/// and read from other instances.
pub struct RelayHub {
    _shmem: Shmem,
    shared: *const HubShared,
}

// SAFETY: `shared` points into the shared mapping; all access is via atomics +
// seqlock-guarded raw pointers (see PublisherSlot). The mapping outlives the handle.
unsafe impl Send for RelayHub {}
unsafe impl Sync for RelayHub {}

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
        let _ = seq.compare_exchange(cur, cur.wrapping_add(1), Ordering::AcqRel, Ordering::Relaxed);
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
    if c == 1 {
        let hb = heartbeat.load(Ordering::Acquire);
        let stale = hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS;
        if stale {
            let _ = claimed.compare_exchange(1, 0, Ordering::AcqRel, Ordering::Relaxed);
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
/// publisher registry (`PublisherSlot` has a `generation` counter, consumer
/// slots don't need one since the multi-writer collision this guards against
/// is specific to publishers racing to reclaim a stale relay slot). Returns
/// the new generation on a won claim, `None` if the slot is live and held.
fn try_claim_gen(
    claimed: &AtomicU32,
    heartbeat: &AtomicU64,
    generation: &AtomicU32,
    now_ms: u64,
) -> Option<u32> {
    let mut c = claimed.load(Ordering::Acquire);
    if c == 1 {
        let hb = heartbeat.load(Ordering::Acquire);
        let stale = hb == 0 || now_ms.wrapping_sub(hb) > STALE_MS;
        if stale {
            let _ = claimed.compare_exchange(1, 0, Ordering::AcqRel, Ordering::Relaxed);
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

impl RelayHub {
    fn open_or_create() -> Option<RelayHub> {
        let size = core::mem::size_of::<HubShared>();

        let (shmem, is_creator) = match ShmemConf::new().os_id(SHM_OS_ID).size(size).create() {
            Ok(m) => (m, true),
            Err(_) => (ShmemConf::new().os_id(SHM_OS_ID).open().ok()?, false),
        };

        let shared = shmem.as_ptr() as *const HubShared;

        if is_creator {
            unsafe {
                (*shared).version.store(VERSION, Ordering::Release);
                (*shared).magic.store(MAGIC, Ordering::Release);
            }
        } else {
            let mut spins = 0u32;
            while unsafe { (*shared).magic.load(Ordering::Acquire) } != MAGIC {
                std::thread::yield_now();
                spins += 1;
                if spins > 1_000_000 {
                    return None;
                }
            }
        }

        Some(RelayHub {
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
    ///   0..MAX_SLOTS). Store both — `generation` must be passed to every
    ///   `write`/`touch` call so the hub can tell if this claim is still
    ///   valid (see `write`).
    /// - `None` if all slots are occupied by live instances
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some((slot, generation)) = hub.claim_slot(lx_shm::now_ms()) {
    ///     // Store slot + generation, use both to publish data
    /// }
    /// ```
    pub fn claim_slot(&self, now_ms: u64) -> Option<(u8, u32)> {
        for idx in 0..MAX_SLOTS {
            let s = unsafe { &(*self.shared).slots[idx] };
            if let Some(generation) =
                try_claim_gen(&s.claimed, &s.heartbeat_ms, &s.generation, now_ms)
            {
                s.seq.store(0, Ordering::Release);
                return Some((idx as u8, generation));
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
        s.heartbeat_ms.store(0, Ordering::Release);
        s.claimed.store(0, Ordering::Release);
    }

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
    ///   - Non-empty: only the consumer with matching display_name receives it
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
            *s.name_len.get() = write_name_bytes(s.name.get() as *mut u8, label);
            *s.target_len.get() = write_name_bytes(s.target.get() as *mut u8, target);

            let bins_ptr = s.bins.get() as *mut f32;
            let n = bins.len().min(SPECTRUM_BINS);
            std::ptr::copy_nonoverlapping(bins.as_ptr(), bins_ptr, n);
            for i in n..SPECTRUM_BINS {
                *bins_ptr.add(i) = -90.0;
            }

            let be_ptr = s.band_energy.get() as *mut f32;
            let m = band_energy.len().min(EQ_BANDS);
            std::ptr::copy_nonoverlapping(band_energy.as_ptr(), be_ptr, m);
            for i in m..EQ_BANDS {
                *be_ptr.add(i) = -90.0;
            }
        }

        // Mark payload ready *after* all payload writes are done. This is the
        // only ordering-sensitive flag read outside the seqlock fast-path.
        s.active.store(1, Ordering::Release);

        seqlock_end(&s.seq, seq0);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
        true
    }

    /// Update metadata and heartbeat WITHOUT writing spectrum data.
    ///
    /// Useful for keeping a publisher alive when audio is not actively being
    /// published (e.g., when transport is stopped). Updates label, target, and
    /// heartbeat but leaves bins and band_energy untouched, so consumers continue
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
            *s.name_len.get() = write_name_bytes(s.name.get() as *mut u8, label);
            *s.target_len.get() = write_name_bytes(s.target.get() as *mut u8, target);
        }

        s.active.store(1, Ordering::Release);

        seqlock_end(&s.seq, seq0);
        s.heartbeat_ms.store(now_ms, Ordering::Release);
        true
    }

    /// Read spectrum data from all publishers targeting this consumer.
    ///
    /// Returns a list of (publisher_slot, publisher_label, spectrum_bins) tuples
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
    pub fn read_active(&self, my_name: &str, now_ms: u64) -> Vec<(u8, String, Vec<f32>)> {
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
                let mut bins = vec![0.0f32; SPECTRUM_BINS];
                let (name_len, target_len) = unsafe {
                    std::ptr::copy_nonoverlapping(
                        s.bins.get() as *const f32,
                        bins.as_mut_ptr(),
                        SPECTRUM_BINS,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.name.get() as *const u8,
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    std::ptr::copy_nonoverlapping(
                        s.target.get() as *const u8,
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
                    let target = String::from_utf8_lossy(&target_buf[..target_len]);
                    if target.is_empty() || target == my_name {
                        let name = String::from_utf8_lossy(&name_buf[..name_len]).into_owned();
                        out.push((idx as u8, name, bins));
                    }
                    break;
                }
            }
        }
        out
    }

    /// Diagnostic dump of all publisher slots — which are active, their labels,
    /// targets, and whether they match `my_name`. Not audio-thread optimal
    /// (allocates strings per slot); use only for debugging routing issues.
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
                now_ms.wrapping_sub(hb) as i64
            };
            let stale = hb == 0 || age > STALE_MS as i64;

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
                            s.name.get() as *const u8,
                            name_buf.as_mut_ptr(),
                            MAX_NAME_LEN,
                        );
                        std::ptr::copy_nonoverlapping(
                            s.target.get() as *const u8,
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

            out.push((idx as u8, raw_active, age, label, target, matches));
        }
        out
    }

    /// Raw atomic snapshot of a slot: (claimed, generation, seq, heartbeat_ms).
    /// For debugging — no seqlock, just the admin fields.
    pub fn slot_raw_state(&self, slot: u8) -> Option<(u32, u32, u32, u64)> {
        let idx = slot as usize;
        if idx >= MAX_SLOTS {
            return None;
        }
        let s = unsafe { &(*self.shared).slots[idx] };
        Some((
            s.claimed.load(Ordering::Acquire),
            s.generation.load(Ordering::Acquire),
            s.seq.load(Ordering::Acquire),
            s.heartbeat_ms.load(Ordering::Acquire),
        ))
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
                    s.band_energy.get() as *const f32,
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
                        s.name.get() as *const u8,
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (*s.name_len.get() as usize).min(MAX_NAME_LEN)
                };
                fence(Ordering::Acquire);
                if seq1 == s.seq.load(Ordering::Acquire) {
                    let slot_name = String::from_utf8_lossy(&name_buf[..name_len]);
                    if slot_name == name {
                        return self
                            .read_band_energy(idx as u8, now_ms)
                            .map(|e| (idx as u8, e));
                    }
                }
                break;
            }
        }
        None
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
    /// - `Some(slot_index)` if a free slot was claimed (index 0..MAX_CONSUMERS)
    /// - `None` if all consumer slots are occupied
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(slot) = hub.claim_consumer_slot(lx_shm::now_ms()) {
    ///     hub.write_consumer_name(slot, "My Analyzer", lx_shm::now_ms());
    /// }
    /// ```
    pub fn claim_consumer_slot(&self, now_ms: u64) -> Option<u8> {
        for idx in 0..MAX_CONSUMERS {
            let s = unsafe { &(*self.shared).consumers[idx] };
            if try_claim(&s.claimed, &s.heartbeat_ms, now_ms) {
                s.seq.store(0, Ordering::Release);
                return Some(idx as u8);
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

        let seq0 = seqlock_begin(&s.seq);
        unsafe {
            *s.name_len.get() = write_name_bytes(s.name.get() as *mut u8, name);
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
                    s.name.get() as *const u8,
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
    ///
    /// Audio-thread safe: no allocation, lock-free.
    ///
    /// # Arguments
    ///
    /// * `name` - The consumer name to search for (exact match)
    /// * `now_ms` - Current wall-clock time (stale check)
    ///
    /// # Returns
    ///
    /// `true` if a live consumer instance is advertising exactly this name,
    /// `false` otherwise.
    pub fn consumer_exists(&self, name: &str, now_ms: u64) -> bool {
        let mut buf = [0u8; MAX_NAME_LEN];
        for idx in 0..MAX_CONSUMERS {
            if let Some(n) = self.read_consumer_slot(idx, now_ms, &mut buf) {
                if &buf[..n] == name.as_bytes() {
                    return true;
                }
            }
        }
        false
    }

    /// Get the name of the single live consumer, if exactly one exists.
    ///
    /// Useful for auto-targeting: if only one consumer is connected, target it
    /// automatically without user interaction.
    ///
    /// Audio-thread safe: no allocation, lock-free.
    ///
    /// # Arguments
    ///
    /// * `now_ms` - Current wall-clock time (stale check)
    /// * `out` - Output buffer (must be at least `MAX_NAME_LEN` bytes)
    ///
    /// # Returns
    ///
    /// - `Some(name_length)` if exactly one live consumer exists
    ///   (the name is written to `out[..name_length]`)
    /// - `None` if zero or multiple consumers are live
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut name_buf = [0u8; lx_shm::MAX_NAME_LEN];
    /// if let Some(len) = hub.single_consumer_name(lx_shm::now_ms(), &mut name_buf) {
    ///     let name = std::str::from_utf8(&name_buf[..len]).unwrap_or("invalid");
    ///     // Auto-target to the single consumer
    /// }
    /// ```
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
    pub fn read_consumers(&self, now_ms: u64) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
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
                        s.name.get() as *const u8,
                        name_buf.as_mut_ptr(),
                        MAX_NAME_LEN,
                    );
                    (*s.name_len.get() as usize).min(MAX_NAME_LEN)
                };
                fence(Ordering::Acquire);
                let seq2 = s.seq.load(Ordering::Acquire);
                if seq1 == seq2 {
                    let name = String::from_utf8_lossy(&name_buf[..name_len]).into_owned();
                    if !name.is_empty() && !out.contains(&name) {
                        out.push(name);
                    }
                    break;
                }
            }
        }
        out
    }
}

/// Get the process-global hub.
///
/// Returns a reference to the shared-memory hub, creating it if this is the
/// first call. The hub is initialized once and never dropped (held in a static
/// `OnceLock` for the process lifetime).
///
/// # Returns
///
/// - `Some(&RelayHub)` if the hub was successfully created or opened
/// - `None` if the shared-memory segment could not be mapped (rare; usually
///   indicates a system error or resource exhaustion). Callers should treat
///   `None` as "no publishers available" and fall back gracefully.
///
/// # Example
///
/// ```ignore
/// if let Some(hub) = lx_shm::relay_hub() {
///     if let Some(slot) = hub.claim_slot(lx_shm::now_ms()) {
///         // Use the slot
///     }
/// }
/// ```
pub fn relay_hub() -> Option<&'static RelayHub> {
    static HUB: OnceLock<Option<RelayHub>> = OnceLock::new();
    HUB.get_or_init(RelayHub::open_or_create).as_ref()
}

/// Resolve which consumer name a relay publisher should target.
///
/// - Empty `selected` → broadcast (`Some("")`).
/// - Exact live consumer match → `Some(selected)`.
/// - Exactly one live consumer (stale/wrong target) → auto-target it.
/// - Stale target with multiple live consumers → broadcast (`Some("")`) so
///   publish does not silently stop when a persisted name no longer exists.
pub fn resolve_relay_target(hub: &RelayHub, selected: &str, now_ms: u64) -> Option<String> {
    if selected.is_empty() {
        return Some(String::new());
    }
    if hub.consumer_exists(selected, now_ms) {
        return Some(selected.to_string());
    }
    let lucents = hub.read_consumers(now_ms);
    if lucents.len() == 1 {
        return Some(lucents[0].clone());
    }
    // ponytail: broadcast beats silent drop when the saved target is stale
    Some(String::new())
}

#[cfg(test)]
mod resolve_target_tests {
    use super::*;

    #[test]
    fn stale_target_with_no_consumers_broadcasts() {
        let hub = RelayHub::open_or_create().expect("hub");
        let now = now_ms();
        assert_eq!(
            resolve_relay_target(&hub, "Hub 1", now).as_deref(),
            Some("")
        );
    }

    #[test]
    fn consumer_heartbeat_is_discoverable() {
        let hub = RelayHub::open_or_create().expect("hub");
        let now = now_ms();
        assert!(now > 0, "now_ms() returned 0 — heartbeats would be invisible");
        let slot = hub
            .claim_consumer_slot(now)
            .expect("free consumer slot");
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


