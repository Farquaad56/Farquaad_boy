//! Application eframe : interface utilisateur et rendu.
//!
//! Partie 5 du guide d'implémentation : écran PPU (framebuffer rendu en zero-copy bytemuck)
//! + panneau de debug CPU/PPU avec contrôles d'exécution (Run / Step / Frame / Reset).

use crate::cpu::flags::Flags;
use crate::cpu::opcodes::disasm;
use crate::emulator::{Emulator, FRAME_TCYCLES};
use crate::ppu::{PPU, SCREEN_HEIGHT, SCREEN_WIDTH};
use egui::{Color32, Visuals};

/// État de l'application FarquaadGB.
pub struct FarquaadGBApp {
    emulator: Emulator,
    /// Nom du fichier ROM chargé (pour affichage).
    rom_name: Option<String>,
    /// Exécution automatique : une frame par update UI.
    running: bool,
    /// Fenêtre de debug « Pattern Table » (VRAM $8000-$97FF).
    show_pattern_table: bool,
    /// Texture écran (framebuffer PPU, NEAREST pour le pixel art).
    texture: Option<egui::TextureHandle>,
}

impl FarquaadGBApp {
    /// Crée l'application et configure le contexte egui (thème sombre, fond noir).
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = Visuals::dark();
        visuals.window_fill = Color32::BLACK;
        cc.egui_ctx.set_style(egui::Style {
            visuals,
            ..Default::default()
        });

        Self {
            emulator: Emulator::new(),
            rom_name: None,
            running: false,
            show_pattern_table: false,
            texture: None,
        }
    }

    /// Ouvre une boîte de dialogue et charge la ROM sélectionnée dans l'émulateur.
    fn load_rom(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Game Boy ROM", &["gb", "gbc"])
            .pick_file()
        {
            match std::fs::read(&path) {
                Ok(data) => {
                    self.emulator.load_rom(data);
                    self.rom_name = path.file_name().and_then(|n| n.to_str()).map(String::from);
                    self.running = true; // démarrage immédiat (séquence de boot + jeu)
                    log::info!("ROM chargée : {:?}", path);
                }
                Err(err) => log::error!("Échec de la lecture de {:?} : {err}", path),
            }
        }
    }

    /// Charge le programme test CPU intégré (partie 3).
    fn load_test_program(&mut self) {
        self.emulator.load_test_program();
        self.rom_name = Some("CPU TEST (intégré)".to_string());
        self.running = true; // exécution immédiate
    }

    /// Lit le titre du jeu (16 octets) dans l'en-tête cartouche à 0x0134.
    fn rom_title(&self) -> String {
        (0..16u16)
            .map(|i| self.emulator.mmu.read(0x0134 + i))
            .map(|b| if (0x20..=0x7E).contains(&b) { b as char } else { ' ' })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// Dessine la fenêtre de debug « Pattern Table » : les 384 tuiles de la VRAM ($8000-$97FF) en grille
    /// de 16×24, colorées selon la palette BGP (teintes DMG).
    fn show_pattern_table_debug(&self, ui: &mut egui::Ui) {
        let vram = &self.emulator.mmu.vram;
        let bgp = self.emulator.mmu.ppu.bgp;

        const TILE_COLS: usize = 16; // 16 colonnes de tuiles (16 × 24 = 384 tuiles)
        const TILE_ROWS: usize = 24;
        const PITCH: usize = 9; // tuile de 8 px + séparateur noir de 1 px

        let width = TILE_COLS * PITCH - 1; // 143 px
        let height = TILE_ROWS * PITCH - 1; // 215 px
        let mut pixels = vec![0xFF00_0000u32; width * height];

        for tile in 0..(TILE_COLS * TILE_ROWS) {
            let addr = tile * 16; // offset depuis $8000 dans vram
            let col = (tile % TILE_COLS) * PITCH;
            let row = (tile / TILE_COLS) * PITCH;
            for ty in 0..8usize {
                let upper = vram[addr + ty]; // bitplan 1 : MSB de chaque paire de pixels
                let lower = vram[addr + 8 + ty]; // bitplan 2 : LSB de chaque paire de pixels
                for tx in 0..8usize {
                    let value = (((upper >> (7 - tx)) & 1) << 1) | ((lower >> (7 - tx)) & 1);
                    pixels[(row + ty) * width + col + tx] = PPU::shade(bgp >> (value * 2));
                }
            }
        }

        let image = egui::ColorImage::from_rgba_unmultiplied([width, height], bytemuck::cast_slice(&pixels));
        let texture = ui.ctx().load_texture("pattern_table", image, egui::TextureOptions::NEAREST);
        ui.add(egui::Image::new(&texture).fit_to_exact_size(egui::vec2(width as f32 * 3.0, height as f32 * 3.0)));
    }

    /// Dessine le panneau de debug CPU.
    fn show_cpu_debug(&mut self, ui: &mut egui::Ui) {
        let cpu = &self.emulator.cpu;
        let mmu = &self.emulator.mmu;

        ui.heading("🔍 CPU Debug");
        ui.separator();

        // Registres (hexadécimal).
        egui::Grid::new("cpu_debug_registers").show(ui, |g| {
            g.label("AF");
            g.monospace(format!("{:04X}", cpu.af()));
            g.end_row();
            g.label("BC");
            g.monospace(format!("{:04X}", ((cpu.b as u16) << 8) | cpu.c as u16));
            g.end_row();
            g.label("DE");
            g.monospace(format!("{:04X}", ((cpu.d as u16) << 8) | cpu.e as u16));
            g.end_row();
            g.label("HL");
            g.monospace(format!("{:04X}", cpu.hl()));
            g.end_row();
            g.label("SP");
            g.monospace(format!("{:04X}", cpu.sp));
            g.end_row();
            g.label("PC");
            g.monospace(format!("{:04X}", cpu.pc));
            g.end_row();
        });

        // Drapeaux et IME.
        let f = cpu.flags();
        ui.label(format!(
            "Flags  Z:{} N:{} H:{} C:{}",
            u8::from(f.contains(Flags::Z)),
            u8::from(f.contains(Flags::N)),
            u8::from(f.contains(Flags::H)),
            u8::from(f.contains(Flags::C))
        ));
        ui.label(format!("IME : {}", if cpu.ime { "on" } else { "off" }));

        // Instruction suivante à l'adresse PC.
        ui.monospace(format!("{:04X}: {}", cpu.pc, disasm(cpu, mmu)));

        // Compteurs.
        ui.separator();
        ui.label(format!("T-cycles : {}", self.emulator.t_cycles));
        ui.label(format!("Instructions : {}", self.emulator.instructions));

        // PPU (étape 1 : registres + mode courant).
        ui.separator();
        let ppu = &mmu.ppu;
        ui.heading("📺 PPU");
        ui.label(format!("LY : {} / 153", ppu.ly));
        ui.label(format!(
            "Mode : {}",
            if (ppu.lcdc & 0x80) == 0 {
                "Off (LCD éteint)"
            } else {
                match ppu.mode {
                    0 => "HBlank",
                    1 => "VBlank",
                    2 => "OAM Scan",
                    _ => "Drawing",
                }
            }
        ));
        ui.label(format!("Dot : {} / 456 | LYC : {}", ppu.dots, ppu.lyc));

        // Port série (partie 7) : dernière ligne reçue sur le câble link.
        ui.separator();
        let serial = &mmu.serial;
        ui.heading("📡 Serial");
        if serial.last_line().is_empty() {
            ui.weak("Aucune donnée reçue sur le port link ($FF01/$FF02) pour l'instant.");
        } else {
            ui.monospace(serial.last_line());
        }

        // Contrôles d'exécution.
        ui.separator();
        let has_rom = mmu.rom_size() > 0;
        if !has_rom {
            ui.weak("Chargez une ROM ou le test CPU pour exécuter du code.");
        }
        ui.checkbox(&mut self.running, "▶ Run (1 frame/update)");
        if has_rom && ui.button("⏭ Step 1 instruction").clicked() {
            self.emulator.step();
        }
        if has_rom && ui.button(format!("⚡ Frame ({FRAME_TCYCLES} T-cycles)")).clicked() {
            self.emulator.run_frame();
        }
        if ui.button("↺ Reset CPU").clicked() {
            self.emulator.reset();
        }
    }
}

impl eframe::App for FarquaadGBApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Exécution automatique : une frame par update UI (~60 FPS).
        if self.running && self.emulator.mmu.rom_size() > 0 {
            self.emulator.run_frame();
            // Force egui à se redessiner sans attendre un événement souris/clavier :
            // sinon l'application passe en veille et le CPU n'avance que sur les événements UI.
            ctx.request_repaint();
        }

        // Texture écran (zero-copy via bytemuck, NEAREST pour le pixel art).
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [SCREEN_WIDTH, SCREEN_HEIGHT],
            bytemuck::cast_slice(&self.emulator.mmu.ppu.framebuffer),
        );
        match &mut self.texture {
            Some(texture) => texture.set(image, egui::TextureOptions::NEAREST),
            None => {
                self.texture =
                    Some(ctx.load_texture("gb_screen", image, egui::TextureOptions::NEAREST))
            }
        }

        // Barre de menu.
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                if ui.button("📂 Ouvrir ROM").clicked() {
                    self.load_rom();
                }
                if ui.button("🧪 CPU Test").clicked() {
                    self.load_test_program();
                }
                ui.checkbox(&mut self.show_pattern_table, "🔍 Pattern Table");
                ui.separator();
                match &self.rom_name {
                    Some(name) => {
                        let title = self.rom_title();
                        let size_kib = self.emulator.mmu.rom_size() / 1024;
                        if title.is_empty() {
                            ui.label(format!("{name} ({size_kib} KiB)"));
                        } else {
                            ui.label(format!("{name} ({size_kib} KiB) — « {title} »"));
                        }
                    }
                    None => {
                        ui.weak("Aucune ROM chargée");
                    }
                }
            });
        });

        // Zone écran : framebuffer PPU affiché en 4× (640×576).
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, Color32::BLACK);
            ui.vertical_centered(|ui| {
                if let Some(texture) = &self.texture {
                    ui.add(
                        egui::Image::new(texture).fit_to_exact_size(egui::vec2(640.0, 576.0)),
                    );
                } else {
                    ui.heading("…");
                }
                if self.rom_name.is_none() {
                    ui.weak("Chargez une ROM to start (l'écran montre l'état power-on)");
                }
            });
        });

        // Panneau de debug CPU (côté droit).
        egui::SidePanel::right("cpu_debug")
            .default_width(250.0)
            .min_width(190.0)
            .show(ctx, |ui| {
                self.show_cpu_debug(ui);
            });

        // Fenêtre de debug « Pattern Table » (VRAM $8000-$97FF).
        if self.show_pattern_table {
            egui::Window::new("🔍 Pattern Table (VRAM $8000)")
                .show(ctx, |ui| {
                    self.show_pattern_table_debug(ui);
                });
        }
    }
}