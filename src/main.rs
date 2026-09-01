//! FarquaadGB — Émulateur Game Boy (DMG).
//!
//! Point d'entrée : crée la fenêtre eframe et lance la boucle UI.
//! Partie 1 du guide d'implémentation : fenêtre vide à fond noir.

mod app;
mod apu;
mod cpu;
mod emulator;
mod joypad;
mod mmu;
mod ppu;
mod serial;
mod timer;

fn main() -> eframe::Result {
    // Initialiser env_logger en respectant la variable d'environnement RUST_LOG.
    // Par défaut, niveau "info" si RUST_LOG n'est pas défini (et sortie toujours colorisée).
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("info") // niveau par défaut
            .default_write_style_or("always"),
    )
    .init();

    log::info!("FarquaadGB starting...");

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FarquaadGB")
            .with_inner_size(egui::vec2(940.0, 640.0)), // écran (160x144 @ 4× = 640×576) + panneau debug CPU/PPU
        ..Default::default()
    };

    eframe::run_native(
        "FarquaadGB",
        native_options,
        Box::new(|cc| Ok(Box::new(app::FarquaadGBApp::new(cc)))),
    )
}
