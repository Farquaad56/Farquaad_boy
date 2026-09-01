//! Serial Communication Controller (SCC) : registres SB ($FF01) et SC ($FF02).
//!
//! Partie 7 : émulé un câble link **sans appareil branché de l'autre côté**
//! (PanDocs « Serial Data Transfer ») :
//! - mode master (horloge interne, SC = $81) : le transfert d'un octet dure
//!   64 T-cycles (8 bits × 8 cycles à 8192 Hz). À la fin, le bit 7 de SC est
//!   effacé automatiquement, l'interruption série est demandée (bit 3 du registre IF : $FF0F |= $08)
//!   et SB se lit $FF (câble débranché → bits reçus tous à 1).
//! - mode esclave (horloge externe, SC = $80) : le transfert reste en attente
//!   indéfiniment (aucun appareil hôte ne fournit d'horloge).
//!
//! Chaque octet transmis par une ROM en mode master est capturé : les
//! caractères imprimables sont assemblés en lignes qui sont émises dans la
//! console de log hôte (`log::info!`) dès qu'un retour chariot (0x0A) arrive.
//! C'est ainsi que les ROMs de test (retrio/gb-test-roms, cpu_instrs…)
//! rapportent leurs résultats par le port game link : elles écrivent le
//! caractère dans SB puis $81 dans SC, sans polling ni interruption.

/// Durée d'un transfert d'octet à horloge interne (8192 Hz × 8 bits = 64 T-cycles).
const TRANSFER_TCYCLES: u32 = 64;

/// Serial Communication Controller (registres $FF01-$FF02), synchronisé sur les T-cycles.
#[allow(clippy::upper_case_acronyms)]
#[derive(Default)]
pub struct Serial {
    /// Registre SB ($FF01) : prochain octet à émettre avant un transfert.
    pub sb: u8,
    /// Registre SC ($FF02) : seuls les bits 7 (transfert activé) et 0 (horloge interne) sont écriturables.
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

    /// Écriture de SC ($FF02) : seuls les bits 7 et 0 sont pris en compte.
    ///
    /// - bit 7 à 0 : le transfert en cours est abandonné.
    /// - bit 7 + bit 1 (SC = $81, horloge interne / master) : un nouveau
    ///   transfert de 64 T-cycles démarre ; l'octet émis est la valeur courante
    ///   de SB, capturée immédiatement (une nouvelle écriture $81 pendant un
    ///   transfert en cours le redémarre — les ROMs espacent leurs transferts
    ///   de plusieurs milliers de cycles, ce qui ne se produit jamais).
    /// - bit 7 seul (SC = $80, horloge externe / esclave) : le transfert reste
    ///   en attente d'une horloge qui n'arrivera jamais ; le bit 7 se lit à 1.
    pub fn write_sc(&mut self, value: u8) {
        self.sc = (self.sc & !0x81) | (value & 0x81);
        if self.sc & 0x80 == 0 {
            self.remaining = None; // transfert désactivé
        } else if self.sc & 0x01 != 0 {
            self.capture(self.sb); // octet émis sur le câble link
            self.remaining = Some(TRANSFER_TCYCLES);
        } else {
            self.remaining = None; // horloge externe : en attente indéfinie (bit 7 de SC reste à 1)
        }
    }

    /// Fait avancer la SCC de `cycles` T-cycles.
    /// Renvoie true si un transfert à horloge interne s'est achevé pendant ce pas
    /// (l'appelant doit alors poser le bit 0 du registre IF — interruption série).
    pub fn tick(&mut self, cycles: u32) -> bool {
        match self.remaining {
            Some(remaining) if remaining <= cycles => {
                self.remaining = None;
                self.sc &= !0x80; // bit 7 effacé automatiquement à la fin du transfert
                self.sb = 0xFF; // câble débranché : l'octet « reçu » est tout en 1
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

    /// Dernière ligne complète reçue sur le port link (« » si aucune).
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
    fn master_transfer_completes_after_exactly_64_t_cycles() {
        let mut s = Serial::new();
        transmit(&mut s, b'X');
        assert_eq!(s.sc, 0x81); // bit 7 à 1 : transfert en cours

        for _ in 0..63 {
            assert!(!s.tick(1)); // pas encore achevé…
            assert_eq!(s.sc & 0x80, 0x80);
        }
        assert!(s.tick(1)); // …le 64e T-cycle achève le transfert
        assert_eq!(s.sc, 0x01); // bit 7 effacé automatiquement (bit 0 conservé)
        assert_eq!(s.read_sb(), 0xFF); // câble débranché → octet « reçu » tout en 1
    }

    #[test]
    fn completion_event_fires_only_once() {
        let mut s = Serial::new();
        transmit(&mut s, b'X');
        assert!(s.tick(64)); // achevé
        assert!(!s.tick(4)); // aucun autre transfert en cours
        assert!(!s.tick(10_000));
    }

    #[test]
    fn external_clock_transfer_never_completes() {
        let mut s = Serial::new();
        transmit(&mut s, b'Y');
        s.write_sc(0x80); // bascule en horloge externe : le transfert reste en attente
        assert_eq!(s.sc, 0x80);
        for _ in 0..1_000 {
            assert!(!s.tick(4));
        }
        assert_eq!(s.sc & 0x80, 0x80); // toujours « en cours » (bit 7 à 1)
    }

    #[test]
    fn writing_sc_without_bit7_aborts_transfer() {
        let mut s = Serial::new();
        transmit(&mut s, b'Z');
        assert_eq!(s.sc & 0x80, 0x80);
        s.write_sc(0x01); // bit 7 à 0 : transfert abandonné
        assert!(!s.tick(TRANSFER_TCYCLES));
    }

    #[test]
    fn sc_write_keeps_only_bits_7_and_0() {
        let mut s = Serial::new();
        s.write_sc(0xFF);
        assert_eq!(s.sc, 0x81); // les autres bits ne sont pas écriturables sur DMG
    }

    #[test]
    fn transmitted_bytes_are_captured_in_transcript() {
        let mut s = Serial::new();
        for &byte in b"OK\n" {
            transmit(&mut s, byte);
            s.tick(TRANSFER_TCYCLES + 4); // espace les transferts comme le font les ROMs
        }
        assert_eq!(s.take_transcript(), b"OK\n");
        assert_eq!(s.take_transcript(), Vec::<u8>::new()); // consommé
    }

    #[test]
    fn newline_flushes_the_line_and_updates_last_line() {
        let mut s = Serial::new();
        for &byte in b"All tests passed!\n" {
            transmit(&mut s, byte);
            s.tick(TRANSFER_TCYCLES + 4);
        }
        assert_eq!(s.last_line(), "All tests passed!");

        // Une ligne vide (un seul \n) ne doit pas produire de log parasite.
        transmit(&mut s, b'\n');
        s.tick(TRANSFER_TCYCLES + 4);
        assert_eq!(s.last_line(), "");
    }

    #[test]
    fn non_printable_bytes_are_rendered_as_dots() {
        let mut s = Serial::new();
        for byte in [0x01u8, b'A', 0xFF, b'\n'] {
            transmit(&mut s, byte);
            s.tick(TRANSFER_TCYCLES + 4);
        }
        assert_eq!(s.last_line(), ".A.");
    }

    #[test]
    fn partial_line_stays_buffered_until_newline() {
        let mut s = Serial::new();
        for &byte in b"partial" {
            transmit(&mut s, byte);
            s.tick(TRANSFER_TCYCLES + 4);
        }
        assert_eq!(s.last_line(), ""); // pas encore de \n → aucune ligne complète
        assert_eq!(s.take_transcript(), b"partial");
    }
}


