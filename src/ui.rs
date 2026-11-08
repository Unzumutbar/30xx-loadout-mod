//! The in-game loadout menu (Dear ImGui overlay). Toggle with F8.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use hudhook::imgui::{Condition, Context, Image, Io, TextureId, TreeNodeFlags, Ui};
use hudhook::{ImguiRenderLoop, MessageFilter, RenderContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F8};

use crate::config::game_dir;
use crate::items::{self, Cat, Item, ITEMS};
use crate::{on_frame, state};

static VISIBLE: AtomicBool = AtomicBool::new(false);

/// Icons uploaded per frame, so opening the game never stalls on ~400 PNG decodes.
const ICONS_PER_FRAME: usize = 24;

#[derive(Default)]
pub struct Menu {
    f8_down: bool,
    filter: [String; 2],
    shown_hint_frames: u32,
    /// Inventory editor: which local player, and the "Add items" search text.
    inv_slot: usize,
    inv_filter: String,
    textures: HashMap<&'static str, TextureId>,
    /// Icon paths not uploaded yet (filled on first frame).
    pending: Option<Vec<&'static str>>,
    failed: HashSet<&'static str>,
}

fn find(id: i32) -> Option<&'static Item> {
    ITEMS.iter().find(|i| i.id == id)
}

/// Decode a PNG into RGBA8 and upscale small pixel-art icons with nearest neighbour,
/// so they stay sharp when drawn larger than 32x32.
fn load_png(path: &str) -> Option<(Vec<u8>, u32, u32)> {
    let file = std::fs::File::open(game_dir().join(path)).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let px = (w * h) as usize;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..px * 4].to_vec(),
        png::ColorType::Rgb => buf[..px * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..px * 2].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None, // EXPAND turns palettes into RGB(A)
    };
    let scale = if w.max(h) <= 64 { 4 } else { 1 };
    if scale == 1 {
        return Some((rgba, w, h));
    }
    let (sw, sh) = (w * scale, h * scale);
    let mut out = vec![0u8; (sw * sh * 4) as usize];
    for y in 0..sh {
        for x in 0..sw {
            let s = (((y / scale) * w + x / scale) * 4) as usize;
            let d = ((y * sw + x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    Some((out, sw, sh))
}

impl Menu {
    fn poll_toggle(&mut self) {
        let down = unsafe { GetAsyncKeyState(VK_F8 as i32) } as u16 & 0x8000 != 0;
        if down && !self.f8_down {
            VISIBLE.fetch_xor(true, Ordering::SeqCst);
        }
        self.f8_down = down;
    }

    fn upload_icons(&mut self, rc: &mut dyn RenderContext) {
        let pending = self.pending.get_or_insert_with(|| {
            let mut v: Vec<&'static str> = ITEMS.iter().map(|i| i.icon).filter(|p| !p.is_empty()).collect();
            v.sort_unstable();
            v.dedup();
            v
        });
        for _ in 0..ICONS_PER_FRAME {
            let Some(path) = pending.pop() else { return };
            match load_png(path).and_then(|(d, w, h)| rc.load_texture(&d, w, h).ok()) {
                Some(t) => {
                    self.textures.insert(path, t);
                }
                None => {
                    crate::log!("icon failed to load: {path}");
                    self.failed.insert(path);
                }
            }
        }
    }

    /// Draw an item's icon (or an empty placeholder of the same size).
    fn icon(&self, ui: &Ui, it: &Item, size: f32) {
        match self.textures.get(it.icon) {
            Some(&t) => Image::new(t, [size, size]).build(ui),
            None => ui.dummy([size, size]),
        }
    }

    fn tooltip(&self, ui: &Ui, it: &Item) {
        let s = ui.io().font_global_scale;
        ui.tooltip(|| {
            let _wrap = ui.push_text_wrap_pos_with_pos(ui.current_font_size() * 26.0);
            self.icon(ui, it, 64.0 * s);
            ui.same_line();
            ui.group(|| {
                ui.text(items::name(it));
                ui.text_disabled(it.cat.label());
                if it.icon.is_empty() {
                    ui.text_disabled("(no icon in game files - may be an unused item)");
                }
            });
            if !items::text(it).desc.is_empty() {
                ui.separator();
                ui.text_colored([1.0, 0.85, 0.4, 1.0], &items::text(it).desc);
            }
            if !items::text(it).lore.is_empty() {
                ui.spacing();
                ui.text_wrapped(&items::text(it).lore);
            }
        });
    }

    /// Icon + name for any item id (catalogue items get icon and tooltip).
    fn item_label(&self, ui: &Ui, id: i32, size: f32) {
        match items::by_id(id) {
            Some(it) => {
                self.icon(ui, it, size);
                let mut hovered = ui.is_item_hovered();
                ui.same_line();
                ui.text(items::name(it));
                hovered |= ui.is_item_hovered();
                if hovered {
                    self.tooltip(ui, it);
                }
            }
            None => {
                ui.dummy([size, size]);
                ui.same_line();
                ui.text(items::name_of(id));
            }
        }
    }

    fn inventory_tab(&mut self, ui: &Ui) {
        let s = ui.io().font_global_scale;
        let row_icon = 26.0 * s;

        let present: Vec<bool> = (0..2).map(|i| unsafe { crate::local_player(i).is_some() }).collect();
        if !present[self.inv_slot] {
            self.inv_slot = if present[0] { 0 } else { 1 };
        }
        for slot in 0..2 {
            if present[slot] {
                if slot > 0 {
                    ui.same_line();
                }
                ui.radio_button(format!("Player {}", slot + 1), &mut self.inv_slot, slot);
            }
        }
        let Some(player) = (unsafe { crate::local_player(self.inv_slot) }) else {
            ui.text_disabled("No local player loaded yet - start or continue a run.");
            return;
        };
        let inv = unsafe { crate::game::inventory(player) };
        let slot = self.inv_slot;
        let mut edits: Vec<crate::Edit> = Vec::new();
        ui.text_disabled("Changes apply immediately using the game's own add/remove code.");

        ui.child_window("inv").size([0.0, 0.0]).border(true).build(|| {
            // --- Augs / Boons / Hexes: stackable ------------------------------------
            if ui.collapsing_header(format!("Augs, Boons & Hexes ({})", inv.augs.len()), TreeNodeFlags::DEFAULT_OPEN) {
                if inv.augs.is_empty() {
                    ui.text_disabled("None");
                }
                for &(id, count) in &inv.augs {
                    let _id = ui.push_id_int(id);
                    if ui.small_button(" - ") {
                        edits.push(crate::Edit { slot, item: id, delta: -1 });
                    }
                    ui.same_line();
                    ui.text(format!("{count:>2}"));
                    ui.same_line();
                    if ui.small_button(" + ") {
                        edits.push(crate::Edit { slot, item: id, delta: 1 });
                    }
                    ui.same_line();
                    if ui.small_button("Remove") {
                        edits.push(crate::Edit { slot, item: id, delta: -count });
                    }
                    ui.same_line();
                    self.item_label(ui, id, row_icon);
                }
            }
            // --- Cores / primary weapons ----------------------------------------------
            if ui.collapsing_header(format!("Cores & Weapons ({})", inv.cores.len()), TreeNodeFlags::DEFAULT_OPEN) {
                if inv.cores.is_empty() {
                    ui.text_disabled("None");
                }
                for (n, &id) in inv.cores.iter().enumerate() {
                    let _id = ui.push_id_usize(10_000 + n);
                    if ui.small_button("Remove") {
                        edits.push(crate::Edit { slot, item: id, delta: -1 });
                    }
                    ui.same_line();
                    self.item_label(ui, id, row_icon);
                }
            }
            // --- Powers (read-only) -------------------------------------------------------
            if ui.collapsing_header(format!("Powers ({}) - view only", inv.powers.len()), TreeNodeFlags::empty()) {
                ui.text_disabled("Powers live in fixed slots; removing them isn't supported yet.");
                for &id in &inv.powers {
                    self.item_label(ui, id, row_icon);
                }
            }
            // --- Add any item -------------------------------------------------------------
            if ui.collapsing_header("Add items", TreeNodeFlags::empty()) {
                ui.input_text("Find", &mut self.inv_filter).hint("name or description").build();
                let needle = self.inv_filter.to_lowercase();
                if needle.len() < 2 {
                    ui.text_disabled("Type at least 2 letters.");
                } else {
                    let hits = ITEMS.iter().filter(|i| {
                        items::name(i).to_lowercase().contains(&needle) || items::text(i).desc.to_lowercase().contains(&needle)
                    });
                    for it in hits.take(40) {
                        let _id = ui.push_id_int(20_000 + it.id);
                        if ui.small_button("Add") {
                            edits.push(crate::Edit { slot, item: it.id, delta: 1 });
                        }
                        ui.same_line();
                        self.item_label(ui, it.id, row_icon);
                    }
                }
            }
        });

        if !edits.is_empty() {
            if let Ok(mut st) = state().lock() {
                st.edits.extend(edits);
            }
        }
    }

    fn player_tab(&mut self, ui: &Ui, slot: usize) {
        let s = ui.io().font_global_scale;
        let row_icon = 26.0 * s;
        let mut st = state().lock().unwrap();
        let mut changed = false;

        // --- current selection -------------------------------------------------
        let sel = st.config.players[slot].clone();
        ui.text(format!("Starting items for Player {} ({} selected)", slot + 1, sel.len()));
        if ui.small_button("Clear all") {
            st.config.players[slot].clear();
            changed = true;
        }
        if slot == 1 {
            ui.same_line();
            if ui.small_button("Copy Player 1's list") {
                st.config.players[1] = st.config.players[0].clone();
                changed = true;
            }
        }
        ui.child_window("selected").size([0.0, 170.0 * s]).border(true).build(|| {
            if sel.is_empty() {
                ui.text_disabled("Nothing selected - tick items below.");
            }
            for (n, id) in sel.iter().enumerate() {
                let _id = ui.push_id_usize(n);
                if ui.small_button("x") {
                    st.config.players[slot].retain(|x| x != id);
                    changed = true;
                }
                ui.same_line();
                match find(*id) {
                    Some(it) => {
                        self.icon(ui, it, row_icon);
                        let hovered = ui.is_item_hovered();
                        ui.same_line();
                        ui.group(|| {
                            ui.text(items::name(it));
                            if !items::text(it).desc.is_empty() {
                                ui.same_line();
                                ui.text_disabled(format!("- {}", &items::text(it).desc));
                            }
                        });
                        if hovered || ui.is_item_hovered() {
                            self.tooltip(ui, it);
                        }
                    }
                    None => ui.text(format!("item #{id}")),
                }
            }
        });

        // --- catalogue -----------------------------------------------------------
        ui.input_text("Search", &mut self.filter[slot]).hint("name or description").build();
        let needle = self.filter[slot].to_lowercase();
        ui.child_window("catalogue").size([0.0, 0.0]).border(true).build(|| {
            for cat in Cat::ALL {
                let matches: Vec<&Item> = ITEMS
                    .iter()
                    .filter(|i| i.cat == cat)
                    .filter(|i| {
                        needle.is_empty()
                            || items::name(i).to_lowercase().contains(&needle)
                            || i.key.contains(&needle)
                            || items::text(i).desc.to_lowercase().contains(&needle)
                    })
                    .collect();
                if matches.is_empty() {
                    continue;
                }
                let flags = if needle.is_empty() { TreeNodeFlags::empty() } else { TreeNodeFlags::DEFAULT_OPEN };
                let _cid = ui.push_id(cat.label());
                if !ui.collapsing_header(format!("{} ({})", cat.label(), matches.len()), flags) {
                    continue;
                }
                for it in matches {
                    let _iid = ui.push_id_int(it.id);
                    let mut on = st.config.players[slot].contains(&it.id);
                    let toggled = ui.checkbox("##on", &mut on);
                    ui.same_line();
                    self.icon(ui, it, row_icon);
                    let mut hovered = ui.is_item_hovered();
                    ui.same_line();
                    // Clicking the name toggles too.
                    let clicked = ui.selectable(items::name(it));
                    hovered |= ui.is_item_hovered();
                    if hovered {
                        self.tooltip(ui, it);
                    }
                    if toggled || clicked {
                        let now_on = if clicked { !st.config.players[slot].contains(&it.id) } else { on };
                        st.config.players[slot].retain(|x| *x != it.id);
                        if now_on {
                            st.config.players[slot].push(it.id);
                        }
                        changed = true;
                    }
                }
            }
        });

        if changed {
            st.config.save();
        }
    }
}

impl ImguiRenderLoop for Menu {
    fn before_render<'a>(&'a mut self, ctx: &mut Context, rc: &'a mut dyn RenderContext) {
        self.poll_toggle();
        let io = ctx.io_mut();
        io.mouse_draw_cursor = VISIBLE.load(Ordering::Relaxed);
        // Scale the UI with the game's resolution (designed at 720p).
        io.font_global_scale = (io.display_size[1] / 720.0).clamp(1.0, 3.0);
        self.upload_icons(rc);
    }

    fn render(&mut self, ui: &mut Ui) {
        // Runs on the game's render thread: safe place to execute "Give now".
        on_frame();

        if !VISIBLE.load(Ordering::Relaxed) {
            // Small hint for the first ~10 seconds after the game starts.
            if self.shown_hint_frames < 600 {
                self.shown_hint_frames += 1;
                ui.window("loadout_hint")
                    .position([10.0, 10.0], Condition::Always)
                    .no_decoration()
                    .always_auto_resize(true)
                    .bg_alpha(0.6)
                    .no_inputs()
                    .build(|| ui.text("30XX Loadout mod loaded - press F8"));
            }
            return;
        }

        let s = ui.io().font_global_scale;
        ui.window("30XX Loadout  (F8 to close)")
            .size([620.0 * s, 680.0 * s], Condition::FirstUseEver)
            .position([40.0, 40.0], Condition::FirstUseEver)
            .build(|| {
                {
                    let mut st = state().lock().unwrap();
                    let color = if st.hooks_ok { [0.4, 1.0, 0.4, 1.0] } else { [1.0, 0.4, 0.4, 1.0] };
                    ui.text_colored(color, &st.status);
                    if ui.checkbox("Enabled (give these items at the start of every new run)", &mut st.config.enabled) {
                        st.config.save();
                    }
                    if ui.button("Give selected items now") {
                        st.give_now = true;
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text(
                            "Gives each local player their list immediately.\n\
                             Use this only inside a run (e.g. if you started the run\n\
                             before choosing). Pressing it twice gives items twice.",
                        );
                    }
                    for h in &st.history {
                        ui.text_disabled(h);
                    }
                }
                ui.separator();
                ui.text_disabled("Hover an item for its description.");
                if let Some(_tb) = ui.tab_bar("players") {
                    for slot in 0..2 {
                        if let Some(_t) = ui.tab_item(format!("Player {}", slot + 1)) {
                            self.player_tab(ui, slot);
                        }
                    }
                    if let Some(_t) = ui.tab_item("Inventory (live)") {
                        self.inventory_tab(ui);
                    }
                }
            });
    }

    fn message_filter(&self, _io: &Io) -> MessageFilter {
        if VISIBLE.load(Ordering::Relaxed) {
            MessageFilter::InputAll
        } else {
            MessageFilter::empty()
        }
    }
}
