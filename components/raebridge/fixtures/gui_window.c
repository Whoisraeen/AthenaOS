// RaeBridge GUI smoketest fixture — the guest-machine-code half of the
// notepad-class gate. Pure Win32 (no CRT): a custom entry `rae_entry` that
// RETURNS to the RaeBridge loader (run_pe_returning), so it needs no ExitProcess
// and can run alongside the console fixtures.
//
// It registers a class, creates+shows a window, and calls UpdateWindow to drive
// a SYNCHRONOUS WM_PAINT into the WndProc, which paints a white background +
// "HI" text. Every import (RegisterClassExW/CreateWindowExW/ShowWindow/
// UpdateWindow/DefWindowProcW + BeginPaint/EndPaint/FillRect/CreateSolidBrush/
// TextOutW) is one RaeBridge has IAT-wired, so it resolves fully.
//
// Build (dev box, from a vcvars64 shell):
//   cl /nologo /c /O1 /GS- gui_window.c
//   link /nologo /NODEFAULTLIB /ENTRY:rae_entry /SUBSYSTEM:CONSOLE \
//        /OUT:gui_window.exe gui_window.obj user32.lib gdi32.lib
#include <windows.h>

static LRESULT CALLBACK WndProc(HWND h, UINT m, WPARAM wp, LPARAM lp) {
    if (m == WM_PAINT) {
        PAINTSTRUCT ps;
        HDC dc = BeginPaint(h, &ps);
        HBRUSH br = CreateSolidBrush(RGB(255, 255, 255));
        FillRect(dc, &ps.rcPaint, br);
        TextOutW(dc, 4, 4, L"HI", 2);
        EndPaint(h, &ps);
        return 0;
    }
    return DefWindowProcW(h, m, wp, lp);
}

void rae_entry(void) {
    WNDCLASSEXW wc;
    wc.cbSize = sizeof(WNDCLASSEXW);
    wc.style = 0;
    wc.lpfnWndProc = WndProc;
    wc.cbClsExtra = 0;
    wc.cbWndExtra = 0;
    wc.hInstance = 0;
    wc.hIcon = 0;
    wc.hCursor = 0;
    wc.hbrBackground = 0;
    wc.lpszMenuName = 0;
    wc.lpszClassName = L"RaeGui";
    wc.hIconSm = 0;
    RegisterClassExW(&wc);

    HWND h = CreateWindowExW(0, L"RaeGui", L"hi", WS_OVERLAPPEDWINDOW,
                             0, 0, 64, 32, 0, 0, 0, 0);
    ShowWindow(h, SW_SHOW);
    UpdateWindow(h); // synchronous WM_PAINT -> WndProc paints
    // No message loop: this is the paint-on-show proof; return to the loader.
}
