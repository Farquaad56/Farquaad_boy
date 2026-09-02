//! En-tête de cartouche — PanDocs « The Cartridge Header ».
//!
//! Analyse et valide les 80 octets d'en-tête ($0100-$014F) présents au début de chaque ROM Game Boy :
//! logo Nintendo, titre, drapeaux CGB/SGB, type de cartouche (MBC + RAM/batterie/RTC),
//! tailles ROM/RAM, code de destination, codes de licence, version et sommes de contrôle.

use crate::mbc::{HEADER_TYPE_ADDR, MbcType, ROM_BANK_SIZE};

/// Adresse de début de l'en-tête de cartouche ($0100).
#[allow(dead_code)] // API publique de l'en-tête cartouche — utilisée par les parties futures (affichage UI, sauvegarde d'état).
pub const HEADER_START: usize = 0x0100;
/// Dernier octet (inclus) de l'en-tête de cartouche ($014F) : un en-tête complet fait 80 octets.
pub const HEADER_END: usize = 0x014F;

// --- Adresses des champs (PanDocs « The Cartridge Header ») ---
/// Logo Nintendo ($0104-$0133), 48 octets.
pub const LOGO_START: usize = 0x0104;
/// Longueur du logo Nintendo en octets (48).
pub const LOGO_LEN: usize = 0x30;
/// Titre du jeu ($0134-$0143), 16 octets.
pub const TITLE_START: usize = 0x0134;
/// Longueur du champ titre en octets (16 ; réduit à 15 ou 11 sur les cartouches CGB/SGB).
pub const TITLE_LEN: usize = 0x10;
/// Code de licence nouveau ($0144-$0145), significatif seulement si l'ancien code vaut $33.
pub const NEW_LICENSEE_START: usize = 0x0144;
/// Drapeau CGB ($0143) : $80 = supporte les extensions CGB, $C0 = GBC uniquement (bit 7 déclenche le mode CGB).
pub const CGB_FLAG_ADDR: u16 = 0x0143;
/// Drapeau SGB ($0146) : $03 = supporte Super Game Boy.
pub const SGB_FLAG_ADDR: u16 = 0x0146;
/// Code de taille ROM ($0148).
pub const ROM_SIZE_ADDR: u16 = 0x0148;
/// Code de taille RAM externe ($0149).
pub const RAM_SIZE_ADDR: u16 = 0x0149;
/// Code de destination ($014A) : $00 = Japon, $01 = hors Japon.
pub const DESTINATION_CODE_ADDR: u16 = 0x014A;
/// Code de licence ancien ($014B) ; $33 → utiliser les codes de licence nouveaux à la place.
pub const OLD_LICENSEE_ADDR: u16 = 0x014B;
/// Numéro de version du Mask ROM ($014C).
pub const MASK_ROM_VERSION_ADDR: u16 = 0x014C;
/// Somme de contrôle d'en-tête ($014D), calculée sur $0134-$014C.
pub const HEADER_CHECKSUM_ADDR: u16 = 0x014D;
/// Somme de contrôle globale ($014E-$014F, big-endian) — non vérifiée par le matériel.
pub const GLOBAL_CHECKSUM_START: u16 = 0x014E;

/// Logo Nintendo officiel ($0104-$0133) — PanDocs « The Cartridge Header ».
/// S'il ne correspond pas à ce dump, la boot ROM se verrouille sur le matériel réel.
pub const NINTENDO_LOGO: [u8; LOGO_LEN] = [
    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00, 0x0D,
    0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99,
    0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
];

/// Erreur lors de l'analyse de l'en-tête de cartouche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    /// Le fichier ROM est trop court pour contenir un en-tête complet ($0100-$014F).
    RomTooShort { required: usize, actual: usize },
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderError::RomTooShort { required, actual } => write!(
                f,
                "ROM trop courte pour contenir un en-tête de cartouche complet ($0100-$014F) : {actual} octets < {required} octets"
            ),
        }
    }
}

impl std::error::Error for HeaderError {}

/// Informations extraites de l'en-tête de cartouche ($0100-$014F) — PanDocs « The Cartridge Header ».
#[derive(Debug, Clone)]
#[allow(dead_code)] // API publique de l'en-tête cartouche — utilisée par les tests et les parties futures (affichage UI, sauvegarde d'état).
pub struct CartridgeHeader {
    /// Le logo Nintendo est-il valide ? (les 48 octets $0104-$0133 correspondent au dump officiel).
    pub logo_valid: bool,
    /// Titre du jeu ($0134-$0143), ASCII majuscule, sans le bourrage de fin.
    pub title: String,
    /// Drapeau CGB brut ($0143) : $80 = supporte les extensions CGB (rétrocompatible DMG), $C0 = GBC uniquement.
    pub cgb_flag: u8,
    /// Le jeu supporte-t-il Super Game Boy ? ($0146 == $03).
    pub sgb_supported: bool,
    /// Type de cartouche brut ($0147) — identifie le MBC et la présence de RAM/batterie/RTC.
    pub cartridge_type: u8,
    /// Type de contrôleur mémoire détecté depuis $0147 (tableau PanDocs « The Cartridge Header »).
    pub mbc_type: MbcType,
    /// Taille ROM en octets déclarée par $0148 (None si le code est inconnu).
    pub rom_size_bytes: Option<usize>,
    /// Taille de la RAM externe en octets déclarée par $0149 (0 = pas de RAM).
    pub ram_size_bytes: usize,
    /// Code de destination ($014A) : $00 = Japon, $01 = hors Japon.
    pub destination_code: u8,
    /// Code de licence ancien ($014B) ; $33 → les codes de licence nouveaux doivent être utilisés à la place.
    pub old_licensee_code: u8,
    /// Code de licence nouveau ($0144-$0145), significatif seulement si l'ancien code vaut $33.
    pub new_licensee_code: [u8; 2],
    /// Numéro de version du Mask ROM ($014C).
    pub mask_rom_version: u8,
    /// Somme de contrôle d'en-tête stockée à $014D.
    pub header_checksum: u8,
    /// Somme de contrôle d'en-tête calculée sur $0134-$014C (formule PanDocs).
    pub computed_header_checksum: u8,
    /// La somme de contrôle d'en-tête est-elle valide ? ($014D == valeur calculée).
    pub header_checksum_valid: bool,
    /// Somme de contrôle globale stockée à $014E-$014F (big-endian) — ignorée par le matériel.
    pub global_checksum: u16,
}

#[allow(dead_code)] // API publique de l'en-tête cartouche — utilisée par les tests et les parties futures (affichage UI, sauvegarde d'état).
impl CartridgeHeader {
    /// Le jeu supporte-t-il les extensions CGB ? (bit 7 du drapeau $0143 posé).
    pub fn supports_cgb(&self) -> bool {
        self.cgb_flag & 0x80 != 0
    }

    /// Le jeu fonctionne-t-il uniquement sur GBC ? ($0143 == $C0 — le matériel ignore le bit 6,
    /// donc cela se comporte comme $80 en pratique).
    pub fn is_gbc_only(&self) -> bool {
        self.cgb_flag == 0xC0
    }

    /// La cartouche contient-elle de la RAM externe ? (le type $0147 inclut « +RAM »).
    pub fn has_ram(&self) -> bool {
        matches!(
            self.cartridge_type,
            0x02 | 0x03 | 0x08 | 0x09 | 0x0C | 0x0D | 0x10 | 0x12 | 0x13 | 0x1A | 0x1B | 0x1D | 0x1E
                | 0x22 | 0xFF
        )
    }

    /// La cartouche contient-elle une batterie ? (le type $0147 inclut « +BATTERY »).
    pub fn has_battery(&self) -> bool {
        matches!(
            self.cartridge_type,
            0x03 | 0x06 | 0x09 | 0x0D | 0x0F | 0x10 | 0x13 | 0x1B | 0x1E | 0x22 | 0xFF
        )
    }

    /// La cartouche contient-elle une horloge RTC ? (MBC3 + TIMER, $0147 = $0F/$10).
    pub fn has_rtc(&self) -> bool {
        matches!(self.cartridge_type, 0x0F | 0x10)
    }

    /// Le code de destination est-il le Japon ? ($014A == $00).
    pub fn is_japanese(&self) -> bool {
        self.destination_code == 0x00
    }
}

/// Analyse l'en-tête de cartouche d'une ROM ($0100-$014F) — PanDocs « The Cartridge Header ».
///
/// Renvoie une erreur si la ROM est trop courte pour contenir un en-tête complet.
/// Un logo Nintendo ou une somme de contrôle d'en-tête invalide ne produit PAS d'erreur :
/// ils sont signalés dans la structure renvoyée (certains homebrews n'ont pas le logo),
/// afin que l'appelant puisse afficher un avertissement et continuer.
pub fn parse_header(rom: &[u8]) -> Result<CartridgeHeader, HeaderError> {
    if rom.len() < HEADER_END + 1 {
        return Err(HeaderError::RomTooShort {
            required: HEADER_END + 1,
            actual: rom.len(),
        });
    }

    let cgb_flag = rom[CGB_FLAG_ADDR as usize];
    let sgb_supported = rom[SGB_FLAG_ADDR as usize] == 0x03; // $03 = supporte SGB
    let cartridge_type = rom[HEADER_TYPE_ADDR as usize];
    let header_checksum = rom[HEADER_CHECKSUM_ADDR as usize];
    let computed_header_checksum = compute_header_checksum(rom);

    Ok(CartridgeHeader {
        logo_valid: is_nintendo_logo_valid(rom),
        title: decode_title(&rom[TITLE_START..TITLE_START + TITLE_LEN]),
        cgb_flag,
        sgb_supported,
        cartridge_type,
        mbc_type: MbcType::from_header_byte(cartridge_type),
        rom_size_bytes: rom_size_from_code(rom[ROM_SIZE_ADDR as usize]),
        ram_size_bytes: ram_size_from_code(rom[RAM_SIZE_ADDR as usize]),
        destination_code: rom[DESTINATION_CODE_ADDR as usize],
        old_licensee_code: rom[OLD_LICENSEE_ADDR as usize],
        new_licensee_code: [rom[NEW_LICENSEE_START], rom[NEW_LICENSEE_START + 1]],
        mask_rom_version: rom[MASK_ROM_VERSION_ADDR as usize],
        header_checksum,
        computed_header_checksum,
        header_checksum_valid: header_checksum == computed_header_checksum,
        // $014E-$014F : somme de contrôle globale (big-endian) — non vérifiée par le matériel.
        global_checksum: u16::from_be_bytes([
            rom[GLOBAL_CHECKSUM_START as usize],
            rom[GLOBAL_CHECKSUM_START as usize + 1],
        ]),
    })
}

/// Calcule la somme de contrôle d'en-tête sur $0134-$014C (PanDocs « The Cartridge Header ») :
/// `checksum = checksum - rom[address] - 1` pour chaque adresse, en partant de 0.
pub fn compute_header_checksum(rom: &[u8]) -> u8 {
    let mut checksum: u8 = 0;
    for &byte in &rom[TITLE_START..=MASK_ROM_VERSION_ADDR as usize] {
        checksum = checksum.wrapping_sub(byte).wrapping_sub(1);
    }
    checksum
}

/// Valide le logo Nintendo ($0104-$0133) contre le dump officiel (PanDocs « The Cartridge Header »).
pub fn is_nintendo_logo_valid(rom: &[u8]) -> bool {
    rom.len() >= LOGO_START + LOGO_LEN && rom[LOGO_START..LOGO_START + LOGO_LEN] == NINTENDO_LOGO
}

/// Décode le titre ($0134-$0143) : ASCII majuscule, bourgé de $00.
fn decode_title(bytes: &[u8]) -> String {
    bytes.iter()
        .map(|&b| if (0x20..=0x7E).contains(&b) { b as char } else { ' ' })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Taille ROM en octets depuis le code $0148 (PanDocs « The Cartridge Header ») : 32 KiB × (1 << valeur).
pub fn rom_size_from_code(code: u8) -> Option<usize> {
    match code {
        0x00..=0x08 => Some((32 * 1024) << code), // $00 = 32 KiB … $07 = 4 MiB, $08 = 8 MiB
        0x52 => Some(72 * ROM_BANK_SIZE),   // 1.1 MiB (taille non officielle, note PanDocs)
        0x53 => Some(80 * ROM_BANK_SIZE),   // 1.2 MiB
        0x54 => Some(96 * ROM_BANK_SIZE),   // 1.5 MiB
        _ => None,                          // code inconnu
    }
}

/// Taille de la RAM externe en octets depuis le code $0149 (PanDocs « The Cartridge Header »).
pub fn ram_size_from_code(code: u8) -> usize {
    match code {
        0x00 => 0,          // pas de RAM
        0x01 => 2 * 1024,   // listée comme 2 KiB dans les docs non officielles (PanDocs la marque « unused »)
        0x02 => 8 * 1024,   // 1 banque de 8 KiB
        0x03 => 32 * 1024,  // 4 banques de 8 KiB
        0x04 => 128 * 1024, // 16 banques de 8 KiB (MBC5)
        0x05 => 64 * 1024,  // 8 banques de 8 KiB
        _ => 0,             // code inconnu : pas de RAM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construit une ROM de 32 KiB avec le logo officiel et la somme de contrôle d'en-tête valide.
    fn rom_with_valid_header() -> Vec<u8> {
        let mut rom = vec![0u8; 0x4000];
        rom[LOGO_START..LOGO_START + LOGO_LEN].copy_from_slice(&NINTENDO_LOGO);
        rom[TITLE_START..TITLE_START + TITLE_LEN]
            .copy_from_slice(b"FARQUAADGB\0\0\0\0\0\0"); // titre bourgé de $00
        rom[HEADER_CHECKSUM_ADDR as usize] = compute_header_checksum(&rom);
        rom
    }

    #[test]
    fn parse_valid_header() {
        let header = parse_header(&rom_with_valid_header()).expect("en-tête complet");
        assert!(header.logo_valid); // logo officiel présent
        assert_eq!(header.title, "FARQUAADGB");
        assert!(header.header_checksum_valid); // $014D == valeur calculée sur $0134-$014C
        assert_eq!(header.cartridge_type, 0x00); // ROM ONLY (octet nul)
        assert_eq!(header.mbc_type, MbcType::RomOnly);
        assert_eq!(header.rom_size_bytes, Some(32 * 1024)); // $0148 = $00 → 32 KiB
        assert_eq!(header.ram_size_bytes, 0); // $0149 = $00 → pas de RAM
        assert!(header.is_japanese()); // $014A = $00 → Japon
        assert!(!header.sgb_supported); // $0146 != $03
        assert!(!header.supports_cgb()); // bit 7 du drapeau CGB à 0
    }

    #[test]
    fn invalid_logo_is_reported_not_fatal() {
        let mut rom = rom_with_valid_header();
        rom[LOGO_START] ^= 0xFF; // corrompt le premier octet du logo
        let header = parse_header(&rom).expect("l'analyse ne doit pas échouer");
        assert!(!header.logo_valid); // avertissement : la boot ROM se verrouillerait sur le matériel réel
    }

    #[test]
    fn invalid_checksum_is_reported_not_fatal() {
        let mut rom = rom_with_valid_header();
        rom[HEADER_CHECKSUM_ADDR as usize] ^= 0xFF; // corrompt $014D
        let header = parse_header(&rom).expect("l'analyse ne doit pas échouer");
        assert!(!header.header_checksum_valid);
    }

    #[test]
    fn checksum_formula_matches_pandocs_reference() {
        // En-tête $0134-$014C tout en zéro (25 octets) : `checksum = 0 - 0 - 1` répété 25 fois → -25 mod 256.
        let rom = vec![0u8; HEADER_END + 1];
        assert_eq!(compute_header_checksum(&rom), 0xE7); // 256 - 25
    }

    #[test]
    fn too_short_rom_is_an_error() {
        let err = parse_header(&vec![0x42u8; 0x100]).unwrap_err();
        assert_eq!(err, HeaderError::RomTooShort { required: 0x0150, actual: 0x100 });
    }

    #[test]
    fn cgb_and_sgb_flags() {
        let mut rom = rom_with_valid_header();
        rom[CGB_FLAG_ADDR as usize] = 0x80; // supporte les extensions CGB (rétrocompatible DMG)
        rom[SGB_FLAG_ADDR as usize] = 0x03; // supporte SGB
        let header = parse_header(&rom).unwrap();
        assert!(header.supports_cgb());
        assert!(!header.is_gbc_only());
        assert!(header.sgb_supported);

        rom[CGB_FLAG_ADDR as usize] = 0xC0; // GBC uniquement (le matériel ignore le bit 6)
        let header = parse_header(&rom).unwrap();
        assert!(header.supports_cgb());
        assert!(header.is_gbc_only());
    }

    #[test]
    fn rom_and_ram_sizes() {
        let mut rom = rom_with_valid_header();
        rom[ROM_SIZE_ADDR as usize] = 0x04; // 512 KiB
        rom[RAM_SIZE_ADDR as usize] = 0x03; // 32 KiB (4 banques de 8 KiB)
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.rom_size_bytes, Some(512 * 1024));
        assert_eq!(header.ram_size_bytes, 32 * 1024);

        rom[ROM_SIZE_ADDR as usize] = 0x07; // 4 MiB
        rom[RAM_SIZE_ADDR as usize] = 0x05; // 64 KiB (8 banques de 8 KiB)
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.rom_size_bytes, Some(4 * 1024 * 1024));
        assert_eq!(header.ram_size_bytes, 64 * 1024);

        rom[ROM_SIZE_ADDR as usize] = 0x08; // 8 MiB
        rom[RAM_SIZE_ADDR as usize] = 0x04; // 128 KiB (16 banques de 8 KiB, MBC5)
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.rom_size_bytes, Some(8 * 1024 * 1024));
        assert_eq!(header.ram_size_bytes, 128 * 1024);

        rom[ROM_SIZE_ADDR as usize] = 0x52; // taille non officielle : 72 banques
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.rom_size_bytes, Some(72 * ROM_BANK_SIZE));

        rom[ROM_SIZE_ADDR as usize] = 0xFF; // code inconnu
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.rom_size_bytes, None);
    }

    #[test]
    fn cartridge_type_flags_ram_battery_rtc() {
        let mut rom = rom_with_valid_header();
        rom[HEADER_TYPE_ADDR as usize] = 0x0F; // MBC3 + TIMER + BATTERY
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.mbc_type, MbcType::Mbc3);
        assert!(!header.has_ram());
        assert!(header.has_battery());
        assert!(header.has_rtc());

        rom[HEADER_TYPE_ADDR as usize] = 0x1B; // MBC5 + RAM + BATTERY
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.mbc_type, MbcType::Mbc5);
        assert!(header.has_ram());
        assert!(header.has_battery());
        assert!(!header.has_rtc());

        rom[HEADER_TYPE_ADDR as usize] = 0x01; // MBC1 (sans RAM)
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.mbc_type, MbcType::Mbc1);
        assert!(!header.has_ram());
        assert!(!header.has_battery());
    }

    #[test]
    fn destination_code_overseas() {
        let mut rom = rom_with_valid_header();
        rom[DESTINATION_CODE_ADDR as usize] = 0x01; // hors Japon
        let header = parse_header(&rom).unwrap();
        assert!(!header.is_japanese());
    }

    #[test]
    fn global_checksum_is_stored_but_ignored() {
        let mut rom = rom_with_valid_header();
        rom[GLOBAL_CHECKSUM_START as usize] = 0xAB; // $014E (octet haut, big-endian)
        rom[GLOBAL_CHECKSUM_START as usize + 1] = 0xCD; // $014F (octet bas)
        let header = parse_header(&rom).unwrap();
        assert_eq!(header.global_checksum, 0xABCD);
    }
}
