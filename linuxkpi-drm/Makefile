# linuxkpi-drm — incremental build of the real amdgpu driver against LinuxKPI.
# Host-first (WSL/gcc). See README.md + SCOPE.md. GPL source lives in vendor/ (fetched).
#
# Override AMDGPU_KVER / VENDOR to point at an already-extracted tree (e.g. the WSL
# working copy): make mes-structs VENDOR=$HOME/amdgpu-linuxkpi/vendor

AMDGPU_KVER ?= 7.0.12
VENDOR      ?= vendor
SRC         := $(VENDOR)/linux-$(AMDGPU_KVER)
AMD         := $(SRC)/drivers/gpu/drm/amd
BUILD       := build

# amd's own header search paths (these ARE in the vendored tree)
AMD_INCLUDES := \
  -I $(AMD)/include \
  -I $(AMD)/include/asic_reg \
  -I $(AMD)/amdgpu \
  -I $(AMD)/amdkfd \
  -I $(AMD)/ras \
  -I $(AMD)/ras/ras_mgr \
  -I $(AMD)/ras/rascore \
  -I $(AMD)/display \
  -I $(AMD)/display/include \
  -I $(AMD)/display/dc \
  -I $(AMD)/display/modules/inc \
  -I $(AMD)/pm/inc \
  -I $(AMD)/pm/swsmu/inc \
  -I $(AMD)/pm/powerplay/inc

# our MPL LinuxKPI shim (this directory) — MUST come first so every <linux/*.h>
# resolves to our shim, never a kernel-internal header. `amd-stubs` is FIRST so
# its SCOPE-aligned opaque stubs (amdgpu_dm.h, dm_irq_params, mod_freesync) shadow
# the real display-core headers and cut the DC type graph the MES path never uses.
SHIM_INCLUDES := -I include/amd-stubs -I include

# the REAL vendored DRM core + uapi headers (FreeBSD drm-kmod model, SCOPE.md):
# the drm/*.h API is maintained as a unit with amdgpu and its struct layouts must
# byte-match what amdgpu reads, so we use the upstream headers verbatim rather than
# hand-reshimming them — and let them compile against our linux/*.h shim layer
# (their only deps are dma-fence/completion/xarray/workqueue, which raeen_linuxkpi
# already implements). fetch-source.sh pulls include/drm + include/uapi/drm only
# (no include/linux), so there is no conflict with our shims. GPL, vendored,
# git-ignored — used at BUILD time exactly like the amdgpu .c it serves.
DRM_INCLUDES := -I $(SRC)/include -I $(SRC)/include/uapi

CC      ?= gcc
# kernel C dialect knobs amdgpu assumes
CFLAGS  ?= -std=gnu11 -fno-strict-aliasing -fsyntax-only -Wno-unknown-pragmas

.PHONY: all mes-structs probe clean check-src

all: mes-structs

check-src:
	@test -f $(AMD)/amdgpu/mes_v11_0.c || { \
	  echo "ERROR: amdgpu source not found under $(AMD)."; \
	  echo "Run ./fetch-source.sh first (or pass VENDOR=<path-to-extracted-tree>)."; \
	  exit 1; }

# MILESTONE 1 (GREEN): the MES packet union + MQD struct compile.
# This is the MES-specific definition layer our Rust reimpl mirrors.
mes-structs: check-src
	@mkdir -p $(BUILD)
	@printf '#include <stdint.h>\n#include <stdbool.h>\n#include "mes_v11_api_def.h"\n#include "v11_structs.h"\nint _probe[ sizeof(union MESAPI_SET_HW_RESOURCES) > 0 && sizeof(struct v11_compute_mqd) > 0 ? 1 : -1 ];\n' > $(BUILD)/_mes_structs.c
	$(CC) $(CFLAGS) $(BUILD)/_mes_structs.c -I $(AMD)/include
	@echo "M1 OK: MES set_hw_resources packet + v11 MQD structs compile."

# Show the next wall: try to preprocess mes_v11_0.c and list the first missing headers.
probe: check-src
	@echo "=== mes_v11_0.c: first missing include (the M2 wall) ==="
	@$(CC) -E $(AMD)/amdgpu/mes_v11_0.c $(SHIM_INCLUDES) $(AMD_INCLUDES) $(DRM_INCLUDES) -o /dev/null 2>&1 | grep -m1 "fatal error" || echo "(preprocesses — M2 reached!)"

# MILESTONE 3 (in progress): full typecheck of mes_v11_0.c. Surfaces the struct/
# macro definition gaps (amdgpu_device layout, WREG32_SOC15/REG_SET_FIELD, ...).
# `make m3` prints the error count + the first diagnostics — the M3 grind queue.
m3 compile: check-src
	@echo "=== mes_v11_0.c: M3 typecheck (error count + first diagnostics) ==="
	@n=$$($(CC) $(CFLAGS) $(AMD)/amdgpu/mes_v11_0.c $(SHIM_INCLUDES) $(AMD_INCLUDES) $(DRM_INCLUDES) 2>&1 | grep -c "error:"); \
	  echo "M3 errors remaining: $$n"; \
	  if [ "$$n" = "0" ]; then echo "M3 OK: mes_v11_0.c TYPECHECKS."; else \
	  $(CC) $(CFLAGS) $(AMD)/amdgpu/mes_v11_0.c $(SHIM_INCLUDES) $(AMD_INCLUDES) $(DRM_INCLUDES) 2>&1 | grep "error:" | head -8; fi

clean:
	rm -rf $(BUILD)
