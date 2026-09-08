//! Boot ROM images for the Game Boy (PanDocs « Boot ROM » / GBCTR Chapter 7).
//!
//! The real 256-byte **DMG** boot ROM, cross-checked byte-by-byte against the reference
//! disassembly of codeberg.org/ISSOtm/gb-bootroms (`src/dmg.asm`, la source citée par PanDocs
//! « Power Up Sequence ») — MD5 `d1488193a6adb40f1070122a762e1369`. Cinq octets corrompus dans
//! l'ancien dump (`32`→`77` @ $001C, `0C`→`1D` @ $006D, `2C`→`24` @ $0072, et deux `87`→`86`
//! @ $00F4/$00F9) ont été corrigés : le checksum de l'en-tête (`ADD A,(HL)` au lieu de `ADD A,H`)
//! ne verrouillait plus la boot ROM en boucle infinie, et le hand-off vers $0100 a lieu pour un
//! logo + checksum valides.
//!
//! On power-on the boot ROM is mapped at `$0000-$00FF` and the CPU starts execution
//! from `$0000`. It validates the cartridge logo (`$0104-$0133`), scrolls it to the
//! center of the screen, plays a "di-ding" sound, then writes an odd value to rBANK
//! (`$FF50`) to unmap itself before the game code runs at `$0100`. While mapped, all
//! reads from `$0000-$00FF` are served by this ROM and writes to that region are
//! ignored; reads/writes of `$0100+` still reach the cartridge.
//!
//! The image is loaded by default from the external file `rom/Boot_room.gb` (256 bytes,
//! relative to the working directory or next to the executable); if that file is absent
//! or invalid, the embedded [`DMG_BOOT_ROM`] below is used as a safe fallback.

use std::fs;
use std::path::{Path, PathBuf};

/// Taille exacte d'une Boot ROM DMG (octets).
pub const BOOT_ROM_SIZE: usize = 256;

/// Chemin par défaut du fichier externe de la Boot ROM (relatif au répertoire de travail ;
/// un second essai est fait à côté de l'exécutable — voir [`default_boot_rom`]).
pub const DEFAULT_BOOT_ROM_PATH: &str = "rom/Boot_room.gb";

/// The real DMG boot ROM (256 bytes), cross-checked against the reference disassembly
/// (codeberg.org/ISSOtm/gb-bootroms `src/dmg.asm` — MD5 `d1488193a6adb40f1070122a762e1369`).
pub const DMG_BOOT_ROM: [u8; BOOT_ROM_SIZE] = [
    0x31, 0xfe, 0xff, 0xaf, 0x21, 0xff, 0x9f, 0x32, 0xcb, 0x7c, 0x20, 0xfb, 0x21, 0x26, 0xff, 0x0e,
    0x11, 0x3e, 0x80, 0x32, 0xe2, 0x0c, 0x3e, 0xf3, 0xe2, 0x32, 0x3e, 0x77, 0x32, 0x3e, 0xfc, 0xe0,
    0x47, 0x11, 0x04, 0x01, 0x21, 0x10, 0x80, 0x1a, 0xcd, 0x95, 0x00, 0xcd, 0x96, 0x00, 0x13, 0x7b,
    0xfe, 0x34, 0x20, 0xf3, 0x11, 0xd8, 0x00, 0x06, 0x08, 0x1a, 0x13, 0x22, 0x23, 0x05, 0x20, 0xf9,
    0x3e, 0x19, 0xea, 0x10, 0x99, 0x21, 0x2f, 0x99, 0x0e, 0x0c, 0x3d, 0x28, 0x08, 0x32, 0x0d, 0x20,
    0xf9, 0x2e, 0x0f, 0x18, 0xf3, 0x67, 0x3e, 0x64, 0x57, 0xe0, 0x42, 0x3e, 0x91, 0xe0, 0x40, 0x04,
    0x1e, 0x02, 0x0e, 0x0c, 0xf0, 0x44, 0xfe, 0x90, 0x20, 0xfa, 0x0d, 0x20, 0xf7, 0x0c, 0x20, 0xf2,
    0x0e, 0x13, 0x2c, 0x7c, 0x1e, 0x83, 0xfe, 0x62, 0x28, 0x06, 0x1e, 0xc1, 0xfe, 0x64, 0x20, 0x06,
    0x7b, 0xe2, 0x0c, 0x3e, 0x87, 0xe2, 0xf0, 0x42, 0x90, 0xe0, 0x42, 0x15, 0x20, 0xd2, 0x05, 0x20,
    0x4f, 0x16, 0x20, 0x18, 0xcb, 0x4f, 0x06, 0x04, 0xc5, 0xcb, 0x11, 0x17, 0xc1, 0xcb, 0x11, 0x17,
    0x05, 0x20, 0xf5, 0x22, 0x23, 0x22, 0x23, 0xc9, 0xce, 0xed, 0x66, 0x66, 0xcc, 0x0d, 0x00, 0x0b,
    0x03, 0x73, 0x00, 0x83, 0x00, 0x0c, 0x00, 0x0d, 0x00, 0x08, 0x11, 0x1f, 0x88, 0x89, 0x00, 0x0e,
    0xdc, 0xcc, 0x6e, 0xe6, 0xdd, 0xdd, 0xd9, 0x99, 0xbb, 0xbb, 0x67, 0x63, 0x6e, 0x0e, 0xec, 0xcc,
    0xdd, 0xdc, 0x99, 0x9f, 0xbb, 0xb9, 0x33, 0x3e, 0x3c, 0x42, 0xb9, 0xa5, 0xb9, 0xa5, 0x42, 0x3c,
    0x21, 0x04, 0x01, 0x11, 0xa8, 0x00, 0x1a, 0x13, 0xbe, 0x20, 0xfe, 0x23, 0x7d, 0xfe, 0x34, 0x20,
    0xf5, 0x06, 0x19, 0x78, 0x87, 0x23, 0x05, 0x20, 0xfb, 0x87, 0x20, 0xfe, 0x3e, 0x01, 0xe0, 0x50,
];

// La boot ROM MGB (pocket-color) réelle n'est pas encore embarquée : tant qu'un dump vérifié n'est pas
// disponible, la MGB se comporte comme la DMG au hand-off — voir GBCTR Table 7.1
// (MD5 `71a378e71ff30b2d8a1f02bf5c7896aa`).


/// Charge la Boot ROM depuis un fichier `.gb` de exactement [`BOOT_ROM_SIZE`] octets.
/// Retourne une erreur si le fichier est absent ou n'a pas la bonne taille.
pub fn load_boot_rom_from_file(path: impl AsRef<Path>) -> Result<[u8; BOOT_ROM_SIZE], String> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(format!("Fichier Boot ROM introuvable : {}", path.display()));
    }

    let data = fs::read(path).map_err(|e| format!("Erreur de lecture du fichier {} : {e}", path.display()))?;

    if data.len() != BOOT_ROM_SIZE {
        return Err(format!(
            "La Boot ROM doit faire exactement {} octets, obtenu : {}",
            BOOT_ROM_SIZE,
            data.len()
        ));
    }

    let mut rom = [0u8; BOOT_ROM_SIZE];
    rom.copy_from_slice(&data);
    Ok(rom)
}

/// Chemins candidats du fichier externe de la Boot ROM : d'abord relatif au répertoire de travail
/// (`cargo run` depuis la racine du projet), puis à côté de l'exécutable (lancement de l'exe
/// depuis un autre CWD).
fn candidate_boot_rom_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(DEFAULT_BOOT_ROM_PATH)];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let next_to_exe = dir.join("rom").join("Boot_room.gb");
            if !paths.contains(&next_to_exe) {
                paths.push(next_to_exe);
            }
        }
    }
    paths
}

/// Image de la Boot ROM par défaut : le fichier externe [`DEFAULT_BOOT_ROM_PATH`] s'il est lisible et
/// valide (256 octets), sinon l'image DMG embarquée [`DMG_BOOT_ROM`] en repli sécurisé.
pub fn default_boot_rom() -> [u8; BOOT_ROM_SIZE] {
    for path in candidate_boot_rom_paths() {
        match load_boot_rom_from_file(&path) {
            Ok(rom) => {
                log::info!("[BootROM] Boot ROM chargée depuis : {}", path.display());
                return rom;
            }
            Err(err) => log::debug!("[BootROM] {} — essai du chemin suivant.", err),
        }
    }
    log::warn!(
        "[BootROM] Fichier externe introuvable ou invalide ({} et à côté de l'exécutable) — utilisation de la Boot ROM DMG embarquée (repli).",
        DEFAULT_BOOT_ROM_PATH
    );
    DMG_BOOT_ROM
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Répertoire temporaire unique par test : `farquaadgb_bootrom_<nom>` dans le temp du système.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("farquaadgb_bootrom_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("impossible de créer le répertoire temporaire");
        dir
    }

    #[test]
    fn load_boot_rom_from_file_missing_errors() {
        let dir = temp_dir("missing");
        let err = load_boot_rom_from_file(dir.join("absent.gb")).unwrap_err();
        assert!(err.contains("introuvable"), "message inattendu : {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_boot_rom_from_file_wrong_size_errors() {
        let dir = temp_dir("size");
        let path = dir.join("court.gb");
        fs::write(&path, vec![0x31; BOOT_ROM_SIZE - 1]).expect("impossible d'écrire le fichier de test");

        let err = load_boot_rom_from_file(&path).unwrap_err();
        assert!(err.contains("256"), "message inattendu : {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_boot_rom_from_file_ok() {
        let dir = temp_dir("ok");
        let path = dir.join("boot.gb");
        fs::write(&path, DMG_BOOT_ROM).expect("impossible d'écrire le fichier de test");

        let rom = load_boot_rom_from_file(&path).unwrap();
        assert_eq!(rom, DMG_BOOT_ROM); // octet par octet
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn embedded_dmg_boot_rom_has_canonical_edges() {
        // Garde-fou : les 8 premiers octets du dump DMG vérifié (MD5 d1488193…).
        assert_eq!(&DMG_BOOT_ROM[..8], &[0x31, 0xfe, 0xff, 0xaf, 0x21, 0xff, 0x9f, 0x32]);
        // …et les 4 derniers : le boot ROM se termine par LD A,$01 / OUT ($50),A (dé-mappage via rBANK).
        assert_eq!(&DMG_BOOT_ROM[BOOT_ROM_SIZE - 4..], &[0x3e, 0x01, 0xe0, 0x50]);
        // Garde-fou contre la ré-introduction des octets corrompus de l'ancien dump :
        // le checksum de l'en-tête est `ADD A,(HL)` (0x87) aux deux endroits — avec 0x86 (`ADD A,H`),
        // la boot ROM se verrouillait en boucle infinie et ne dé-mappait jamais.
        assert_eq!(DMG_BOOT_ROM[0xF4], 0x87);
        assert_eq!(DMG_BOOT_ROM[0xF9], 0x87);
    }
}

