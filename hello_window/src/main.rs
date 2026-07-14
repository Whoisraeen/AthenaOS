#![no_std]
#![no_main]

#[allow(unused_imports)]
use raekit;

use raegfx::Canvas;

const SURFACE_VIRT: u64 = 0x0000_8888_0000;
const WIN_W: usize = 400;
const WIN_H: usize = 250;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let id = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if id == u64::MAX {
        raekit::sys::exit(1);
    }

    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    canvas.fill_rect(0, 0, WIN_W, WIN_H, 0xFF1E293B);

    canvas.fill_rect(0, 0, WIN_W, 32, 0xFF3B82F6);
    canvas.draw_text(12, 8, "Hello Window", 0xFFFFFFFF, None);

    canvas.fill_rect(WIN_W - 32, 0, 32, 32, 0xFFEF4444);
    canvas.draw_text(WIN_W - 22, 8, "X", 0xFFFFFFFF, None);

    canvas.draw_text(40, 80, "Hello from AthenaOS!", 0xFFE2E8F0, None);
    canvas.draw_text(
        40,
        110,
        "This is a real userspace process",
        0xFF94A3B8,
        None,
    );
    canvas.draw_text(
        40,
        130,
        "running in its own address space.",
        0xFF94A3B8,
        None,
    );
    canvas.draw_text(40, 160, "Spawned from the start menu.", 0xFF94A3B8, None);

    for x in 0..WIN_W {
        canvas.draw_pixel(x, 0, 0xFF475569);
        canvas.draw_pixel(x, WIN_H - 1, 0xFF475569);
    }
    for y in 0..WIN_H {
        canvas.draw_pixel(0, y, 0xFF475569);
        canvas.draw_pixel(WIN_W - 1, y, 0xFF475569);
    }

    raekit::sys::surface_present(id, 200, 150);

    loop {
        raekit::sys::yield_now();
    }
}
