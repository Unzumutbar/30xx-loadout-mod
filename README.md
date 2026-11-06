# 30XX Loadout Mod

Choose the upgrades each player starts a run with in **30XX**: Cores, Weapons & Powers,
Power/Weapon Upgrades, Augs, Boons and Hexes. Pick them in an in-game menu with the
game's own icons and descriptions. Works solo and in local coop; online coop is
supported but not fully tested yet.

> Unofficial fan mod. Not affiliated with or endorsed by Batterystaple Games.
> It contains no game assets: icons and text are read from your own install at runtime.

## Install
1. Get `dinput8.dll` (build it, see below).
2. Copy it into the game folder next to `30XX.exe`
   (Steam → 30XX → Manage → Browse local files).
3. Start the game. "30XX Loadout mod loaded - press F8" appears top-left for a few seconds.

**Uninstall:** delete `dinput8.dll`. Optionally also delete `loadout30xx.json` and `loadout30xx.log`.

## Use
* **F8** opens/closes the menu. The game ignores input while it is open.
* Tick items in the **Player 1** / **Player 2** tabs, or click their names.
  Hover an item for its full description. Search matches names and descriptions.
* Your choice is saved to `loadout30xx.json` in the game folder.
* When a new run starts, every local player receives their list automatically,
  at the same moment the game applies Codebreakers.
* **Give selected items now** gives the lists immediately, if you already started a
  run. Pressing it twice gives everything twice.
* Untick **Enabled** to play vanilla without uninstalling.

## Coop
* **Local coop:** Player 1 and Player 2 have separate lists.
* **Online coop:** both players install the mod. Each player sets up the **Player 1**
  tab on their own PC (that's "you" on your machine). Your items are sent to your
  partner using the game's own item-sync message, the same one the *Loadout*
  Codebreaker uses. If an online client gets nothing at run start, use
  *Give selected items now* in the first room, and please open an issue with
  `loadout30xx.log` attached.

## How it works
`dinput8.dll` is a proxy DLL: the game imports `DirectInput8Create` from it, the mod
forwards that call to the real system DLL, and on a background thread it:

* hooks the game's run-start routine that applies Codebreakers (`ApplyCodebreakers`)
  and calls the game's own `GiveItem` for each selected item;
* for online play, queues `protoga <playerId> <count> <items…>` network messages,
  the format the game already uses to sync randomly rolled starting items;
* draws a Dear ImGui overlay through [hudhook](https://github.com/veeenu/hudhook)
  (DirectX 11 and 9).

All addresses target the current Steam build. Before hooking, the mod checks byte
signatures at every address it uses. After a game update the menu shows
"Disabled: signature mismatch" instead of risking a crash.

Item ids, keys and icon paths in `src/items.rs` were extracted from the game
executable's item and icon tables. Names and descriptions come from the game's
`data/c.bin` at runtime.

## Build
Requires Rust (MSVC toolchain) on Windows.

```bash
cargo build --release
```

The output is `target/release/dinput8.dll`.

## Notes
* Items without an icon in the game files are mostly unused 20XX-era leftovers.
  They are listed but may behave oddly.
* Giving a character the other character's weapons or powers is allowed; the game
  decides what happens.

## License
MIT. See [LICENSE](LICENSE).
