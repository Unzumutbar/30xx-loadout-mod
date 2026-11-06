//! Everything that touches 30XX.exe internals.
//!
//! All addresses are RVAs (offsets from the module base) for the 30XX.exe build that
//! ships `C:\30xx-git\30xx-exile\30XX.pdb` (10,246,144 bytes). `verify()` checks byte
//! signatures at every address before anything is hooked, so a game update disables
//! the mod instead of crashing the game.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::log;

/// `void ApplyCodebreakers(Player*)` - called once per player when a new run starts
/// (it is what grants Codebreaker starting items such as "Blue Streak").
pub const RVA_APPLY_CODEBREAKERS: usize = 0x4d33a0;
/// `void GiveItem(Player*, int itemId, bool silent)`.
pub const RVA_GIVE_ITEM: usize = 0x4b1440;
/// `std::string& std::string::assign(std::string*, const char*, size_t)`.
const RVA_STRING_ASSIGN: usize = 0xf9f0;
/// `std::string::~string` (tidy).
const RVA_STRING_DTOR: usize = 0xf970;
/// `std::vector<std::string>::_Emplace_reallocate(vec*, where, std::string&&)`.
const RVA_VEC_EMPLACE_REALLOC: usize = 0x47a80;
/// `std::vector<std::string>` of outgoing network messages.
const RVA_NET_OUTBOX: usize = 0x4b67618;
/// `Player*` of the first local player on this machine.
const RVA_LOCAL_PLAYER: usize = 0xa79be0;
/// Network role: 0 = offline, 1 = online host, 2 = online client.
const RVA_NET_ROLE: usize = 0x964b68;
/// Current game mode.
const RVA_GAME_MODE: usize = 0xa7a214;
/// Offset of the network/player id inside a Player object.
const PLAYER_NET_ID: usize = 0x80;

/// (rva, expected bytes) pairs that must match before we touch the game.
const SIGNATURES: &[(usize, &[u8])] = &[
    (RVA_APPLY_CODEBREAKERS, &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x48, 0x89, 0x7c, 0x24, 0x20, 0x55]),
    (RVA_GIVE_ITEM, &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x74, 0x24, 0x10, 0x57, 0x48, 0x81, 0xec, 0x00, 0x01]),
    (RVA_STRING_ASSIGN, &[0x48, 0x89, 0x5c, 0x24, 0x10, 0x48, 0x89, 0x6c, 0x24, 0x18, 0x56, 0x57, 0x41, 0x57]),
    (RVA_STRING_DTOR, &[0x40, 0x53, 0x48, 0x83, 0xec, 0x20, 0x48, 0x8b, 0x51, 0x18]),
    (RVA_VEC_EMPLACE_REALLOC, &[0x48, 0x89, 0x5c, 0x24, 0x18, 0x56, 0x57, 0x41, 0x56]),
    // "protoga %d %d %d" format string and the "volunteer" item key: data layout check.
    (0x8b96f0, b"protoga %d %d %d"),
    (0x8b07b0, b"volunteer\0"),
];

static BASE: AtomicUsize = AtomicUsize::new(0);

pub fn base() -> usize {
    BASE.load(Ordering::Relaxed)
}

fn at(rva: usize) -> usize {
    base() + rva
}

/// Resolve the module base and check every signature. Returns a human-readable error.
pub fn verify() -> Result<(), String> {
    let b = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if b == 0 {
        return Err("GetModuleHandle failed".into());
    }
    BASE.store(b, Ordering::Relaxed);
    for (rva, sig) in SIGNATURES {
        let actual = unsafe { std::slice::from_raw_parts((b + rva) as *const u8, sig.len()) };
        if actual != *sig {
            return Err(format!(
                "signature mismatch at +{rva:#x} (game was probably updated): expected {:02x?}, found {:02x?}",
                sig, actual
            ));
        }
    }
    Ok(())
}

type GiveItemFn = unsafe extern "system" fn(player: *mut c_void, item: i32, silent: u8);
type StrAssignFn = unsafe extern "system" fn(s: *mut MsvcString, p: *const u8, len: usize) -> *mut MsvcString;
type StrDtorFn = unsafe extern "system" fn(s: *mut MsvcString);
type VecEmplaceFn = unsafe extern "system" fn(vec: *mut c_void, at: *mut MsvcString, s: *mut MsvcString) -> *mut MsvcString;

/// MSVC x64 `std::string` layout.
#[repr(C)]
struct MsvcString {
    buf: [u8; 16],
    size: usize,
    cap: usize,
}

impl MsvcString {
    fn empty() -> Self {
        Self { buf: [0; 16], size: 0, cap: 15 }
    }
}

pub unsafe fn give_item(player: *mut c_void, item: i32) {
    let f: GiveItemFn = std::mem::transmute(at(RVA_GIVE_ITEM));
    f(player, item, 0);
}

pub unsafe fn net_id(player: *mut c_void) -> i32 {
    *((player as usize + PLAYER_NET_ID) as *const i32)
}

pub unsafe fn net_role() -> i32 {
    *(at(RVA_NET_ROLE) as *const i32)
}

pub unsafe fn game_mode() -> i32 {
    *(at(RVA_GAME_MODE) as *const i32)
}

pub unsafe fn local_player() -> *mut c_void {
    *(at(RVA_LOCAL_PLAYER) as *const *mut c_void)
}

/// Which local slot (0 = Player 1, 1 = Player 2) this player is, or None if the player
/// belongs to another machine (online coop). Mirrors the game's own check in its
/// run-start code: local players have ids `L` and `L + 1`, where `L` is the id of the
/// first local player.
pub unsafe fn local_slot(player: *mut c_void) -> Option<usize> {
    let lp = local_player();
    if lp.is_null() || player.is_null() {
        return None;
    }
    match net_id(player) - net_id(lp) {
        0 => Some(0),
        1 => Some(1),
        _ => None,
    }
}

/// The game's list of all players (`std::vector<Player*>`).
pub unsafe fn players() -> Vec<*mut c_void> {
    let begin = *(at(0x4b67540) as *const *const *mut c_void);
    let end = *(at(0x4b67548) as *const *const *mut c_void);
    if begin.is_null() || end < begin {
        return Vec::new();
    }
    let n = (end as usize - begin as usize) / 8;
    if n > 8 {
        return Vec::new();
    }
    std::slice::from_raw_parts(begin, n).to_vec()
}

/// Queue a raw network message exactly like the game does (push_back into its outbox).
unsafe fn queue_net_message(msg: &str) {
    let assign: StrAssignFn = std::mem::transmute(at(RVA_STRING_ASSIGN));
    let dtor: StrDtorFn = std::mem::transmute(at(RVA_STRING_DTOR));
    let realloc: VecEmplaceFn = std::mem::transmute(at(RVA_VEC_EMPLACE_REALLOC));

    let mut s = MsvcString::empty();
    assign(&mut s, msg.as_ptr(), msg.len());

    let vec = at(RVA_NET_OUTBOX) as *mut c_void;
    let end_slot = (vec as usize + 8) as *mut *mut MsvcString;
    let cap_slot = (vec as usize + 16) as *mut *mut MsvcString;
    let end = *end_slot;
    if end != *cap_slot {
        // Move-construct in place, then leave `s` empty (same as the game's inlined path).
        std::ptr::copy_nonoverlapping(&s as *const MsvcString, end, 1);
        *end_slot = end.add(1);
        s = MsvcString::empty();
    } else {
        realloc(vec, end, &mut s);
    }
    dtor(&mut s);
}

/// Tell the other machine(s) in an online session that `player` received `items`.
/// Uses the game's own "protoga <playerId> <count> <items...>" message, which the
/// receiver answers by giving those items to that player.
pub unsafe fn broadcast_items(player: *mut c_void, items: &[i32]) {
    let id = net_id(player);
    for chunk in items.chunks(16) {
        let mut msg = format!("protoga {} {}", id, chunk.len());
        for it in chunk {
            msg.push_str(&format!(" {it}"));
        }
        log!("net: {msg}");
        queue_net_message(&msg);
    }
}
