#!/usr/bin/env bash
cd ~/raeenos || exit 1
B=target/x86_64-unknown-none/release/amdgpud
echo "=== find the prologue of the fn containing 0x5d016 (scan back for push rbp / sub 0x168) ==="
objdump -d --start-address=0x5cb00 --stop-address=0x5d02c "$B" 2>/dev/null | grep -E "push|sub .*rsp|call|lea|mov.*0x1,|mov .*\(%r|jmp|probe: device|log|str" | head -60
