//! Windows API Set schema resolution — the MinWin virtual-DLL layer.
//!
//! Concept alignment (`RaeenOS_Concept.md` §Compatibility): RaeBridge must
//! resolve Windows imports *the way Windows does*, as one integrated runtime —
//! not with per-app hacks. Modern MSVC / UCRT binaries almost never import from
//! `kernel32.dll` directly; they import from **API Set contract DLLs** such as
//! `api-ms-win-core-synch-l1-2-0.dll` or `api-ms-win-crt-runtime-l1-1-0.dll`.
//! The real Windows loader redirects each contract to a physical host DLL
//! (`kernel32`, `kernelbase`, `ucrtbase`, `advapi32`, …) using the schema baked
//! into `apisetschema.dll`. Without that redirection every modern binary's
//! most-imported symbols (QueryPerformanceCounter, GetCurrentProcess, HeapAlloc,
//! the entire CRT startup set) resolve to fail-loud stubs and the program dies
//! at its first call.
//!
//! The exact `api-set -> host` table below was generated from the real
//! forwarder stubs shipped in `C:\Windows\System32\downlevel\` on
//! Windows 10.0.26200 (see `components/raebridge/tools/apiset-map.ps1`), i.e.
//! ground truth from a shipping Windows, not a hand-guessed heuristic. Each
//! real host is then remapped to the RaeBridge module that actually implements
//! those exports:
//!   * `kernelbase` / `kernel32` -> our `kernel32`
//!   * `ucrtbase`  / `vcruntime` -> our `msvcrt`
//!   * `combase`                 -> our `ole32`
//! A version-independent prefix fallback ([`apiset_prefix_fallback`]) covers
//! contracts that ship only as virtual sets (no physical stub) or newer
//! version suffixes not in the captured table, so the resolver degrades to the
//! correct host rather than to nothing.

extern crate alloc;
use alloc::string::String;

/// Lowercase ASCII copy (no_std, no locale games — DLL names are ASCII).
fn to_ascii_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push((c as u8 + 32) as char);
        } else {
            out.push(c);
        }
    }
    out
}

/// Canonicalize a DLL name exactly as it appears in a PE import descriptor to
/// the RaeBridge module DLL name (lowercase, `.dll` suffix) that hosts its
/// exports.
///
/// * API Set contracts (`api-ms-win-*`, `ext-ms-win-*`) are redirected via the
///   schema table + prefix fallback.
/// * Physical host DLLs that alias onto a RaeBridge module (`kernelbase`,
///   `ucrtbase`, `combase`, …) are folded onto that module.
/// * Any other name passes through unchanged (just lowercased).
///
/// Idempotent: canonical module names map to themselves, so calling this twice
/// yields the same result.
pub fn canonical_dll(dll: &str) -> String {
    let lc = to_ascii_lower(dll);
    let stem = lc.strip_suffix(".dll").unwrap_or(lc.as_str());

    let module = if stem.starts_with("api-ms-win-") || stem.starts_with("ext-ms-win-") {
        host_to_module(apiset_host(stem))
    } else {
        host_to_module(stem)
    };

    let mut out = String::with_capacity(module.len() + 4);
    out.push_str(module);
    out.push_str(".dll");
    out
}

/// True if the name is an API Set contract DLL (with or without `.dll`).
pub fn is_api_set(dll: &str) -> bool {
    let lc = to_ascii_lower(dll);
    lc.starts_with("api-ms-win-") || lc.starts_with("ext-ms-win-")
}

/// Fold a physical host DLL name onto the RaeBridge module that implements it.
fn host_to_module(host: &str) -> &str {
    match host {
        "kernelbase" | "kernel32" => "kernel32",
        "ucrtbase" | "ucrtbased" | "msvcrt" | "vcruntime140" | "vcruntime140d" | "vcruntime" => {
            "msvcrt"
        }
        "combase" => "ole32",
        other => other,
    }
}

/// Map an API Set contract stem (no `.dll`) to its real host DLL, from the
/// captured Windows 10.0.26200 forwarder schema. Falls back to a
/// version-independent prefix rule for contracts not in the table.
fn apiset_host(stem: &str) -> &'static str {
    match stem {
        // --- generated from System32\downlevel forwarders (ground truth) ---
        "api-ms-win-base-util-l1-1-0" => "advapi32",
        "api-ms-win-core-com-l1-1-0" => "ole32",
        "api-ms-win-core-comm-l1-1-0" => "kernel32",
        "api-ms-win-core-console-l1-1-0" => "kernel32",
        "api-ms-win-core-datetime-l1-1-0" => "kernel32",
        "api-ms-win-core-datetime-l1-1-1" => "kernel32",
        "api-ms-win-core-debug-l1-1-0" => "kernel32",
        "api-ms-win-core-debug-l1-1-1" => "kernel32",
        "api-ms-win-core-delayload-l1-1-0" => "kernel32",
        "api-ms-win-core-errorhandling-l1-1-0" => "kernel32",
        "api-ms-win-core-errorhandling-l1-1-1" => "kernel32",
        "api-ms-win-core-fibers-l1-1-0" => "kernel32",
        "api-ms-win-core-fibers-l1-1-1" => "kernel32",
        "api-ms-win-core-file-l1-1-0" => "kernel32",
        "api-ms-win-core-file-l1-2-0" => "kernel32",
        "api-ms-win-core-file-l1-2-1" => "kernel32",
        "api-ms-win-core-file-l2-1-0" => "kernel32",
        "api-ms-win-core-file-l2-1-1" => "kernel32",
        "api-ms-win-core-handle-l1-1-0" => "kernel32",
        "api-ms-win-core-heap-l1-1-0" => "kernel32",
        "api-ms-win-core-heap-obsolete-l1-1-0" => "kernel32",
        "api-ms-win-core-interlocked-l1-1-0" => "kernel32",
        "api-ms-win-core-io-l1-1-0" => "kernel32",
        "api-ms-win-core-io-l1-1-1" => "kernel32",
        "api-ms-win-core-kernel32-legacy-l1-1-0" => "kernel32",
        "api-ms-win-core-kernel32-legacy-l1-1-1" => "kernel32",
        "api-ms-win-core-kernel32-private-l1-1-0" => "kernel32",
        "api-ms-win-core-kernel32-private-l1-1-1" => "kernel32",
        "api-ms-win-core-libraryloader-l1-1-0" => "kernel32",
        "api-ms-win-core-libraryloader-l1-1-1" => "kernel32",
        "api-ms-win-core-localization-l1-2-0" => "kernel32",
        "api-ms-win-core-localization-l1-2-1" => "kernel32",
        "api-ms-win-core-localization-obsolete-l1-2-0" => "kernel32",
        "api-ms-win-core-memory-l1-1-0" => "kernel32",
        "api-ms-win-core-memory-l1-1-1" => "kernel32",
        "api-ms-win-core-memory-l1-1-2" => "kernel32",
        "api-ms-win-core-namedpipe-l1-1-0" => "kernel32",
        "api-ms-win-core-privateprofile-l1-1-0" => "kernel32",
        "api-ms-win-core-privateprofile-l1-1-1" => "kernel32",
        "api-ms-win-core-processenvironment-l1-1-0" => "kernel32",
        "api-ms-win-core-processenvironment-l1-2-0" => "kernel32",
        "api-ms-win-core-processthreads-l1-1-0" => "kernel32",
        "api-ms-win-core-processthreads-l1-1-1" => "kernel32",
        "api-ms-win-core-processthreads-l1-1-2" => "kernel32",
        "api-ms-win-core-processtopology-obsolete-l1-1-0" => "kernel32",
        "api-ms-win-core-profile-l1-1-0" => "kernel32",
        "api-ms-win-core-realtime-l1-1-0" => "kernel32",
        "api-ms-win-core-registry-l1-1-0" => "advapi32",
        "api-ms-win-core-registry-l2-1-0" => "advapi32",
        "api-ms-win-core-rtlsupport-l1-1-0" => "ntdll",
        "api-ms-win-core-shlwapi-legacy-l1-1-0" => "shlwapi",
        "api-ms-win-core-shlwapi-obsolete-l1-1-0" => "shlwapi",
        "api-ms-win-core-shutdown-l1-1-0" => "advapi32",
        "api-ms-win-core-stringansi-l1-1-0" => "user32",
        "api-ms-win-core-string-l1-1-0" => "kernel32",
        "api-ms-win-core-string-l2-1-0" => "user32",
        "api-ms-win-core-stringloader-l1-1-1" => "user32",
        "api-ms-win-core-string-obsolete-l1-1-0" => "kernel32",
        "api-ms-win-core-synch-l1-1-0" => "kernel32",
        "api-ms-win-core-synch-l1-2-0" => "kernel32",
        "api-ms-win-core-sysinfo-l1-1-0" => "kernel32",
        "api-ms-win-core-sysinfo-l1-2-0" => "kernel32",
        "api-ms-win-core-sysinfo-l1-2-1" => "kernel32",
        "api-ms-win-core-threadpool-l1-2-0" => "kernel32",
        "api-ms-win-core-threadpool-legacy-l1-1-0" => "kernel32",
        "api-ms-win-core-threadpool-private-l1-1-0" => "kernel32",
        "api-ms-win-core-timezone-l1-1-0" => "kernel32",
        "api-ms-win-core-url-l1-1-0" => "shlwapi",
        "api-ms-win-core-util-l1-1-0" => "kernel32",
        "api-ms-win-core-version-l1-1-0" => "version",
        "api-ms-win-core-wow64-l1-1-0" => "kernel32",
        "api-ms-win-core-xstate-l1-1-0" => "ntdll",
        "api-ms-win-core-xstate-l2-1-0" => "kernel32",
        "api-ms-win-crt-conio-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-convert-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-environment-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-filesystem-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-heap-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-locale-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-math-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-multibyte-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-private-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-process-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-runtime-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-stdio-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-string-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-time-l1-1-0" => "ucrtbase",
        "api-ms-win-crt-utility-l1-1-0" => "ucrtbase",
        "api-ms-win-devices-config-l1-1-0" => "cfgmgr32",
        "api-ms-win-devices-config-l1-1-1" => "cfgmgr32",
        "api-ms-win-eventing-classicprovider-l1-1-0" => "advapi32",
        "api-ms-win-eventing-consumer-l1-1-0" => "advapi32",
        "api-ms-win-eventing-controller-l1-1-0" => "advapi32",
        "api-ms-win-eventing-legacy-l1-1-0" => "advapi32",
        "api-ms-win-eventing-provider-l1-1-0" => "advapi32",
        "api-ms-win-eventlog-legacy-l1-1-0" => "advapi32",
        "api-ms-win-security-base-l1-1-0" => "advapi32",
        "api-ms-win-security-cryptoapi-l1-1-0" => "advapi32",
        "api-ms-win-security-lsalookup-l2-1-0" => "advapi32",
        "api-ms-win-security-lsalookup-l2-1-1" => "advapi32",
        "api-ms-win-security-lsapolicy-l1-1-0" => "advapi32",
        "api-ms-win-security-provider-l1-1-0" => "advapi32",
        "api-ms-win-security-sddl-l1-1-0" => "advapi32",
        "api-ms-win-service-core-l1-1-0" => "advapi32",
        "api-ms-win-service-core-l1-1-1" => "advapi32",
        "api-ms-win-service-management-l1-1-0" => "advapi32",
        "api-ms-win-service-management-l2-1-0" => "advapi32",
        "api-ms-win-service-private-l1-1-0" => "advapi32",
        "api-ms-win-service-private-l1-1-1" => "advapi32",
        "api-ms-win-service-winsvc-l1-1-0" => "advapi32",
        "api-ms-win-shcore-stream-l1-1-0" => "shlwapi",
        // --- not a captured physical stub: version-independent fallback ---
        _ => apiset_prefix_fallback(stem),
    }
}

/// Version-independent fallback for API Set contracts not present as physical
/// forwarder stubs (virtual-only sets, or version suffixes newer than the
/// captured schema). Routes by contract family to the dominant host observed in
/// the ground-truth table.
fn apiset_prefix_fallback(stem: &str) -> &'static str {
    if stem.starts_with("api-ms-win-crt-") {
        return "ucrtbase";
    }
    if stem.starts_with("api-ms-win-core-com-") || stem.starts_with("api-ms-win-core-winrt") {
        return "ole32";
    }
    if stem.starts_with("api-ms-win-core-registry-")
        || stem.starts_with("api-ms-win-security-")
        || stem.starts_with("api-ms-win-eventing-")
        || stem.starts_with("api-ms-win-eventlog-")
        || stem.starts_with("api-ms-win-service-")
        || stem.starts_with("api-ms-win-base-util-")
    {
        return "advapi32";
    }
    if stem.starts_with("api-ms-win-devices-") {
        return "cfgmgr32";
    }
    if stem.starts_with("api-ms-win-shcore-")
        || stem.starts_with("api-ms-win-core-url-")
        || stem.starts_with("api-ms-win-core-shlwapi-")
    {
        return "shlwapi";
    }
    if stem.starts_with("api-ms-win-core-version") {
        return "version";
    }
    // The overwhelming majority of core-* contracts (and any unknown ext-ms-win)
    // are kernel32/kernelbase exports.
    "kernel32"
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn exact_schema_entries_redirect_to_module() {
        // ground-truth entries fold onto the RaeBridge module that hosts them
        assert_eq!(
            canonical_dll("api-ms-win-core-synch-l1-2-0.dll"),
            "kernel32.dll"
        );
        assert_eq!(
            canonical_dll("api-ms-win-core-profile-l1-1-0.dll"),
            "kernel32.dll"
        );
        assert_eq!(
            canonical_dll("api-ms-win-core-heap-l1-1-0.dll"),
            "kernel32.dll"
        );
        assert_eq!(
            canonical_dll("api-ms-win-core-registry-l1-1-0.dll"),
            "advapi32.dll"
        );
        assert_eq!(
            canonical_dll("api-ms-win-core-rtlsupport-l1-1-0.dll"),
            "ntdll.dll"
        );
        assert_eq!(canonical_dll("api-ms-win-core-com-l1-1-0.dll"), "ole32.dll");
        assert_eq!(
            canonical_dll("api-ms-win-security-base-l1-1-0.dll"),
            "advapi32.dll"
        );
        // ucrtbase host folds onto our msvcrt module
        assert_eq!(
            canonical_dll("api-ms-win-crt-runtime-l1-1-0.dll"),
            "msvcrt.dll"
        );
        assert_eq!(
            canonical_dll("api-ms-win-crt-string-l1-1-0.dll"),
            "msvcrt.dll"
        );
    }

    #[test]
    fn prefix_fallback_covers_uncaptured_versions() {
        // a version suffix newer than the captured schema still routes correctly
        assert_eq!(
            canonical_dll("api-ms-win-core-synch-l1-9-9.dll"),
            "kernel32.dll"
        );
        assert_eq!(
            canonical_dll("api-ms-win-crt-math-l9-9-9.dll"),
            "msvcrt.dll"
        );
        assert_eq!(
            canonical_dll("api-ms-win-core-registry-l9-9-9.dll"),
            "advapi32.dll"
        );
        // wholly unknown core contract -> kernel32 (dominant host)
        assert_eq!(
            canonical_dll("api-ms-win-core-brandnew-l1-1-0.dll"),
            "kernel32.dll"
        );
    }

    #[test]
    fn host_aliases_fold_onto_modules() {
        assert_eq!(canonical_dll("kernelbase.dll"), "kernel32.dll");
        assert_eq!(canonical_dll("ucrtbase.dll"), "msvcrt.dll");
        assert_eq!(canonical_dll("vcruntime140.dll"), "msvcrt.dll");
        assert_eq!(canonical_dll("combase.dll"), "ole32.dll");
    }

    #[test]
    fn non_apiset_names_pass_through_lowercased() {
        assert_eq!(canonical_dll("KERNEL32.DLL"), "kernel32.dll");
        assert_eq!(canonical_dll("User32.dll"), "user32.dll");
        assert_eq!(canonical_dll("shell32.dll"), "shell32.dll");
        assert_eq!(canonical_dll("xinput1_4.dll"), "xinput1_4.dll");
    }

    #[test]
    fn canonicalization_is_idempotent() {
        for d in [
            "api-ms-win-core-synch-l1-2-0.dll",
            "ucrtbase.dll",
            "KERNEL32.DLL",
            "shell32.dll",
        ] {
            let once = canonical_dll(d);
            let twice = canonical_dll(&once);
            assert_eq!(once, twice, "canonicalization must be idempotent");
        }
    }

    #[test]
    fn is_api_set_detects_contracts() {
        assert!(is_api_set("api-ms-win-core-synch-l1-2-0.dll"));
        assert!(is_api_set("EXT-MS-WIN-foo-l1-1-0.dll"));
        assert!(!is_api_set("kernel32.dll"));
    }
}
