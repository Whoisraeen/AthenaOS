#![allow(dead_code)]

extern crate alloc;

use alloc::{collections::BTreeMap, string::String, vec::Vec};
use spin::Mutex;

// ─── Slab Cache ─────────────────────────────────────────────────────────────

pub struct SlabCache {
    name: String,
    object_size: usize,
    align: usize,
    objects_per_slab: u32,
    flags: SlabFlags,
    slabs_full: Vec<Slab>,
    slabs_partial: Vec<Slab>,
    slabs_free: Vec<Slab>,
    total_objects: u64,
    active_objects: u64,
    total_slabs: u64,
    ctor: Option<fn(*mut u8)>,
    dtor: Option<fn(*mut u8)>,
    stats: SlabStats,
}

pub struct Slab {
    page_addr: u64,
    num_objects: u32,
    free_count: u32,
    free_list: Vec<u16>,
    color_offset: u16,
}

#[derive(Clone, Copy)]
pub struct SlabFlags {
    pub reclaimable: bool,
    pub no_reap: bool,
    pub poison: bool,
    pub red_zone: bool,
    pub hwcache_align: bool,
    pub account: bool,
    pub mergeable: bool,
}

impl SlabFlags {
    pub fn default() -> Self {
        Self {
            reclaimable: false,
            no_reap: false,
            poison: false,
            red_zone: false,
            hwcache_align: true,
            account: true,
            mergeable: true,
        }
    }

    pub fn reclaimable() -> Self {
        Self {
            reclaimable: true,
            no_reap: false,
            poison: false,
            red_zone: false,
            hwcache_align: true,
            account: true,
            mergeable: false,
        }
    }

    pub fn debug() -> Self {
        Self {
            reclaimable: false,
            no_reap: false,
            poison: true,
            red_zone: true,
            hwcache_align: true,
            account: true,
            mergeable: false,
        }
    }
}

pub struct SlabStats {
    pub allocs: u64,
    pub frees: u64,
    pub alloc_fastpath: u64,
    pub alloc_slowpath: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub grow: u64,
    pub shrink: u64,
    pub errors: u64,
    pub max_objects: u64,
}

impl SlabStats {
    fn new() -> Self {
        Self {
            allocs: 0,
            frees: 0,
            alloc_fastpath: 0,
            alloc_slowpath: 0,
            cache_hits: 0,
            cache_misses: 0,
            grow: 0,
            shrink: 0,
            errors: 0,
            max_objects: 0,
        }
    }
}

impl Slab {
    fn new(page_addr: u64, num_objects: u32, color_offset: u16) -> Self {
        let free_list = (0..num_objects as u16).collect();
        Self {
            page_addr,
            num_objects,
            free_count: num_objects,
            free_list,
            color_offset,
        }
    }

    fn alloc(&mut self, object_size: usize) -> Option<*mut u8> {
        if self.free_count == 0 {
            return None;
        }
        let idx = self.free_list.pop()?;
        self.free_count -= 1;
        let offset = self.color_offset as u64 + (idx as u64 * object_size as u64);
        Some((self.page_addr + offset) as *mut u8)
    }

    fn free(&mut self, ptr: *mut u8, object_size: usize) -> bool {
        let addr = ptr as u64;
        if addr < self.page_addr {
            return false;
        }
        let offset = addr - self.page_addr - self.color_offset as u64;
        if offset % object_size as u64 != 0 {
            return false;
        }
        let idx = (offset / object_size as u64) as u16;
        if idx >= self.num_objects as u16 {
            return false;
        }
        if self.free_list.contains(&idx) {
            return false;
        }
        self.free_list.push(idx);
        self.free_count += 1;
        true
    }

    fn is_full(&self) -> bool {
        self.free_count == 0
    }

    fn is_empty(&self) -> bool {
        self.free_count == self.num_objects
    }

    fn contains(&self, ptr: *mut u8, object_size: usize) -> bool {
        let addr = ptr as u64;
        let slab_size = self.num_objects as u64 * object_size as u64 + self.color_offset as u64;
        addr >= self.page_addr && addr < self.page_addr + slab_size
    }
}

impl SlabCache {
    fn new(
        name: &str,
        object_size: usize,
        align: usize,
        flags: SlabFlags,
        ctor: Option<fn(*mut u8)>,
    ) -> Self {
        let aligned_size = Self::align_up(object_size, align);
        let page_size: u64 = 4096;
        let objects_per_slab = (page_size as usize / aligned_size).max(1) as u32;

        Self {
            name: String::from(name),
            object_size: aligned_size,
            align,
            objects_per_slab,
            flags,
            slabs_full: Vec::new(),
            slabs_partial: Vec::new(),
            slabs_free: Vec::new(),
            total_objects: 0,
            active_objects: 0,
            total_slabs: 0,
            ctor,
            dtor: None,
            stats: SlabStats::new(),
        }
    }

    fn align_up(size: usize, align: usize) -> usize {
        if align == 0 {
            return size;
        }
        (size + align - 1) & !(align - 1)
    }

    fn alloc_object(&mut self) -> Result<*mut u8, SlabError> {
        self.stats.allocs += 1;

        if let Some(slab) = self.slabs_partial.last_mut() {
            if let Some(ptr) = slab.alloc(self.object_size) {
                self.stats.alloc_fastpath += 1;
                self.active_objects += 1;

                let is_full = slab.is_full();
                if is_full {
                    let slab = self.slabs_partial.pop().unwrap();
                    self.slabs_full.push(slab);
                }

                if let Some(ctor) = self.ctor {
                    ctor(ptr);
                }
                return Ok(ptr);
            }
        }

        self.stats.alloc_slowpath += 1;

        if let Some(mut slab) = self.slabs_free.pop() {
            let ptr = slab
                .alloc(self.object_size)
                .ok_or(SlabError::InternalError)?;
            self.active_objects += 1;

            if slab.is_full() {
                self.slabs_full.push(slab);
            } else {
                self.slabs_partial.push(slab);
            }

            if let Some(ctor) = self.ctor {
                ctor(ptr);
            }
            return Ok(ptr);
        }

        self.grow()?;
        self.stats.grow += 1;

        if let Some(slab) = self.slabs_partial.last_mut() {
            if let Some(ptr) = slab.alloc(self.object_size) {
                self.active_objects += 1;

                let is_full = slab.is_full();
                if is_full {
                    let slab = self.slabs_partial.pop().unwrap();
                    self.slabs_full.push(slab);
                }

                if let Some(ctor) = self.ctor {
                    ctor(ptr);
                }
                return Ok(ptr);
            }
        }

        self.stats.errors += 1;
        Err(SlabError::OutOfMemory)
    }

    fn free_object(&mut self, ptr: *mut u8) -> Result<(), SlabError> {
        self.stats.frees += 1;

        for i in 0..self.slabs_full.len() {
            if self.slabs_full[i].contains(ptr, self.object_size) {
                if self.slabs_full[i].free(ptr, self.object_size) {
                    self.active_objects -= 1;
                    if let Some(dtor) = self.dtor {
                        dtor(ptr);
                    }
                    let slab = self.slabs_full.remove(i);
                    if slab.is_empty() {
                        self.slabs_free.push(slab);
                    } else {
                        self.slabs_partial.push(slab);
                    }
                    return Ok(());
                }
                return Err(SlabError::DoubleFree);
            }
        }

        for i in 0..self.slabs_partial.len() {
            if self.slabs_partial[i].contains(ptr, self.object_size) {
                if self.slabs_partial[i].free(ptr, self.object_size) {
                    self.active_objects -= 1;
                    if let Some(dtor) = self.dtor {
                        dtor(ptr);
                    }
                    if self.slabs_partial[i].is_empty() {
                        let slab = self.slabs_partial.remove(i);
                        self.slabs_free.push(slab);
                    }
                    return Ok(());
                }
                return Err(SlabError::DoubleFree);
            }
        }

        self.stats.errors += 1;
        Err(SlabError::InvalidPointer)
    }

    fn grow(&mut self) -> Result<(), SlabError> {
        static NEXT_PAGE: spin::Mutex<u64> = spin::Mutex::new(0xFFFF_8000_0000_0000);

        let mut next = NEXT_PAGE.lock();
        let page_addr = *next;
        *next += 4096;
        drop(next);

        let color_offset = ((self.total_slabs as u16) * 64) % (self.align as u16).max(1);
        let slab = Slab::new(page_addr, self.objects_per_slab, color_offset);

        self.total_objects += self.objects_per_slab as u64;
        self.total_slabs += 1;

        if self.total_objects > self.stats.max_objects {
            self.stats.max_objects = self.total_objects;
        }

        self.slabs_partial.push(slab);
        Ok(())
    }

    fn shrink(&mut self) -> u64 {
        if self.flags.no_reap {
            return 0;
        }
        let freed = self.slabs_free.len() as u64;
        self.total_slabs -= freed;
        self.total_objects -= freed * self.objects_per_slab as u64;
        self.slabs_free.clear();
        self.stats.shrink += freed;
        freed
    }

    fn info(&self) -> SlabCacheInfo {
        SlabCacheInfo {
            name: self.name.clone(),
            object_size: self.object_size,
            objects_per_slab: self.objects_per_slab,
            total_slabs: self.total_slabs,
            active_objects: self.active_objects,
            total_objects: self.total_objects,
            memory_used: self.total_slabs * 4096,
        }
    }
}

// ─── Slab Allocator ─────────────────────────────────────────────────────────

pub struct SlabAllocator {
    caches: BTreeMap<String, SlabCache>,
    default_caches: Vec<String>,
    total_memory: u64,
    used_memory: u64,
}

pub struct SlabCacheInfo {
    pub name: String,
    pub object_size: usize,
    pub objects_per_slab: u32,
    pub total_slabs: u64,
    pub active_objects: u64,
    pub total_objects: u64,
    pub memory_used: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlabError {
    OutOfMemory,
    CacheExists,
    CacheNotFound,
    InvalidPointer,
    DoubleFree,
    InvalidSize,
    CacheInUse,
    InternalError,
}

impl SlabAllocator {
    pub fn new() -> Self {
        let mut alloc = Self {
            caches: BTreeMap::new(),
            default_caches: Vec::new(),
            total_memory: 0,
            used_memory: 0,
        };
        alloc.init_default_caches();
        alloc
    }

    pub fn create_cache(
        &mut self,
        name: &str,
        size: usize,
        align: usize,
        flags: SlabFlags,
        ctor: Option<fn(*mut u8)>,
    ) -> Result<(), SlabError> {
        if self.caches.contains_key(name) {
            return Err(SlabError::CacheExists);
        }
        if size == 0 {
            return Err(SlabError::InvalidSize);
        }
        let cache = SlabCache::new(name, size, align, flags, ctor);
        self.caches.insert(String::from(name), cache);
        Ok(())
    }

    pub fn destroy_cache(&mut self, name: &str) -> Result<(), SlabError> {
        if let Some(cache) = self.caches.get(name) {
            if cache.active_objects > 0 {
                return Err(SlabError::CacheInUse);
            }
            self.caches.remove(name);
            Ok(())
        } else {
            Err(SlabError::CacheNotFound)
        }
    }

    pub fn alloc(&mut self, cache_name: &str) -> Result<*mut u8, SlabError> {
        if let Some(cache) = self.caches.get_mut(cache_name) {
            let ptr = cache.alloc_object()?;
            self.used_memory += cache.object_size as u64;
            Ok(ptr)
        } else {
            Err(SlabError::CacheNotFound)
        }
    }

    pub fn free(&mut self, cache_name: &str, ptr: *mut u8) -> Result<(), SlabError> {
        if let Some(cache) = self.caches.get_mut(cache_name) {
            let size = cache.object_size as u64;
            cache.free_object(ptr)?;
            self.used_memory = self.used_memory.saturating_sub(size);
            Ok(())
        } else {
            Err(SlabError::CacheNotFound)
        }
    }

    pub fn kmalloc(&mut self, size: usize) -> Result<*mut u8, SlabError> {
        if size == 0 {
            return Err(SlabError::InvalidSize);
        }
        let cache_name = self.find_size_cache(size).ok_or(SlabError::InvalidSize)?;
        let name = String::from(cache_name);
        self.alloc(&name)
    }

    pub fn kfree(&mut self, ptr: *mut u8, size: usize) -> Result<(), SlabError> {
        let cache_name = self.find_size_cache(size).ok_or(SlabError::InvalidSize)?;
        let name = String::from(cache_name);
        self.free(&name, ptr)
    }

    pub fn shrink_cache(&mut self, cache_name: &str) -> u64 {
        if let Some(cache) = self.caches.get_mut(cache_name) {
            let freed = cache.shrink();
            self.total_memory = self.total_memory.saturating_sub(freed * 4096);
            freed
        } else {
            0
        }
    }

    pub fn shrink_all(&mut self) -> u64 {
        let mut total_freed = 0u64;
        let names: Vec<String> = self.caches.keys().cloned().collect();
        for name in names {
            if let Some(cache) = self.caches.get_mut(&name) {
                if cache.flags.reclaimable || !cache.flags.no_reap {
                    total_freed += cache.shrink();
                }
            }
        }
        total_freed
    }

    pub fn cache_info(&self, name: &str) -> Option<SlabCacheInfo> {
        self.caches.get(name).map(|c| c.info())
    }

    pub fn slabinfo(&self) -> Vec<SlabCacheInfo> {
        self.caches.values().map(|c| c.info()).collect()
    }

    fn grow_cache(&mut self, cache_name: &str) -> Result<(), SlabError> {
        if let Some(cache) = self.caches.get_mut(cache_name) {
            cache.grow()?;
            self.total_memory += 4096;
            Ok(())
        } else {
            Err(SlabError::CacheNotFound)
        }
    }

    fn find_size_cache(&self, size: usize) -> Option<&str> {
        let sizes = [
            8, 16, 32, 64, 96, 128, 192, 256, 512, 1024, 2048, 4096, 8192,
        ];
        for &s in &sizes {
            if size <= s {
                let name = alloc::format!("kmalloc-{}", s);
                if self.caches.contains_key(&name) {
                    // We can't return a reference to a local, so match directly
                    for (k, _) in self.caches.iter() {
                        if *k == name {
                            return Some(k.as_str());
                        }
                    }
                }
            }
        }
        None
    }

    fn init_default_caches(&mut self) {
        let sizes = [
            8, 16, 32, 64, 96, 128, 192, 256, 512, 1024, 2048, 4096, 8192,
        ];
        for &size in &sizes {
            let name = alloc::format!("kmalloc-{}", size);
            let flags = SlabFlags::default();
            let cache = SlabCache::new(&name, size, 8, flags, None);
            self.default_caches.push(name.clone());
            self.caches.insert(name, cache);
        }

        let special_caches = [
            ("task_struct", 2048usize, 64usize),
            ("inode_cache", 512, 64),
            ("dentry_cache", 256, 64),
            ("file_cache", 256, 64),
            ("mm_struct", 1024, 64),
            ("vm_area_struct", 128, 64),
            ("signal_cache", 64, 8),
            ("pid_cache", 64, 8),
            ("cred_cache", 128, 8),
            ("buffer_head", 104, 8),
            ("radix_tree_node", 576, 64),
            ("bio_cache", 256, 8),
        ];

        for &(name, size, align) in &special_caches {
            let flags = SlabFlags {
                reclaimable: true,
                no_reap: false,
                poison: false,
                red_zone: false,
                hwcache_align: true,
                account: true,
                mergeable: false,
            };
            let cache = SlabCache::new(name, size, align, flags, None);
            self.caches.insert(String::from(name), cache);
        }
    }

    pub fn cache_count(&self) -> usize {
        self.caches.len()
    }

    pub fn total_active_objects(&self) -> u64 {
        self.caches.values().map(|c| c.active_objects).sum()
    }

    pub fn total_allocated_slabs(&self) -> u64 {
        self.caches.values().map(|c| c.total_slabs).sum()
    }
}

// ─── Memory Reclaimer (kswapd equivalent) ───────────────────────────────────

pub struct MemoryReclaimer {
    lru_active_anon: Vec<PageFrame>,
    lru_inactive_anon: Vec<PageFrame>,
    lru_active_file: Vec<PageFrame>,
    lru_inactive_file: Vec<PageFrame>,
    watermarks: Watermarks,
    scan_rate: u32,
    reclaim_target: u64,
    last_reclaim: u64,
    total_reclaimed: u64,
    total_scanned: u64,
    oom_triggered: u64,
}

pub struct PageFrame {
    pub phys: u64,
    pub flags: PageFrameFlags,
    pub access_count: u32,
    pub last_access: u64,
    pub mapping: Option<u64>,
    pub index: u64,
}

#[derive(Clone, Copy)]
pub struct PageFrameFlags {
    pub referenced: bool,
    pub dirty: bool,
    pub locked: bool,
    pub writeback: bool,
    pub active: bool,
    pub unevictable: bool,
    pub swapcache: bool,
    pub anon: bool,
    pub file: bool,
    pub mmap: bool,
}

impl PageFrameFlags {
    pub fn new_anon() -> Self {
        Self {
            referenced: false,
            dirty: false,
            locked: false,
            writeback: false,
            active: true,
            unevictable: false,
            swapcache: false,
            anon: true,
            file: false,
            mmap: false,
        }
    }

    pub fn new_file() -> Self {
        Self {
            referenced: false,
            dirty: false,
            locked: false,
            writeback: false,
            active: true,
            unevictable: false,
            swapcache: false,
            anon: false,
            file: true,
            mmap: false,
        }
    }
}

pub struct Watermarks {
    pub min: u64,
    pub low: u64,
    pub high: u64,
    pub current: u64,
    pub total: u64,
}

impl Watermarks {
    pub fn new(total: u64) -> Self {
        Self {
            min: total / 64,
            low: total / 32,
            high: total / 16,
            current: total,
            total,
        }
    }

    pub fn below_min(&self) -> bool {
        self.current < self.min
    }

    pub fn below_low(&self) -> bool {
        self.current < self.low
    }

    pub fn above_high(&self) -> bool {
        self.current >= self.high
    }

    pub fn free_percentage(&self) -> u64 {
        if self.total == 0 {
            return 0;
        }
        (self.current * 100) / self.total
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    None,
    Low,
    Medium,
    High,
    Critical,
}

pub struct ReclaimStats {
    pub scanned: u64,
    pub reclaimed: u64,
    pub writeback: u64,
    pub oom_kills: u64,
    pub active_anon: u64,
    pub inactive_anon: u64,
    pub active_file: u64,
    pub inactive_file: u64,
}

impl MemoryReclaimer {
    pub fn new(total_memory: u64) -> Self {
        Self {
            lru_active_anon: Vec::new(),
            lru_inactive_anon: Vec::new(),
            lru_active_file: Vec::new(),
            lru_inactive_file: Vec::new(),
            watermarks: Watermarks::new(total_memory),
            scan_rate: 32,
            reclaim_target: 0,
            last_reclaim: 0,
            total_reclaimed: 0,
            total_scanned: 0,
            oom_triggered: 0,
        }
    }

    pub fn add_page(&mut self, frame: PageFrame, active: bool) {
        if frame.flags.unevictable {
            return;
        }
        if frame.flags.anon {
            if active {
                self.lru_active_anon.push(frame);
            } else {
                self.lru_inactive_anon.push(frame);
            }
        } else {
            if active {
                self.lru_active_file.push(frame);
            } else {
                self.lru_inactive_file.push(frame);
            }
        }
    }

    pub fn remove_page(&mut self, phys: u64) -> Option<PageFrame> {
        if let Some(pos) = self.lru_active_anon.iter().position(|p| p.phys == phys) {
            return Some(self.lru_active_anon.remove(pos));
        }
        if let Some(pos) = self.lru_inactive_anon.iter().position(|p| p.phys == phys) {
            return Some(self.lru_inactive_anon.remove(pos));
        }
        if let Some(pos) = self.lru_active_file.iter().position(|p| p.phys == phys) {
            return Some(self.lru_active_file.remove(pos));
        }
        if let Some(pos) = self.lru_inactive_file.iter().position(|p| p.phys == phys) {
            return Some(self.lru_inactive_file.remove(pos));
        }
        None
    }

    pub fn mark_accessed(&mut self, phys: u64) {
        for page in self.lru_inactive_anon.iter_mut() {
            if page.phys == phys {
                page.flags.referenced = true;
                page.access_count += 1;
                return;
            }
        }
        for page in self.lru_inactive_file.iter_mut() {
            if page.phys == phys {
                page.flags.referenced = true;
                page.access_count += 1;
                return;
            }
        }
        for page in self.lru_active_anon.iter_mut() {
            if page.phys == phys {
                page.access_count += 1;
                return;
            }
        }
        for page in self.lru_active_file.iter_mut() {
            if page.phys == phys {
                page.access_count += 1;
                return;
            }
        }
    }

    pub fn mark_dirty(&mut self, phys: u64) {
        let lists: [&mut Vec<PageFrame>; 4] = unsafe {
            let s = self as *mut Self;
            [
                &mut (*s).lru_active_anon,
                &mut (*s).lru_inactive_anon,
                &mut (*s).lru_active_file,
                &mut (*s).lru_inactive_file,
            ]
        };
        for list in lists {
            for page in list.iter_mut() {
                if page.phys == phys {
                    page.flags.dirty = true;
                    return;
                }
            }
        }
    }

    pub fn shrink_page_cache(&mut self, target: u64) -> u64 {
        let mut reclaimed = 0u64;

        while reclaimed < target && !self.lru_inactive_file.is_empty() {
            if let Some(page) = self.lru_inactive_file.first() {
                if page.flags.locked || page.flags.writeback {
                    let page = self.lru_inactive_file.remove(0);
                    self.lru_inactive_file.push(page);
                    continue;
                }
                if page.flags.dirty {
                    if !self.try_writeback(page) {
                        let page = self.lru_inactive_file.remove(0);
                        self.lru_inactive_file.push(page);
                        continue;
                    }
                }
            }
            self.lru_inactive_file.remove(0);
            reclaimed += 4096;
            self.watermarks.current += 4096;
        }

        self.total_reclaimed += reclaimed;
        reclaimed
    }

    pub fn shrink_slab_caches(&mut self, slab: &mut SlabAllocator, target: u64) -> u64 {
        let mut reclaimed = 0u64;
        let pages_needed = target / 4096;

        let shrunk = slab.shrink_all();
        reclaimed += shrunk * 4096;

        if reclaimed < target {
            let names: Vec<String> = slab.caches.keys().cloned().collect();
            for name in names {
                if reclaimed >= pages_needed * 4096 {
                    break;
                }
                let freed = slab.shrink_cache(&name);
                reclaimed += freed * 4096;
            }
        }

        self.watermarks.current += reclaimed;
        self.total_reclaimed += reclaimed;
        reclaimed
    }

    pub fn kswapd_run(&mut self, slab: &mut SlabAllocator) -> u64 {
        if !self.should_reclaim() {
            return 0;
        }

        let target = self.watermarks.high - self.watermarks.current;
        let mut reclaimed = 0u64;

        self.age_active_list();

        reclaimed += self.shrink_page_cache(target / 2);

        if reclaimed < target {
            reclaimed += self.shrink_slab_caches(slab, target - reclaimed);
        }

        if reclaimed < target {
            let anon_target = target - reclaimed;
            let mut anon_reclaimed = 0u64;

            while anon_reclaimed < anon_target && !self.lru_inactive_anon.is_empty() {
                if let Some(page) = self.lru_inactive_anon.first() {
                    if page.flags.locked || page.flags.writeback || page.flags.unevictable {
                        let page = self.lru_inactive_anon.remove(0);
                        self.lru_inactive_anon.push(page);
                        break;
                    }
                }
                self.lru_inactive_anon.remove(0);
                anon_reclaimed += 4096;
            }

            self.watermarks.current += anon_reclaimed;
            reclaimed += anon_reclaimed;
        }

        self.last_reclaim = reclaimed;
        self.total_reclaimed += reclaimed;

        if self.watermarks.below_min() && reclaimed == 0 {
            self.oom_triggered += 1;
        }

        reclaimed
    }

    pub fn direct_reclaim(&mut self, target: u64) -> u64 {
        let mut reclaimed = 0u64;

        while reclaimed < target {
            if let Some(page) = self.evict_page() {
                reclaimed += 4096;
                self.watermarks.current += 4096;
                let _ = page;
            } else {
                break;
            }
        }

        self.total_reclaimed += reclaimed;
        reclaimed
    }

    pub fn should_reclaim(&self) -> bool {
        self.watermarks.below_low()
    }

    pub fn memory_pressure(&self) -> MemoryPressure {
        if self.watermarks.below_min() {
            MemoryPressure::Critical
        } else if self.watermarks.current < self.watermarks.low {
            MemoryPressure::High
        } else if self.watermarks.current < self.watermarks.high {
            MemoryPressure::Medium
        } else if self.watermarks.free_percentage() < 20 {
            MemoryPressure::Low
        } else {
            MemoryPressure::None
        }
    }

    fn age_active_list(&mut self) {
        let scan_count = self.scan_rate.min(self.lru_active_file.len() as u32);
        let mut to_demote: Vec<usize> = Vec::new();

        for i in 0..scan_count as usize {
            if i >= self.lru_active_file.len() {
                break;
            }
            if !self.lru_active_file[i].flags.referenced {
                to_demote.push(i);
            } else {
                self.lru_active_file[i].flags.referenced = false;
            }
        }

        for (offset, idx) in to_demote.iter().enumerate() {
            let actual_idx = idx - offset;
            if actual_idx < self.lru_active_file.len() {
                let mut page = self.lru_active_file.remove(actual_idx);
                page.flags.active = false;
                self.lru_inactive_file.push(page);
            }
        }

        let anon_scan = self.scan_rate.min(self.lru_active_anon.len() as u32);
        let mut anon_demote: Vec<usize> = Vec::new();

        for i in 0..anon_scan as usize {
            if i >= self.lru_active_anon.len() {
                break;
            }
            if !self.lru_active_anon[i].flags.referenced {
                anon_demote.push(i);
            } else {
                self.lru_active_anon[i].flags.referenced = false;
            }
        }

        for (offset, idx) in anon_demote.iter().enumerate() {
            let actual_idx = idx - offset;
            if actual_idx < self.lru_active_anon.len() {
                let mut page = self.lru_active_anon.remove(actual_idx);
                page.flags.active = false;
                self.lru_inactive_anon.push(page);
            }
        }

        self.total_scanned += (scan_count + anon_scan) as u64;
    }

    pub fn promote_to_active(&mut self, phys: u64) {
        if let Some(pos) = self.lru_inactive_file.iter().position(|p| p.phys == phys) {
            let mut page = self.lru_inactive_file.remove(pos);
            page.flags.active = true;
            page.flags.referenced = true;
            self.lru_active_file.push(page);
            return;
        }
        if let Some(pos) = self.lru_inactive_anon.iter().position(|p| p.phys == phys) {
            let mut page = self.lru_inactive_anon.remove(pos);
            page.flags.active = true;
            page.flags.referenced = true;
            self.lru_active_anon.push(page);
        }
    }

    pub fn demote_to_inactive(&mut self, phys: u64) {
        if let Some(pos) = self.lru_active_file.iter().position(|p| p.phys == phys) {
            let mut page = self.lru_active_file.remove(pos);
            page.flags.active = false;
            page.flags.referenced = false;
            self.lru_inactive_file.push(page);
            return;
        }
        if let Some(pos) = self.lru_active_anon.iter().position(|p| p.phys == phys) {
            let mut page = self.lru_active_anon.remove(pos);
            page.flags.active = false;
            page.flags.referenced = false;
            self.lru_inactive_anon.push(page);
        }
    }

    fn try_writeback(&self, frame: &PageFrame) -> bool {
        if frame.flags.locked || frame.flags.writeback {
            return false;
        }
        frame.flags.dirty && frame.mapping.is_some()
    }

    fn evict_page(&mut self) -> Option<PageFrame> {
        if let Some(page) = self.lru_inactive_file.first() {
            if !page.flags.locked && !page.flags.writeback && !page.flags.dirty {
                return Some(self.lru_inactive_file.remove(0));
            }
        }

        for i in 0..self.lru_inactive_file.len() {
            let page = &self.lru_inactive_file[i];
            if !page.flags.locked && !page.flags.writeback && !page.flags.dirty {
                return Some(self.lru_inactive_file.remove(i));
            }
        }

        if let Some(page) = self.lru_inactive_anon.first() {
            if !page.flags.locked && !page.flags.writeback {
                return Some(self.lru_inactive_anon.remove(0));
            }
        }

        for i in 0..self.lru_inactive_anon.len() {
            let page = &self.lru_inactive_anon[i];
            if !page.flags.locked && !page.flags.writeback {
                return Some(self.lru_inactive_anon.remove(i));
            }
        }

        None
    }

    fn scan_inactive(&mut self, count: u32) -> Vec<u64> {
        let mut candidates = Vec::new();
        let file_count = count.min(self.lru_inactive_file.len() as u32);

        for i in 0..file_count as usize {
            if i >= self.lru_inactive_file.len() {
                break;
            }
            let page = &self.lru_inactive_file[i];
            if page.flags.referenced {
                self.lru_inactive_file[i].flags.referenced = false;
            } else if !page.flags.locked && !page.flags.writeback {
                candidates.push(page.phys);
            }
        }

        let anon_count = (count - file_count).min(self.lru_inactive_anon.len() as u32);
        for i in 0..anon_count as usize {
            if i >= self.lru_inactive_anon.len() {
                break;
            }
            let page = &self.lru_inactive_anon[i];
            if page.flags.referenced {
                self.lru_inactive_anon[i].flags.referenced = false;
            } else if !page.flags.locked && !page.flags.writeback {
                candidates.push(page.phys);
            }
        }

        self.total_scanned += (file_count + anon_count) as u64;
        candidates
    }

    pub fn stats(&self) -> ReclaimStats {
        ReclaimStats {
            scanned: self.total_scanned,
            reclaimed: self.total_reclaimed,
            writeback: 0,
            oom_kills: self.oom_triggered,
            active_anon: self.lru_active_anon.len() as u64,
            inactive_anon: self.lru_inactive_anon.len() as u64,
            active_file: self.lru_active_file.len() as u64,
            inactive_file: self.lru_inactive_file.len() as u64,
        }
    }

    pub fn total_pages(&self) -> u64 {
        (self.lru_active_anon.len()
            + self.lru_inactive_anon.len()
            + self.lru_active_file.len()
            + self.lru_inactive_file.len()) as u64
    }

    pub fn set_watermarks(&mut self, min: u64, low: u64, high: u64) {
        self.watermarks.min = min;
        self.watermarks.low = low;
        self.watermarks.high = high;
    }

    pub fn set_scan_rate(&mut self, rate: u32) {
        self.scan_rate = rate;
    }

    pub fn consume_pages(&mut self, count: u64) {
        self.watermarks.current = self.watermarks.current.saturating_sub(count * 4096);
    }

    pub fn release_pages(&mut self, count: u64) {
        self.watermarks.current =
            (self.watermarks.current + count * 4096).min(self.watermarks.total);
    }
}

// ─── Global State ───────────────────────────────────────────────────────────

pub static SLAB_ALLOCATOR: Mutex<Option<SlabAllocator>> = Mutex::new(None);
pub static MEMORY_RECLAIMER: Mutex<Option<MemoryReclaimer>> = Mutex::new(None);

pub fn init() {
    *SLAB_ALLOCATOR.lock() = Some(SlabAllocator::new());
    *MEMORY_RECLAIMER.lock() = Some(MemoryReclaimer::new(256 * 1024 * 1024));
}
