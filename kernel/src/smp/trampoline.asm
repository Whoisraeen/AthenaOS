; AthKernel AP boot trampoline.
;
; Lives at physical address 0x8000 (SIPI vector 0x08). Walks the Application
; Processor through real mode → protected mode → long mode, then far-jumps
; to a Rust ap_entry whose absolute address (and the per-AP stack + PML4)
; the BSP patches into the boot block before sending SIPI.
;
; ORG 0x8000 — NASM computes every label as if loaded at 0x8000, so the
; literal addresses embedded in instructions resolve correctly when the
; trampoline is copied to that physical address.
;
; SIPI entry: CS:IP = 0x0800:0x0000, so the AP starts executing at the very
; first byte (offset 0x0). We must emit a short jump here to skip over the
; boot block data. A `jmp short` is 2 bytes (0xEB, rel8).
;
; Boot block layout (kept in lock-step with smp.rs::boot_block::*):
;   +0x000  jmp_skip     2 bytes   short jump over boot block
;   +0x002  _pad         6 bytes   padding to keep 8-byte alignment
;   +0x008  pml4         qword     physical address of BSP's PML4 (CR3 value)
;   +0x010  stack_top    qword     virtual address of this AP's kernel stack top
;   +0x018  entry        qword     absolute virtual address of Rust ap_entry
;   +0x020  apic_id      word      this AP's APIC id (passed in RDI)
;   +0x022  _padding     word

[BITS 16]
[ORG 0x8000]

ap_trampoline_start:

; ── Entry point: skip over boot block data ─────────────────────────────────
    jmp short ap_real_entry         ; 2-byte short jmp (0xEB, rel8)
    times 6 db 0                   ; pad to offset 0x08

; ── Boot block (BSP patches these before sending SIPI) ────────────────────
ap_boot_pml4:        dq 0          ; offset 0x08
ap_boot_stack_top:   dq 0          ; offset 0x10
ap_boot_entry:       dq 0          ; offset 0x18
ap_boot_apic_id:     dw 0          ; offset 0x20
                     dw 0          ; padding (offset 0x22)

; ── Real mode (CS = SIPI_VECTOR = 0x800, IP = 0) ──────────────────────────
ap_real_entry:
    cli
    cld

    ; Mirror CS into DS/ES/SS so we can use offset-only addressing for
    ; lgdt — DS:ofs computes (CS << 4) + ofs = 0x8000 + ofs.
    mov ax, cs
    mov ds, ax
    mov es, ax
    mov ss, ax

    ; Load the 32-bit GDT. The descriptor's "base" field already contains the
    ; absolute physical address 0x8XXX of pmode_gdt because we ORG'd at 0x8000.
    o32 lgdt [ap_pmode_gdt_desc - 0x8000]    ; offset relative to CS base

    ; Enable Protected Mode (CR0.PE = bit 0).
    mov eax, cr0
    or  eax, 1
    mov cr0, eax

    ; Far jump to 32-bit code. NASM emits the right 0x66/EA prefix for us.
    jmp dword 0x08:ap_pmode_entry

; ── Protected mode ────────────────────────────────────────────────────────
[BITS 32]
ap_pmode_entry:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; CR4.PAE (bit 5) — required for IA-32e.
    mov eax, cr4
    or  eax, 0x20
    mov cr4, eax

    ; Load the BSP's PML4 from the boot block. The low 32 bits are enough
    ; on QEMU defaults (PML4 frame is in low memory); a future hardening
    ; pass writes the full 64 bits via a register pair.
    mov eax, [ap_boot_pml4]
    mov cr3, eax

    ; Enable Long Mode (IA32_EFER.LME = bit 8) and NX support
    ; (EFER.NXE = bit 11). NXE is critical: the kernel's page tables use
    ; the NX bit (bit 63 of PTEs); without NXE those bits are "reserved"
    ; and every page walk triggers #PF with error code 0x8.
    mov ecx, 0xC0000080
    rdmsr
    or  eax, 0x900          ; LME (0x100) | NXE (0x800)
    wrmsr

    ; Enable paging (CR0.PG = bit 31). After this, instruction fetches go
    ; through CR3 — which is why the BSP identity-maps the trampoline page.
    mov eax, cr0
    or  eax, 0x80000000
    mov cr0, eax

    ; Load the 64-bit GDT and far-jump into a 64-bit CS to leave compat mode.
    lgdt [ap_lmode_gdt_desc]
    jmp 0x08:ap_lmode_entry

; ── Long mode (CS now has L=1) ────────────────────────────────────────────
[BITS 64]
ap_lmode_entry:
    ; Data selectors are mostly cosmetic in long mode but settle them anyway
    ; so legacy instructions don't trap.
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; Stack pointer from boot block.
    mov rsp, [ap_boot_stack_top]

    ; First arg (rdi) = our APIC id, for the Rust function to identify itself.
    movzx rdi, word [ap_boot_apic_id]

    ; Tail-jump to Rust ap_entry. Beyond this point the AP is running normal
    ; kernel code at high virtual addresses; we never return here.
    mov rax, [ap_boot_entry]
    jmp rax

; ── GDTs + descriptor pointers ────────────────────────────────────────────

align 8
ap_pmode_gdt_desc:
    dw ap_pmode_gdt_end - ap_pmode_gdt_start - 1
    dd ap_pmode_gdt_start
ap_pmode_gdt_start:
    dq 0                          ; null
    dq 0x00CF9A000000FFFF         ; 32-bit code: base 0, limit 4 GiB, RX
    dq 0x00CF92000000FFFF         ; 32-bit data: base 0, limit 4 GiB, RW
ap_pmode_gdt_end:

align 8
ap_lmode_gdt_desc:
    dw ap_lmode_gdt_end - ap_lmode_gdt_start - 1
    dd ap_lmode_gdt_start
ap_lmode_gdt_start:
    dq 0                          ; null
    dq 0x00AF9A000000FFFF         ; 64-bit code: L=1, RX
    dq 0x00AF92000000FFFF         ; 64-bit data: RW
ap_lmode_gdt_end:

ap_trampoline_end:
