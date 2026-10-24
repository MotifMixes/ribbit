//! The built-in debugger: a memory viewer that edits like a hex editor, and
//! a CPU window (registers, the code around the PC, pause, step, continue,
//! breakpoints), for whichever console is running.
//!
//! The windows only draw a `Snapshot` the app takes each frame (the rows on
//! screen, the registers, the code) and hand back `Action`s; the app reads
//! and writes the machine (app.rs `debug_snapshot`, `apply_debug`), so the
//! emulation thread is held only for that moment. Reads and writes go
//! around the hardware registers (no side effects): RAM, VRAM, SRAM and
//! ROMs only.
//!
//! On the desktop each opens as a window of its own (`Popouts`), beside the
//! game; on Android, or where the graphics driver cannot show a second
//! window, they are drawn inside Ribbit's window (`windows`).

use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

/// A stretch of a console's memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub name: &'static str,
    pub base: u32,
    pub len: u32,
    pub writable: bool,
}

/// What the windows show this frame.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// The console ("Leapster", "LeapPad"…).
    pub system: String,
    pub regions: Vec<Region>,
    /// The bytes on screen, from `DebugUi::top` (None: not readable).
    pub bytes: Vec<Option<u8>>,
    pub registers: Vec<(String, u32)>,
    pub pc: Option<u32>,
    /// Instructions from `DebugUi::code_at` (address, length, text).
    pub code: Vec<(u32, u32, String)>,
    pub breakpoints: Vec<u32>,
    pub paused: bool,
    /// Whether this console can pause, step and stop at breakpoints.
    pub can_step: bool,
}

/// What the windows ask the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Poke(u32, u8),
    Pause,
    Continue,
    Step,
    AddBreakpoint(u32),
    RemoveBreakpoint(u32),
}

/// Rows of 16 bytes the memory window shows.
pub const ROWS: u32 = 24;

/// The windows' own state.
#[derive(Debug, Default)]
pub struct DebugUi {
    pub show_memory: bool,
    pub show_cpu: bool,
    /// The region chosen, and the first address on screen.
    pub region: usize,
    pub top: u32,
    goto: String,
    /// The byte being edited and its first hex digit, once typed.
    cursor: Option<u32>,
    nibble: Option<u8>,
    /// Where the code listing starts (follows the PC unless set).
    pub code_at: Option<u32>,
    code_goto: String,
    breakpoint_text: String,
    status: String,
    /// Drawn inside Ribbit's window, not in windows of their own.
    pub in_app: bool,
}

impl DebugUi {
    /// Whether any window is open (the app takes a snapshot only then).
    pub fn open(&self) -> bool {
        self.show_memory || self.show_cpu
    }

    /// The first address the memory window wants, in `region`.
    pub fn view(&mut self, regions: &[Region]) -> u32 {
        if let Some(r) = regions.get(self.region.min(regions.len().saturating_sub(1))) {
            let end = r.base.saturating_add(r.len.saturating_sub(16 * ROWS));
            self.top = self.top.clamp(r.base, end.max(r.base)) & !15;
        }
        self.top
    }
}

fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches("0X").replace('_', "");
    u32::from_str_radix(&s, 16).ok()
}

/// Draws the open windows.
pub fn windows(ctx: &egui::Context, ui_state: &mut DebugUi, snap: &Snapshot, actions: &mut Vec<Action>) {
    if ui_state.show_memory {
        memory_window(ctx, ui_state, snap, actions);
    }
    if ui_state.show_cpu {
        cpu_window(ctx, ui_state, snap, actions);
    }
}

fn memory_window(ctx: &egui::Context, st: &mut DebugUi, snap: &Snapshot, actions: &mut Vec<Action>) {
    let mut open = true;
    egui::Window::new(format!("Memory — {}", snap.system))
        .open(&mut open)
        .resizable(false)
        .default_width(640.0)
        .default_pos(ctx.content_rect().left_top() + egui::vec2(16.0, 40.0))
        .show(ctx, |ui| memory_ui(ui, st, snap, actions));
    if !open {
        st.show_memory = false;
    }
}

pub fn memory_ui(ui: &mut egui::Ui, st: &mut DebugUi, snap: &Snapshot, actions: &mut Vec<Action>) {
    let mono = egui::FontId::monospace(13.0);
    ui.horizontal(|ui| {
        let names: Vec<&str> = snap.regions.iter().map(|r| r.name).collect();
        let current = names.get(st.region).copied().unwrap_or("—");
        egui::ComboBox::from_id_salt("debug-region").selected_text(current).show_ui(ui, |ui| {
            for (i, r) in snap.regions.iter().enumerate() {
                if ui.selectable_label(st.region == i, format!("{}  {:08X}", r.name, r.base)).clicked() {
                    st.region = i;
                    st.top = r.base;
                    st.cursor = None;
                }
            }
        });
        ui.label("Go to");
        let edit = ui.add(egui::TextEdit::singleline(&mut st.goto).desired_width(90.0).hint_text("address"));
        if edit.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(a) = parse_hex(&st.goto)
        {
            if let Some(i) = snap.regions.iter().position(|r| a >= r.base && a - r.base < r.len) {
                st.region = i;
            }
            st.top = a & !15;
            st.cursor = Some(a);
        }
        for (label, delta) in [
            ("Up 8", -(16 * ROWS as i64) * 8),
            ("Up", -(16 * ROWS as i64)),
            ("Down", 16 * ROWS as i64),
            ("Down 8", 16 * ROWS as i64 * 8),
        ] {
            if ui
                .small_button(label)
                .on_hover_text(if delta.abs() > 16 * ROWS as i64 { "8 pages" } else { "A page" })
                .clicked()
            {
                st.top = (st.top as i64 + delta).max(0) as u32;
            }
        }
    });
    let region = snap.regions.get(st.region).cloned();
    // Scrolling the list moves through the region a row at a time.
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if ui.ui_contains_pointer() && scroll.abs() > 0.5 {
        let rows = (scroll / 20.0).round() as i64;
        st.top = (st.top as i64 - rows.clamp(-64, 64) * 16).max(0) as u32;
    }
    ui.separator();
    let mut header = String::from("Address   ");
    for k in 0..16 {
        header.push_str(&format!("{k:02X} "));
    }
    header.push_str(" ASCII");
    ui.label(egui::RichText::new(header).font(mono.clone()).weak());
    for row in 0..ROWS {
        let addr = st.top + row * 16;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(
                egui::RichText::new(format!("{addr:08X}  ")).font(mono.clone()).color(egui::Color32::from_gray(150)),
            );
            let mut ascii = String::new();
            for k in 0..16 {
                let a = addr + k;
                let byte = snap.bytes.get((row * 16 + k) as usize).copied().flatten();
                let text = match (st.cursor == Some(a), st.nibble, byte) {
                    (true, Some(n), _) => format!("{n:X}_ "),
                    (_, _, Some(b)) => format!("{b:02X} "),
                    _ => "-- ".to_string(),
                };
                let mut rich = egui::RichText::new(text).font(mono.clone());
                if st.cursor == Some(a) {
                    rich = rich.background_color(egui::Color32::from_rgb(40, 90, 200)).color(egui::Color32::WHITE);
                } else if snap.pc.is_some_and(|pc| a == pc) {
                    rich = rich.color(egui::Color32::from_rgb(240, 200, 80));
                }
                if ui.add(egui::Label::new(rich).sense(egui::Sense::click())).clicked() {
                    st.cursor = Some(a);
                    st.nibble = None;
                }
                ascii.push(match byte {
                    Some(b) if (0x20..0x7F).contains(&b) => b as char,
                    Some(_) => '.',
                    None => ' ',
                });
            }
            ui.label(egui::RichText::new(format!(" {ascii}")).font(mono.clone()).color(egui::Color32::from_gray(170)));
        });
    }
    ui.separator();
    // Typing hex digits writes the byte under the cursor (two digits a
    // byte, then on to the next); arrows move it.
    if let Some(c) = st.cursor {
        let writable = region.as_ref().is_some_and(|r| r.writable && c >= r.base && c - r.base < r.len);
        ui.horizontal(|ui| {
            ui.label(format!("{c:08X}"));
            ui.label(if writable { "type two hex digits to change it" } else { "read only here" });
        });
        let typed: Vec<u8> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect::<String>()
                .chars()
                .filter_map(|ch| ch.to_digit(16).map(|d| d as u8))
                .collect()
        });
        let mut cursor = c;
        for d in typed {
            if !writable {
                break;
            }
            match st.nibble.take() {
                None => st.nibble = Some(d),
                Some(hi) => {
                    actions.push(Action::Poke(cursor, hi << 4 | d));
                    cursor += 1;
                }
            }
        }
        let moves = ui.input(|i| {
            [
                (egui::Key::ArrowLeft, -1i64),
                (egui::Key::ArrowRight, 1),
                (egui::Key::ArrowUp, -16),
                (egui::Key::ArrowDown, 16),
            ]
            .iter()
            .filter(|(k, _)| i.key_pressed(*k))
            .map(|(_, d)| *d)
            .sum::<i64>()
        });
        cursor = (cursor as i64 + moves).max(0) as u32;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            st.cursor = None;
            st.nibble = None;
        } else {
            st.cursor = Some(cursor);
            // Keep it on screen.
            if cursor < st.top {
                st.top = cursor & !15;
            } else if cursor >= st.top + 16 * ROWS {
                st.top = (cursor & !15) - 16 * (ROWS - 1);
            }
        }
    } else {
        ui.label(egui::RichText::new("Click a byte to change it. Scroll or use the arrows to move.").weak());
    }
}

fn cpu_window(ctx: &egui::Context, st: &mut DebugUi, snap: &Snapshot, actions: &mut Vec<Action>) {
    let mut open = true;
    // Opens beside the memory window, not over it.
    egui::Window::new(format!("Debugger — {}", snap.system))
        .open(&mut open)
        .resizable(false)
        .default_pos(egui::pos2((ctx.content_rect().right() - 600.0).max(16.0), ctx.content_rect().top() + 40.0))
        .show(ctx, |ui| cpu_ui(ui, st, snap, actions));
    if !open {
        st.show_cpu = false;
    }
}

pub fn cpu_ui(ui: &mut egui::Ui, st: &mut DebugUi, snap: &Snapshot, actions: &mut Vec<Action>) {
    let mono = egui::FontId::monospace(13.0);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(snap.can_step, |ui| {
            if snap.paused {
                if ui.button("▶ Continue").clicked() {
                    actions.push(Action::Continue);
                }
                if ui.button("Step").on_hover_text("Run one instruction").clicked() {
                    actions.push(Action::Step);
                }
            } else if ui.button("⏸ Pause").clicked() {
                actions.push(Action::Pause);
            }
        });
        if let Some(pc) = snap.pc {
            ui.label(
                egui::RichText::new(format!("PC {pc:08X}"))
                    .font(mono.clone())
                    .color(egui::Color32::from_rgb(240, 200, 80)),
            );
        }
        ui.label(egui::RichText::new(if snap.paused { "paused" } else { "running" }).weak());
    });
    if !snap.can_step {
        ui.label(egui::RichText::new("Pause, step and breakpoints are not available for this console yet.").weak());
    }
    ui.separator();
    ui.horizontal_top(|ui| {
        // Registers, two to a line.
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("Registers").strong());
            egui::Grid::new("debug-registers").spacing([10.0, 2.0]).show(ui, |ui| {
                for pair in snap.registers.chunks(2) {
                    for (n, v) in pair {
                        ui.add(egui::Label::new(egui::RichText::new(n.as_str()).font(mono.clone()).weak()).extend());
                        ui.add(egui::Label::new(egui::RichText::new(format!("{v:08X}")).font(mono.clone())).extend());
                    }
                    ui.end_row();
                }
            });
        });
        // (Spacing, not a separator: a separator here would stretch
        // the window to the screen's height.)
        ui.add_space(16.0);
        // The code from the PC (or where chosen).
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Code").strong());
                let edit =
                    ui.add(egui::TextEdit::singleline(&mut st.code_goto).desired_width(80.0).hint_text("address"));
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    st.code_at = parse_hex(&st.code_goto);
                }
                if ui.small_button("PC").on_hover_text("Follow the PC").clicked() {
                    st.code_at = None;
                    st.code_goto.clear();
                }
            });
            for (addr, _, text) in &snap.code {
                let bp = snap.breakpoints.contains(addr);
                let here = snap.pc == Some(*addr);
                let line = format!("{} {addr:08X}  {text}", if bp { "●" } else { " " });
                let mut rich = egui::RichText::new(line).font(mono.clone());
                if here {
                    rich = rich
                        .background_color(egui::Color32::from_rgb(70, 60, 20))
                        .color(egui::Color32::from_rgb(250, 220, 120));
                } else if bp {
                    rich = rich.color(egui::Color32::from_rgb(240, 110, 100));
                }
                let r = ui
                    .add(egui::Label::new(rich).extend().sense(egui::Sense::click()))
                    .on_hover_text("Click: breakpoint on or off");
                if r.clicked() && snap.can_step {
                    actions.push(if bp { Action::RemoveBreakpoint(*addr) } else { Action::AddBreakpoint(*addr) });
                }
            }
        });
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Breakpoint at");
        ui.add(egui::TextEdit::singleline(&mut st.breakpoint_text).desired_width(90.0).hint_text("address"));
        if ui.add_enabled(snap.can_step, egui::Button::new("Add")).clicked() {
            match parse_hex(&st.breakpoint_text) {
                Some(a) => {
                    actions.push(Action::AddBreakpoint(a));
                    st.status.clear();
                }
                None => st.status = "Not a hex address".into(),
            }
        }
        if !st.status.is_empty() {
            ui.label(egui::RichText::new(&st.status).weak());
        }
    });
    if !snap.breakpoints.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for &b in &snap.breakpoints {
                if ui
                    .small_button(egui::RichText::new(format!("● {b:08X} ×")).monospace())
                    .on_hover_text("Remove")
                    .clicked()
                {
                    actions.push(Action::RemoveBreakpoint(b));
                }
            }
        });
    }
}

/// The Leapster's memory read without side effects (RAM, VRAM, SRAM, ROMs),
/// as the instruction decoder reads it: unmapped bytes read as zero.
struct PeekBus<'a>(&'a ribbit_memory::SystemBus);

impl ribbit_memory::Bus for PeekBus<'_> {
    fn read8(&mut self, addr: u32) -> u8 {
        self.0.peek8(addr).unwrap_or(0)
    }
    fn read16(&mut self, addr: u32) -> u16 {
        u16::from_le_bytes([self.read8(addr), self.read8(addr.wrapping_add(1))])
    }
    fn read32(&mut self, addr: u32) -> u32 {
        self.read16(addr) as u32 | (self.read16(addr.wrapping_add(2)) as u32) << 16
    }
    fn write8(&mut self, _: u32, _: u8) {}
    fn write16(&mut self, _: u32, _: u16) {}
    fn write32(&mut self, _: u32, _: u32) {}
}

/// The Leapster's names for its special core registers.
fn leapster_reg_name(r: usize) -> String {
    match r {
        26 => "r26 gp".into(),
        27 => "r27 fp".into(),
        28 => "r28 sp".into(),
        29 => "ilink1".into(),
        30 => "ilink2".into(),
        31 => "blink".into(),
        _ => format!("r{r}"),
    }
}

/// What the windows show for a Leapster (Leapster TV, L-MAX, Leapster 2).
pub fn leapster_snapshot(m: &ribbit_core::Machine, dbg: &mut DebugUi, breakpoints: &[u32], paused: bool) -> Snapshot {
    use ribbit_memory::map::{BIOS_MIRROR, CART_BASE, RAM_BASE, SRAM_BASE, VRAM_BASE, VRAM_SIZE};
    let bus = &m.bus;
    let regions = vec![
        Region { name: "RAM", base: RAM_BASE, len: bus.ram().len() as u32, writable: true },
        Region { name: "VRAM", base: VRAM_BASE, len: VRAM_SIZE, writable: true },
        Region { name: "On-chip SRAM", base: SRAM_BASE, len: bus.sram().len() as u32, writable: true },
        Region { name: "BIOS", base: BIOS_MIRROR, len: 0x0040_0000, writable: false },
        Region { name: "Cartridge", base: CART_BASE, len: 0x0100_0000, writable: false },
    ];
    let top = dbg.view(&regions);
    let bytes = (0..16 * ROWS).map(|k| bus.peek8(top.wrapping_add(k))).collect();
    let cpu = &m.cpu;
    let mut registers: Vec<(String, u32)> = (0..32).map(|r| (leapster_reg_name(r), cpu.regs[r])).collect();
    registers.extend([
        ("status32".to_string(), cpu.status32),
        ("lp_start".to_string(), cpu.lp_start),
        ("lp_end".to_string(), cpu.lp_end),
        ("lp_count".to_string(), cpu.regs.get(60).copied().unwrap_or(0)),
    ]);
    let mut at = dbg.code_at.unwrap_or(cpu.pc);
    let mut code = Vec::new();
    let mut peek = PeekBus(bus);
    for _ in 0..24 {
        let i = ribbit_cpu::decode::decode(&mut peek, at);
        code.push((at, i.len, ribbit_cpu::disasm::disassemble(&i)));
        at = at.wrapping_add(i.len.max(2));
    }
    Snapshot {
        system: "Leapster".into(),
        regions,
        bytes,
        registers,
        pc: Some(cpu.pc),
        code,
        breakpoints: breakpoints.to_vec(),
        paused,
        can_step: true,
    }
}

/// Which of the debugger's windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Memory,
    Cpu,
}

/// One pop-out window: its surface, and an egui of its own.
pub struct Popout {
    surface: crate::gpu::ToolSurface,
    ctx: egui::Context,
    state: egui_winit::State,
    title: String,
    drawn: Option<Instant>,
}

/// The debugger's windows of their own.
#[derive(Default)]
pub struct Popouts {
    memory: Option<Popout>,
    cpu: Option<Popout>,
    /// One of them has the keyboard (the game is not "in the background").
    pub focused: bool,
}

/// How often a pop-out is redrawn: often enough to watch memory change,
/// without costing the game.
const REDRAW: Duration = Duration::from_millis(33);

impl Popouts {
    fn slot(&mut self, w: Which) -> &mut Option<Popout> {
        match w {
            Which::Memory => &mut self.memory,
            Which::Cpu => &mut self.cpu,
        }
    }

    /// Which window `id` is, if one of these.
    pub fn which(&self, id: WindowId) -> Option<Which> {
        let is = |p: &Option<Popout>| p.as_ref().is_some_and(|p| p.surface.window.id() == id);
        if is(&self.memory) {
            Some(Which::Memory)
        } else if is(&self.cpu) {
            Some(Which::Cpu)
        } else {
            None
        }
    }

    /// Opens the windows `st` asks for and closes the others. A window
    /// that cannot be shown (the driver) puts the debugger inside Ribbit's.
    pub fn sync(&mut self, event_loop: &ActiveEventLoop, gpu: &crate::gpu::Gpu, st: &mut DebugUi, system: &str) {
        for (w, want) in [(Which::Memory, st.show_memory), (Which::Cpu, st.show_cpu)] {
            let want = want && !st.in_app;
            let slot = self.slot(w);
            if !want {
                *slot = None;
                continue;
            }
            let title = match w {
                Which::Memory => format!("Memory — {system}"),
                Which::Cpu => format!("Debugger — {system}"),
            };
            if let Some(p) = slot {
                if p.title != title {
                    p.surface.window.set_title(&title);
                    p.title = title;
                }
                continue;
            }
            match open(event_loop, gpu, w, &title) {
                Ok(p) => *slot = Some(p),
                Err(e) => {
                    tracing::warn!(target: "ribbit", "the debugger's own window: {e:#}; shown inside Ribbit's instead");
                    st.in_app = true;
                }
            }
        }
        if self.memory.is_none() && self.cpu.is_none() {
            self.focused = false;
        }
    }

    /// A window event for window `w`.
    pub fn event(&mut self, w: Which, event: &winit::event::WindowEvent, gpu: &crate::gpu::Gpu, st: &mut DebugUi) {
        use winit::event::WindowEvent as E;
        let Some(p) = self.slot(w).as_mut() else { return };
        let _ = p.state.on_window_event(&p.surface.window, event);
        match event {
            E::CloseRequested | E::Destroyed => {
                *self.slot(w) = None;
                match w {
                    Which::Memory => st.show_memory = false,
                    Which::Cpu => st.show_cpu = false,
                }
                self.focused = false;
            }
            E::Resized(size) => gpu.resize_tool(&mut p.surface, *size),
            E::Focused(f) => self.focused = *f,
            _ => {}
        }
    }

    /// Asks each window for a picture now and then.
    pub fn request_redraws(&self) {
        for p in [&self.memory, &self.cpu].into_iter().flatten() {
            if p.drawn.is_none_or(|t| t.elapsed() >= REDRAW) {
                p.surface.window.request_redraw();
            }
        }
    }

    /// Draws window `w`.
    pub fn draw(
        &mut self,
        w: Which,
        gpu: &crate::gpu::Gpu,
        st: &mut DebugUi,
        snap: &Snapshot,
        actions: &mut Vec<Action>,
    ) -> anyhow::Result<()> {
        let Some(p) = self.slot(w).as_mut() else { return Ok(()) };
        p.drawn = Some(Instant::now());
        let raw = p.state.take_egui_input(&p.surface.window);
        let out = p.ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| match w {
                    Which::Memory => memory_ui(ui, st, snap, actions),
                    Which::Cpu => cpu_ui(ui, st, snap, actions),
                });
            });
        });
        p.state.handle_platform_output(&p.surface.window, out.platform_output);
        let ppp = out.pixels_per_point;
        let jobs = p.ctx.tessellate(out.shapes, ppp);
        let mut deltas = out.textures_delta;
        let result = gpu.render_tool(&mut p.surface, &jobs, &deltas, ppp);
        // Handled by `render_tool` (debug builds of egui panic when an
        // unhandled delta is dropped).
        deltas.clear();
        result
    }
}

fn open(event_loop: &ActiveEventLoop, gpu: &crate::gpu::Gpu, w: Which, title: &str) -> anyhow::Result<Popout> {
    let size = match w {
        Which::Memory => winit::dpi::LogicalSize::new(720.0, 760.0),
        Which::Cpu => winit::dpi::LogicalSize::new(900.0, 720.0),
    };
    let attrs = Window::default_attributes()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size(winit::dpi::LogicalSize::new(320.0, 240.0))
        .with_window_icon(crate::icon::window_icon());
    let window = Arc::new(event_loop.create_window(attrs)?);
    let surface = gpu.tool_surface(window.clone())?;
    let ctx = crate::app::egui_context();
    let state = egui_winit::State::new(
        ctx.clone(),
        egui::ViewportId::ROOT,
        &window,
        Some(window.scale_factor() as f32),
        None,
        None,
    );
    Ok(Popout { surface, ctx, state, title: title.to_string(), drawn: None })
}
