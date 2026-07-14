#!/usr/bin/env python3
"""Rename all rae*/athena* paths and identifiers to ath*/ath* in AthenaOS."""
from __future__ import annotations

import os
import re
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent if False else Path.cwd()

SKIP_DIRS = {".git", "target", "node_modules"}

# Directory / top-level crate renames (old -> new). Longest keys applied first in text.
DIR_RENAMES: list[tuple[str, str]] = [
    # athena_* first (longer)
    ("ath_linuxkpi", "ath_linuxkpi"),
    ("ath_amdgpu", "ath_amdgpu"),
    ("ath_nvidia", "ath_nvidia"),
    ("ath_drm", "ath_drm"),
    # rae_* underscored
    ("ath_render_broker", "ath_render_broker"),
    ("ath_driver_api", "ath_driver_api"),
    ("ath_markdown", "ath_markdown"),
    ("ath_keychain", "ath_keychain"),
    ("ath_formats", "ath_formats"),
    ("ath_deflate", "ath_deflate"),
    ("ath_encode", "ath_encode"),
    ("ath_crypto", "ath_crypto"),
    ("ath_tokens", "ath_tokens"),
    ("ath_regex", "ath_regex"),
    ("ath_image", "ath_image"),
    ("ath_files", "ath_files"),
    ("ath_jpeg", "ath_jpeg"),
    ("ath_webp", "ath_webp"),
    ("ath_xlsx", "ath_xlsx"),
    ("ath_docx", "ath_docx"),
    ("ath_mail", "ath_mail"),
    ("ath_mime", "ath_mime"),
    ("ath_hash", "ath_hash"),
    ("ath_json", "ath_json"),
    ("ath_toml", "ath_toml"),
    ("ath_diff", "ath_diff"),
    ("ath_calc", "ath_calc"),
    ("ath_time", "ath_time"),
    ("ath_csv", "ath_csv"),
    ("ath_gif", "ath_gif"),
    ("ath_bmp", "ath_bmp"),
    ("ath_png", "ath_png"),
    ("ath_pdf", "ath_pdf"),
    ("ath_mp4", "ath_mp4"),
    ("ath_otp", "ath_otp"),
    ("ath_pim", "ath_pim"),
    ("ath_pwa", "ath_pwa"),
    ("ath_tar", "ath_tar"),
    ("ath_zip", "ath_zip"),
    ("ath_kv", "ath_kv"),
    ("ath_js", "ath_js"),
    ("ath_abi", "ath_abi"),
    # compound apps / bins
    ("athbridge_host", "athbridge_host"),
    ("athbridge_run", "athbridge_run"),
    ("hello_athui", "hello_athui"),
    ("athinstaller", "athinstaller"),
    ("athaccessibility", "athaccessibility"),
    ("athlangd", "athlangd"),
    ("ath-sh", "ath-sh"),
    # plain rae*
    ("athbridge", "athbridge"),
    ("athcontainer", "athcontainer"),
    ("athpackage", "athpackage"),
    ("athsettings", "athsettings"),
    ("athupdate", "athupdate"),
    ("athbackup", "athbackup"),
    ("athcloud", "athcloud"),
    ("athfont", "athfont"),
    ("athmedia", "athmedia"),
    ("athlocale", "athlocale"),
    ("athinput", "athinput"),
    ("athstore", "athstore"),
    ("athshell", "athshell"),
    ("athshield", "athshield"),
    ("athsync", "athsync"),
    ("athprint", "athprint"),
    ("athplay", "athplay"),
    ("athaudio", "athaudio"),
    ("athwasm", "athwasm"),
    ("athgfx", "athgfx"),
    ("athnet", "athnet"),
    ("athkit", "athkit"),
    ("athlang", "athlang"),
    ("athhid", "athhid"),
    ("athfat", "athfat"),
    ("athpkg", "athpkg"),
    ("athssh", "athssh"),
    ("athweb", "athweb"),
    ("athvpn", "athvpn"),
    ("athui", "athui"),
    ("athai", "athai"),
    ("athid", "athid"),
    ("athfs", "athfs"),
]

# Extra identifier renames (not directory names)
EXTRA_TEXT: list[tuple[str, str]] = [
    ("ELFOSABI_ATHENAOS", "ELFOSABI_ATHENAOS"),
    ("SYS_ATHENA", "SYS_ATHENA"),
    ("/proc/athena", "/proc/athena"),
    ("proc/athena", "proc/athena"),
    ("ATHENA_AGENT", "ATHENA_AGENT"),
    ("ATHENA_ACCEL", "ATHENA_ACCEL"),
    ("ATHENA_SMP", "ATHENA_SMP"),
    ("ATHENA_AMDGPU", "ATHENA_AMDGPU"),
    ("ATHENDRV", "ATHENDRV"),
    ("ATHENA_HIBERNATE", "ATHENA_HIBERNATE"),
    ("ATHENAOS", "ATHENAOS"),
    ("athenaos", "athenaos"),
    ("Athena", "Athena"),  # after Whoisraeen protected
    ("athena", "athena"),
]

TEXT_EXT = {
    ".rs", ".md", ".toml", ".txt", ".yml", ".yaml", ".json", ".sh", ".ps1",
    ".py", ".c", ".h", ".cpp", ".hpp", ".S", ".ld", ".cfg", ".ini", ".svg",
    ".html", ".css", ".js", ".ts", ".tsx", ".mdc", ".wgsl", ".bat", ".service",
    ".desktop", ".lock", ".gn", ".bazel",
}


def protect(text: str) -> tuple[str, dict[str, str]]:
    """Mask Whoisraeen so Athena/athena replaces cannot touch it."""
    tokens = {}
    def sub(m):
        k = f"__PROTECT_{len(tokens)}__"
        tokens[k] = m.group(0)
        return k
    text = re.sub(r"Whoisraeen", sub, text)
    return text, tokens


def unprotect(text: str, tokens: dict[str, str]) -> str:
    for k, v in tokens.items():
        text = text.replace(k, v)
    return text


def build_text_repls() -> list[tuple[str, str]]:
    # Longest-first from DIR_RENAMES + EXTRA
    pairs = list(DIR_RENAMES) + list(EXTRA_TEXT)
    # Also common path forms already covered by dir names
    pairs.sort(key=lambda x: len(x[0]), reverse=True)
    # Dedup keeping first
    seen = set()
    out = []
    for a, b in pairs:
        if a in seen:
            continue
        seen.add(a)
        out.append((a, b))
    return out


def iter_files():
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            yield Path(dirpath) / name


def is_text_file(path: Path) -> bool:
    if path.suffix.lower() in TEXT_EXT:
        return True
    if path.name in {
        "Dockerfile", "Makefile", "LICENSE", "AGENTS.md", "CLAUDE.md",
        "README.md", "Cargo.toml", "Cargo.lock", "GOAL_PROMPT.md",
        "MasterChecklist.md", "OWNERSHIP.toml", "base.toml",
    }:
        return True
    return False


def rename_filesystem():
    """Rename dirs and files containing rae/athena. Deepest paths first."""
    # Collect dirs to rename
    dirs = []
    for dirpath, dirnames, _ in os.walk(ROOT, topdown=False):
        p = Path(dirpath)
        if any(x in p.parts for x in SKIP_DIRS):
            continue
        name = p.name
        for old, new in DIR_RENAMES:
            if name == old:
                dirs.append((p, p.with_name(new)))
                break
    # Also rename files whose names contain old tokens
    files = []
    for path in iter_files():
        if any(x in path.parts for x in SKIP_DIRS):
            continue
        new_name = path.name
        changed = False
        for old, new in sorted(DIR_RENAMES, key=lambda x: len(x[0]), reverse=True):
            if old in new_name:
                new_name = new_name.replace(old, new)
                changed = True
        if changed and new_name != path.name:
            files.append((path, path.with_name(new_name)))

    for src, dst in dirs:
        if src.exists() and not dst.exists():
            print(f"DIR {src.relative_to(ROOT)} -> {dst.name}")
            src.rename(dst)
        elif src.exists() and dst.exists():
            print(f"SKIP DIR exists: {dst}")

    for src, dst in files:
        if not src.exists():
            continue
        if dst.exists():
            print(f"SKIP FILE exists: {dst}")
            continue
        print(f"FILE {src.relative_to(ROOT)} -> {dst.name}")
        src.rename(dst)


def rewrite_texts():
    repls = build_text_repls()
    changed = 0
    for path in iter_files():
        if any(x in path.parts for x in SKIP_DIRS):
            continue
        if not is_text_file(path):
            # try small files without nulls
            try:
                raw = path.read_bytes()
            except Exception:
                continue
            if b"\x00" in raw[:2048] or len(raw) > 2_000_000:
                continue
            try:
                text = raw.decode("utf-8")
            except UnicodeDecodeError:
                continue
        else:
            try:
                raw = path.read_bytes()
                if b"\x00" in raw[:2048]:
                    continue
                text = raw.decode("utf-8")
            except Exception:
                continue

        protected, tokens = protect(text)
        orig = protected
        for old, new in repls:
            if old in protected:
                protected = protected.replace(old, new)
        if protected != orig:
            out = unprotect(protected, tokens)
            path.write_bytes(out.encode("utf-8"))
            changed += 1
    print(f"text_files_changed={changed}")


def main():
    os.chdir(ROOT)
    print("== rewrite texts (paths still old) ==")
    # Actually: rename FS first then rewrite, OR rewrite then rename.
    # Rewrite-first updates Cargo.toml to new paths before dirs move → broken.
    # Rename-first then rewrite: Cargo.toml still old paths until rewrite.
    # Do rename FS, then rewrite texts.
    print("== rename filesystem ==")
    rename_filesystem()
    print("== rewrite texts ==")
    rewrite_texts()
    # Second pass rewrite in case some refs used mixed forms
    rewrite_texts()
    print("done")


if __name__ == "__main__":
    main()
