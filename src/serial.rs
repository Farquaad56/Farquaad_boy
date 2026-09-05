//! Serial Communication Controller (SCC) : registres SB ($FF01) et SC ($FF02).
//!
//! PanDocs « Serial Data Transfer » : le port série émet un octet à la fois sur
//! le câble link, **sans appareil branché de l'autre côté**. Les ROMs de test
//! (retrio/gb-test-roms, cpu_instrs…) rapportent leurs résultats par ce port :
//! elles écrivent le caractère dans SB puis $81 dans SC (bit 7 = Transfer
//! enable), sans polling ni interruption.
//!
//! Chaque octet émis en mode master est affiché immédiatement dans la console hôte
//! (stdout) : caractères ASCII imprimables tels quels, 0x0A → newline,
//! 0x0D → carriage return, tout autre octet en hexadécimal `[XX]`. Le transfert dure
//! 64 T-cycles (8 bits × 8 clocks at 8192 Hz) : le bit 7 de SC se lit à 1 pendant ce temps
//! (« This bit is automatically set to 0 at the end of transfer », PanDocs), puis
//! `tick()` lève l'interruption série (bit 3 du registre IF : $FF0F |= $08) — géré par
//! emulator.rs. SB se lit alors $FF (câble débranché → bits reçus tous à 1, PanDocs « Disconnects »).
//! En mode esclave (bit 0 = 0), le transfert reste en attente indéfiniment : aucun appareil hôte ne fournit d'horloge.
//!
//! Le transcript complet des octets émis est aussi exposé à l'interface (fenêtre « 🔌 Serial Monitor »
//! activable depuis the menu Debug) : la ROM de test n'y est pas décompilée, seules les octes transmis sur le câble link y sont affichés.

use std::io::{self, Write};

/// Durée d'un transfert d'octet à horloge interne (8192 Hz × 8 bits = 64 T-cycles).
const TRANSFER_TCYCLES: u32 = 64;

/// Serial Communication Controller (registres $FF01-$FF02), synchronisé sur les T-cycles.
#[allow(clippy::upper_case_acronyms)]
#[derive(Default)]
pub struct Serial {
    /// Registre SB ($FF01) : prochain octet à émettre avant un transfert.
    pub sb: u8,
    /// Registre SC ($FF02) : seuls les bits 7 (transfert activé) et 0 (horloge interne) sont écriturables sur DMG.
    pub sc: u8,
    /// T-cycles restants avant la fin du transfert en cours (None = aucun transfert actif).
    remaining: Option<u32>,
    /// Ligne en cours d'assemblage depuis les octets transmis.
    line: Vec<u8>,
    /// Transcript complet de tous les octets transmis depuis le dernier reset.
    transcript: Vec<u8>,
    /// Dernière ligne complète reçue (pour l'affichage dans l'UI).
    last_line: String,
}

impl Serial {
    /// Crée une SCC à l'état power-on (SB = $00, SC = $00, aucun transfert en cours).
    #[allow(dead_code)] // Utilisé uniquement par les tests dans la cible binaire (power_on utilise Serial::default())
    pub fn new() -> Self {
        Self::default()
    }

    /// Écriture de SB ($FF01) : mémorise le prochain octet à émettre.
    pub fn write_sb(&mut self, value: u8) {
        self.sb = value;
    }

    /// Lecture de SB ($FF01).
    ///
    /// Avant un transfert il contient l'octet à émettre ; après un transfert
    /// achevé il contient l'octet « reçu » — $FF sur un câble débranché.
    pub fn read_sb(&self) -> u8 {
        self.sb
    }

    /// Écriture de SC ($FF02) : bit 7 à 1 démarre le transfert.
    ///
    /// - bit 7 à 0 : le transfert en cours est abandonné (s'il y en a un).
    /// - bit 7 + bit 1 (SC = $81, horloge interne / master) : l'octet courant de SB is émis on the câble link and affiché in the console hôte (stdout); le transfert dure `TRANSFER_TCYCLES` T-cycles — pendant ce temps le bit 7 se lit à 1, puis il est effacé automatiquement at the end (« This bit is automatically set to 0 at the end of transfer », PanDocs) and `tick()` lève l'interruption série (bit 3 of IF). Une nouvelle écriture $81 pendant un transfert en cours le redémarre — les ROMs espacent leurs transferts de several thousand cycles, ce qui ne se produit jamais.
    /// - bit 7 seul (SC = $80, horloge externe / esclave) : the transfer reste en attente d'une horloge which n'arrivera jamais ; le bit 7 se lit à 1.
    /// Seuls les bits 7 and 0 are écriturables sur DMG.
    pub fn write_sc(&mut self, value: u8) {
        // Bit 7 = Transfer Start Flag : seuls les bits 7 et 0 sont pris en compte (DMG).
        self.sc = (self.sc & !0x81) | (value & 0x81);

        if self.sc & 0x80 == 0 {
            self.remaining = None; // bit 7 à 0 : no transfer (le transfert en cours est abandonné)
        } else if self.sc & 0x01 != 0 {
            // Mode master (horloge interne) : l'octet de SB is émis on the câble link.
            let character = self.sb;

            if character >= 0x20 && character <= 0x7E {
                print!("{}", character as char); // ASCII imprimable
            } else if character == 0x0A {
                println!(); // Newline
            } else if character == 0x0D {
                print!("\r"); // Carriage return
            } else {
                print!("[{:02X}]", character); // Non-imprimable : hex
            }

            io::stdout().flush().unwrap(); // afficher immédiatement

            self.remaining = Some(TRANSFER_TCYCLES); // 64 T-cycles (8 bits × 8 clocks) — le bit 7 de SC reste à 1 pendant ce temps
            self.capture(character); // transcript + dernière ligne (panneau UI)

            log::info!(
                "[Serial] Character sent: 0x{:02X} ('{}')",
                character,
                if character >= 0x20 && character <= 0x7E {
                    character as char
                } else {
                    '?'
                }
            );
        } else {
            self.remaining = None; // horloge externe : en attente indéfinie (the bit 7 of SC reste à 1)
        }
    }

    /// Fait avancer la SCC de `cycles` T-cycles.
    /// Renvoie true si un transfert à horloge interne s'est achevé pendant ce pas —
    /// l'appelant doit then poser the bit 3 of the registre IF ($FF0F |= $08) — interruption série (PanDocs « Interrupt Sources »).
    /// At that moment, le bit 7 de SC is effacé automatiquement and SB se lit $FF
    /// (câble débranched → l'octet « reçu » tout en 1, PanDocs « Disconnects »).
    pub fn tick(&mut self, cycles: u32) -> bool {
        match self.remaining {
            Some(remaining) if remaining <= cycles => {
                self.remaining = None;
                self.sc &= !0x80; // bit 7 effacé automatiquement à la fin du transfert (PanDocs)
                self.sb = 0xFF; // câble débranched : l'octet « reçu » is tout en 1
                true
            }
            Some(remaining) => {
                self.remaining = Some(remaining - cycles);
                false
            }
            None => false,
        }
    }

    /// Capture un octet émis sur le câble link : transcript + tampon de ligne.
    fn capture(&mut self, byte: u8) {
        self.transcript.push(byte);
        match byte {
            0x0A => {
                // Retour chariot : la ligne est complète → console de log hôte.
                let line = std::mem::take(&mut self.line);
                self.last_line = Self::render_line(&line);
                if !self.last_line.is_empty() {
                    log::info!("GB serial: {}", self.last_line);
                }
            }
            0x0D => {} // retour chariot Windows : ignoré (les ROMs n'envoient que \n)
            _ => self.line.push(byte),
        }
    }

    /// Rend une ligne capturée lisible pour la console de log (non imprimables → '.').
    fn render_line(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|&b| match b {
                0x20..=0x7E => b as char,
                _ => '.',
            })
            .collect()
    }

    /// Transcript complet des octets transmis (consommé — le buffer est vidé).
    #[allow(dead_code)] // Utilisé uniquement par les tests dans la cible binaire
    pub fn take_transcript(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.transcript)
    }

    /// Nombre d'octets capturés depuis le dernier reset/clear (compteur de la fenêtre « 🔌 Serial Monitor »).
    pub fn transcript_len(&self) -> usize {
        self.transcript.len()
    }

    /// Rend le transcript complet en texte lisible pour the fenêtre « 🔌 Serial Monitor » : ASCII
    /// imprimable tel quel, `0x0A` → newline, tout autre octet en hexadécimal `[XX]` — même
    /// convention que l'affichage stdout de [`Self::write_sc`]. La ROM n'est pas décompilée : seules
    /// les octes émis sur le câble link sont affichés.
    pub fn rendered_transcript(&self) -> String {
        let mut text = String::new();
        for &byte in &self.transcript {
            match byte {
                0x20..=0x7E => text.push(byte as char),
                0x0A => text.push('\n'),
                0x0D => {} // retour chariot Windows : ignoré (les ROMs n'envoient que \n)
                _ => text.push_str(&format!("[{:02X}]", byte)),
            }
        }
        text
    }

    /// Vide le transcript et la ligne en cours d'assemblage (bouton « Clear » de the fenêtre UI) ;
    /// les registres SB/SC eux-mêmes ne sont pas touchés.
    pub fn clear_transcript(&mut self) {
        self.transcript.clear();
        self.line.clear();
    }

    /// Dernière ligne complète reçue sur le port link (« » si aucune) — affichée dans the fenêtre « 🔌 Serial Monitor ».
    pub fn last_line(&self) -> &str {
        &self.last_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Démarre un transfert master de `byte` (écriture SB puis SC = $81).
    fn transmit(s: &mut Serial, byte: u8) {
        s.write_sb(byte);
        s.write_sc(0x81);
    }

    #[test]
    fn power_on_state() {
        let mut s = Serial::new();
        assert_eq!(s.read_sb(), 0);
        assert_eq!(s.sc, 0);
        assert!(!s.tick(4)); // aucun transfert en cours
        assert_eq!(s.last_line(), "");
    }

    #[test]
    fn master_transfer_holds_bit7_for_64_cycles_then_fires_the_interrupt() {
        let mut s = Serial::new();
        transmit(&mut s, b'X');
        assert_eq!(s.sc, 0x81); // bit 7 stays set during the transfer (PanDocs)

        for _ in 0..63 {
            assert!(!s.tick(1)); // not complete yet: no interrupt before 64 T-cycles have elapsed
        }
        assert_eq!(s.sc, 0x81); // still transferring after 63 T-cycles

        assert!(s.tick(1)); // 64th T-cycle: transfer done → serial interrupt raised once
        assert!(!s.tick(1)); // ...and only once
        assert_eq!(s.sc, 0x01); // bit 7 cleared automatically at the end of transfer (PanDocs)
        assert_eq!(s.read_sb(), 0xFF); // cable unplugged → received byte all 1s
    }

    #[test]
    fn writing_sc_without_bit7_aborts_transfer() {
        let mut s = Serial::new();
        transmit(&mut s, b'X');
        assert_eq!(s.sc, 0x81); // transfer in progress: bit 7 set
        s.write_sc(0x01); // bit 7 to 0: the transfer is aborted (PanDocs)
        assert!(!s.tick(4)); // no interrupt — nothing completed
        assert_eq!(s.sc, 0x01);
    }

    #[test]
    fn completion_event_fires_only_once() {
        let mut s = Serial::new();
        transmit(&mut s, b'X');
        assert!(s.tick(64)); // done at the end of the 64 T-cycles
        assert!(!s.tick(4)); // no other transfer in progress
        assert!(!s.tick(10_000));
    }

    #[test]
    fn external_clock_transfer_never_completes() {
        let mut s = Serial::new();
        transmit(&mut s, b'Y');
        s.write_sc(0x80); // bit 7 set (external clock): the byte is emitted as well
        assert_eq!(s.sc, 0x80); // bit 7 stays set: waiting for a host clock that never comes
        assert!(!s.tick(1_000)); // no serial interrupt ever raised
    }

    #[test]
    fn writing_sc_without_bit7_does_not_emit() {
        let mut s = Serial::new();
        s.write_sb(b'Z');
        s.write_sc(0x01); // bit 7 to 0: no transfer, nothing emitted
        assert_eq!(s.sc, 0x01);
        assert!(!s.tick(4)); // no interrupt
    }

    #[test]
    fn sc_write_keeps_only_bits_7_and_0() {
        let mut s = Serial::new();
        s.write_sc(0x42); // bit 7 to 0, bit 0 to 0: bits 6-1 are read-only (always 0 on DMG)
        assert_eq!(s.sc, 0x00);
        s.write_sc(0xFF); // bit 7 set + bit 0 set: transfer started → bit 7 stays set until completion, bit 0 kept
        assert_eq!(s.sc, 0x81);
    }

    #[test]
    fn transmitted_bytes_are_captured_in_transcript() {
        let mut s = Serial::new();
        for &byte in b"OK\n" {
            transmit(&mut s, byte);
            s.tick(4);
        }
        assert_eq!(s.take_transcript(), b"OK\n");
        assert_eq!(s.take_transcript(), Vec::<u8>::new()); // consommé
    }

    #[test]
    fn newline_flushes_the_line_and_updates_last_line() {
        let mut s = Serial::new();
        for &byte in b"All tests passed!\n" {
            transmit(&mut s, byte);
            s.tick(4);
        }
        assert_eq!(s.last_line(), "All tests passed!");

        // Une ligne vide (un seul \n) ne doit pas produire de log parasite.
        transmit(&mut s, b'\n');
        s.tick(4);
        assert_eq!(s.last_line(), "");
    }

    #[test]
    fn non_printable_bytes_are_rendered_as_dots() {
        let mut s = Serial::new();
        for byte in [0x01u8, b'A', 0xFF, b'\n'] {
            transmit(&mut s, byte);
            s.tick(4);
        }
        assert_eq!(s.last_line(), ".A.");
    }

    #[test]
    fn partial_line_stays_buffered_until_newline() {
        let mut s = Serial::new();
        for &byte in b"partial" {
            transmit(&mut s, byte);
            s.tick(4);
        }
        assert_eq!(s.last_line(), ""); // pas encore de \n → aucune ligne complète
        assert_eq!(s.take_transcript(), b"partial");
    }
}
