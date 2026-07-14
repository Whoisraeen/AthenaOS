//! Minimal initial user stack for native AthenaOS ELF tasks (relibc crt0).
//!
//! crt0 `_start` passes `rsp` to `relibc_crt0` as a pointer to `argc` on the
//! stack (SysV psABI). Without this image, relibc startup reads garbage.

/// Build argc/argv/envp/auxv for a fresh process and return the user `rsp`.
pub fn setup_relibc_stack(stack_top: u64) -> u64 {
    let mut sp = stack_top;
    // auxv terminator AT_NULL (16 bytes)
    sp = sp.saturating_sub(16);
    // AT_PHNUM (16 bytes)
    sp = sp.saturating_sub(16);
    // AT_PHENT (16 bytes)
    sp = sp.saturating_sub(16);
    // AT_PHDR (16 bytes)
    sp = sp.saturating_sub(16);
    // envp terminator
    sp = sp.saturating_sub(8);
    // argv terminator (argc may be 0)
    sp = sp.saturating_sub(8);
    // argc = 0
    sp = sp.saturating_sub(8);
    sp
}

/// Write the relibc stack image into mapped user stack pages.
pub fn write_relibc_stack_image(
    stack_top: u64,
    phoff: u64,
    phentsize: u16,
    phnum: u16,
    page_starts: &[(u64, x86_64::structures::paging::PhysFrame)],
) -> u64 {
    let initial_rsp = setup_relibc_stack(stack_top);
    let image_end = stack_top;
    if image_end < initial_rsp {
        return initial_rsp;
    }
    let total_len = (image_end - initial_rsp) as usize;
    let mut image = alloc::vec![0u8; total_len];
    let mut off = 0usize;

    // argc = 0
    image[off..off + 8].copy_from_slice(&0i64.to_le_bytes());
    off += 8;
    // argv[0] = NULL
    image[off..off + 8].copy_from_slice(&0u64.to_le_bytes());
    off += 8;
    // envp[0] = NULL
    image[off..off + 8].copy_from_slice(&0u64.to_le_bytes());
    off += 8;

    // AT_PHDR = 3
    image[off..off + 8].copy_from_slice(&3u64.to_le_bytes());
    off += 8;
    image[off..off + 8].copy_from_slice(&phoff.to_le_bytes());
    off += 8;

    // AT_PHENT = 4
    image[off..off + 8].copy_from_slice(&4u64.to_le_bytes());
    off += 8;
    image[off..off + 8].copy_from_slice(&(phentsize as u64).to_le_bytes());
    off += 8;

    // AT_PHNUM = 5
    image[off..off + 8].copy_from_slice(&5u64.to_le_bytes());
    off += 8;
    image[off..off + 8].copy_from_slice(&(phnum as u64).to_le_bytes());
    off += 8;

    // AT_NULL = 0
    image[off..off + 8].copy_from_slice(&0u64.to_le_bytes());
    off += 8;
    image[off..off + 8].copy_from_slice(&0u64.to_le_bytes());
    // off += 8; // (unused)

    let phys_off = *crate::memory::PHYS_MEM_OFFSET.get().unwrap();
    for (page_start, frame) in page_starts {
        let page_start = *page_start;
        let page_end = page_start + 4096;
        let copy_start = core::cmp::max(page_start, initial_rsp);
        let copy_end = core::cmp::min(page_end, image_end);
        if copy_start >= copy_end {
            continue;
        }
        let src_off = (copy_start - initial_rsp) as usize;
        let dst_off = (copy_start - page_start) as usize;
        let len = (copy_end - copy_start) as usize;
        let frame_virt = phys_off + frame.start_address().as_u64();
        let frame_ptr = frame_virt.as_mut_ptr::<u8>();
        unsafe {
            core::ptr::write_bytes(frame_ptr, 0, 4096);
            core::ptr::copy_nonoverlapping(
                image.as_ptr().add(src_off),
                frame_ptr.add(dst_off),
                len,
            );
        }
    }
    initial_rsp
}
