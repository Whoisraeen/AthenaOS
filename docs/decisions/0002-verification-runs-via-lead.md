# ADR 0002 — Verification builds/boots run via the lead (subagent Bash is sandboxed)

- Status: accepted
- Date: 2026-06-17
- Owner: athena-lead (autonomous)

## Context
In this unattended session, subagent Bash execution is auto-DENIED (sandbox). athena-verifier
reported it could not run the gates, `cargo ... build`, or the QEMU `--ci` boot — every command
was refused. The lead's own Bash works. Toolchain available to the lead:
- Windows MINGW64: cargo 1.98 nightly, rustc, and `qemu-system-x86_64.exe` under `Program Files`.
- WSL2 Ubuntu-22.04 (stopped): qemu-system-x86_64 + cargo + a WSL-native clone at
  `/home/athena/athenaos` (the faster KVM path per project memory).

## Decision
The LEAD runs the real build + QEMU boot to produce machine evidence (serial log), then hands the
log path to athena-verifier, which parses it (Read/Grep) and renders the hard PASS/FAIL verdict and
checks the R10 4-artifact contract. Implementers still own code; the lead is the build executor
only because the sandbox blocks subagents. Evidence is still real — never paraphrased, never faked.
Build location: prefer the WSL clone for heavy builds (avoids the OneDrive filter-driver hazard);
use the Windows tree for quick checks.

## Rationale
The charter requires real build+boot evidence and a verifier gate. With subagent Bash blocked, the
only way to honor "never fake green" is for the lead to execute the build and route the artifact to
the verifier for judgment. Reversible: if subagent Bash is later enabled, hand execution back.

## How to reverse
Re-enable subagent Bash permissions; then athena-verifier runs the full cycle itself per ADR 0001.
