//! FarquaadGB — Émulateur Game Boy (DMG).
//!
//! Point d'entrée : crée la fenêtre eframe et lance la boucle UI.
//! Partie 1 du guide d'implémentation : fenêtre vide à fond noir.

mod app;
mod apu;
mod cartridge;
mod cpu;
mod emulator;
mod joypad;
mod mbc;
mod mmu;
mod ppu;
mod serial;
mod timer;

#[cfg(all(not(target_arch = "wasm32"), target_os = "windows"))]
use winit::platform::windows::EventLoopBuilderExtWindows;

fn main() {
    // 1. Initialisation robuste qui lit RUST_LOG, ou fallback sur "debug" si vide
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

    // 2. Message de test garanti pour vérifier que le logger fonctionne
    log::info!("🚀 [TEST] FarquaadGB démarre ! Si tu vois ceci, les logs fonctionnent.");
    log::debug!("🔍 [DEBUG] Ce message ne s'affiche que si le niveau debug est actif.");

    // 3. ROM à charger automatiquement : premier argument de ligne de commande .gb/.gbc
    //    (ex. `cargo run -- assets/cpu_instrs.gb`) — pratique pour les validations sans GUI.
    let initial_rom = std::env::args()
        .skip(1)
        .find(|arg| arg.ends_with(".gb") || arg.ends_with(".gbc"));

    // 4. La boucle d'événements tourne sur un thread dédié à la pile généreuse : les structures de
    //    l'émulateur sont volumineuses (framebuffer PPU ~92 KiB, VRAM/WRAM 8 KiB chacune) et, en build
    //    debug, les retourner par valeur imbrique plusieurs centaines de Ko d'images de pile au-dessus
    //    de la pile déjà profonde d'eframe/winit — ce qui déborde des 1 Mio par défaut du thread main
    //    Windows (STATUS_STACK_OVERFLOW). winit n'exige pas le thread main OS sur Windows.
    //    (NativeOptions n'étant pas Send, il est construit dans le thread ; seul `initial_rom` y est déplacé.)
    let handle = std::thread::Builder::new()
        .name("FarquaadGB UI".into())
        .stack_size(32 * 1024 * 1024) // 32 Mio : marge large pour les builds debug non optimisés
        .spawn({
            move || {
                let mut native_options = eframe::NativeOptions {
                    viewport: egui::ViewportBuilder::default()
                        .with_title("FarquaadGB")
                        .with_inner_size(egui::vec2(940.0, 640.0)), // écran (160x144 @ 4× = 640×576) + panneau debug CPU/PPU
                    ..Default::default()
                };
                // winit 0.30 refuse de créer la boucle d'événements hors du thread main Windows sans
                // cet opt-in explicite (panique « Initializing the event loop outside of the main thread »).
                native_options.event_loop_builder = Some(Box::new(|_builder| {
                    #[cfg(target_os = "windows")]
                    _builder.with_any_thread(true); // winit 0.30 : autorise la boucle d'événements hors du thread main
                }));
                // eframe::Error n'est pas Send : on ne propage qu'un simple booléen entre threads.
                eframe::run_native(
                    "FarquaadGB",
                    native_options,
                    Box::new(move |cc| Ok(Box::new(app::FarquaadGBApp::new(cc, initial_rom)))),
                )
                .is_ok()
            }
        })
        .expect("impossible de créer le thread interface");

    match handle.join() {
        Ok(true) => {} // fermeture normale de la fenêtre
        Ok(false) => log::error!("eframe a rencontré une erreur (voir les logs ci-dessus)."),
        Err(_) => log::error!("Le thread interface a paniqué."),
    }
}
