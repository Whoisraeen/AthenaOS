#include <windows.h>
int main(void) {
    const char *s = "real windows exe\n";
    DWORD n = 0;
    WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), s, 17, &n, 0);
    return 0;
}
