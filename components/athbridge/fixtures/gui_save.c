// AthBridge notepad-flow fixture — types text and saves it to a file.
// Pure Win32, custom returning entry `rae_entry`. Injects two keystrokes
// ('H','I') as WM_KEYDOWN to its own window; the standard message pump
// (GetMessage -> TranslateMessage -> DispatchMessage) turns them into WM_CHAR,
// the WndProc accumulates them, and on the 2nd char writes the typed text to
// C:\out.txt (CreateFileW CREATE_ALWAYS + WriteFile) and PostQuitMessage. This
// is the "types + saves" half of the notepad-class gate.
//
// Build (vcvars64 shell):
//   cl /nologo /c /O1 /GS- gui_save.c
//   link /nologo /NODEFAULTLIB /ENTRY:rae_entry /SUBSYSTEM:CONSOLE \
//        /OUT:gui_save.exe gui_save.obj user32.lib gdi32.lib kernel32.lib
#include <windows.h>

static unsigned char g_buf[16];
static int g_n = 0;

static void save_typed(void) {
    HANDLE f = CreateFileW(L"C:\\out.txt", GENERIC_WRITE, 0, 0,
                           CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, 0);
    if (f != INVALID_HANDLE_VALUE) {
        DWORD written = 0;
        WriteFile(f, g_buf, (DWORD)g_n, &written, 0);
        CloseHandle(f);
    }
}

static LRESULT CALLBACK WndProc(HWND h, UINT m, WPARAM wp, LPARAM lp) {
    if (m == WM_CHAR) {
        if (g_n < (int)sizeof(g_buf)) {
            g_buf[g_n++] = (unsigned char)wp;
        }
        if (g_n == 2) {
            save_typed();
            PostQuitMessage(0);
        }
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
    wc.lpszClassName = L"RaeSave";
    wc.hIconSm = 0;
    RegisterClassExW(&wc);

    HWND h = CreateWindowExW(0, L"RaeSave", L"s", WS_OVERLAPPEDWINDOW,
                             0, 0, 64, 32, 0, 0, 0, 0);
    // Inject "HI" as keystrokes (VK letter codes ARE ASCII).
    PostMessageW(h, WM_KEYDOWN, 'H', 0);
    PostMessageW(h, WM_KEYDOWN, 'I', 0);

    MSG msg;
    while (GetMessageW(&msg, 0, 0, 0)) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}
