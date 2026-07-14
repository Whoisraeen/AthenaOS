import capstone
data=open('firmware/amdgpu/gc_11_0_1_mes.bin','rb').read()
U=256
md=capstone.Cs(capstone.CS_ARCH_RISCV, capstone.CS_MODE_RISCV64)
def dump(lo,hi,label):
    print(f"\n=== {label}: {lo:#x}..{hi:#x} ===")
    for i in md.disasm(data[U+lo:U+hi], lo):
        print(f"  {i.address:#07x}: {i.mnemonic:<10} {i.op_str}")
dump(0x7600,0x76d0,"set_hw_resources handler @0x7600")
dump(0x1d4c0,0x1d530,"func @0x1d4c0 (the 0xf0100168 deref)")
# full sweep
end=len(data)-U; allins=[]; a=0
while a<end:
    n=0
    for i in md.disasm(data[U+a:U+min(a+8192,end)], a):
        allins.append((i.address,i.mnemonic,i.op_str)); a=i.address+i.size; n+=1
    if n==0: a+=4
print(f"\nswept {len(allins)} instrs")
# STORES to offset 0x168 (where the per-pipe pointer is initialized)
print("\n=== STORES to 0x168 (sd/sw ..., 0x168(rX)) — the pointer init ===")
for idx,(addr,mn,ops) in enumerate(allins):
    if mn in ('sd','sw') and '0x168(' in ops:
        ctx=allins[max(0,idx-3):idx+2]
        print("  "+" | ".join(f"{x:#x}:{m} {o}" for x,m,o in ctx))
# CSR reads (pipe-id / hart-id detection → per-pipe branching)
print("\n=== CSR reads (csrr — pipe/hart id detection) ===")
seen=set()
for addr,mn,ops in allins:
    if mn.startswith('csr') and ops not in seen:
        seen.add(ops); print(f"  {addr:#x}: {mn} {ops}")
