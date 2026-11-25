//! Title memory by address, for translated shaders that read and write
//! through raw 40-bit addresses (`victory_shader::emit::SLOT_MEMORY`: the
//! console's ray-tracing structure builder and the programs that walk its
//! structures; the structures themselves lie in plain title memory, in no
//! buffer object).
//!
//! The copies are the ones every other access uses ([`crate::shadow`]: a
//! title buffer's copy, or the 4 MiB window of memory outside buffers), so
//! a root view and a raw address reach the same host bytes. This module
//! keeps what a shader needs to find them:
//!
//! - the **directory**: per 64 KiB page of title address space the number
//!   of the copy that holds the page (0: none) and the page's byte offset
//!   in it; a page whose owner changes (the title released or registered
//!   a buffer there) gives later binds a new directory, while lists
//!   recorded with the old one keep it (after DexNom's journal);
//! - per view heap a run of slots with a raw unordered access view of each
//!   numbered copy (slot 0 of the run: a null view, for pages without a
//!   copy);
//! - the **faults**: the page numbers shaders found empty, read back here
//!   once a frame, so the copies exist from the next frame on. A pointer a
//!   shader takes out of memory cannot be known before it runs; the
//!   addresses a dispatch is given as root views are made ready before it.
//!
//! **A page that is not there when a shader touches it is a lost access**:
//! the read gives 0, the store is dropped, and the fault note only helps
//! the next frame. For the structure builder that is the same as a dropped
//! stage, so before a dispatch the host makes ready everything it can see
//! the dispatch name: the whole copy around each root view, around each
//! pair of root constants that is a title address and around each root
//! constant that is one in 256-byte units (the builder passes structures
//! all three ways), and the 4 MiB after each (a structure may cross a
//! window's end).
//! shortcut: what a stage reaches only through pointers it reads out of
//! memory (InitHeaders' table of structure addresses, LoadInstances'
//! instance records) is not made ready unless something else named it;
//! the fault count in the log says whether that happens. The whole answer
//! is the extent of the title's allocation around an address, which the
//! memory module of the layer knows and this one does not ask yet.
//!
//! shortcut: a copy that is numbered is kept for the rest of the run. Each
//! view heap's run of slots grows (doubling, from [`FIRST_RUN`]) as more
//! copies are numbered; older runs stay valid for lists recorded with them,
//! and a shader only uses the views its bind prepared ([`Ready::count`];
//! after DexNom 921d8d3). Every dispatch of such a shader brings all numbered copies up to
//! date and, if it can store, declares all of them written (the bytes are
//! compared on the CPU afterwards, as for any unordered access view): a
//! dirty-page mask written by the shader would bound both.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use victory_shader::emit::{MEMORY_FAULTS, MEMORY_PAGES};
use windows::Win32::Graphics::Direct3D12::*;

use crate::descriptor::host::HostHeap;
use crate::host::{HostList, host};
use crate::shadow::Shadow;

/// Slots of a heap's first run (its null view and 1023 copies). A listed
/// frame of the opening cutscene names about 50 windows; with the
/// ray-tracing builder on, 1023 are used up in the first second.
const FIRST_RUN: u32 = 1024;
/// A window of memory outside buffers (`shadow.rs`).
const CELL: u64 = 4 << 20;
/// A directory page.
const PAGE: u64 = 1 << 16;

// A title may register or release buffers while another thread records
// GPU work: the changed page spans, newest last. A reader that fell more
// than the journal behind rechecks all its pages. Nothing is journalled
// until a shader has used the directory.
static CHANGES: OnceLock<Mutex<VecDeque<(u64, u64)>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Title memory `[start, start + len)` may now belong to another buffer.
pub(crate) fn ownership_changed(start: u64, len: u64) {
    if let Some(changes) = CHANGES.get() {
        let mut changes = changes.lock().unwrap_or_else(|e| e.into_inner());
        if changes.len() == 256 {
            changes.pop_front();
        }
        changes.push_back((start / PAGE, start.saturating_add(len).div_ceil(PAGE)));
        GENERATION.fetch_add(1, Ordering::Release);
    }
}

struct Memory {
    /// Buffer, its mapping, its GPU address.
    directory: (ID3D12Resource, *mut u32, u64),
    faults: (ID3D12Resource, *mut u32, u64),
    /// Copy `n` is `copies[n - 1]`.
    copies: Vec<Arc<Shadow>>,
    /// Per host view heap (by address): the first slot of its current run,
    /// the run's length, and how many copies have their view there.
    heaps: HashMap<usize, (u32, u32, u32)>,
    /// Copy numbers by shadow id.
    numbers: HashMap<u64, usize>,
    /// Mapped directory pages and the copy each has.
    pages: BTreeMap<u64, usize>,
    /// Per copy: how many directory pages it holds now. A copy with none
    /// (its buffer released, the pages another's) is neither brought up to
    /// date nor copied back: its bytes would land on the new owner's.
    owned: Vec<u32>,
    /// Per copy: the runs of title memory its directory pages cover, all
    /// that a shader can reach through it (refreshed and copied back).
    ranges: Vec<Vec<(u64, u64)>>,
    /// Copies whose runs need working out again.
    recalculate: BTreeSet<usize>,
    /// What is naming memory now ([`Source`]) and, per source, how many
    /// copies it got numbered (diagnostics in the log).
    source: usize,
    numbered_by: [u64; 5],
    /// Pages a shader marked as stored to (`memory_marks`), for the run.
    written: BTreeSet<u64>,
    /// Per copy: the runs of its pages in `written`, all that is copied
    /// back after a storing shader.
    written_ranges: Vec<Vec<(u64, u64)>>,
    /// The journal generation the directory agrees with.
    generation: u64,
    /// Bumped with each new directory (lists keep each one they bind).
    revision: u64,
    /// The frame the faults were last read in.
    frame: u64,
    /// Pages asked for this frame that are not the title's memory.
    refused: std::collections::HashSet<u64>,
}

// SAFETY: the mappings are of buffers the struct owns; all access is under
// the mutex.
unsafe impl Send for Memory {}

static MEMORY: Mutex<Option<Memory>> = Mutex::new(None);

/// What a draw binds.
pub(crate) struct Ready {
    pub(crate) directory: u64,
    pub(crate) faults: u64,
    /// The view heap slot of the null view; copy `n`'s view is `n` further.
    pub(crate) base: u32,
    /// Copies with a view in that run at this bind: other recording threads
    /// can number more (the directory is shared), which this bind's shader
    /// must treat as missing.
    pub(crate) count: u32,
}

impl Memory {
    fn new() -> Option<Memory> {
        let host = host()?;
        let make = |dwords: u64, what: &str| {
            let (buffer, data) = host.shared_buffer(dwords * 4)?;
            crate::host::name(&buffer, || format!("title memory by address: {what}"));
            // SAFETY: a fresh mapping of at least that many bytes.
            unsafe { std::ptr::write_bytes(data, 0, (dwords * 4) as usize) };
            // SAFETY: a live buffer.
            let gpu = unsafe { buffer.GetGPUVirtualAddress() };
            Some((buffer, data.cast::<u32>(), gpu))
        };
        tracing::info!("title memory by address: a shader reads or writes through raw addresses");
        CHANGES.get_or_init(Default::default);
        Some(Memory {
            directory: make(u64::from(MEMORY_PAGES) * 2, "directory")?,
            faults: make(fault_dwords(), "faults")?,
            copies: Vec::new(),
            heaps: HashMap::new(),
            numbers: HashMap::new(),
            pages: BTreeMap::new(),
            owned: Vec::new(),
            ranges: Vec::new(),
            recalculate: BTreeSet::new(),
            written: BTreeSet::new(),
            written_ranges: Vec::new(),
            source: 0,
            numbered_by: [0; 5],
            generation: GENERATION.load(Ordering::Acquire),
            revision: 0,
            frame: u64::MAX,
            refused: Default::default(),
        })
    }

    /// The copy holding all of the page at `start`, if it is the title's
    /// memory and shaders can write it, and the page's byte in that copy.
    fn page_owner(start: u64) -> Option<(Arc<Shadow>, u32)> {
        if !crate::texture::committed(start as *const u8, PAGE as usize) {
            return None;
        }
        // The copy a root view at that address would get; a buffer that
        // ends inside the page is passed over for what holds all of it.
        let whole = |s: &Arc<Shadow>| s.offset_in(start).is_some_and(|_| s.left(start) >= PAGE);
        let shadow = crate::shadow::find(start, 1)
            .filter(whole)
            .or_else(|| crate::shadow::find(start, PAGE).filter(whole))?;
        // SAFETY: a live buffer.
        let flags = unsafe { shadow.buffer.GetDesc() }.Flags;
        if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 {
            return None;
        }
        // The page's byte from the resource's byte 0 (a copy in place can
        // begin before the title's allocation), where the view starts.
        shadow.offset_in(start)?;
        let offset = u32::try_from(shadow.offset(start)).ok()?;
        Some((shadow, offset))
    }

    /// The copy's number, given one on first sight; kept for the run, as a
    /// recorded list may still use it after the title reuses the address.
    fn number(&mut self, shadow: &Arc<Shadow>) -> Option<usize> {
        if let Some(&n) = self.numbers.get(&shadow.id()) {
            return Some(n);
        }
        // The directory holds a one-based u32 number.
        if self.copies.len() >= u32::MAX as usize - 1 {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::warn!("title memory by address: no number left for another copy; its pages read zeros"));
            return None;
        }
        self.numbers.insert(shadow.id(), self.copies.len());
        self.copies.push(shadow.clone());
        self.numbered_by[self.source] += 1;
        if self.copies.len() % 256 == 0 {
            let [view, pair, unit, fault, inside] = self.numbered_by;
            tracing::info!(
                copies = self.copies.len(),
                view, pair, unit, fault, inside,
                "title memory by address: copies numbered by source (root view, address in two constants, constant in 256-byte units, fault note, page of a copy numbered for another)"
            );
        }
        self.owned.push(0);
        self.ranges.push(Vec::new());
        self.written_ranges.push(Vec::new());
        Some(self.copies.len() - 1)
    }

    fn set_page(&mut self, page: u64, mapping: Option<(usize, u32)>) {
        let old = match mapping {
            Some((n, _)) => self.pages.insert(page, n),
            None => self.pages.remove(&page),
        };
        if let Some(old) = old {
            self.owned[old] -= 1;
            self.recalculate.insert(old);
        }
        let (number, offset) = match mapping {
            Some((n, offset)) => {
                self.owned[n] += 1;
                self.recalculate.insert(n);
                (n as u32 + 1, offset)
            }
            None => (0, 0),
        };
        // The offset first: a shader that sees the number sees it too.
        // SAFETY: inside the directory's mapping.
        unsafe {
            let entry = self.directory.1.add(page as usize * 2);
            entry.add(1).write_volatile(offset);
            entry.write_volatile(number);
        }
    }

    /// Pages whose owner changed since the directory was made get their
    /// new copy (or none) in a new directory: recorded lists read the old.
    fn reconcile(&mut self) -> Option<bool> {
        let now = GENERATION.load(Ordering::Acquire);
        if self.generation == now {
            return Some(false);
        }
        let mut check = BTreeSet::new();
        {
            let changes = CHANGES.get()?.lock().unwrap_or_else(|e| e.into_inner());
            let now = GENERATION.load(Ordering::Relaxed);
            let count = now - self.generation;
            if count > changes.len() as u64 {
                check.extend(self.pages.keys().copied());
            } else {
                for &(first, end) in changes.iter().rev().take(count as usize) {
                    check.extend(self.pages.range(first..end).map(|(&page, _)| page));
                }
            }
            self.generation = now;
        }
        let mut replaced = false;
        let mut moved = 0u64;
        for page in check {
            let old = self.pages[&page];
            let owner = Self::page_owner(page * PAGE);
            if owner.as_ref().is_some_and(|(s, _)| Arc::ptr_eq(s, &self.copies[old])) {
                continue;
            }
            if !replaced {
                let next = Self::table(u64::from(MEMORY_PAGES) * 2, "directory")?;
                // SAFETY: equal-sized tables; the GPU only reads directories.
                unsafe { std::ptr::copy_nonoverlapping(self.directory.1, next.1, MEMORY_PAGES as usize * 2) };
                self.directory = next;
                self.revision += 1;
                replaced = true;
            }
            let mapping = owner.and_then(|(s, offset)| self.number(&s).map(|n| (n, offset)));
            self.set_page(page, mapping);
            moved += 1;
        }
        if replaced {
            static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if SAID.fetch_add(1, Ordering::Relaxed) < 20 {
                tracing::info!(pages = moved, "title memory by address: pages changed owner; a new directory");
            }
        }
        self.refused.clear();
        Some(replaced)
    }

    fn update_ranges(&mut self) {
        fn add(ranges: &mut Vec<(u64, u64)>, at: u64, end: u64) {
            if let Some((first, bytes)) = ranges.last_mut()
                && *first + *bytes == at
            {
                *bytes = end - *first;
            } else {
                ranges.push((at, end - at));
            }
        }
        for index in std::mem::take(&mut self.recalculate) {
            let (start, len) = self.copies[index].span();
            let (ranges, written) = (&mut self.ranges[index], &mut self.written_ranges[index]);
            ranges.clear();
            written.clear();
            for (&page, &owner) in self.pages.range(start / PAGE..(start + len).div_ceil(PAGE)) {
                if owner != index {
                    continue;
                }
                // (the page's part inside the copy)
                let at = (page * PAGE).max(start);
                let end = ((page + 1) * PAGE).min(start + len);
                add(ranges, at, end);
                if self.written.contains(&page) {
                    add(written, at, end);
                }
            }
        }
    }

    fn table(dwords: u64, what: &str) -> Option<(ID3D12Resource, *mut u32, u64)> {
        let (buffer, data) = host()?.shared_buffer(dwords * 4)?;
        crate::host::name(&buffer, || format!("title memory by address: {what}"));
        // SAFETY: a fresh mapping of at least that many bytes.
        unsafe { std::ptr::write_bytes(data, 0, (dwords * 4) as usize) };
        // SAFETY: a live buffer.
        let gpu = unsafe { buffer.GetGPUVirtualAddress() };
        Some((buffer, data.cast::<u32>(), gpu))
    }

    /// Gives the page of `address` its copy, if it has none and the memory
    /// is the title's.
    fn map(&mut self, address: u64) {
        let page = address / PAGE;
        // The last entry stays empty: addresses past the directory read it.
        if page >= u64::from(MEMORY_PAGES) - 1 || self.pages.contains_key(&page) {
            return;
        }
        // A shader's pointer, or a constant taken for an address, may be
        // anything: only memory the title has.
        if self.refused.contains(&page) {
            return;
        }
        let Some((shadow, offset)) = Self::page_owner(page * PAGE) else {
            self.refused.insert(page);
            return;
        };
        let count = self.copies.len();
        let Some(n) = self.number(&shadow) else { return };
        self.set_page(page, Some((n, offset)));
        if n < count {
            return;
        }
        // A new copy: every page it holds whole, not only the one asked
        // for (a structure is more than 64 KiB); a page of a title buffer
        // inside it with a copy of its own gets that one.
        let (first, len) = shadow.span();
        let end = ((first + len) / PAGE).min(u64::from(MEMORY_PAGES) - 1);
        let source = std::mem::replace(&mut self.source, Source::INSIDE);
        for p in first.div_ceil(PAGE)..end {
            if p == page || self.pages.contains_key(&p) {
                continue;
            }
            if let Some((owner, off)) = Self::page_owner(p * PAGE)
                && let Some(number) = self.number(&owner)
            {
                self.set_page(p, Some((number, off)));
            }
        }
        self.source = source;
    }

    /// The pages shaders found empty since the last frame.
    fn take_faults(&mut self) {
        let mut pages = Vec::new();
        for i in 1..MEMORY_FAULTS as usize {
            // SAFETY: inside the faults' mapping; the GPU may be writing,
            // a number missed now is written again next frame.
            let at = unsafe { self.faults.1.add(i) };
            // SAFETY: as above.
            let page = unsafe { at.read_volatile() };
            if page != 0 {
                pages.push(page);
                // SAFETY: as above.
                unsafe { at.write_volatile(0) };
            }
        }
        if !pages.is_empty() {
            // Each is an access that read 0 or was dropped.
            static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 50 {
                tracing::warn!(
                    pages = pages.len(),
                    first = format_args!("{:#x}", u64::from(pages[0]) * PAGE),
                    "title memory by address: shaders touched pages that had no copy (those accesses read 0 or were dropped); the copies are made now"
                );
            }
        }
        self.source = Source::FAULT;
        for page in pages {
            self.map(u64::from(page) * PAGE);
        }
        if victory_shader::emit::memory_marks() {
            // Pages stores went to since the last look (only mapped pages
            // are marked): copied back from now on.
            let mut new = 0u64;
            let mapped: Vec<(u64, usize)> = self.pages.iter().map(|(&p, &n)| (p, n)).collect();
            for (page, owner) in mapped {
                // SAFETY: inside the faults' mapping (`fault_dwords`).
                let at = unsafe { self.faults.1.add((MEMORY_FAULTS as u64 + page) as usize) };
                // SAFETY: as above; the GPU may be writing a 1 meanwhile.
                if unsafe { at.read_volatile() } != 0 {
                    // SAFETY: as above.
                    unsafe { at.write_volatile(0) };
                    if self.written.insert(page) {
                        self.recalculate.insert(owner);
                        new += 1;
                    }
                }
            }
            if new != 0 {
                static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if SAID.fetch_add(1, Ordering::Relaxed) < 20 {
                    tracing::info!(new, written = self.written.len(), mapped = self.pages.len(), "title memory by address: pages shaders stored to (copied back from now on)");
                }
            }
        }
    }

    /// What a dispatch names: the copy around `address` and the 4 MiB
    /// after it.
    fn name(&mut self, address: u64) {
        self.map(address);
        if next_cell() {
            self.map((address & !(CELL - 1)) + CELL);
        }
    }
}

/// Indices of `Memory::numbered_by`.
struct Source;
impl Source {
    const VIEW: usize = 0;
    const PAIR: usize = 1;
    const UNIT: usize = 2;
    const FAULT: usize = 3;
    const INSIDE: usize = 4;
}

/// `VICTORY_MEMORY_UNIT_CONSTANTS=0`: a root constant read as an address
/// in 256-byte units names nothing (pages it alone would reach come from
/// fault notes, a frame later).
fn unit_constants() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_MEMORY_UNIT_CONSTANTS").is_none_or(|v| v != "0"))
}

/// `VICTORY_MEMORY_NEXT_CELL=0`: a named address does not also name the
/// 4 MiB after it.
fn next_cell() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_MEMORY_NEXT_CELL").is_none_or(|v| v != "0"))
}

/// The faults buffer: fault notes, then (`memory_marks`) a dword per page.
fn fault_dwords() -> u64 {
    let marks = if victory_shader::emit::memory_marks() { MEMORY_PAGES } else { 0 };
    u64::from(MEMORY_FAULTS) + u64::from(marks)
}

/// `VICTORY_MEMORY_JOURNAL=0`: directory pages keep their first copy for
/// the run, as before (no new directory when a page changes owner).
fn journal() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_MEMORY_JOURNAL").is_none_or(|v| v != "0"))
}

/// Before a draw whose shader reads title memory by address: what its
/// root arguments name gets its copies (`addresses`: its root views;
/// `constants`: its root constants, see the module), every numbered copy
/// has its view in `heap`'s run and is brought up to date when `list` is
/// submitted (and, with `writes`, copied back after). None if the host
/// cannot hold it; the draw is then dropped.
pub(crate) fn ready(
    list: &HostList,
    heap: &HostHeap,
    addresses: &[u64],
    constants: &[u32],
    writes: bool,
) -> Option<Ready> {
    let mut guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Memory::new();
    }
    let m = guard.as_mut()?;
    // (copies that got pages back are brought up to date in this list too)
    let remapped = journal() && m.reconcile()?;
    let frame = crate::frame::current();
    if m.frame != frame {
        m.frame = frame;
        m.refused.clear();
        m.take_faults();
    }
    let numbered = m.copies.len();
    m.source = Source::VIEW;
    for &address in addresses {
        m.name(address);
    }
    // A title address in two constants (low dword, then bits 32 to 39), or
    // in one in 256-byte units. A constant that only looks like one names
    // memory nothing reads; one that is not the title's memory is refused.
    for (i, &c) in constants.iter().enumerate() {
        if let Some(&high) = constants.get(i + 1)
            && (1..=0xFF).contains(&high)
            && c >= 0x1_0000
        {
            m.source = Source::PAIR;
            m.name(u64::from(c) | u64::from(high) << 32);
        }
        if c >= 0x0100_0000 && unit_constants() {
            m.source = Source::UNIT;
            m.name(u64::from(c) << 8);
        }
    }
    let key = std::ptr::from_ref(heap) as usize;
    let needed = u32::try_from(m.copies.len()).unwrap_or(u32::MAX - 1) + 1;
    let mut run = m.heaps.get(&key).copied();
    if run.is_none_or(|(_, slots, _)| needed > slots && slots < heap.dynamic_capacity()) {
        // Held for the heap's life: not given back with a list's slots, as
        // lists recorded with an older run still use it.
        let slots = needed
            .checked_next_power_of_two()
            .unwrap_or(u32::MAX)
            .max(FIRST_RUN)
            .min(heap.dynamic_capacity());
        let fresh = heap.dynamic(slots).map(|base| (base, slots)).or_else(|| {
            // A fragmented heap: a first run of what fits.
            let small = needed.min(FIRST_RUN);
            (run.is_none() && small < slots).then(|| heap.dynamic(small).map(|b| (b, small))).flatten()
        });
        match fresh {
            Some((base, slots)) => {
                // Every slot a null view: a slot is only used once filled,
                // but a stale descriptor is never left for the driver.
                for i in 0..slots {
                    heap.null_raw_uav_at(base + i);
                }
                if let Some((_, old, _)) = run {
                    tracing::info!(previous = old, slots, "title memory by address: grew the run of copy views");
                }
                run = Some((base, slots, 0));
            }
            None if run.is_none() => return None,
            None => {}
        }
    }
    let (base, slots, made) = run?;
    let count = (needed - 1).min(slots - 1);
    if count + 1 < needed {
        static WARN: std::sync::Once = std::sync::Once::new();
        WARN.call_once(|| {
            tracing::warn!(copies = needed - 1, views = count, "title memory by address: the view heap holds no more copy views; the rest read zeros")
        });
    }
    for (n, shadow) in m.copies.iter().enumerate().take(count as usize).skip(made as usize) {
        // The whole resource from its byte 0, as the directory's offsets
        // count; a buffer view holds at most 2^27 elements (D3D12), past
        // which reads give zeros.
        // SAFETY: a live buffer.
        let width = unsafe { shadow.buffer.GetDesc() }.Width;
        const MOST: u64 = 4 << 27;
        if width > MOST {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| tracing::warn!(width, "title memory by address: a copy over 512 MiB; its view covers the first 512 MiB"));
        }
        heap.raw_uav_at(base + 1 + n as u32, &shadow.buffer, 0, width.min(MOST));
    }
    m.heaps.insert(key, (base, slots, made.max(count)));
    // Once per list, and for copies numbered since: the builder alone
    // dispatches hundreds of times a frame.
    // shortcut: a copy another list numbers meanwhile is brought up to date
    // by that list's submission, which may run after this one's.
    const READ: usize = usize::MAX - 0x6d;
    const WRITE: usize = usize::MAX - 0x6e;
    let first_read = list.first_use(READ);
    let first_write = writes && list.first_use(WRITE);
    // Copies that got pages since the last bind: brought up to date in
    // this list too. Only the pages a shader can reach through the
    // directory, not the whole copy (after DexNom's ranges).
    let changed = std::mem::take(&mut m.recalculate);
    m.recalculate.clone_from(&changed);
    m.update_ranges();
    for (n, shadow) in m.copies.iter().enumerate().take(count as usize) {
        if m.owned[n] == 0 {
            continue;
        }
        // (also copies whose view this bind made: a run grew)
        let fresh = remapped || n >= numbered.min(made as usize) || changed.contains(&n);
        let read = first_read || fresh;
        let write = writes && (first_write || fresh);
        if !(read || write) {
            continue;
        }
        if read {
            for &(start, len) in &m.ranges[n] {
                shadow.refresh(list, start, len);
            }
        }
        if write {
            // Only pages shaders marked as stored to; a page's first
            // stores (before the frame's look at the marks) stay on the
            // card (`memory_marks`).
            let ranges = if victory_shader::emit::memory_marks() { &m.written_ranges[n] } else { &m.ranges[n] };
            for &(start, len) in ranges {
                shadow.gpu_writes(list, start, len);
            }
        }
    }
    if first_read {
        list.keep(&m.faults.0);
    }
    // Each directory the list binds lives until it has run.
    const DIRECTORY: usize = usize::MAX - 0x7000_0000;
    if list.first_use(DIRECTORY - m.revision as usize) {
        list.keep(&m.directory.0);
    }
    Some(Ready {
        directory: m.directory.2,
        faults: m.faults.2,
        base,
        count,
    })
}
