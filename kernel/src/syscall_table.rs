#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use spin::Mutex;

// ─── Syscall Numbers (Linux x86_64 ABI) ──────────────────────────────────────

// File I/O
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSTAT: u64 = 6;
pub const SYS_POLL: u64 = 7;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_PREAD64: u64 = 17;
pub const SYS_PWRITE64: u64 = 18;
pub const SYS_READV: u64 = 19;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_PIPE: u64 = 22;
pub const SYS_SELECT: u64 = 23;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_MREMAP: u64 = 25;
pub const SYS_MSYNC: u64 = 26;
pub const SYS_MINCORE: u64 = 27;
pub const SYS_MADVISE: u64 = 28;
pub const SYS_SHMGET: u64 = 29;
pub const SYS_SHMAT: u64 = 30;
pub const SYS_SHMCTL: u64 = 31;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_PAUSE: u64 = 34;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETITIMER: u64 = 36;
pub const SYS_ALARM: u64 = 37;
pub const SYS_SETITIMER: u64 = 38;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SENDFILE: u64 = 40;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_ACCEPT: u64 = 43;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_SENDMSG: u64 = 46;
pub const SYS_RECVMSG: u64 = 47;
pub const SYS_SHUTDOWN: u64 = 48;
pub const SYS_BIND: u64 = 49;
pub const SYS_LISTEN: u64 = 50;
pub const SYS_GETSOCKNAME: u64 = 51;
pub const SYS_GETPEERNAME: u64 = 52;
pub const SYS_SOCKETPAIR: u64 = 53;
pub const SYS_SETSOCKOPT: u64 = 54;
pub const SYS_GETSOCKOPT: u64 = 55;
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_VFORK: u64 = 58;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_SEMGET: u64 = 64;
pub const SYS_SEMOP: u64 = 65;
pub const SYS_SEMCTL: u64 = 66;
pub const SYS_SHMDT: u64 = 67;
pub const SYS_MSGGET: u64 = 68;
pub const SYS_MSGSND: u64 = 69;
pub const SYS_MSGRCV: u64 = 70;
pub const SYS_MSGCTL: u64 = 71;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_FLOCK: u64 = 73;
pub const SYS_FSYNC: u64 = 74;
pub const SYS_FDATASYNC: u64 = 75;
pub const SYS_TRUNCATE: u64 = 76;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_GETDENTS: u64 = 78;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_FCHDIR: u64 = 81;
pub const SYS_RENAME: u64 = 82;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_CREAT: u64 = 85;
pub const SYS_LINK: u64 = 86;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_SYMLINK: u64 = 88;
pub const SYS_READLINK: u64 = 89;
pub const SYS_CHMOD: u64 = 90;
pub const SYS_FCHMOD: u64 = 91;
pub const SYS_CHOWN: u64 = 92;
pub const SYS_FCHOWN: u64 = 93;
pub const SYS_LCHOWN: u64 = 94;
pub const SYS_UMASK: u64 = 95;
pub const SYS_GETTIMEOFDAY: u64 = 96;
pub const SYS_GETRLIMIT: u64 = 97;
pub const SYS_GETRUSAGE: u64 = 98;
pub const SYS_SYSINFO: u64 = 99;
pub const SYS_TIMES: u64 = 100;
pub const SYS_PTRACE: u64 = 101;
pub const SYS_GETUID: u64 = 102;
pub const SYS_SYSLOG: u64 = 103;
pub const SYS_GETGID: u64 = 104;
pub const SYS_SETUID: u64 = 105;
pub const SYS_SETGID: u64 = 106;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_SETPGID: u64 = 109;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_GETPGRP: u64 = 111;
pub const SYS_SETSID: u64 = 112;
pub const SYS_SETREUID: u64 = 113;
pub const SYS_SETREGID: u64 = 114;
pub const SYS_GETGROUPS: u64 = 115;
pub const SYS_SETGROUPS: u64 = 116;
pub const SYS_SETRESUID: u64 = 117;
pub const SYS_GETRESUID: u64 = 118;
pub const SYS_SETRESGID: u64 = 119;
pub const SYS_GETRESGID: u64 = 120;
pub const SYS_GETPGID: u64 = 121;
pub const SYS_SETFSUID: u64 = 122;
pub const SYS_SETFSGID: u64 = 123;
pub const SYS_GETSID: u64 = 124;
pub const SYS_CAPGET: u64 = 125;
pub const SYS_CAPSET: u64 = 126;
pub const SYS_RT_SIGPENDING: u64 = 127;
pub const SYS_RT_SIGTIMEDWAIT: u64 = 128;
pub const SYS_RT_SIGQUEUEINFO: u64 = 129;
pub const SYS_RT_SIGSUSPEND: u64 = 130;
pub const SYS_SIGALTSTACK: u64 = 131;
pub const SYS_UTIME: u64 = 132;
pub const SYS_MKNOD: u64 = 133;
pub const SYS_USELIB: u64 = 134;
pub const SYS_PERSONALITY: u64 = 135;
pub const SYS_USTAT: u64 = 136;
pub const SYS_STATFS: u64 = 137;
pub const SYS_FSTATFS: u64 = 138;
pub const SYS_SYSFS: u64 = 139;
pub const SYS_GETPRIORITY: u64 = 140;
pub const SYS_SETPRIORITY: u64 = 141;
pub const SYS_SCHED_SETPARAM: u64 = 142;
pub const SYS_SCHED_GETPARAM: u64 = 143;
pub const SYS_SCHED_SETSCHEDULER: u64 = 144;
pub const SYS_SCHED_GETSCHEDULER: u64 = 145;
pub const SYS_SCHED_GET_PRIORITY_MAX: u64 = 146;
pub const SYS_SCHED_GET_PRIORITY_MIN: u64 = 147;
pub const SYS_SCHED_RR_GET_INTERVAL: u64 = 148;
pub const SYS_MLOCK: u64 = 149;
pub const SYS_MUNLOCK: u64 = 150;
pub const SYS_MLOCKALL: u64 = 151;
pub const SYS_MUNLOCKALL: u64 = 152;
pub const SYS_VHANGUP: u64 = 153;
pub const SYS_MODIFY_LDT: u64 = 154;
pub const SYS_PIVOT_ROOT: u64 = 155;
pub const SYS_SYSCTL: u64 = 156;
pub const SYS_PRCTL: u64 = 157;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_ADJTIMEX: u64 = 159;
pub const SYS_SETRLIMIT: u64 = 160;
pub const SYS_CHROOT: u64 = 161;
pub const SYS_SYNC: u64 = 162;
pub const SYS_ACCT: u64 = 163;
pub const SYS_SETTIMEOFDAY: u64 = 164;
pub const SYS_MOUNT: u64 = 165;
pub const SYS_UMOUNT2: u64 = 166;
pub const SYS_SWAPON: u64 = 167;
pub const SYS_SWAPOFF: u64 = 168;
pub const SYS_REBOOT: u64 = 169;
pub const SYS_SETHOSTNAME: u64 = 170;
pub const SYS_SETDOMAINNAME: u64 = 171;
pub const SYS_IOPL: u64 = 172;
pub const SYS_IOPERM: u64 = 173;
pub const SYS_CREATE_MODULE: u64 = 174;
pub const SYS_INIT_MODULE: u64 = 175;
pub const SYS_DELETE_MODULE: u64 = 176;
pub const SYS_GET_KERNEL_SYMS: u64 = 177;
pub const SYS_QUERY_MODULE: u64 = 178;
pub const SYS_QUOTACTL: u64 = 179;
pub const SYS_NFSSERVCTL: u64 = 180;
pub const SYS_GETPMSG: u64 = 181;
pub const SYS_PUTPMSG: u64 = 182;
pub const SYS_AFS_SYSCALL: u64 = 183;
pub const SYS_TUXCALL: u64 = 184;
pub const SYS_SECURITY: u64 = 185;
pub const SYS_GETTID: u64 = 186;
pub const SYS_READAHEAD: u64 = 187;
pub const SYS_SETXATTR: u64 = 188;
pub const SYS_LSETXATTR: u64 = 189;
pub const SYS_FSETXATTR: u64 = 190;
pub const SYS_GETXATTR: u64 = 191;
pub const SYS_LGETXATTR: u64 = 192;
pub const SYS_FGETXATTR: u64 = 193;
pub const SYS_LISTXATTR: u64 = 194;
pub const SYS_LLISTXATTR: u64 = 195;
pub const SYS_FLISTXATTR: u64 = 196;
pub const SYS_REMOVEXATTR: u64 = 197;
pub const SYS_LREMOVEXATTR: u64 = 198;
pub const SYS_FREMOVEXATTR: u64 = 199;
pub const SYS_TKILL: u64 = 200;
pub const SYS_TIME: u64 = 201;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_SCHED_SETAFFINITY: u64 = 203;
pub const SYS_SCHED_GETAFFINITY: u64 = 204;
pub const SYS_SET_THREAD_AREA: u64 = 205;
pub const SYS_IO_SETUP: u64 = 206;
pub const SYS_IO_DESTROY: u64 = 207;
pub const SYS_IO_GETEVENTS: u64 = 208;
pub const SYS_IO_SUBMIT: u64 = 209;
pub const SYS_IO_CANCEL: u64 = 210;
pub const SYS_GET_THREAD_AREA: u64 = 211;
pub const SYS_LOOKUP_DCOOKIE: u64 = 212;
pub const SYS_EPOLL_CREATE: u64 = 213;
pub const SYS_EPOLL_CTL_OLD: u64 = 214;
pub const SYS_EPOLL_WAIT_OLD: u64 = 215;
pub const SYS_REMAP_FILE_PAGES: u64 = 216;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_RESTART_SYSCALL: u64 = 219;
pub const SYS_SEMTIMEDOP: u64 = 220;
pub const SYS_FADVISE64: u64 = 221;
pub const SYS_TIMER_CREATE: u64 = 222;
pub const SYS_TIMER_SETTIME: u64 = 223;
pub const SYS_TIMER_GETTIME: u64 = 224;
pub const SYS_TIMER_GETOVERRUN: u64 = 225;
pub const SYS_TIMER_DELETE: u64 = 226;
pub const SYS_CLOCK_SETTIME: u64 = 227;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_CLOCK_GETRES: u64 = 229;
pub const SYS_CLOCK_NANOSLEEP: u64 = 230;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_TGKILL: u64 = 234;
pub const SYS_UTIMES: u64 = 235;
pub const SYS_VSERVER: u64 = 236;
pub const SYS_MBIND: u64 = 237;
pub const SYS_SET_MEMPOLICY: u64 = 238;
pub const SYS_GET_MEMPOLICY: u64 = 239;
pub const SYS_MQ_OPEN: u64 = 240;
pub const SYS_MQ_UNLINK: u64 = 241;
pub const SYS_MQ_TIMEDSEND: u64 = 242;
pub const SYS_MQ_TIMEDRECEIVE: u64 = 243;
pub const SYS_MQ_NOTIFY: u64 = 244;
pub const SYS_MQ_GETSETATTR: u64 = 245;
pub const SYS_KEXEC_LOAD: u64 = 246;
pub const SYS_WAITID: u64 = 247;
pub const SYS_ADD_KEY: u64 = 248;
pub const SYS_REQUEST_KEY: u64 = 249;
pub const SYS_KEYCTL: u64 = 250;
pub const SYS_IOPRIO_SET: u64 = 251;
pub const SYS_IOPRIO_GET: u64 = 252;
pub const SYS_INOTIFY_INIT: u64 = 253;
pub const SYS_INOTIFY_ADD_WATCH: u64 = 254;
pub const SYS_INOTIFY_RM_WATCH: u64 = 255;
pub const SYS_MIGRATE_PAGES: u64 = 256;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT: u64 = 258;
pub const SYS_MKNODAT: u64 = 259;
pub const SYS_FCHOWNAT: u64 = 260;
pub const SYS_FUTIMESAT: u64 = 261;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_RENAMEAT: u64 = 264;
pub const SYS_LINKAT: u64 = 265;
pub const SYS_SYMLINKAT: u64 = 266;
pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FCHMODAT: u64 = 268;
pub const SYS_FACCESSAT: u64 = 269;
pub const SYS_PSELECT6: u64 = 270;
pub const SYS_PPOLL: u64 = 271;
pub const SYS_UNSHARE: u64 = 272;
pub const SYS_SET_ROBUST_LIST: u64 = 273;
pub const SYS_GET_ROBUST_LIST: u64 = 274;
pub const SYS_SPLICE: u64 = 275;
pub const SYS_TEE: u64 = 276;
pub const SYS_SYNC_FILE_RANGE: u64 = 277;
pub const SYS_VMSPLICE: u64 = 278;
pub const SYS_MOVE_PAGES: u64 = 279;
pub const SYS_UTIMENSAT: u64 = 280;
pub const SYS_EPOLL_PWAIT: u64 = 281;
pub const SYS_SIGNALFD: u64 = 282;
pub const SYS_TIMERFD_CREATE: u64 = 283;
pub const SYS_EVENTFD: u64 = 284;
pub const SYS_FALLOCATE: u64 = 285;
pub const SYS_TIMERFD_SETTIME: u64 = 286;
pub const SYS_TIMERFD_GETTIME: u64 = 287;
pub const SYS_ACCEPT4: u64 = 288;
pub const SYS_SIGNALFD4: u64 = 289;
pub const SYS_EVENTFD2: u64 = 290;
pub const SYS_EPOLL_CREATE1: u64 = 291;
pub const SYS_DUP3: u64 = 292;
pub const SYS_PIPE2: u64 = 293;
pub const SYS_INOTIFY_INIT1: u64 = 294;
pub const SYS_PREADV: u64 = 295;
pub const SYS_PWRITEV: u64 = 296;
pub const SYS_RT_TGSIGQUEUEINFO: u64 = 297;
pub const SYS_PERF_EVENT_OPEN: u64 = 298;
pub const SYS_RECVMMSG: u64 = 299;
pub const SYS_FANOTIFY_INIT: u64 = 300;
pub const SYS_FANOTIFY_MARK: u64 = 301;
pub const SYS_PRLIMIT64: u64 = 302;
pub const SYS_NAME_TO_HANDLE_AT: u64 = 303;
pub const SYS_OPEN_BY_HANDLE_AT: u64 = 304;
pub const SYS_CLOCK_ADJTIME: u64 = 305;
pub const SYS_SYNCFS: u64 = 306;
pub const SYS_SENDMMSG: u64 = 307;
pub const SYS_SETNS: u64 = 308;
pub const SYS_GETCPU: u64 = 309;
pub const SYS_PROCESS_VM_READV: u64 = 310;
pub const SYS_PROCESS_VM_WRITEV: u64 = 311;
pub const SYS_KCMP: u64 = 312;
pub const SYS_FINIT_MODULE: u64 = 313;
pub const SYS_SCHED_SETATTR: u64 = 314;
pub const SYS_SCHED_GETATTR: u64 = 315;
pub const SYS_RENAMEAT2: u64 = 316;
pub const SYS_SECCOMP: u64 = 317;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_MEMFD_CREATE: u64 = 319;
pub const SYS_KEXEC_FILE_LOAD: u64 = 320;
pub const SYS_BPF: u64 = 321;
pub const SYS_EXECVEAT: u64 = 322;
pub const SYS_USERFAULTFD: u64 = 323;
pub const SYS_MEMBARRIER: u64 = 324;
pub const SYS_MLOCK2: u64 = 325;
pub const SYS_COPY_FILE_RANGE: u64 = 326;
pub const SYS_PREADV2: u64 = 327;
pub const SYS_PWRITEV2: u64 = 328;
pub const SYS_PKEY_MPROTECT: u64 = 329;
pub const SYS_PKEY_ALLOC: u64 = 330;
pub const SYS_PKEY_FREE: u64 = 331;
pub const SYS_STATX: u64 = 332;
pub const SYS_IO_PGETEVENTS: u64 = 333;
pub const SYS_RSEQ: u64 = 334;
pub const SYS_PIDFD_SEND_SIGNAL: u64 = 424;
pub const SYS_IO_URING_SETUP: u64 = 425;
pub const SYS_IO_URING_ENTER: u64 = 426;
pub const SYS_IO_URING_REGISTER: u64 = 427;
pub const SYS_OPEN_TREE: u64 = 428;
pub const SYS_MOVE_MOUNT: u64 = 429;
pub const SYS_FSOPEN: u64 = 430;
pub const SYS_FSCONFIG: u64 = 431;
pub const SYS_FSMOUNT: u64 = 432;
pub const SYS_FSPICK: u64 = 433;
pub const SYS_PIDFD_OPEN: u64 = 434;
pub const SYS_CLONE3: u64 = 435;
pub const SYS_CLOSE_RANGE: u64 = 436;
pub const SYS_OPENAT2: u64 = 437;
pub const SYS_PIDFD_GETFD: u64 = 438;
pub const SYS_FACCESSAT2: u64 = 439;
pub const SYS_PROCESS_MADVISE: u64 = 440;
pub const SYS_EPOLL_PWAIT2: u64 = 441;
pub const SYS_MOUNT_SETATTR: u64 = 442;
pub const SYS_QUOTACTL_FD: u64 = 443;
pub const SYS_LANDLOCK_CREATE_RULESET: u64 = 444;
pub const SYS_LANDLOCK_ADD_RULE: u64 = 445;
pub const SYS_LANDLOCK_RESTRICT_SELF: u64 = 446;
pub const SYS_MEMFD_SECRET: u64 = 447;
pub const SYS_PROCESS_MRELEASE: u64 = 448;
pub const SYS_FUTEX_WAITV: u64 = 449;
pub const SYS_SET_MEMPOLICY_HOME_NODE: u64 = 450;

// ─── Error Codes (negated errno) ─────────────────────────────────────────────

pub const EPERM: i64 = -1;
pub const ENOENT: i64 = -2;
pub const ESRCH: i64 = -3;
pub const EINTR: i64 = -4;
pub const EIO: i64 = -5;
pub const ENXIO: i64 = -6;
pub const E2BIG: i64 = -7;
pub const ENOEXEC: i64 = -8;
pub const EBADF: i64 = -9;
pub const ECHILD: i64 = -10;
pub const EAGAIN: i64 = -11;
pub const ENOMEM: i64 = -12;
pub const EACCES: i64 = -13;
pub const EFAULT: i64 = -14;
pub const ENOTBLK: i64 = -15;
pub const EBUSY: i64 = -16;
pub const EEXIST: i64 = -17;
pub const EXDEV: i64 = -18;
pub const ENODEV: i64 = -19;
pub const ENOTDIR: i64 = -20;
pub const EISDIR: i64 = -21;
pub const EINVAL: i64 = -22;
pub const ENFILE: i64 = -23;
pub const EMFILE: i64 = -24;
pub const ENOTTY: i64 = -25;
pub const ETXTBSY: i64 = -26;
pub const EFBIG: i64 = -27;
pub const ENOSPC: i64 = -28;
pub const ESPIPE: i64 = -29;
pub const EROFS: i64 = -30;
pub const EMLINK: i64 = -31;
pub const EPIPE: i64 = -32;
pub const EDOM: i64 = -33;
pub const ERANGE: i64 = -34;
pub const EDEADLK: i64 = -35;
pub const ENAMETOOLONG: i64 = -36;
pub const ENOLCK: i64 = -37;
pub const ENOSYS: i64 = -38;
pub const ENOTEMPTY: i64 = -39;
pub const ELOOP: i64 = -40;
pub const EWOULDBLOCK: i64 = -11; // Same as EAGAIN
pub const ENOMSG: i64 = -42;
pub const EIDRM: i64 = -43;
pub const ECHRNG: i64 = -44;
pub const EL2NSYNC: i64 = -45;
pub const EL3HLT: i64 = -46;
pub const EL3RST: i64 = -47;
pub const ELNRNG: i64 = -48;
pub const EUNATCH: i64 = -49;
pub const ENOCSI: i64 = -50;
pub const EL2HLT: i64 = -51;
pub const EBADE: i64 = -52;
pub const EBADR: i64 = -53;
pub const EXFULL: i64 = -54;
pub const ENOANO: i64 = -55;
pub const EBADRQC: i64 = -56;
pub const EBADSLT: i64 = -57;
pub const EBFONT: i64 = -59;
pub const ENOSTR: i64 = -60;
pub const ENODATA: i64 = -61;
pub const ETIME: i64 = -62;
pub const ENOSR: i64 = -63;
pub const ENONET: i64 = -64;
pub const ENOPKG: i64 = -65;
pub const EREMOTE: i64 = -66;
pub const ENOLINK: i64 = -67;
pub const EADV: i64 = -68;
pub const ESRMNT: i64 = -69;
pub const ECOMM: i64 = -70;
pub const EPROTO: i64 = -71;
pub const EMULTIHOP: i64 = -72;
pub const EDOTDOT: i64 = -73;
pub const EBADMSG: i64 = -74;
pub const EOVERFLOW: i64 = -75;
pub const ENOTUNIQ: i64 = -76;
pub const EBADFD: i64 = -77;
pub const EREMCHG: i64 = -78;
pub const ELIBACC: i64 = -79;
pub const ELIBBAD: i64 = -80;
pub const ELIBSCN: i64 = -81;
pub const ELIBMAX: i64 = -82;
pub const ELIBEXEC: i64 = -83;
pub const EILSEQ: i64 = -84;
pub const ERESTART: i64 = -85;
pub const ESTRPIPE: i64 = -86;
pub const EUSERS: i64 = -87;
pub const ENOTSOCK: i64 = -88;
pub const EDESTADDRREQ: i64 = -89;
pub const EMSGSIZE: i64 = -90;
pub const EPROTOTYPE: i64 = -91;
pub const ENOPROTOOPT: i64 = -92;
pub const EPROTONOSUPPORT: i64 = -93;
pub const ESOCKTNOSUPPORT: i64 = -94;
pub const EOPNOTSUPP: i64 = -95;
pub const EPFNOSUPPORT: i64 = -96;
pub const EAFNOSUPPORT: i64 = -97;
pub const EADDRINUSE: i64 = -98;
pub const EADDRNOTAVAIL: i64 = -99;
pub const ENETDOWN: i64 = -100;
pub const ENETUNREACH: i64 = -101;
pub const ENETRESET: i64 = -102;
pub const ECONNABORTED: i64 = -103;
pub const ECONNRESET: i64 = -104;
pub const ENOBUFS: i64 = -105;
pub const EISCONN: i64 = -106;
pub const ENOTCONN: i64 = -107;
pub const ESHUTDOWN: i64 = -108;
pub const ETOOMANYREFS: i64 = -109;
pub const ETIMEDOUT: i64 = -110;
pub const ECONNREFUSED: i64 = -111;
pub const EHOSTDOWN: i64 = -112;
pub const EHOSTUNREACH: i64 = -113;
pub const EALREADY: i64 = -114;
pub const EINPROGRESS: i64 = -115;
pub const ESTALE: i64 = -116;
pub const EUCLEAN: i64 = -117;
pub const ENOTNAM: i64 = -118;
pub const ENAVAIL: i64 = -119;
pub const EISNAM: i64 = -120;
pub const EREMOTEIO: i64 = -121;
pub const EDQUOT: i64 = -122;
pub const ENOMEDIUM: i64 = -123;
pub const EMEDIUMTYPE: i64 = -124;
pub const ECANCELED: i64 = -125;
pub const ENOKEY: i64 = -126;
pub const EKEYEXPIRED: i64 = -127;
pub const EKEYREVOKED: i64 = -128;
pub const EKEYREJECTED: i64 = -129;
pub const EOWNERDEAD: i64 = -130;
pub const ENOTRECOVERABLE: i64 = -131;
pub const ERFKILL: i64 = -132;
pub const EHWPOISON: i64 = -133;

// ─── Syscall Structures ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct SyscallArgs {
    pub arg0: u64,
    pub arg1: u64,
    pub arg2: u64,
    pub arg3: u64,
    pub arg4: u64,
    pub arg5: u64,
}

impl SyscallArgs {
    pub fn new(a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> Self {
        Self {
            arg0: a0,
            arg1: a1,
            arg2: a2,
            arg3: a3,
            arg4: a4,
            arg5: a5,
        }
    }

    pub fn empty() -> Self {
        Self {
            arg0: 0,
            arg1: 0,
            arg2: 0,
            arg3: 0,
            arg4: 0,
            arg5: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SyscallFlags {
    pub needs_fd_table: bool,
    pub needs_mm: bool,
    pub may_block: bool,
    pub restartable: bool,
}

impl SyscallFlags {
    pub const fn none() -> Self {
        Self {
            needs_fd_table: false,
            needs_mm: false,
            may_block: false,
            restartable: false,
        }
    }

    pub const fn file_io() -> Self {
        Self {
            needs_fd_table: true,
            needs_mm: false,
            may_block: true,
            restartable: true,
        }
    }

    pub const fn memory() -> Self {
        Self {
            needs_fd_table: false,
            needs_mm: true,
            may_block: false,
            restartable: false,
        }
    }

    pub const fn process() -> Self {
        Self {
            needs_fd_table: false,
            needs_mm: false,
            may_block: true,
            restartable: false,
        }
    }

    pub const fn network() -> Self {
        Self {
            needs_fd_table: true,
            needs_mm: false,
            may_block: true,
            restartable: true,
        }
    }

    pub const fn signal() -> Self {
        Self {
            needs_fd_table: false,
            needs_mm: false,
            may_block: false,
            restartable: false,
        }
    }
}

#[derive(Clone)]
pub struct SyscallEntry {
    pub number: u64,
    pub name: &'static str,
    pub handler: fn(SyscallArgs) -> i64,
    pub arg_count: u8,
    pub flags: SyscallFlags,
}

// ─── Syscall Table ───────────────────────────────────────────────────────────

pub struct SyscallTable {
    handlers: BTreeMap<u64, SyscallEntry>,
    total_calls: u64,
    per_syscall_count: BTreeMap<u64, u64>,
    last_error: Option<(u64, i64)>,
}

pub static SYSCALL_TABLE: Mutex<Option<SyscallTable>> = Mutex::new(None);

impl SyscallTable {
    pub fn new() -> Self {
        let mut table = Self {
            handlers: BTreeMap::new(),
            total_calls: 0,
            per_syscall_count: BTreeMap::new(),
            last_error: None,
        };
        table.register_defaults();
        table
    }

    pub fn register(
        &mut self,
        number: u64,
        name: &'static str,
        handler: fn(SyscallArgs) -> i64,
        arg_count: u8,
        flags: SyscallFlags,
    ) {
        self.handlers.insert(
            number,
            SyscallEntry {
                number,
                name,
                handler,
                arg_count,
                flags,
            },
        );
    }

    pub fn dispatch(&mut self, number: u64, args: SyscallArgs) -> i64 {
        self.total_calls += 1;
        *self.per_syscall_count.entry(number).or_insert(0) += 1;

        if let Some(entry) = self.handlers.get(&number) {
            let handler = entry.handler;
            let result = handler(args);
            if result < 0 {
                self.last_error = Some((number, result));
            }
            result
        } else {
            self.last_error = Some((number, ENOSYS));
            ENOSYS
        }
    }

    pub fn stats(&self) -> &BTreeMap<u64, u64> {
        &self.per_syscall_count
    }

    pub fn total_calls(&self) -> u64 {
        self.total_calls
    }

    pub fn last_error(&self) -> Option<(u64, i64)> {
        self.last_error
    }

    pub fn syscall_name(&self, number: u64) -> Option<&str> {
        self.handlers.get(&number).map(|e| e.name)
    }

    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    fn register_defaults(&mut self) {
        // File I/O syscalls
        self.register(SYS_READ, "read", sys_read, 3, SyscallFlags::file_io());
        self.register(SYS_WRITE, "write", sys_write, 3, SyscallFlags::file_io());
        self.register(SYS_OPEN, "open", sys_open, 3, SyscallFlags::file_io());
        self.register(SYS_CLOSE, "close", sys_close, 1, SyscallFlags::file_io());
        self.register(SYS_STAT, "stat", sys_stat, 2, SyscallFlags::file_io());
        self.register(SYS_FSTAT, "fstat", sys_fstat, 2, SyscallFlags::file_io());
        self.register(SYS_LSTAT, "lstat", sys_lstat, 2, SyscallFlags::file_io());
        self.register(SYS_POLL, "poll", sys_poll, 3, SyscallFlags::file_io());
        self.register(SYS_LSEEK, "lseek", sys_lseek, 3, SyscallFlags::file_io());
        self.register(SYS_IOCTL, "ioctl", sys_ioctl, 3, SyscallFlags::file_io());
        self.register(
            SYS_PREAD64,
            "pread64",
            sys_pread64,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_PWRITE64,
            "pwrite64",
            sys_pwrite64,
            4,
            SyscallFlags::file_io(),
        );
        self.register(SYS_READV, "readv", sys_readv, 3, SyscallFlags::file_io());
        self.register(SYS_WRITEV, "writev", sys_writev, 3, SyscallFlags::file_io());
        self.register(SYS_ACCESS, "access", sys_access, 2, SyscallFlags::file_io());
        self.register(SYS_PIPE, "pipe", sys_pipe, 1, SyscallFlags::file_io());
        self.register(SYS_SELECT, "select", sys_select, 5, SyscallFlags::file_io());
        self.register(SYS_DUP, "dup", sys_dup, 1, SyscallFlags::file_io());
        self.register(SYS_DUP2, "dup2", sys_dup2, 2, SyscallFlags::file_io());
        self.register(
            SYS_SENDFILE,
            "sendfile",
            sys_sendfile,
            4,
            SyscallFlags::file_io(),
        );
        self.register(SYS_FCNTL, "fcntl", sys_fcntl, 3, SyscallFlags::file_io());
        self.register(SYS_FLOCK, "flock", sys_flock, 2, SyscallFlags::file_io());
        self.register(SYS_FSYNC, "fsync", sys_fsync, 1, SyscallFlags::file_io());
        self.register(
            SYS_FDATASYNC,
            "fdatasync",
            sys_fdatasync,
            1,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_TRUNCATE,
            "truncate",
            sys_truncate,
            2,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_FTRUNCATE,
            "ftruncate",
            sys_ftruncate,
            2,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_GETDENTS,
            "getdents",
            sys_getdents,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_GETDENTS64,
            "getdents64",
            sys_getdents64,
            3,
            SyscallFlags::file_io(),
        );
        self.register(SYS_GETCWD, "getcwd", sys_getcwd, 2, SyscallFlags::file_io());
        self.register(SYS_CHDIR, "chdir", sys_chdir, 1, SyscallFlags::file_io());
        self.register(SYS_FCHDIR, "fchdir", sys_fchdir, 1, SyscallFlags::file_io());
        self.register(SYS_RENAME, "rename", sys_rename, 2, SyscallFlags::file_io());
        self.register(SYS_MKDIR, "mkdir", sys_mkdir, 2, SyscallFlags::file_io());
        self.register(SYS_RMDIR, "rmdir", sys_rmdir, 1, SyscallFlags::file_io());
        self.register(SYS_CREAT, "creat", sys_creat, 2, SyscallFlags::file_io());
        self.register(SYS_LINK, "link", sys_link, 2, SyscallFlags::file_io());
        self.register(SYS_UNLINK, "unlink", sys_unlink, 1, SyscallFlags::file_io());
        self.register(
            SYS_SYMLINK,
            "symlink",
            sys_symlink,
            2,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_READLINK,
            "readlink",
            sys_readlink,
            3,
            SyscallFlags::file_io(),
        );
        self.register(SYS_CHMOD, "chmod", sys_chmod, 2, SyscallFlags::file_io());
        self.register(SYS_FCHMOD, "fchmod", sys_fchmod, 2, SyscallFlags::file_io());
        self.register(SYS_CHOWN, "chown", sys_chown, 3, SyscallFlags::file_io());
        self.register(SYS_FCHOWN, "fchown", sys_fchown, 3, SyscallFlags::file_io());
        self.register(SYS_LCHOWN, "lchown", sys_lchown, 3, SyscallFlags::file_io());
        self.register(SYS_UMASK, "umask", sys_umask, 1, SyscallFlags::none());

        // Memory management syscalls
        self.register(SYS_MMAP, "mmap", sys_mmap, 6, SyscallFlags::memory());
        self.register(
            SYS_MPROTECT,
            "mprotect",
            sys_mprotect,
            3,
            SyscallFlags::memory(),
        );
        self.register(SYS_MUNMAP, "munmap", sys_munmap, 2, SyscallFlags::memory());
        self.register(SYS_BRK, "brk", sys_brk, 1, SyscallFlags::memory());
        self.register(SYS_MREMAP, "mremap", sys_mremap, 5, SyscallFlags::memory());
        self.register(SYS_MSYNC, "msync", sys_msync, 3, SyscallFlags::memory());
        self.register(
            SYS_MINCORE,
            "mincore",
            sys_mincore,
            3,
            SyscallFlags::memory(),
        );
        self.register(
            SYS_MADVISE,
            "madvise",
            sys_madvise,
            3,
            SyscallFlags::memory(),
        );
        self.register(SYS_MLOCK, "mlock", sys_mlock, 2, SyscallFlags::memory());
        self.register(
            SYS_MUNLOCK,
            "munlock",
            sys_munlock,
            2,
            SyscallFlags::memory(),
        );
        self.register(
            SYS_MLOCKALL,
            "mlockall",
            sys_mlockall,
            1,
            SyscallFlags::memory(),
        );
        self.register(
            SYS_MUNLOCKALL,
            "munlockall",
            sys_munlockall,
            0,
            SyscallFlags::memory(),
        );
        self.register(SYS_MLOCK2, "mlock2", sys_mlock2, 3, SyscallFlags::memory());

        // Process management syscalls
        self.register(SYS_CLONE, "clone", sys_clone, 5, SyscallFlags::process());
        self.register(SYS_FORK, "fork", sys_fork, 0, SyscallFlags::process());
        self.register(SYS_VFORK, "vfork", sys_vfork, 0, SyscallFlags::process());
        self.register(SYS_EXECVE, "execve", sys_execve, 3, SyscallFlags::process());
        self.register(SYS_EXIT, "exit", sys_exit, 1, SyscallFlags::none());
        self.register(
            SYS_EXIT_GROUP,
            "exit_group",
            sys_exit_group,
            1,
            SyscallFlags::none(),
        );
        self.register(SYS_WAIT4, "wait4", sys_wait4, 4, SyscallFlags::process());
        self.register(SYS_WAITID, "waitid", sys_waitid, 5, SyscallFlags::process());
        self.register(SYS_GETPID, "getpid", sys_getpid, 0, SyscallFlags::none());
        self.register(SYS_GETPPID, "getppid", sys_getppid, 0, SyscallFlags::none());
        self.register(SYS_GETTID, "gettid", sys_gettid, 0, SyscallFlags::none());
        self.register(SYS_KILL, "kill", sys_kill, 2, SyscallFlags::signal());
        self.register(SYS_TKILL, "tkill", sys_tkill, 2, SyscallFlags::signal());
        self.register(SYS_TGKILL, "tgkill", sys_tgkill, 3, SyscallFlags::signal());
        self.register(SYS_GETUID, "getuid", sys_getuid, 0, SyscallFlags::none());
        self.register(SYS_GETGID, "getgid", sys_getgid, 0, SyscallFlags::none());
        self.register(SYS_GETEUID, "geteuid", sys_geteuid, 0, SyscallFlags::none());
        self.register(SYS_GETEGID, "getegid", sys_getegid, 0, SyscallFlags::none());
        self.register(SYS_SETUID, "setuid", sys_setuid, 1, SyscallFlags::none());
        self.register(SYS_SETGID, "setgid", sys_setgid, 1, SyscallFlags::none());
        self.register(SYS_SETPGID, "setpgid", sys_setpgid, 2, SyscallFlags::none());
        self.register(SYS_GETPGRP, "getpgrp", sys_getpgrp, 0, SyscallFlags::none());
        self.register(SYS_GETPGID, "getpgid", sys_getpgid, 1, SyscallFlags::none());
        self.register(SYS_SETSID, "setsid", sys_setsid, 0, SyscallFlags::none());
        self.register(SYS_GETSID, "getsid", sys_getsid, 1, SyscallFlags::none());
        self.register(SYS_PRCTL, "prctl", sys_prctl, 5, SyscallFlags::none());
        self.register(
            SYS_ARCH_PRCTL,
            "arch_prctl",
            sys_arch_prctl,
            2,
            SyscallFlags::none(),
        );
        self.register(SYS_CLONE3, "clone3", sys_clone3, 2, SyscallFlags::process());

        // Signal syscalls
        self.register(
            SYS_RT_SIGACTION,
            "rt_sigaction",
            sys_rt_sigaction,
            4,
            SyscallFlags::signal(),
        );
        self.register(
            SYS_RT_SIGPROCMASK,
            "rt_sigprocmask",
            sys_rt_sigprocmask,
            4,
            SyscallFlags::signal(),
        );
        self.register(
            SYS_RT_SIGRETURN,
            "rt_sigreturn",
            sys_rt_sigreturn,
            0,
            SyscallFlags::signal(),
        );
        self.register(
            SYS_RT_SIGPENDING,
            "rt_sigpending",
            sys_rt_sigpending,
            2,
            SyscallFlags::signal(),
        );
        self.register(
            SYS_RT_SIGTIMEDWAIT,
            "rt_sigtimedwait",
            sys_rt_sigtimedwait,
            4,
            SyscallFlags::signal(),
        );
        self.register(
            SYS_RT_SIGQUEUEINFO,
            "rt_sigqueueinfo",
            sys_rt_sigqueueinfo,
            3,
            SyscallFlags::signal(),
        );
        self.register(
            SYS_RT_SIGSUSPEND,
            "rt_sigsuspend",
            sys_rt_sigsuspend,
            2,
            SyscallFlags::signal(),
        );
        self.register(
            SYS_SIGALTSTACK,
            "sigaltstack",
            sys_sigaltstack,
            2,
            SyscallFlags::signal(),
        );
        self.register(SYS_PAUSE, "pause", sys_pause, 0, SyscallFlags::signal());

        // IPC syscalls
        self.register(SYS_SHMGET, "shmget", sys_shmget, 3, SyscallFlags::none());
        self.register(SYS_SHMAT, "shmat", sys_shmat, 3, SyscallFlags::none());
        self.register(SYS_SHMCTL, "shmctl", sys_shmctl, 3, SyscallFlags::none());
        self.register(SYS_SHMDT, "shmdt", sys_shmdt, 1, SyscallFlags::none());
        self.register(SYS_SEMGET, "semget", sys_semget, 3, SyscallFlags::none());
        self.register(SYS_SEMOP, "semop", sys_semop, 3, SyscallFlags::none());
        self.register(SYS_SEMCTL, "semctl", sys_semctl, 4, SyscallFlags::none());
        self.register(SYS_MSGGET, "msgget", sys_msgget, 2, SyscallFlags::none());
        self.register(SYS_MSGSND, "msgsnd", sys_msgsnd, 4, SyscallFlags::none());
        self.register(SYS_MSGRCV, "msgrcv", sys_msgrcv, 5, SyscallFlags::none());
        self.register(SYS_MSGCTL, "msgctl", sys_msgctl, 3, SyscallFlags::none());

        // Network syscalls
        self.register(SYS_SOCKET, "socket", sys_socket, 3, SyscallFlags::network());
        self.register(
            SYS_CONNECT,
            "connect",
            sys_connect,
            3,
            SyscallFlags::network(),
        );
        self.register(SYS_ACCEPT, "accept", sys_accept, 3, SyscallFlags::network());
        self.register(SYS_SENDTO, "sendto", sys_sendto, 6, SyscallFlags::network());
        self.register(
            SYS_RECVFROM,
            "recvfrom",
            sys_recvfrom,
            6,
            SyscallFlags::network(),
        );
        self.register(
            SYS_SENDMSG,
            "sendmsg",
            sys_sendmsg,
            3,
            SyscallFlags::network(),
        );
        self.register(
            SYS_RECVMSG,
            "recvmsg",
            sys_recvmsg,
            3,
            SyscallFlags::network(),
        );
        self.register(
            SYS_SHUTDOWN,
            "shutdown",
            sys_shutdown,
            2,
            SyscallFlags::network(),
        );
        self.register(SYS_BIND, "bind", sys_bind, 3, SyscallFlags::network());
        self.register(SYS_LISTEN, "listen", sys_listen, 2, SyscallFlags::network());
        self.register(
            SYS_GETSOCKNAME,
            "getsockname",
            sys_getsockname,
            3,
            SyscallFlags::network(),
        );
        self.register(
            SYS_GETPEERNAME,
            "getpeername",
            sys_getpeername,
            3,
            SyscallFlags::network(),
        );
        self.register(
            SYS_SOCKETPAIR,
            "socketpair",
            sys_socketpair,
            4,
            SyscallFlags::network(),
        );
        self.register(
            SYS_SETSOCKOPT,
            "setsockopt",
            sys_setsockopt,
            5,
            SyscallFlags::network(),
        );
        self.register(
            SYS_GETSOCKOPT,
            "getsockopt",
            sys_getsockopt,
            5,
            SyscallFlags::network(),
        );
        self.register(
            SYS_ACCEPT4,
            "accept4",
            sys_accept4,
            4,
            SyscallFlags::network(),
        );
        self.register(
            SYS_RECVMMSG,
            "recvmmsg",
            sys_recvmmsg,
            5,
            SyscallFlags::network(),
        );
        self.register(
            SYS_SENDMMSG,
            "sendmmsg",
            sys_sendmmsg,
            4,
            SyscallFlags::network(),
        );

        // Timer syscalls
        self.register(
            SYS_NANOSLEEP,
            "nanosleep",
            sys_nanosleep,
            2,
            SyscallFlags::process(),
        );
        self.register(
            SYS_GETITIMER,
            "getitimer",
            sys_getitimer,
            2,
            SyscallFlags::none(),
        );
        self.register(SYS_ALARM, "alarm", sys_alarm, 1, SyscallFlags::none());
        self.register(
            SYS_SETITIMER,
            "setitimer",
            sys_setitimer,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_TIMER_CREATE,
            "timer_create",
            sys_timer_create,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_TIMER_SETTIME,
            "timer_settime",
            sys_timer_settime,
            4,
            SyscallFlags::none(),
        );
        self.register(
            SYS_TIMER_GETTIME,
            "timer_gettime",
            sys_timer_gettime,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_TIMER_GETOVERRUN,
            "timer_getoverrun",
            sys_timer_getoverrun,
            1,
            SyscallFlags::none(),
        );
        self.register(
            SYS_TIMER_DELETE,
            "timer_delete",
            sys_timer_delete,
            1,
            SyscallFlags::none(),
        );
        self.register(
            SYS_CLOCK_SETTIME,
            "clock_settime",
            sys_clock_settime,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_CLOCK_GETTIME,
            "clock_gettime",
            sys_clock_gettime,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_CLOCK_GETRES,
            "clock_getres",
            sys_clock_getres,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_CLOCK_NANOSLEEP,
            "clock_nanosleep",
            sys_clock_nanosleep,
            4,
            SyscallFlags::process(),
        );
        self.register(
            SYS_GETTIMEOFDAY,
            "gettimeofday",
            sys_gettimeofday,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SETTIMEOFDAY,
            "settimeofday",
            sys_settimeofday,
            2,
            SyscallFlags::none(),
        );
        self.register(SYS_TIME, "time", sys_time, 1, SyscallFlags::none());
        self.register(
            SYS_ADJTIMEX,
            "adjtimex",
            sys_adjtimex,
            1,
            SyscallFlags::none(),
        );

        // Scheduler syscalls
        self.register(
            SYS_SCHED_YIELD,
            "sched_yield",
            sys_sched_yield,
            0,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_SETPARAM,
            "sched_setparam",
            sys_sched_setparam,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_GETPARAM,
            "sched_getparam",
            sys_sched_getparam,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_SETSCHEDULER,
            "sched_setscheduler",
            sys_sched_setscheduler,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_GETSCHEDULER,
            "sched_getscheduler",
            sys_sched_getscheduler,
            1,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_GET_PRIORITY_MAX,
            "sched_get_priority_max",
            sys_sched_get_priority_max,
            1,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_GET_PRIORITY_MIN,
            "sched_get_priority_min",
            sys_sched_get_priority_min,
            1,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_RR_GET_INTERVAL,
            "sched_rr_get_interval",
            sys_sched_rr_get_interval,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_SETAFFINITY,
            "sched_setaffinity",
            sys_sched_setaffinity,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_GETAFFINITY,
            "sched_getaffinity",
            sys_sched_getaffinity,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_SETATTR,
            "sched_setattr",
            sys_sched_setattr,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SCHED_GETATTR,
            "sched_getattr",
            sys_sched_getattr,
            4,
            SyscallFlags::none(),
        );

        // Filesystem syscalls
        self.register(SYS_STATFS, "statfs", sys_statfs, 2, SyscallFlags::file_io());
        self.register(
            SYS_FSTATFS,
            "fstatfs",
            sys_fstatfs,
            2,
            SyscallFlags::file_io(),
        );
        self.register(SYS_UTIME, "utime", sys_utime, 2, SyscallFlags::file_io());
        self.register(SYS_MKNOD, "mknod", sys_mknod, 3, SyscallFlags::file_io());
        self.register(
            SYS_PIVOT_ROOT,
            "pivot_root",
            sys_pivot_root,
            2,
            SyscallFlags::file_io(),
        );
        self.register(SYS_CHROOT, "chroot", sys_chroot, 1, SyscallFlags::file_io());
        self.register(SYS_SYNC, "sync", sys_sync, 0, SyscallFlags::file_io());
        self.register(SYS_MOUNT, "mount", sys_mount, 5, SyscallFlags::file_io());
        self.register(
            SYS_UMOUNT2,
            "umount2",
            sys_umount2,
            2,
            SyscallFlags::file_io(),
        );
        self.register(SYS_SWAPON, "swapon", sys_swapon, 2, SyscallFlags::file_io());
        self.register(
            SYS_SWAPOFF,
            "swapoff",
            sys_swapoff,
            1,
            SyscallFlags::file_io(),
        );
        self.register(SYS_OPENAT, "openat", sys_openat, 4, SyscallFlags::file_io());
        self.register(
            SYS_MKDIRAT,
            "mkdirat",
            sys_mkdirat,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_MKNODAT,
            "mknodat",
            sys_mknodat,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_FCHOWNAT,
            "fchownat",
            sys_fchownat,
            5,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_UNLINKAT,
            "unlinkat",
            sys_unlinkat,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_RENAMEAT,
            "renameat",
            sys_renameat,
            4,
            SyscallFlags::file_io(),
        );
        self.register(SYS_LINKAT, "linkat", sys_linkat, 5, SyscallFlags::file_io());
        self.register(
            SYS_SYMLINKAT,
            "symlinkat",
            sys_symlinkat,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_READLINKAT,
            "readlinkat",
            sys_readlinkat,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_FCHMODAT,
            "fchmodat",
            sys_fchmodat,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_FACCESSAT,
            "faccessat",
            sys_faccessat,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_RENAMEAT2,
            "renameat2",
            sys_renameat2,
            5,
            SyscallFlags::file_io(),
        );
        self.register(SYS_STATX, "statx", sys_statx, 5, SyscallFlags::file_io());
        self.register(SYS_SYNCFS, "syncfs", sys_syncfs, 1, SyscallFlags::file_io());
        self.register(
            SYS_OPENAT2,
            "openat2",
            sys_openat2,
            4,
            SyscallFlags::file_io(),
        );

        // Extended attributes
        self.register(
            SYS_SETXATTR,
            "setxattr",
            sys_setxattr,
            5,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_LSETXATTR,
            "lsetxattr",
            sys_lsetxattr,
            5,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_FSETXATTR,
            "fsetxattr",
            sys_fsetxattr,
            5,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_GETXATTR,
            "getxattr",
            sys_getxattr,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_LGETXATTR,
            "lgetxattr",
            sys_lgetxattr,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_FGETXATTR,
            "fgetxattr",
            sys_fgetxattr,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_LISTXATTR,
            "listxattr",
            sys_listxattr,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_LLISTXATTR,
            "llistxattr",
            sys_llistxattr,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_FLISTXATTR,
            "flistxattr",
            sys_flistxattr,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_REMOVEXATTR,
            "removexattr",
            sys_removexattr,
            2,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_LREMOVEXATTR,
            "lremovexattr",
            sys_lremovexattr,
            2,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_FREMOVEXATTR,
            "fremovexattr",
            sys_fremovexattr,
            2,
            SyscallFlags::file_io(),
        );

        // Epoll syscalls
        self.register(
            SYS_EPOLL_CREATE,
            "epoll_create",
            sys_epoll_create,
            1,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_EPOLL_WAIT,
            "epoll_wait",
            sys_epoll_wait,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_EPOLL_CTL,
            "epoll_ctl",
            sys_epoll_ctl,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_EPOLL_PWAIT,
            "epoll_pwait",
            sys_epoll_pwait,
            5,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_EPOLL_CREATE1,
            "epoll_create1",
            sys_epoll_create1,
            1,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_EPOLL_PWAIT2,
            "epoll_pwait2",
            sys_epoll_pwait2,
            6,
            SyscallFlags::file_io(),
        );

        // Inotify syscalls
        self.register(
            SYS_INOTIFY_INIT,
            "inotify_init",
            sys_inotify_init,
            0,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_INOTIFY_ADD_WATCH,
            "inotify_add_watch",
            sys_inotify_add_watch,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_INOTIFY_RM_WATCH,
            "inotify_rm_watch",
            sys_inotify_rm_watch,
            2,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_INOTIFY_INIT1,
            "inotify_init1",
            sys_inotify_init1,
            1,
            SyscallFlags::file_io(),
        );

        // io_uring syscalls
        self.register(
            SYS_IO_URING_SETUP,
            "io_uring_setup",
            sys_io_uring_setup,
            2,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_IO_URING_ENTER,
            "io_uring_enter",
            sys_io_uring_enter,
            6,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_IO_URING_REGISTER,
            "io_uring_register",
            sys_io_uring_register,
            4,
            SyscallFlags::file_io(),
        );

        // Misc syscalls
        self.register(SYS_UNAME, "uname", sys_uname, 1, SyscallFlags::none());
        self.register(SYS_SYSINFO, "sysinfo", sys_sysinfo, 1, SyscallFlags::none());
        self.register(
            SYS_GETRLIMIT,
            "getrlimit",
            sys_getrlimit,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_SETRLIMIT,
            "setrlimit",
            sys_setrlimit,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_PRLIMIT64,
            "prlimit64",
            sys_prlimit64,
            4,
            SyscallFlags::none(),
        );
        self.register(
            SYS_GETRUSAGE,
            "getrusage",
            sys_getrusage,
            2,
            SyscallFlags::none(),
        );
        self.register(SYS_PTRACE, "ptrace", sys_ptrace, 4, SyscallFlags::process());
        self.register(SYS_REBOOT, "reboot", sys_reboot, 4, SyscallFlags::none());
        self.register(SYS_SYSLOG, "syslog", sys_syslog, 3, SyscallFlags::none());
        self.register(
            SYS_GETRANDOM,
            "getrandom",
            sys_getrandom,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_MEMFD_CREATE,
            "memfd_create",
            sys_memfd_create,
            2,
            SyscallFlags::none(),
        );
        self.register(SYS_SECCOMP, "seccomp", sys_seccomp, 3, SyscallFlags::none());
        self.register(SYS_BPF, "bpf", sys_bpf, 3, SyscallFlags::none());
        self.register(SYS_FUTEX, "futex", sys_futex, 6, SyscallFlags::process());
        self.register(
            SYS_SET_TID_ADDRESS,
            "set_tid_address",
            sys_set_tid_address,
            1,
            SyscallFlags::none(),
        );
        self.register(SYS_GETCPU, "getcpu", sys_getcpu, 3, SyscallFlags::none());
        self.register(SYS_PIPE2, "pipe2", sys_pipe2, 2, SyscallFlags::file_io());
        self.register(SYS_DUP3, "dup3", sys_dup3, 3, SyscallFlags::file_io());
        self.register(
            SYS_FALLOCATE,
            "fallocate",
            sys_fallocate,
            4,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_COPY_FILE_RANGE,
            "copy_file_range",
            sys_copy_file_range,
            6,
            SyscallFlags::file_io(),
        );
        self.register(SYS_SPLICE, "splice", sys_splice, 6, SyscallFlags::file_io());
        self.register(SYS_TEE, "tee", sys_tee, 4, SyscallFlags::file_io());
        self.register(
            SYS_VMSPLICE,
            "vmsplice",
            sys_vmsplice,
            4,
            SyscallFlags::file_io(),
        );

        // Module syscalls
        self.register(
            SYS_INIT_MODULE,
            "init_module",
            sys_init_module,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_DELETE_MODULE,
            "delete_module",
            sys_delete_module,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_FINIT_MODULE,
            "finit_module",
            sys_finit_module,
            3,
            SyscallFlags::none(),
        );

        // Namespace / container syscalls
        self.register(
            SYS_UNSHARE,
            "unshare",
            sys_unshare,
            1,
            SyscallFlags::process(),
        );
        self.register(SYS_SETNS, "setns", sys_setns, 2, SyscallFlags::process());

        // Misc newer syscalls
        self.register(
            SYS_CLOSE_RANGE,
            "close_range",
            sys_close_range,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_PIDFD_OPEN,
            "pidfd_open",
            sys_pidfd_open,
            2,
            SyscallFlags::none(),
        );
        self.register(
            SYS_PIDFD_SEND_SIGNAL,
            "pidfd_send_signal",
            sys_pidfd_send_signal,
            4,
            SyscallFlags::signal(),
        );
        self.register(
            SYS_PIDFD_GETFD,
            "pidfd_getfd",
            sys_pidfd_getfd,
            3,
            SyscallFlags::file_io(),
        );
        self.register(
            SYS_LANDLOCK_CREATE_RULESET,
            "landlock_create_ruleset",
            sys_landlock_create_ruleset,
            3,
            SyscallFlags::none(),
        );
        self.register(
            SYS_LANDLOCK_ADD_RULE,
            "landlock_add_rule",
            sys_landlock_add_rule,
            4,
            SyscallFlags::none(),
        );
        self.register(
            SYS_LANDLOCK_RESTRICT_SELF,
            "landlock_restrict_self",
            sys_landlock_restrict_self,
            2,
            SyscallFlags::none(),
        );
    }
}

// ─── Stub Handlers: File I/O ─────────────────────────────────────────────────

fn sys_read(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_write(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_open(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_close(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_stat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fstat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_lstat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_poll(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_lseek(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_ioctl(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pread64(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pwrite64(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_readv(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_writev(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_access(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pipe(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_select(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_dup(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_dup2(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sendfile(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fcntl(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_flock(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fsync(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fdatasync(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_truncate(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_ftruncate(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getdents(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getdents64(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getcwd(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_chdir(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fchdir(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_rename(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mkdir(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_rmdir(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_creat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_link(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_unlink(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_symlink(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_readlink(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_chmod(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fchmod(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_chown(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fchown(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_lchown(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_umask(_args: SyscallArgs) -> i64 {
    0o022
}

// ─── Stub Handlers: Memory Management ───────────────────────────────────────

fn sys_mmap(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mprotect(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_munmap(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_brk(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mremap(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_msync(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mincore(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_madvise(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mlock(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_munlock(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mlockall(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_munlockall(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mlock2(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Process Management ──────────────────────────────────────

fn sys_clone(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fork(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_vfork(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_execve(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_exit(_args: SyscallArgs) -> i64 {
    0
}
fn sys_exit_group(_args: SyscallArgs) -> i64 {
    0
}
fn sys_wait4(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_waitid(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getpid(_args: SyscallArgs) -> i64 {
    1
}
fn sys_getppid(_args: SyscallArgs) -> i64 {
    0
}
fn sys_gettid(_args: SyscallArgs) -> i64 {
    1
}
fn sys_kill(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_tkill(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_tgkill(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getuid(_args: SyscallArgs) -> i64 {
    0
}
fn sys_getgid(_args: SyscallArgs) -> i64 {
    0
}
fn sys_geteuid(_args: SyscallArgs) -> i64 {
    0
}
fn sys_getegid(_args: SyscallArgs) -> i64 {
    0
}
fn sys_setuid(_args: SyscallArgs) -> i64 {
    0
}
fn sys_setgid(_args: SyscallArgs) -> i64 {
    0
}
fn sys_setpgid(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getpgrp(_args: SyscallArgs) -> i64 {
    0
}
fn sys_getpgid(_args: SyscallArgs) -> i64 {
    0
}
fn sys_setsid(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getsid(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_prctl(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_arch_prctl(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_clone3(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Signals ─────────────────────────────────────────────────

fn sys_rt_sigaction(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_rt_sigprocmask(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_rt_sigreturn(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_rt_sigpending(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_rt_sigtimedwait(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_rt_sigqueueinfo(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_rt_sigsuspend(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sigaltstack(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pause(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: IPC ─────────────────────────────────────────────────────

fn sys_shmget(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_shmat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_shmctl(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_shmdt(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_semget(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_semop(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_semctl(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_msgget(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_msgsnd(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_msgrcv(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_msgctl(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Network ─────────────────────────────────────────────────

fn sys_socket(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_connect(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_accept(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sendto(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_recvfrom(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sendmsg(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_recvmsg(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_shutdown(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_bind(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_listen(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getsockname(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getpeername(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_socketpair(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_setsockopt(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getsockopt(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_accept4(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_recvmmsg(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sendmmsg(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Timers ──────────────────────────────────────────────────

fn sys_nanosleep(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getitimer(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_alarm(_args: SyscallArgs) -> i64 {
    0
}
fn sys_setitimer(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_timer_create(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_timer_settime(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_timer_gettime(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_timer_getoverrun(_args: SyscallArgs) -> i64 {
    0
}
fn sys_timer_delete(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_clock_settime(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_clock_gettime(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_clock_getres(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_clock_nanosleep(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_gettimeofday(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_settimeofday(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_time(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_adjtimex(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Scheduler ───────────────────────────────────────────────

fn sys_sched_yield(_args: SyscallArgs) -> i64 {
    0
}
fn sys_sched_setparam(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sched_getparam(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sched_setscheduler(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sched_getscheduler(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sched_get_priority_max(_args: SyscallArgs) -> i64 {
    99
}
fn sys_sched_get_priority_min(_args: SyscallArgs) -> i64 {
    1
}
fn sys_sched_rr_get_interval(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sched_setaffinity(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sched_getaffinity(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sched_setattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sched_getattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Filesystem ──────────────────────────────────────────────

fn sys_statfs(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fstatfs(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_utime(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mknod(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pivot_root(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_chroot(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sync(_args: SyscallArgs) -> i64 {
    0
}
fn sys_mount(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_umount2(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_swapon(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_swapoff(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_openat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mkdirat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_mknodat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fchownat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_unlinkat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_renameat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_linkat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_symlinkat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_readlinkat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fchmodat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_faccessat(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_renameat2(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_statx(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_syncfs(_args: SyscallArgs) -> i64 {
    0
}
fn sys_openat2(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Extended Attributes ─────────────────────────────────────

fn sys_setxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_lsetxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fsetxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_lgetxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fgetxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_listxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_llistxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_flistxattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_removexattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_lremovexattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fremovexattr(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Epoll ───────────────────────────────────────────────────

fn sys_epoll_create(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_epoll_wait(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_epoll_ctl(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_epoll_pwait(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_epoll_create1(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_epoll_pwait2(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Inotify ─────────────────────────────────────────────────

fn sys_inotify_init(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_inotify_add_watch(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_inotify_rm_watch(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_inotify_init1(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: io_uring ────────────────────────────────────────────────

fn sys_io_uring_setup(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_io_uring_enter(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_io_uring_register(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Stub Handlers: Misc ────────────────────────────────────────────────────

fn sys_uname(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_sysinfo(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getrlimit(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_setrlimit(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_prlimit64(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getrusage(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_ptrace(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_reboot(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_syslog(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_getrandom(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_memfd_create(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_seccomp(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_bpf(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_futex(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_set_tid_address(_args: SyscallArgs) -> i64 {
    1
}
fn sys_getcpu(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pipe2(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_dup3(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_fallocate(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_copy_file_range(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_splice(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_tee(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_vmsplice(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_init_module(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_delete_module(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_finit_module(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_unshare(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_setns(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_close_range(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pidfd_open(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pidfd_send_signal(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_pidfd_getfd(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_landlock_create_ruleset(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_landlock_add_rule(_args: SyscallArgs) -> i64 {
    ENOSYS
}
fn sys_landlock_restrict_self(_args: SyscallArgs) -> i64 {
    ENOSYS
}

// ─── Public API ──────────────────────────────────────────────────────────────

pub fn init() {
    let table = SyscallTable::new();
    *SYSCALL_TABLE.lock() = Some(table);
}

pub fn dispatch(number: u64, args: SyscallArgs) -> i64 {
    let mut table = SYSCALL_TABLE.lock();
    if let Some(ref mut t) = *table {
        t.dispatch(number, args)
    } else {
        ENOSYS
    }
}

pub fn syscall_name(number: u64) -> Option<&'static str> {
    let table = SYSCALL_TABLE.lock();
    let name = table.as_ref().and_then(|t| t.syscall_name(number));
    name.map(|n| unsafe { &*(n as *const str) })
}

pub fn total_syscalls() -> u64 {
    let table = SYSCALL_TABLE.lock();
    table.as_ref().map_or(0, |t| t.total_calls())
}

pub fn handler_count() -> usize {
    let table = SYSCALL_TABLE.lock();
    table.as_ref().map_or(0, |t| t.handler_count())
}
