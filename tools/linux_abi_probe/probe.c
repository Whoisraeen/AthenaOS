// AthenaOS Linux-ABI probe — nostdlib, raw x86_64 syscalls.
//
// A tiny freestanding static Linux binary (no libc, no CRT startup) that issues
// the syscalls `kernel/src/linux_syscall.rs` translates (the ~31-syscall vein in
// memory: linux-syscall-oracle-gap-filling) DIRECTLY via the `syscall`
// instruction, self-checks each result, and prints a single FAIL-able line to
// fd 1 (wired to the AthenaOS console by linux_exec), then exits with a matching
// code. No glibc startup path means nothing runs before our checks, so a PASS
// is unambiguously our translation layer working on a real Linux ELF.
//
//   [linux-abi-probe] PASS ...        every checked syscall returned sanely
//   [linux-abi-probe] FAIL: <what>    a translated syscall misbehaved
//
// Build ON ATHENA (the oracle), validate it prints PASS there first:
//   gcc -static -no-pie -nostdlib -ffreestanding -fno-builtin \
//       -fno-stack-protector -fno-tree-loop-distribute-patterns -O2 -o probe probe.c
//   (-fno-stack-protector is REQUIRED: with no TLS the canary read from %fs:0x28
//    faults at address 0x28 before main; Arch gcc defaults to -fstack-protector.)
//   ./probe ; echo $?        (must print PASS, exit 0)

typedef unsigned long u64;
typedef long i64;
typedef unsigned short u16;

#define SYS_read 0
#define SYS_write 1
#define SYS_close 3
#define SYS_mmap 9
#define SYS_munmap 11
#define SYS_getuid 102
#define SYS_uname 63
#define SYS_sysinfo 99
#define SYS_getrandom 318
#define SYS_statx 332
#define SYS_openat 257
#define SYS_clock_gettime 228
#define SYS_pipe2 293
#define SYS_exit_group 231
#define AT_FDCWD -100
#define CLOCK_MONOTONIC 1

static i64 sys6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f) {
    i64 ret;
    register i64 r10 asm("r10") = d;
    register i64 r8 asm("r8") = e;
    register i64 r9 asm("r9") = f;
    asm volatile("syscall"
                 : "=a"(ret)
                 : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9)
                 : "rcx", "r11", "memory");
    return ret;
}

static u64 slen(const char *s) {
    u64 n = 0;
    while (s[n]) n++;
    return n;
}

static void out(const char *s) {
    sys6(SYS_write, 1, (i64)s, (i64)slen(s), 0, 0, 0);
}

static void die(int code) {
    sys6(SYS_exit_group, code, 0, 0, 0, 0, 0);
    for (;;) {}
}

static void fail(const char *tag) {
    out("[linux-abi-probe] FAIL: ");
    out(tag);
    out("\n");
    die(1);
}

// ─── clone(CLONE_THREAD) — the flagship Linux-threads (Proton/Steam) test ────
// A real pthread-style thread: shares VM/FS/FILES/SIGHAND, runs on its OWN
// stack, sets a shared sentinel, then exits. The parent sched_yield-spins on
// the sentinel. This is the exact workload that exposed the per-task syscall
// user-RSP bug (parent + thread interleave syscalls on one BSP-pinned CPU, the
// parent yielding mid-syscall) — `thread ok` + no DOUBLE FAULT verifies the fix.
#define SYS_clone 56
#define SYS_sched_yield 24
#define SYS_exit 60
#define CLONE_VM 0x00000100
#define CLONE_FS 0x00000200
#define CLONE_FILES 0x00000400
#define CLONE_SIGHAND 0x00000800
#define CLONE_THREAD 0x00010000
#define CLONE_CHILD_CLEARTID 0x00200000
#define CLONE_CHILD_SETTID 0x01000000

static volatile long g_thread_sentinel = 0;
static volatile int g_child_tid = -1; // clear_child_tid slot (kernel zeroes on exit)
static unsigned char g_thread_stack[16384] __attribute__((aligned(16)));

static void test_clone_thread(void) {
    // CLONE_CHILD_SETTID writes the new TID into g_child_tid at clone;
    // CLONE_CHILD_CLEARTID makes the kernel ZERO g_child_tid + futex-wake on the
    // thread's exit — the pthread_join primitive. ctid (a4) = &g_child_tid.
    long flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD |
                 CLONE_CHILD_CLEARTID | CLONE_CHILD_SETTID;
    void *child_sp = g_thread_stack + sizeof(g_thread_stack); // grows down
    long ret;
    register long r10 asm("r10") = (long)&g_child_tid; // child_tid ptr
    register long r8 asm("r8") = 0;                    // tls
    // clone(flags, child_sp, ptid=0, ctid=&g_child_tid, tls=0). On the child
    // rax==0 and rsp=child_sp; the child must NOT unwind through C — it sets the
    // shared sentinel and SYS_exits inline. The parent (rax=tid) jumps past it.
    asm volatile("syscall\n\t"
                 "test %%rax, %%rax\n\t"
                 "jnz 1f\n\t"
                 "movq $1, g_thread_sentinel(%%rip)\n\t" // child: sentinel = 1
                 "mov $60, %%rax\n\t"                    // SYS_exit
                 "xor %%edi, %%edi\n\t"
                 "syscall\n\t"
                 "1:\n\t"
                 : "=a"(ret)
                 : "a"(SYS_clone), "D"(flags), "S"(child_sp), "d"(0), "r"(r10),
                   "r"(r8)
                 : "rcx", "r11", "memory");
    if (ret < 0) fail("clone");
    for (long i = 0; i < 100000000 && g_thread_sentinel == 0; i++) {
        sys6(SYS_sched_yield, 0, 0, 0, 0, 0, 0);
    }
    if (g_thread_sentinel == 0) fail("thread-no-run");
    out("[linux-abi-probe] thread ok\n");

    // join: the kernel must zero g_child_tid (CLONE_CHILD_CLEARTID) when the
    // thread exits. Spin-yield until it clears — this is what pthread_join does
    // (futex-wait on the TID slot). A non-clearing kernel hangs here -> FAIL.
    for (long i = 0; i < 100000000 && g_child_tid != 0; i++) {
        sys6(SYS_sched_yield, 0, 0, 0, 0, 0, 0);
    }
    if (g_child_tid != 0) fail("join-no-clear");
    out("[linux-abi-probe] join ok\n");
}

// Real entry: align the stack (the kernel enters at _start with RSP pointing at
// argc, 16-byte aligned; a plain C `_start` prologue would not re-align before
// an SSE access and faults) then call the C body. `and $-16,%rsp` keeps RSP
// 16-aligned; the `call` pushes 8, giving probe_main the ABI-expected RSP%16==8.
__asm__(
    ".global _start\n"
    "_start:\n"
    "  xor %ebp, %ebp\n"
    "  and $-16, %rsp\n"
    "  call probe_main\n"
    "  hlt\n");

void probe_main(void) {
    // Write FIRST so the console/write path is proven before anything can stall,
    // and emit a per-step marker after each syscall so a stall pinpoints which.
    out("[linux-abi-probe] start\n");

    // statx("/") EARLY (right after the proven write) so its result lands inside
    // the first few captured burst lines — settles whether AthenaOS's real-path
    // statx returns promptly. stx_mode (offset 28) must say directory.
    // AT_FDCWD=-100, mask STATX_BASIC_STATS=0x7ff.
    unsigned char stx[256];
    if (sys6(SYS_statx, -100, (i64) "/", 0, 0x7ff, (i64)stx, 0) != 0)
        fail("statx-ret");
    if ((*(u16 *)(stx + 28) & 0170000) != 0040000) fail("statx-not-dir");
    out("[linux-abi-probe] statx ok\n");

    // file I/O — openat + read + close a kernel-provided file. This is the
    // capability every real Linux binary depends on (loaders, configs, assets).
    long fd = sys6(SYS_openat, AT_FDCWD, (i64) "/proc/version", 0 /*O_RDONLY*/, 0,
                   0, 0);
    if (fd < 0) fail("openat");
    unsigned char rbuf[128];
    long n = sys6(SYS_read, fd, (i64)rbuf, sizeof(rbuf), 0, 0, 0);
    if (n <= 0) fail("read");
    (void)sys6(SYS_close, fd, 0, 0, 0, 0, 0);
    out("[linux-abi-probe] fileio ok\n");

    // mmap — anonymous private RW mapping (the allocation primitive every real
    // binary uses for heap/stack/loading). MAP_PRIVATE|MAP_ANONYMOUS=0x22,
    // PROT_READ|PROT_WRITE=3, fd=-1. A negative errno is [-4095,-1]; a real
    // mapping is a large positive address. Then touch it — if the mapping is a
    // stub (address without backing) the store FAULTS and kills the probe.
    long p = sys6(SYS_mmap, 0, 4096, 3, 0x22, -1, 0);
    if (p > -4096 && p < 0) fail("mmap");
    if (p == 0) fail("mmap-null");
    *(volatile unsigned char *)p = 0xAB;
    if (*(volatile unsigned char *)p != 0xAB) fail("mmap-rw");
    (void)sys6(SYS_munmap, p, 4096, 0, 0, 0, 0);
    out("[linux-abi-probe] mmap ok\n");

    // file WRITE — create, write, reopen, read back, verify. The capability a
    // real binary needs to persist anything (game saves, configs, logs).
    // O_CREAT|O_WRONLY|O_TRUNC = 0x241; /tmp is the writable scratch mount
    // (tmpfs) — writable on real Linux too, so the Athena oracle validates it.
    static const char wmsg[16] = "athena-abi-write\n";
    long wfd = sys6(SYS_openat, AT_FDCWD, (i64) "/tmp/athena_abi.txt", 0x241,
                    0644, 0, 0);
    if (wfd < 0) fail("open-w");
    if (sys6(SYS_write, wfd, (i64)wmsg, 16, 0, 0, 0) != 16) fail("write");
    (void)sys6(SYS_close, wfd, 0, 0, 0, 0, 0);
    long rfd = sys6(SYS_openat, AT_FDCWD, (i64) "/tmp/athena_abi.txt", 0, 0, 0, 0);
    if (rfd < 0) fail("reopen");
    unsigned char vbuf[32];
    long rn = sys6(SYS_read, rfd, (i64)vbuf, sizeof(vbuf), 0, 0, 0);
    if (rn != 16) fail("readback-len");
    if (vbuf[0] != 'r' || vbuf[14] != 'e') fail("readback-data");
    (void)sys6(SYS_close, rfd, 0, 0, 0, 0, 0);
    out("[linux-abi-probe] filewrite ok\n");

    // getrandom — must fill all 16 bytes.
    unsigned char rnd[16];
    if (sys6(SYS_getrandom, (i64)rnd, 16, 0, 0, 0, 0) != 16) fail("getrandom");
    out("[linux-abi-probe] getrandom ok\n");

    // sysinfo — totalram (offset 32, struct sizeof 112) must be non-zero.
    unsigned char si[128];
    if (sys6(SYS_sysinfo, (i64)si, 0, 0, 0, 0, 0) != 0) fail("sysinfo-ret");
    if (*(u64 *)(si + 32) == 0) fail("sysinfo-totalram");
    out("[linux-abi-probe] sysinfo ok\n");

    // uname — sysname (offset 0) must be populated.
    unsigned char un[400];
    if (sys6(SYS_uname, (i64)un, 0, 0, 0, 0, 0) != 0) fail("uname-ret");
    if (un[0] == 0) fail("uname-empty");
    out("[linux-abi-probe] uname ok\n");

    // clock_gettime(CLOCK_MONOTONIC) — time, which every game needs for frame
    // pacing. timespec = { i64 sec; i64 nsec; }. Must be non-zero (a stub
    // returns 0) and must advance across a spin (monotonic moves forward).
    unsigned char ts1[16], ts2[16];
    if (sys6(SYS_clock_gettime, CLOCK_MONOTONIC, (i64)ts1, 0, 0, 0, 0) != 0)
        fail("clock-ret");
    u64 t1 = *(u64 *)ts1 * 1000000000ull + *(u64 *)(ts1 + 8);
    if (t1 == 0) fail("clock-zero");
    for (volatile int i = 0; i < 2000000; i++) {
    }
    if (sys6(SYS_clock_gettime, CLOCK_MONOTONIC, (i64)ts2, 0, 0, 0, 0) != 0)
        fail("clock-ret2");
    u64 t2 = *(u64 *)ts2 * 1000000000ull + *(u64 *)(ts2 + 8);
    if (t2 < t1) fail("clock-backward");
    out("[linux-abi-probe] clock ok\n");

    // pipe — self-contained IPC primitive (shells, subprocess comms). Create,
    // write a byte to the write end, read it back from the read end, verify.
    int pfd[2];
    if (sys6(SYS_pipe2, (i64)pfd, 0, 0, 0, 0, 0) != 0) fail("pipe2");
    unsigned char pb = 0x5A;
    if (sys6(SYS_write, pfd[1], (i64)&pb, 1, 0, 0, 0) != 1) fail("pipe-write");
    unsigned char pr = 0;
    if (sys6(SYS_read, pfd[0], (i64)&pr, 1, 0, 0, 0) != 1) fail("pipe-read");
    if (pr != 0x5A) fail("pipe-data");
    (void)sys6(SYS_close, pfd[0], 0, 0, 0, 0, 0);
    (void)sys6(SYS_close, pfd[1], 0, 0, 0, 0, 0);
    out("[linux-abi-probe] pipe ok\n");

    // getuid — must not trap (any value accepted).
    (void)sys6(SYS_getuid, 0, 0, 0, 0, 0, 0);

    // clone(CLONE_THREAD) LAST — it's the riskiest path and exercises the
    // per-task syscall-RSP fix; everything above must already be proven.
    test_clone_thread();

    out("[linux-abi-probe] PASS (nostdlib raw-syscall probe)\n");
    die(0);
}
