use crate::arch::VirtAddr;
use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

pub struct ElfBinary<'a> {
    data: &'a [u8],
}

impl<'a> ElfBinary<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < core::mem::size_of::<Elf64Ehdr>() {
            return Err("File too small");
        }
        if &data[0..4] != b"\x7fELF" {
            return Err("Invalid ELF magic");
        }
        if data[4] != 2 {
            return Err("ELF is not 64-bit");
        }
        if data[5] != 1 {
            return Err("ELF is not little-endian");
        }
        if data[6] != 1 {
            return Err("Unsupported ELF version");
        }
        Ok(ElfBinary { data })
    }

    pub fn load_into_pml4(
        &self,
        pml4: x86_64::structures::paging::PhysFrame,
    ) -> Result<(u64, u64, u16, u16), &'static str> {
        let mut ehdr = Elf64Ehdr {
            e_ident: [0; 16],
            e_type: 0,
            e_machine: 0,
            e_version: 0,
            e_entry: 0,
            e_phoff: 0,
            e_shoff: 0,
            e_flags: 0,
            e_ehsize: 0,
            e_phentsize: 0,
            e_phnum: 0,
            e_shentsize: 0,
            e_shnum: 0,
            e_shstrndx: 0,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.data.as_ptr(),
                &mut ehdr as *mut _ as *mut u8,
                core::mem::size_of::<Elf64Ehdr>(),
            );
        }
        if ehdr.e_type != 2 && ehdr.e_type != 3 {
            return Err("ELF is not executable or position-independent executable");
        }
        if ehdr.e_machine != 62 {
            // EM_X86_64
            return Err("Not an x86_64 ELF");
        }
        if ehdr.e_version != 1 {
            return Err("Unsupported ELF object version");
        }
        if ehdr.e_ehsize as usize != core::mem::size_of::<Elf64Ehdr>() {
            return Err("Unexpected ELF header size");
        }
        if ehdr.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>() {
            return Err("Unexpected ELF program header size");
        }
        const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;
        if ehdr.e_entry >= USER_SPACE_END {
            return Err("ELF entry outside user-space");
        }

        // Bounds-check the program header table
        let ph_table_len = (ehdr.e_phnum as usize)
            .checked_mul(ehdr.e_phentsize as usize)
            .ok_or("ELF program header table overflow")?;
        let ph_end = (ehdr.e_phoff as usize)
            .checked_add(ph_table_len)
            .ok_or("ELF program header table overflow")?;
        if ph_end > self.data.len() {
            return Err("ELF program header table out of bounds");
        }

        let mut global_alloc = crate::memory::GlobalFrameAllocator;
        use x86_64::structures::paging::FrameAllocator;

        for i in 0..ehdr.e_phnum {
            let ph_offset = (i as usize)
                .checked_mul(core::mem::size_of::<Elf64Phdr>())
                .ok_or("ELF program header offset overflow")?;
            let offset = (ehdr.e_phoff as usize)
                .checked_add(ph_offset)
                .ok_or("ELF program header offset overflow")?;
            let mut phdr = Elf64Phdr {
                p_type: 0,
                p_flags: 0,
                p_offset: 0,
                p_vaddr: 0,
                p_paddr: 0,
                p_filesz: 0,
                p_memsz: 0,
                p_align: 0,
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.data[offset..].as_ptr(),
                    &mut phdr as *mut _ as *mut u8,
                    core::mem::size_of::<Elf64Phdr>(),
                );
            }

            // PT_LOAD == 1
            if phdr.p_type == 1 {
                // Skip zero-size segments
                if phdr.p_memsz == 0 {
                    continue;
                }

                if phdr.p_filesz > phdr.p_memsz {
                    return Err("ELF segment file size exceeds memory size");
                }

                let seg_end = phdr
                    .p_vaddr
                    .checked_add(phdr.p_memsz)
                    .ok_or("ELF segment virtual address overflow")?;
                if phdr.p_vaddr >= USER_SPACE_END || seg_end > USER_SPACE_END {
                    return Err("ELF segment virtual address outside user-space");
                }

                // Validate file data bounds
                let file_data_end = (phdr.p_offset as usize)
                    .checked_add(phdr.p_filesz as usize)
                    .ok_or("ELF segment file data overflow")?;
                if file_data_end > self.data.len() {
                    return Err("ELF segment file data out of bounds");
                }

                let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
                if (phdr.p_flags & 2) != 0 {
                    flags |= PageTableFlags::WRITABLE;
                }
                // W^X: mark any segment without PF_X (bit 0) non-executable so a
                // writable .data/.bss/.rodata cannot host injected shellcode.
                // The native loader previously mapped EVERY segment executable —
                // a full W+X address space. Mirrors the Linux-ELF path
                // (task.rs, PF_X gate). (Audit 2026-07-06, finding #4.)
                if (phdr.p_flags & 1) == 0 {
                    flags |= PageTableFlags::NO_EXECUTE;
                }

                let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(phdr.p_vaddr));
                let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(seg_end - 1));

                for page in Page::range_inclusive(start_page, end_page) {
                    let frame = global_alloc
                        .allocate_frame()
                        .ok_or("OOM loading ELF segment")?;

                    let mapped = unsafe {
                        crate::memory::map_page_in_pml4_fallible(pml4, page, frame, flags)
                    };
                    if !mapped {
                        crate::memory::deallocate_frame(frame);
                    }

                    let page_start = page.start_address().as_u64();
                    let page_end = page_start + 4096;

                    let seg_start = phdr.p_vaddr;
                    let file_end = phdr
                        .p_vaddr
                        .checked_add(phdr.p_filesz)
                        .ok_or("ELF segment file virtual address overflow")?;

                    let frame_ptr = unsafe {
                        crate::memory::pml4_page_ptr(pml4, page)
                            .ok_or("ELF segment page not mapped in target PML4")?
                    };

                    unsafe {
                        // When two PT_LOAD segments are not page-aligned they can
                        // share a page (e.g. end of .text and start of .rodata in
                        // the same 4 KiB). On the second segment the page is
                        // ALREADY mapped (`mapped == false`) and frame_ptr points
                        // at the frame the first segment already filled — blindly
                        // zeroing the whole 4 KiB here would erase the earlier
                        // segment's bytes. So only full-page-zero a freshly mapped
                        // frame (clears allocator garbage); for a shared page zero
                        // strictly THIS segment's bss bytes inside the page.
                        if mapped {
                            core::ptr::write_bytes(frame_ptr, 0, 4096);
                        } else {
                            let seg_end = seg_start + phdr.p_memsz;
                            let bss_start = core::cmp::max(page_start, file_end);
                            let bss_end = core::cmp::min(page_end, seg_end);
                            if bss_start < bss_end {
                                core::ptr::write_bytes(
                                    frame_ptr.add((bss_start - page_start) as usize),
                                    0,
                                    (bss_end - bss_start) as usize,
                                );
                            }
                        }

                        let file_data_overlap_start = core::cmp::max(page_start, seg_start);
                        let file_data_overlap_end = core::cmp::min(page_end, file_end);

                        if file_data_overlap_start < file_data_overlap_end {
                            let copy_len =
                                (file_data_overlap_end - file_data_overlap_start) as usize;
                            let file_offset = phdr.p_offset + (file_data_overlap_start - seg_start);

                            core::ptr::copy_nonoverlapping(
                                self.data.as_ptr().add(file_offset as usize),
                                frame_ptr.add((file_data_overlap_start - page_start) as usize),
                                copy_len,
                            );
                        }
                    }
                }
            }
        }

        // ── Apply R_X86_64_RELATIVE relocations (PIE / ET_DYN) ────────────────
        // Segments are mapped at base 0 (vaddr == link address), so each
        // RELATIVE relocation simply stores `addend` at the location `r_offset`.
        // Without this, a PIE's GOT / .init_array function pointers stay null and
        // the program jumps through them into the null page (e.g. relibc crt0).
        if ehdr.e_type == 3 {
            const DT_NULL: u64 = 0;
            const DT_RELA: u64 = 7;
            const DT_RELASZ: u64 = 8;
            const DT_RELAENT: u64 = 9;
            const R_X86_64_RELATIVE: u32 = 8;
            const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;
            let phsz = core::mem::size_of::<Elf64Phdr>();

            // Read one program header by index (table bounds already validated).
            let read_phdr = |i: u16| -> Elf64Phdr {
                let mut ph = Elf64Phdr {
                    p_type: 0,
                    p_flags: 0,
                    p_offset: 0,
                    p_vaddr: 0,
                    p_paddr: 0,
                    p_filesz: 0,
                    p_memsz: 0,
                    p_align: 0,
                };
                let off = ehdr.e_phoff as usize + i as usize * phsz;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.data[off..].as_ptr(),
                        &mut ph as *mut _ as *mut u8,
                        phsz,
                    );
                }
                ph
            };
            // Map a virtual address to its file offset via the PT_LOAD segments.
            let vaddr_to_foff = |v: u64| -> Option<usize> {
                for i in 0..ehdr.e_phnum {
                    let ph = read_phdr(i);
                    if ph.p_type == 1 && v >= ph.p_vaddr && v < ph.p_vaddr + ph.p_filesz {
                        return Some((ph.p_offset + (v - ph.p_vaddr)) as usize);
                    }
                }
                None
            };

            // Locate the DT_RELA table from PT_DYNAMIC.
            let mut rela_v: Option<u64> = None;
            let mut rela_sz: u64 = 0;
            let mut rela_ent: u64 = 24;
            for i in 0..ehdr.e_phnum {
                let ph = read_phdr(i);
                if ph.p_type != 2 {
                    continue;
                } // PT_DYNAMIC
                let base = ph.p_offset as usize;
                let mut p = 0usize;
                while p + 16 <= ph.p_filesz as usize && base + p + 16 <= self.data.len() {
                    let tag =
                        u64::from_le_bytes(self.data[base + p..base + p + 8].try_into().unwrap());
                    let val = u64::from_le_bytes(
                        self.data[base + p + 8..base + p + 16].try_into().unwrap(),
                    );
                    if tag == DT_NULL {
                        break;
                    }
                    match tag {
                        DT_RELA => rela_v = Some(val),
                        DT_RELASZ => rela_sz = val,
                        DT_RELAENT => rela_ent = val,
                        _ => {}
                    }
                    p += 16;
                }
                break;
            }

            if let Some(rv) = rela_v {
                if let Some(rela_foff) = vaddr_to_foff(rv) {
                    let ent = if rela_ent > 0 { rela_ent } else { 24 };
                    let count = (rela_sz / ent) as usize;
                    let mut applied = 0u64;
                    for i in 0..count {
                        let off = rela_foff + i * ent as usize;
                        if off + 24 > self.data.len() {
                            break;
                        }
                        let r_offset =
                            u64::from_le_bytes(self.data[off..off + 8].try_into().unwrap());
                        let r_info =
                            u64::from_le_bytes(self.data[off + 8..off + 16].try_into().unwrap());
                        let r_addend =
                            u64::from_le_bytes(self.data[off + 16..off + 24].try_into().unwrap());
                        if (r_info & 0xffff_ffff) as u32 != R_X86_64_RELATIVE {
                            continue;
                        }
                        if r_offset >= USER_SPACE_END {
                            continue;
                        }
                        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(r_offset));
                        let page_start = page.start_address().as_u64();
                        let in_page = (r_offset - page_start) as usize;
                        if in_page + 8 <= 4096 {
                            // Fast path: the 8-byte value lies within one page.
                            if let Some(fp) = unsafe { crate::memory::pml4_page_ptr(pml4, page) } {
                                unsafe {
                                    core::ptr::write_unaligned(
                                        fp.add(in_page) as *mut u64,
                                        r_addend,
                                    );
                                }
                                applied += 1;
                            }
                        } else {
                            // The 8-byte value straddles a 4 KiB page boundary.
                            // Splitting the store across the two mapped frames is
                            // correct; the old code silently `continue`d, leaving
                            // the slot at its link-time (base-0) value so a PIE
                            // would later jump through a wrong/null pointer with no
                            // diagnostic. Rare (compilers seldom emit a pointer
                            // crossing a page) but a real correctness hole.
                            let value_bytes = r_addend.to_le_bytes();
                            let first_len = 4096 - in_page;
                            let fp0 = unsafe { crate::memory::pml4_page_ptr(pml4, page) };
                            let fp1 = unsafe { crate::memory::pml4_page_ptr(pml4, page + 1) };
                            if let (Some(fp0), Some(fp1)) = (fp0, fp1) {
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        value_bytes.as_ptr(),
                                        fp0.add(in_page),
                                        first_len,
                                    );
                                    core::ptr::copy_nonoverlapping(
                                        value_bytes.as_ptr().add(first_len),
                                        fp1,
                                        8 - first_len,
                                    );
                                }
                                applied += 1;
                            } else {
                                crate::serial_println!(
                                    "[elf-reloc] WARN: page-crossing RELATIVE reloc at {:#x} unmapped; skipped",
                                    r_offset
                                );
                            }
                        }
                    }
                    crate::serial_println!("[elf-reloc] applied {} RELATIVE relocs", applied);
                }
            }
        }

        Ok((ehdr.e_entry, ehdr.e_phoff, ehdr.e_phentsize, ehdr.e_phnum))
    }
}
