# AthenaOS File Structure Reorganization Plan

This document outlines the steps required to clean up the AthenaOS root directory and update the `Cargo.toml` workspace so the build doesn't break. Please execute these file movements and update the Cargo files accordingly.

## 1. Documentation Organization
Move the scattered markdown files into a dedicated subfolder (e.g., `docs/planning/` and `docs/checklists/`):
- `Audit.md` -> `docs/Audit.md`
- `BUG_REPORT.md` -> `.github/ISSUE_TEMPLATE/BUG_REPORT.md`
- `CHECKLIST.md` -> `docs/checklists/CHECKLIST.md`
- `MasterChecklist.md` -> `docs/checklists/MasterChecklist.md`
- `MILESTONE_A_PLAN.md` -> `docs/planning/MILESTONE_A_PLAN.md`
- `NEW_BUG_DISCOVERY.md` -> `.github/ISSUE_TEMPLATE/NEW_BUG_DISCOVERY.md`
- `PRODUCTION_CHECKLIST.md` -> `docs/checklists/PRODUCTION_CHECKLIST.md`
- `kernelchecklist.md` -> `docs/checklists/kernelchecklist.md`
- `LEGACY_GAMING_CONCEPT.md` -> `docs/LEGACY_GAMING_CONCEPT.md`

Leave `README.md`, `CLAUDE.md`, and `AGENTS.md` in the root.

## 2. Test and Example Apps
Create an `examples/` directory in the root and move the following experimental apps into it:
- `hello_linuxkpi/` -> `examples/hello_linuxkpi/`
- `hello_relibc/` -> `examples/hello_relibc/`
- `hello_window/` -> `examples/hello_window/`
- `linux_hello/` -> `examples/linux_hello/`

Delete or move the scratch files to a scratch folder, and add `.exe` / `.pdb` to `.gitignore`:
- `scratch_elf.rs`, `scratch_elf.exe`, `scratch_elf.pdb`

## 3. Daemons and Drivers
Create a `daemons/` directory in the root for user-space drivers:
- `amdgpud/` -> `daemons/amdgpud/`
- `i915d/` -> `daemons/i915d/`
- `driver_supervisor/` -> `daemons/driver_supervisor/`

## 4. Core Services
Create a `services/` directory in the root for top-level system services:
- `conductor/` -> `services/conductor/`
- `user_init/` -> `services/user_init/`
- `raebridge_host/` -> `services/raebridge_host/`
- `raeinstaller/` -> `services/raeinstaller/`

## 5. Update `Cargo.toml`
After performing the file movements, the root `Cargo.toml` workspace `members` and `default-members` lists MUST be updated to reflect the new paths.

Changes to make in `Cargo.toml`:
- `"amdgpud"` -> `"daemons/amdgpud"`
- `"i915d"` -> `"daemons/i915d"`
- `"driver_supervisor"` -> `"daemons/driver_supervisor"`
- `"user_init"` -> `"services/user_init"`
- `"hello_window"` -> `"examples/hello_window"`
- `"hello_relibc"` -> `"examples/hello_relibc"`
- `"hello_linuxkpi"` -> `"examples/hello_linuxkpi"`
- `"linux_hello"` -> `"examples/linux_hello"`
- `"raebridge_host"` -> `"services/raebridge_host"`
- `"raeinstaller"` -> `"services/raeinstaller"`
- Ensure `"conductor"` is properly listed if it is a Cargo package.
