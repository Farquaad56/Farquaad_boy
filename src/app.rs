//! Application eframe : interface utilisateur et rendu.
//!
//! Partie 5 du guide d'implémentation : écran PPU (framebuffer rendu en zero-copy bytemuck)
//! + panneau de debug CPU/PPU avec contrôles d'exécution (Run / Step / Frame / Reset).

use crate::cpu::flags::Flags;
use crate::cpu::opcodes::disasm;
use crate::emulator::{Emulator, FRAME_TCYCLES};
use crate::mmu::MMU;
use crate::ppu::{PPU, SCREEN_HEIGHT, SCREEN_WIDTH};
use egui::{Align2, Color32, FontId, Visuals};

/// État de l'application FarquaadGB.
pub struct FarquaadGBApp {
    emulator: Emulator,
    /// Nom du fichier ROM chargé (pour affichage).
    rom_name: Option<String>,
    /// Exécution automatique : une frame par update UI.
    running: bool,
    /// Fenêtre de debug « Pattern Table » (VRAM $8000-$97FF).
    show_pattern_table: bool,
    /// Fenêtre de debug « Name Table » (cartes de tuiles $9800/$9C00).
    show_name_table: bool,
    /// La fenêtre « Name Table » affiche la carte $9C00-$9FFF (sinon $9800-$9BFF).
    #[allow(dead_code)] // Champ réservé à la partie future qui ajoutera le basculement $9800/$9C00 dans l'UI.
    name_table_show_9c00: bool,
    /// Fenêtre de debug « Processor » (registres/drapeaux CPU, style No$GBA/BGB).
    show_processor: bool,
    /// Fenêtre de debug « IO Map » (carte des registres I/O $FF00-$FFFF).
    show_io_map: bool,
    /// Texture écran (framebuffer PPU, NEAREST pour le pixel art).
    texture: Option<egui::TextureHandle>,
}

impl FarquaadGBApp {
    /// Crée l'application et configure le contexte egui (thème sombre, fond noir).
    /// Si `initial_rom` est fourni (argument de ligne de commande), la ROM est chargée au démarrage.
    pub fn new(cc: &eframe::CreationContext<'_>, initial_rom: Option<String>) -> Self {
        let mut visuals = Visuals::dark();
        visuals.window_fill = Color32::BLACK;
        cc.egui_ctx.set_style(egui::Style {
            visuals,
            ..Default::default()
        });

        let mut this = Self {
            emulator: Emulator::new(),
            rom_name: None,
            running: false,
            show_pattern_table: false,
            show_name_table: false,
            name_table_show_9c00: false,
            show_processor: false,
            show_io_map: false,
            texture: None,
        };

        // Chargement automatique depuis la ligne de commande (ex. `cargo run -- assets/cpu_instrs.gb`).
        if let Some(path) = initial_rom {
            this.load_rom_file(std::path::Path::new(&path));
        }

        eprintln!("[DIAG] FarquaadGBApp::new() terminé"); // TEMP (diagnostic)
        this
    }

    /// Ouvre une boîte de dialogue et charge la ROM sélectionnée dans l'émulateur.
    fn load_rom(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Game Boy ROM", &["gb", "gbc"])
            .pick_file()
        {
            self.load_rom_file(&path);
        }
    }

    /// Charge une ROM depuis le disque dans l'émulateur (partagée entre la boîte de dialogue
    /// et l'argument de ligne de commande).
    fn load_rom_file(&mut self, path: &std::path::Path) {
        eprintln!("[DIAG] load_rom_file début : {:?}", path); // TEMP (diagnostic)
        match std::fs::read(path) {
            Ok(data) => {
                eprintln!("[DIAG] ROM lue : {} octets", data.len()); // TEMP (diagnostic)
                self.emulator.load_rom(data);
                eprintln!("[DIAG] emulator.load_rom terminé"); // TEMP (diagnostic)
                self.rom_name = path.file_name().and_then(|n| n.to_str()).map(String::from);
                self.running = true; // démarrage immédiat (séquence de boot + jeu)
                log::info!("ROM chargée : {:?}", path);
                eprintln!("[DIAG] load_rom_file terminé"); // TEMP (diagnostic)
            }
            Err(err) => log::error!("Échec de la lecture de {:?} : {err}", path),
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

    /// Dessine la fenêtre de debug « Name Table » : la carte de tuiles 32×32 en $9800, chaque cellule colorée
    /// selon l'index (teintes DMG) et affichant la valeur numérique de la tuile en monospace.
    fn show_name_table_debug(&self, ui: &mut egui::Ui) {
        let vram = &self.emulator.mmu.vram;

        const COLS: usize = 32; // carte de tuiles 32×32 ($9800-$9BFF)
        const ROWS: usize = 32;
        const MAP_OFFSET: usize = 0x9800 - 0x8000; // base de la carte dans vram
        const CELL: egui::Vec2 = egui::vec2(20.0, 16.0);

        egui::Grid::new("name_table_grid")
            .spacing(egui::vec2(2.0, 2.0))
            .show(ui, |g| {
                for row in 0..ROWS {
                    for col in 0..COLS {
                        let index = vram[MAP_OFFSET + row * COLS + col];
                        let [r, green, b] = crate::ppu::DMG_SHADES[(index & 0x03) as usize];
                        // Cellule de taille fixe : fond coloré selon la teinte de la tuile.
                        let (rect, _response) = g.allocate_exact_size(CELL, egui::Sense::hover());
                        g.painter().rect_filled(rect, 0.0, Color32::from_rgb(r, green, b));
                        // Texte monospace centré, lisible sur fond clair (noir) ou foncé (blanc).
                        let text_color = if index & 0x03 < 2 { Color32::BLACK } else { Color32::WHITE };
                        g.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            index.to_string(),
                            FontId::monospace(14.0),
                            text_color,
                        );
                    }
                    if row + 1 < ROWS {
                        g.end_row();
                    }
                }
            });
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

    /// Dessine la fenêtre de debug « Processor » style No$GBA/BGB : drapeaux Z/N/H/C, registre F en binaire,
    /// PC et SP (hexadécimal + binaire groupé par nibbles), registres A/F/B/C/D/E/H/L et état IME/HALT.
    fn show_processor_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("🧠 Processor")
            .open(&mut self.show_processor)
            .resizable(false)
            .show(ctx, |ui| {
                let cpu = &self.emulator.cpu;

                // Drapeaux Z N H C (levé = vert, baissé = rouge).
                ui.horizontal(|ui| {
                    let f = cpu.flags();
                    for (name, flag) in [("Z", Flags::Z), ("N", Flags::N), ("H", Flags::H), ("C", Flags::C)] {
                        let color = if f.contains(flag) { Color32::GREEN } else { Color32::RED };
                        ui.label(egui::RichText::new(name).color(color));
                    }
                });

                // Registre F en binaire (8 bits, MSB d'abord).
                ui.horizontal(|ui| {
                    for i in (0..8).rev() {
                        let bit = (cpu.f >> i) & 1;
                        ui.monospace(format!("{bit}"));
                    }
                });

                ui.separator();

                // PC : hexadécimal puis binaire groupé par nibbles.
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("PC").color(Color32::YELLOW));
                    ui.monospace(format!("${:04X}", cpu.pc));
                });
                ui.horizontal(|ui| {
                    for i in (0..16).rev() {
                        let bit = (cpu.pc >> i) & 1;
                        ui.monospace(format!("{bit}"));
                        if i % 4 == 0 && i > 0 {
                            ui.label(" ");
                        }
                    }
                });

                // SP : hexadécimal puis binaire groupé par nibbles.
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("SP").color(Color32::YELLOW));
                    ui.monospace(format!("${:04X}", cpu.sp));
                });
                ui.horizontal(|ui| {
                    for i in (0..16).rev() {
                        let bit = (cpu.sp >> i) & 1;
                        ui.monospace(format!("{bit}"));
                        if i % 4 == 0 && i > 0 {
                            ui.label(" ");
                        }
                    }
                });

                ui.separator();

                // Registres A, F, B, C, D, E, H, L : hexadécimal + binaire (séparateur pleine largeur par paire).
                egui::Grid::new("processor_registers").show(ui, |g| {
                    g.label(egui::RichText::new("A").color(Color32::CYAN));
                    g.monospace(format!("${:02X}", cpu.a));
                    Self::show_binary_8bit(g, cpu.a);
                    g.end_row();

                    g.label(egui::RichText::new("F").color(Color32::CYAN));
                    g.monospace(format!("${:02X}", cpu.f));
                    Self::show_binary_8bit(g, cpu.f);
                    g.end_row();

                    // Séparateur pleine largeur : la ligne dépasse la cellule (clippée aux bords de la fenêtre).
                    g.add(egui::Separator::default().grow(500.0));
                    g.end_row();

                    g.label(egui::RichText::new("B").color(Color32::CYAN));
                    g.monospace(format!("${:02X}", cpu.b));
                    Self::show_binary_8bit(g, cpu.b);
                    g.end_row();

                    g.label(egui::RichText::new("C").color(Color32::CYAN));
                    g.monospace(format!("${:02X}", cpu.c));
                    Self::show_binary_8bit(g, cpu.c);
                    g.end_row();

                    // Séparateur pleine largeur : la ligne dépasse la cellule (clippée aux bords de la fenêtre).
                    g.add(egui::Separator::default().grow(500.0));
                    g.end_row();

                    g.label(egui::RichText::new("D").color(Color32::CYAN));
                    g.monospace(format!("${:02X}", cpu.d));
                    Self::show_binary_8bit(g, cpu.d);
                    g.end_row();

                    g.label(egui::RichText::new("E").color(Color32::CYAN));
                    g.monospace(format!("${:02X}", cpu.e));
                    Self::show_binary_8bit(g, cpu.e);
                    g.end_row();

                    // Séparateur pleine largeur : la ligne dépasse la cellule (clippée aux bords de la fenêtre).
                    g.add(egui::Separator::default().grow(500.0));
                    g.end_row();

                    g.label(egui::RichText::new("H").color(Color32::CYAN));
                    g.monospace(format!("${:02X}", cpu.h));
                    Self::show_binary_8bit(g, cpu.h);
                    g.end_row();

                    g.label(egui::RichText::new("L").color(Color32::CYAN));
                    g.monospace(format!("${:02X}", cpu.l));
                    Self::show_binary_8bit(g, cpu.l);
                    g.end_row();
                });

                ui.separator();

                // IME et HALT.
                ui.horizontal(|ui| {
                    let ime_color = if cpu.ime { Color32::GREEN } else { Color32::RED };
                    ui.label(egui::RichText::new("IME").color(ime_color));
                    ui.label(if cpu.ime { "ON" } else { "OFF" });

                    ui.separator();

                    let halt_color = if cpu.halted { Color32::GREEN } else { Color32::RED };
                    ui.label(egui::RichText::new(if cpu.halted { "HALT" } else { "RUN" }).color(halt_color));
                });

                ui.separator();

                // Étiquettes statiques (style No$GBA/BGB).
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("DOUBLE SPEED").color(Color32::GRAY));
                    ui.separator();
                    ui.label(egui::RichText::new("BOOTROM").color(Color32::GRAY));
                });
            });
    }

    /// Affiche les 8 bits d'une valeur en binaire (MSB d'abord), en monospace.
    fn show_binary_8bit(ui: &mut egui::Ui, value: u8) {
        ui.horizontal(|ui| {
            for i in (0..8).rev() {
                let bit = (value >> i) & 1;
                ui.monospace(format!("{bit}"));
            }
        });
    }

    /// Dessine la fenêtre de debug « IO Map » : carte des registres I/O $FF00-$FFFF groupés par section
    /// (interruptions, LCD, timer, entrée, série), avec valeur hexadécimale et binaire.
    fn show_io_map_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("🔌 IO Map")
            .open(&mut self.show_io_map)
            .resizable(true)
            .show(ctx, |ui| {
                let mmu = &self.emulator.mmu;

                egui::Grid::new("io_map_grid").show(ui, |g| {
                    // Section INTERRUPTS.
                    g.label(egui::RichText::new("INTERRUPTS:").color(Color32::MAGENTA));
                    g.end_row();

                    Self::show_io_register(g, mmu, "$FFFF", "IE", 0xFFFF);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF0F", "IF", 0xFF0F);
                    g.end_row();

                    Self::show_interrupt_line(g, mmu, "VBLNK", 0);
                    g.end_row();
                    Self::show_interrupt_line(g, mmu, "STAT", 1);
                    g.end_row();
                    Self::show_interrupt_line(g, mmu, "TIMER", 2);
                    g.end_row();
                    Self::show_interrupt_line(g, mmu, "SERIAL", 3);
                    g.end_row();
                    Self::show_interrupt_line(g, mmu, "JOYPAD", 4);
                    g.end_row();

                    // Séparateur pleine largeur : la ligne dépasse la cellule (clippée aux bords de la fenêtre).
                    g.add(egui::Separator::default().grow(500.0));
                    g.end_row();

                    // Section LCD.
                    g.label(egui::RichText::new("LCD:").color(Color32::CYAN));
                    g.end_row();

                    Self::show_io_register(g, mmu, "$FF40", "LCDC", 0xFF40);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF41", "STAT", 0xFF41);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF42", "SCY", 0xFF42);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF43", "SCX", 0xFF43);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF44", "LY", 0xFF44);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF45", "LYC", 0xFF45);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF46", "DMA", 0xFF46);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF47", "BGP", 0xFF47);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF48", "OBP0", 0xFF48);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF49", "OBP1", 0xFF49);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF4A", "WY", 0xFF4A);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF4B", "WX", 0xFF4B);
                    g.end_row();

                    // Séparateur pleine largeur : la ligne dépasse la cellule (clippée aux bords de la fenêtre).
                    g.add(egui::Separator::default().grow(500.0));
                    g.end_row();

                    // Section TIMER.
                    g.label(egui::RichText::new("TIMER:").color(Color32::YELLOW));
                    g.end_row();

                    Self::show_io_register(g, mmu, "$FF04", "DIV", 0xFF04);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF05", "TIMA", 0xFF05);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF06", "TMA", 0xFF06);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF07", "TAC", 0xFF07);
                    g.end_row();

                    // Séparateur pleine largeur : la ligne dépasse la cellule (clippée aux bords de la fenêtre).
                    g.add(egui::Separator::default().grow(500.0));
                    g.end_row();

                    // Section INPUT.
                    g.label(egui::RichText::new("INPUT:").color(Color32::GREEN));
                    g.end_row();

                    Self::show_io_register(g, mmu, "$FF00", "JOYP", 0xFF00);
                    g.end_row();

                    // Séparateur pleine largeur : la ligne dépasse la cellule (clippée aux bords de la fenêtre).
                    g.add(egui::Separator::default().grow(500.0));
                    g.end_row();

                    // Section SERIAL.
                    g.label(egui::RichText::new("SERIAL:").color(Color32::LIGHT_BLUE));
                    g.end_row();

                    Self::show_io_register(g, mmu, "$FF01", "SB", 0xFF01);
                    g.end_row();
                    Self::show_io_register(g, mmu, "$FF02", "SC", 0xFF02);
                    g.end_row();
                });
            });
    }

    /// Affiche un registre I/O : adresse, nom, valeur hexadécimale et binaire (8 bits).
    fn show_io_register(ui: &mut egui::Ui, mmu: &MMU, addr_str: &str, name: &str, addr: u16) {
        let value = mmu.read(addr);

        ui.label(egui::RichText::new(addr_str).color(Color32::CYAN));
        ui.label(name);
        ui.monospace(format!("${value:02X}"));
        ui.monospace(format!("({value:08b})"));
    }

    /// Affiche une ligne d'interruption : état du bit dans IF (pendant) et IE (masque).
    fn show_interrupt_line(ui: &mut egui::Ui, mmu: &MMU, name: &str, bit: usize) {
        let if_reg = mmu.read(0xFF0F);
        let ie_reg = mmu.read(0xFFFF);

        let if_bit = ((if_reg >> bit) & 1) == 1;
        let ie_bit = ((ie_reg >> bit) & 1) == 1;

        ui.label(name);
        ui.horizontal(|ui| {
            ui.label("IF:");
            let color = if if_bit { Color32::GREEN } else { Color32::RED };
            ui.label(egui::RichText::new(if if_bit { "1" } else { "0" }).color(color));
        });
        ui.horizontal(|ui| {
            ui.label("IE:");
            let color = if ie_bit { Color32::GREEN } else { Color32::RED };
            ui.label(egui::RichText::new(if ie_bit { "1" } else { "0" }).color(color));
        });
    }
}

impl eframe::App for FarquaadGBApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // TEMP (diagnostic) : marqueur non tamponné pour localiser le stack overflow.
        eprintln!("[DIAG] update() t_cycles={}", self.emulator.t_cycles);
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
                ui.checkbox(&mut self.show_name_table, "🗺 Name Table");
                ui.checkbox(&mut self.show_processor, "🧠 Processor");
                ui.checkbox(&mut self.show_io_map, "🔌 IO Map");
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

        // Fenêtre de debug « Name Table » (carte de tuiles $9800).
        if self.show_name_table {
            egui::Window::new("🗺 Name Table (VRAM $9800/$9C00)")
                .show(ctx, |ui| {
                    self.show_name_table_debug(ui);
                });
        }

        // Fenêtre de debug « Processor » (registres/drapeaux CPU, style No$GBA/BGB) : la visibilité est
        // pilotée par le bouton de fermeture via `.open(&mut self.show_processor)`.
        self.show_processor_window(ctx);

        // Fenêtre de debug « IO Map » (carte des registres I/O $FF00-$FFFF).
        self.show_io_map_window(ctx);
    }
}