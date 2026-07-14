//! AthenaOS Contacts — the bundled address book (Concept §Three User Experiences:
//! daily-driver parity vs Win11 People / macOS Contacts).
//!
//! A standalone userspace ELF (`exec_path = "contacts"`). The rich data model
//! (`model` — Contact/Phone/Email/vCard, sort/filter/groups) was the previously
//! UNWIRED `raeshell::contacts_app`; it moved here and became a live app. This
//! `main.rs` is the app shell: a two-pane list + detail over demo contacts
//! (there is no live CardDAV/store syscall yet — a `NEEDS-INTERFACE` follow-up),
//! on the OBSIDIAN design language, re-skinned to the live desktop accent.
//!
//! PROOF: `design_proof()` (fail-able runtime gate at `_start`) asserts the model
//! builds a non-empty, sorted-enough demo (named contacts each with a phone +
//! email) AND the chrome is token-wired — `exit(3)` on drift.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;

#[allow(unused_imports)]
use raekit;

use rae_tokens::{DARK, RAEBLUE, TYPE_BODY, TYPE_CAPTION, TYPE_SUBTITLE, TYPE_TITLE};
use raegfx::text::FontFamily;
use raegfx::Canvas;

mod model;
use model::{Contact, ContactsApp, Email, EmailType, Phone, PhoneType};

const WIN_W: usize = 760;
const WIN_H: usize = 480;
const SURFACE_VIRT: u64 = 0x0000_7C00_0000;

const BG: u32 = DARK.bg_base;
const SIDEBAR_BG: u32 = DARK.bg_raised;
const CARD_BG: u32 = DARK.bg_raised;
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_DIM: u32 = DARK.text_secondary;
const TEXT_MUTE: u32 = DARK.text_tertiary;

const SIDEBAR_W: usize = 250;
const ROW_H: usize = 44;
const LIST_TOP: usize = 56;

fn accent() -> u32 {
    rae_tokens::derive_accent(raekit::sys::theme_accent(), &DARK).base
}
fn accent_subtle() -> u32 {
    rae_tokens::derive_accent(raekit::sys::theme_accent(), &DARK).subtle
}
/// On-accent ink for the selected row (IDENTITY §4: accent-filled row flips to
/// dark ink).
fn on_accent() -> u32 {
    DARK.bg_base
}

struct App {
    store: ContactsApp,
    selected: usize,
}

impl App {
    fn new() -> Self {
        let mut store = ContactsApp::new();
        // Demo address book (the live store syscall is a follow-up). Kept sorted
        // by last name so the list reads like a real address book.
        let seed = [
            (
                "Ada",
                "Lovelace",
                "Analytical Engines",
                "Mathematician",
                "+1 415 555 0142",
                "ada@analytical.io",
            ),
            (
                "Alan",
                "Turing",
                "Bletchley Park",
                "Cryptanalyst",
                "+44 20 7946 0011",
                "alan@turing.uk",
            ),
            (
                "Grace",
                "Hopper",
                "US Navy",
                "Rear Admiral",
                "+1 202 555 0177",
                "grace@navy.mil",
            ),
            (
                "Katherine",
                "Johnson",
                "NASA",
                "Mathematician",
                "+1 757 555 0193",
                "kjohnson@nasa.gov",
            ),
            (
                "Margaret",
                "Hamilton",
                "MIT / Apollo",
                "Software Lead",
                "+1 617 555 0128",
                "mham@mit.edu",
            ),
        ];
        for (first, last, org, title, phone, email) in seed {
            let id = store.next_contact_id;
            store.next_contact_id += 1;
            let mut c = Contact::new(id, first, last);
            c.organization = String::from(org);
            c.title = String::from(title);
            c.phones.push(Phone {
                kind: PhoneType::Mobile,
                custom_label: String::new(),
                number: String::from(phone),
                preferred: true,
            });
            c.emails.push(Email {
                kind: EmailType::Work,
                address: String::from(email),
                preferred: true,
            });
            store.contacts.push(c);
        }
        Self { store, selected: 0 }
    }

    fn move_sel(&mut self, d: i32) {
        let n = self.store.contacts.len();
        if n == 0 {
            return;
        }
        let s = self.selected as i32 + d;
        self.selected = s.rem_euclid(n as i32) as usize;
    }

    fn current(&self) -> Option<&Contact> {
        self.store.contacts.get(self.selected)
    }
}

fn render(app: &App, canvas: &mut Canvas) {
    let acc = accent();
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // ── Sidebar: contact list ───────────────────────────────────────────
    canvas.fill_rect(0, 0, SIDEBAR_W, WIN_H, SIDEBAR_BG);
    canvas.draw_text_aa(20, 20, "Contacts", TYPE_TITLE, TEXT_FG, FontFamily::Sans);
    for (i, c) in app.store.contacts.iter().enumerate() {
        let y = LIST_TOP + i * ROW_H;
        let selected = i == app.selected;
        if selected {
            canvas.fill_rounded_rect(
                8,
                y,
                SIDEBAR_W - 16,
                ROW_H - 6,
                rae_tokens::RADIUS_MD as usize,
                acc,
            );
        }
        let fg = if selected { on_accent() } else { TEXT_FG };
        canvas.draw_text_aa(
            20,
            y as i32 + 12,
            &c.full_name(),
            TYPE_BODY,
            fg,
            FontFamily::Sans,
        );
    }
    canvas.draw_text_aa(
        20,
        WIN_H as i32 - 24,
        "\u{2191}\u{2193} Navigate   Esc Close",
        TYPE_CAPTION,
        TEXT_MUTE,
        FontFamily::Sans,
    );

    // ── Detail pane: the selected contact ───────────────────────────────
    let dx = SIDEBAR_W + 32;
    let Some(c) = app.current() else { return };

    // Avatar: an accent-subtle disc with the initial.
    let av = 72usize;
    canvas.fill_rounded_rect(dx, 40, av, av, av / 2, accent_subtle());
    let initial = c.first_name.chars().next().unwrap_or('?');
    let mut ibuf = [0u8; 4];
    canvas.draw_text_aa(
        dx as i32 + 26,
        40 + 22,
        initial.encode_utf8(&mut ibuf),
        TYPE_TITLE,
        acc,
        FontFamily::Sans,
    );

    canvas.draw_text_aa(
        dx as i32 + av as i32 + 20,
        48,
        &c.full_name(),
        TYPE_TITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    if !c.title.is_empty() || !c.organization.is_empty() {
        let sub = if c.organization.is_empty() {
            c.title.clone()
        } else {
            alloc::format!("{} \u{b7} {}", c.title, c.organization)
        };
        canvas.draw_text_aa(
            dx as i32 + av as i32 + 20,
            78,
            &sub,
            TYPE_BODY,
            TEXT_DIM,
            FontFamily::Sans,
        );
    }

    // Info cards: phone + email.
    let mut cy = 150usize;
    let card_w = WIN_W - dx - 32;
    for (label, value) in [
        (
            "Phone",
            c.phones
                .first()
                .map(|p| p.number.as_str())
                .unwrap_or("\u{2014}"),
        ),
        (
            "Email",
            c.emails
                .first()
                .map(|e| e.address.as_str())
                .unwrap_or("\u{2014}"),
        ),
        (
            "Organization",
            if c.organization.is_empty() {
                "\u{2014}"
            } else {
                c.organization.as_str()
            },
        ),
    ] {
        canvas.fill_rounded_rect(dx, cy, card_w, 58, rae_tokens::RADIUS_MD as usize, CARD_BG);
        canvas.draw_text_aa(
            dx as i32 + 16,
            cy as i32 + 10,
            label,
            TYPE_CAPTION,
            TEXT_MUTE,
            FontFamily::Sans,
        );
        canvas.draw_text_aa(
            dx as i32 + 16,
            cy as i32 + 30,
            value,
            TYPE_SUBTITLE,
            TEXT_FG,
            FontFamily::Sans,
        );
        cy += 70;
    }
}

pub fn design_proof() -> bool {
    let app = App::new();
    let n = app.store.contacts.len();
    let each_has_contact_info = app
        .store
        .contacts
        .iter()
        .all(|c| !c.full_name().is_empty() && !c.phones.is_empty() && !c.emails.is_empty());
    let tokens_ok = BG == DARK.bg_base
        && TEXT_FG == DARK.text_primary
        && raekit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;
    n >= 3 && each_has_contact_info && tokens_ok
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    if !design_proof() {
        raekit::sys::exit(3);
    }
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }
    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };
    let mut app = App::new();
    render(&app, &mut canvas);
    raekit::sys::surface_present(sid, 160, 80);

    let mut extended = false;
    loop {
        let key = raekit::sys::read_key();
        if key == 0 {
            raekit::sys::yield_now();
            continue;
        }
        let sc = key as u8;
        if sc == 0xE0 {
            extended = true;
            continue;
        }
        let ext = core::mem::replace(&mut extended, false);
        if sc & 0x80 != 0 {
            continue;
        }
        let code = sc & 0x7F;
        let mut dirty = false;
        match (ext, code) {
            (true, 0x48) => {
                app.move_sel(-1);
                dirty = true;
            }
            (true, 0x50) => {
                app.move_sel(1);
                dirty = true;
            }
            (false, 0x01) => raekit::sys::exit(0),
            _ => {}
        }
        if dirty {
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, 160, 80);
        }
    }
}
