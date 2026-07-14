// RaeBridge Notepad-class capstone fixture — integrates EVERY notepad piece in
// one real cl.exe Win32 .exe: a main window with a WndProc, a system "EDIT"
// child that holds the text, a File menu (Save / Exit), typing into the EDIT,
// and a menu-driven File->Save that reads the EDIT via GetWindowTextW, picks a
// path via GetSaveFileNameW, and writes it with CreateFileW/WriteFile — then
// File->Exit. Pure Win32, custom returning entry `rae_entry` (no CRT).
//
// Flow proven end-to-end in QEMU:
//   RegisterClassExW + CreateWindowExW (main, WndProc)
//   CreateWindowExW("EDIT", child)                         -> system EDIT control
//   CreateMenu + CreatePopupMenu + AppendMenuW(Save/Exit) + SetMenu
//   PostMessage(edit, WM_KEYDOWN 'H'/'I') + PeekMessage pump -> EDIT holds "HI"
//   PostMessage(main, WM_COMMAND IDM_SAVE) -> WndProc:
//       GetWindowTextW(edit) -> "HI"; GetSaveFileNameW -> path;
//       CreateFileW(CREATE_ALWAYS)+WriteFile -> C:\note.txt
//   PostMessage(main, WM_COMMAND IDM_EXIT) -> PostQuitMessage -> loop ends
//
// Build (vcvars64 shell):
//   cl /nologo /c /O1 /GS- gui_notepad.c
//   link /nologo /NODEFAULTLIB /ENTRY:rae_entry /SUBSYSTEM:CONSOLE \
//        /OUT:gui_notepad.exe gui_notepad.obj user32.lib gdi32.lib \
//        comdlg32.lib kernel32.lib
#include <windows.h>
#include <commdlg.h>

#define IDM_SAVE 100
#define IDM_EXIT 101

static HWND g_edit;
static OPENFILENAMEW g_ofn;   // static => zero-initialized (.bss), no memset/CRT
static wchar_t g_path[64];

static void do_save(void) {
    wchar_t text[64];
    int n = GetWindowTextW(g_edit, text, 64); // the EDIT child's accumulated "HI"
    // Pre-set the default path into the writable lpstrFile buffer (no lstrcpy).
    const wchar_t *def = L"C:\\note.txt";
    int i = 0;
    while (def[i]) { g_path[i] = def[i]; i++; }
    g_path[i] = 0;
    g_ofn.lStructSize = sizeof(OPENFILENAMEW);
    g_ofn.lpstrFile = g_path;
    g_ofn.nMaxFile = 64;
    if (GetSaveFileNameW(&g_ofn)) {
        HANDLE f = CreateFileW(g_ofn.lpstrFile, GENERIC_WRITE, 0, 0,
                               CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, 0);
        if (f != INVALID_HANDLE_VALUE) {
            unsigned char ascii[64];
            int j = 0;
            while (j < n && j < 64) { ascii[j] = (unsigned char)text[j]; j++; }
            DWORD w = 0;
            WriteFile(f, ascii, (DWORD)j, &w, 0);
            CloseHandle(f);
        }
    }
}

static LRESULT CALLBACK WndProc(HWND h, UINT m, WPARAM wp, LPARAM lp) {
    if (m == WM_COMMAND) {
        if (LOWORD(wp) == IDM_SAVE) { do_save(); return 0; }
        if (LOWORD(wp) == IDM_EXIT) { PostQuitMessage(0); return 0; }
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
    wc.lpszClassName = L"RaeNotepad";
    wc.hIconSm = 0;
    RegisterClassExW(&wc);

    HWND main_w = CreateWindowExW(0, L"RaeNotepad", L"Notepad", WS_OVERLAPPEDWINDOW,
                                  0, 0, 320, 200, 0, 0, 0, 0);
    g_edit = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE,
                             0, 0, 320, 180, main_w, 0, 0, 0);

    HMENU bar = CreateMenu();
    HMENU file = CreatePopupMenu();
    AppendMenuW(file, MF_STRING, IDM_SAVE, L"Save");
    AppendMenuW(file, MF_STRING, IDM_EXIT, L"Exit");
    AppendMenuW(bar, MF_POPUP, (UINT_PTR)file, L"File");
    SetMenu(main_w, bar);

    // Type "HI" into the EDIT child (VK letter codes ARE their ASCII value).
    PostMessageW(g_edit, WM_KEYDOWN, 'H', 0);
    PostMessageW(g_edit, WM_KEYDOWN, 'I', 0);

    MSG msg;
    int i = 0;
    // Phase 1: drain typing so the EDIT accumulates "HI" before we save.
    for (i = 0; i < 16; i++) {
        if (PeekMessageW(&msg, 0, 0, 0, PM_REMOVE)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        } else {
            break;
        }
    }
    // Phase 2: drive File->Save then File->Exit through the menu (WM_COMMAND).
    PostMessageW(main_w, WM_COMMAND, IDM_SAVE, 0);
    PostMessageW(main_w, WM_COMMAND, IDM_EXIT, 0);
    while (GetMessageW(&msg, 0, 0, 0)) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}
