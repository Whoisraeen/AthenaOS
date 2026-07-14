// Real MSVC /MT C++ console exe — the C++-runtime broadening fixture.
//
// Concept §Compatibility: "RaeBridge runs Windows apps on day one." Most real
// Windows software is C++, not C. This fixture proves the C++ runtime runs:
//   - g_init is a NAMESPACE-SCOPE object with a non-trivial constructor, so the
//     MSVC /MT CRT must walk the static-initializer table (`_initterm` over the
//     .CRT$XC* section) BEFORE main. Its ctor prints "ctor ran".
//   - main prints "hello from c++ 7".
// If BOTH lines appear in order and the process exits 0, the C++ static-init
// machinery (the C++-runtime delta over the plain-C printf case) ran correctly.
//
// printf (not std::cout) is used deliberately: it keeps the NEW import surface
// to the genuine C++-runtime delta (the static-ctor table walk + atexit
// registration the /MT C++ CRT installs at startup) rather than dragging in the
// large iostream locale/facet surface.
//
// Compiled: cl /MT /O1 /GS- /EHsc real_msvc_mt_cpp.cpp /link /SUBSYSTEM:CONSOLE
#include <cstdio>

struct Init {
    Init() { printf("ctor ran\n"); }
};

// Namespace-scope object with a non-trivial ctor => emitted into the C++
// static-initializer table the /MT CRT walks via _initterm before main.
static Init g_init;

int main() {
    printf("hello from c++ %d\n", 7);
    return 0;
}
