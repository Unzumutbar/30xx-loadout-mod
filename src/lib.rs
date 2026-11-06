//! 30XX Loadout mod.
//!
//! Ships as `dinput8.dll` in the game folder. The game imports DirectInput8Create from
//! dinput8.dll, so Windows loads this file first; we forward that call to the real
//! system DLL and, on a background thread, install:
//!   * a hook on the game's run-start "apply Codebreakers" routine that gives each
//!     local player the items chosen in the menu, and
//!   * a Dear ImGui overlay (DX11 / DX9) for picking those items (toggle with F8).

mod config;
mod game;
mod items;
mod ui;

use std::ffi::c_void;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use hudhook::hooks::dx11::ImguiDx11Hooks;
use hudhook::hooks::dx9::ImguiDx9Hooks;
use hudhook::mh::{MhHook, MH_EnableHook};
use hudhook::Hudhook;
use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

use config::Config;

// ---------------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------------

pub struct State {
    pub config: Config,
    /// Status line shown in the menu (hook installed / version mismatch / ...).
    pub status: String,
    pub hooks_ok: bool,
    /// Set by the menu's "Give now" button; consumed on the render (game) thread.
    pub give_now: bool,
    /// Log of what was applied, newest last (shown in the menu).
    pub history: Vec<String>,
}

pub fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            config: Config::load(),
            status: "starting...".into(),
            hooks_ok: false,
            give_now: false,
            history: Vec::new(),
        })
    })
}

// ---------------------------------------------------------------------------------
// Logging to <game>/loadout30xx.log
// ---------------------------------------------------------------------------------

pub fn write_log(msg: &str) {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::game_dir().join("loadout30xx.log"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::write_log(&format!($($t)*)) };
}

// ---------------------------------------------------------------------------------
// Giving items
// ---------------------------------------------------------------------------------

/// Give the configured items to `player` if it is controlled from this machine.
/// Returns a short description for the menu history, or None if nothing was done.
unsafe fn apply_loadout(player: *mut c_void, reason: &str) -> Option<String> {
    let slot = game::local_slot(player)?;
    let items = {
        let st = state().lock().ok()?;
        if !st.config.enabled {
            return None;
        }
        st.config.players[slot].clone()
    };
    if items.is_empty() {
        return None;
    }
    for &it in &items {
        game::give_item(player, it);
    }
    let role = game::net_role();
    if role != 0 {
        // Online: let the other machine know, so this player has the items there too.
        game::broadcast_items(player, &items);
    }
    let msg = format!("{reason}: gave {} item(s) to Player {}", items.len(), slot + 1);
    log!("{msg} (netId={}, role={role}, mode={}) items={items:?}", game::net_id(player), game::game_mode());
    Some(msg)
}

fn push_history(msg: String) {
    if let Ok(mut st) = state().lock() {
        st.history.push(msg);
        let n = st.history.len();
        if n > 6 {
            st.history.drain(0..n - 6);
        }
    }
}

// ---------------------------------------------------------------------------------
// Run-start hook
// ---------------------------------------------------------------------------------

type ApplyCbFn = unsafe extern "system" fn(player: *mut c_void);
static ORIG_APPLY_CB: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn apply_codebreakers_detour(player: *mut c_void) {
    let orig: ApplyCbFn = std::mem::transmute(ORIG_APPLY_CB.load(Ordering::Relaxed));
    orig(player);
    log!(
        "run start: player netId={} localSlot={:?} role={} mode={}",
        game::net_id(player),
        game::local_slot(player),
        game::net_role(),
        game::game_mode()
    );
    if let Some(m) = apply_loadout(player, "Run start") {
        push_history(m);
    }
}

/// Called every frame from the overlay (which runs on the game's render thread).
pub fn on_frame() {
    let want = state().lock().map(|mut s| std::mem::take(&mut s.give_now)).unwrap_or(false);
    if !want {
        return;
    }
    let mut any = false;
    unsafe {
        for p in game::players() {
            if let Some(m) = apply_loadout(p, "Manual") {
                push_history(m);
                any = true;
            }
        }
    }
    if !any {
        push_history("Manual: no local player in a run (or nothing selected)".into());
    }
}

// ---------------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------------

static STARTED: AtomicBool = AtomicBool::new(false);

fn set_status(ok: bool, s: impl Into<String>) {
    let s = s.into();
    log!("status: {s}");
    if let Ok(mut st) = state().lock() {
        st.status = s;
        st.hooks_ok = ok;
    }
}

fn init() {
    log!("---- loadout30xx {} starting ----", env!("CARGO_PKG_VERSION"));

    // Overlay first, so the menu can explain problems even if the game hook fails.
    // Hook creation can panic (e.g. if a dummy D3D device can't be created); never let
    // that take the game down - try DX11+DX9, then DX11 alone.
    let built = std::panic::catch_unwind(|| {
        Hudhook::builder()
            .with::<ImguiDx11Hooks>(ui::Menu::default())
            .with::<ImguiDx9Hooks>(ui::Menu::default())
            .build()
    })
    .or_else(|_| {
        log!("DX9 overlay unavailable, using DX11 only");
        std::panic::catch_unwind(|| Hudhook::builder().with::<ImguiDx11Hooks>(ui::Menu::default()).build())
    });
    match built {
        Ok(hud) => {
            if let Err(e) = hud.apply() {
                log!("overlay hooks failed: {e:?}");
            } else {
                log!("overlay hooks installed (F8)");
            }
        }
        Err(_) => log!("overlay could not be created; run-start hook still works with the saved config"),
    }

    if let Err(e) = game::verify() {
        set_status(false, format!("Disabled: {e}"));
        return;
    }
    unsafe {
        let target = (game::base() + game::RVA_APPLY_CODEBREAKERS) as *mut c_void;
        match MhHook::new(target, apply_codebreakers_detour as *mut c_void) {
            Ok(h) => {
                ORIG_APPLY_CB.store(h.trampoline() as usize, Ordering::Relaxed);
                if MH_EnableHook(target) == hudhook::mh::MH_STATUS::MH_OK {
                    set_status(true, "Active - selections are given when a new run starts");
                    std::mem::forget(h);
                } else {
                    set_status(false, "Disabled: could not enable run-start hook");
                }
            }
            Err(e) => set_status(false, format!("Disabled: could not create run-start hook ({e:?})")),
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(_h: HINSTANCE, reason: u32, _r: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH && !STARTED.swap(true, Ordering::SeqCst) {
        std::thread::spawn(init);
    }
    1
}

// ---------------------------------------------------------------------------------
// dinput8.dll proxy export
// ---------------------------------------------------------------------------------

type DI8CreateFn =
    unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;

#[no_mangle]
pub unsafe extern "system" fn DirectInput8Create(
    hinst: *mut c_void,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    static REAL: OnceLock<usize> = OnceLock::new();
    let f = *REAL.get_or_init(|| {
        let mut buf = [0u16; 260];
        let n = GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) as usize;
        let mut path: Vec<u16> = buf[..n].to_vec();
        path.extend("\\dinput8.dll".encode_utf16());
        path.push(0);
        let m = LoadLibraryW(path.as_ptr());
        if m.is_null() {
            return 0;
        }
        GetProcAddress(m, b"DirectInput8Create\0".as_ptr()).map(|p| p as usize).unwrap_or(0)
    });
    if f == 0 {
        return 0x80004005u32 as i32; // E_FAIL
    }
    let real: DI8CreateFn = std::mem::transmute(f);
    real(hinst, version, riid, out, outer)
}
