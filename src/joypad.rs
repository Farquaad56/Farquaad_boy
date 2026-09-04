//! Entrées Joypad : clavier → registre $FF00 (JOYP/P1).
//!
//! PanDocs « Joypad Input » : les huit boutons sont disposés en matrice 2×4. L'écriture de P1
//! sélectionne la ligne des actions (P1.5 = 0) ou celle du D-Pad (P1.4 = 0), puis les bits 3-0
//! renvoient l'état de la ligne sélectionnée — **actif bas** (un bouton enfoncé se lit à 0, pas à 1).
//! Si aucune ligne n'est sélectionnée ($30 écrit), le nibble bas se lit $F (tout relâché) ; avec
//! P1 = $CF (les deux lignes sélectionnées) et aucun bouton enfoncé, la lecture renvoie $CF.
//! Une transition d'appui (relâché → enfoncé) lève le drapeau d'interruption Joypad
//! (bit 4 du registre IF $FF0F — PanDocs « Interrupt Sources »).

/// Index des huit boutons, dans l'ordre des bits des deux lignes (PanDocs « Joypad Input ») :
/// - ligne actions (P1.5 = 0) : bit 3=Start, bit 2=Select, bit 1=B, bit 0=A → indices 0-3 ;
/// - ligne D-Pad (P1.4 = 0)   : bit 3=Down, bit 2=Up, bit 1=Left, bit 0=Right → indices 4-7.
pub const KEY_A: u8 = 0;
pub const KEY_B: u8 = 1;
pub const KEY_SELECT: u8 = 2;
pub const KEY_START: u8 = 3;
pub const KEY_RIGHT: u8 = 4;
pub const KEY_LEFT: u8 = 5;
pub const KEY_UP: u8 = 6;
pub const KEY_DOWN: u8 = 7;

/// Joypad (registre $FF00, P1/JOYP), PanDocs « Joypad Input ».
pub struct Joypad {
    /// État de chaque bouton par index ([`KEY_A`]…[`KEY_DOWN`]) : `true` = enfoncé.
    keys: [bool; 8],
    /// Registre $FF00 : seuls les bits 4-5 (sélection des lignes) sont écriturables ;
    /// valeur power-on post-boot ROM : $CF (les deux lignes sélectionnées, tout relâché).
    p1: u8,
}

impl Default for Joypad {
    fn default() -> Self {
        Self {
            keys: [false; 8], // tous les boutons relâchés
            p1: 0xCF, // P1 = $CF (PanDocs « Power Up Sequence ») : bits 4-5 à 0 → les deux lignes sélectionnées
        }
    }
}

impl Joypad {
    /// Crée un joypad à l'état power-on post-boot ROM (P1 = $CF, tous les boutons relâchés).
    #[allow(dead_code)] // API publique : construction explicite ; le MMU passe par `Default` pour l'instant.
    pub fn new() -> Self {
        Self::default()
    }

    /// Réinitialise le joypad à l'état power-on (boutons relâchés, P1 = $CF).
    #[allow(dead_code)] // Réserve : le bouton « Reset CPU » du widget retiré était son seul usage hors tests.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Lecture de $FF00.
    ///
    /// Les bits 7-6 se lisent toujours à 1 et les bits 5-4 renvoient la sélection écrite ;
    /// si aucune ligne n'est sélectionnée ($30 écrit), le nibble bas se lit $F (PanDocs) —
    /// sinon les bits 3-0 renvoient l'état de la ou des lignes sélectionnées, **actif bas**
    /// (0 = enfoncé, 1 = relâché). Avec P1 = $CF et aucun bouton enfoncé, la lecture renvoie $CF.
    pub fn read(&self) -> u8 {
        let mut value = self.p1 & 0x30; // bits de sélection tels qu'écrits (bits 5-4)
        value |= 0xC0; // bits 7-6 toujours lus à 1
        let dpad_selected = (self.p1 & 0x10) == 0; // P1.4 = 0 → ligne D-Pad sélectionnée
        let buttons_selected = (self.p1 & 0x20) == 0; // P1.5 = 0 → ligne actions sélectionnée
        if !dpad_selected && !buttons_selected {
            value |= 0x0F; // aucune ligne sélectionnée ($30 écrit) → nibble bas lu $F (PanDocs)
        } else {
            let mut pressed: u8 = 0;
            if dpad_selected {
                pressed |= self.line(KEY_RIGHT); // ligne D-Pad : bit 3=Down … bit 0=Right
            }
            if buttons_selected {
                pressed |= self.line(KEY_A); // ligne actions : bit 3=Start … bit 0=A
            }
            value |= !pressed & 0x0F; // actif bas : 0 = enfoncé, 1 = relâché
        }
        value
    }

    /// Écriture de $FF00 : seuls les bits 4-5 (sélection des lignes) sont écriturables — le nibble bas est en lecture seule.
    pub fn write(&mut self, value: u8) {
        self.p1 = (self.p1 & !0x30) | (value & 0x30);
    }
    /// Définit l'état d'un bouton par index ([`KEY_A`]…[`KEY_DOWN`]).
    ///
    /// Renvoie `true` sur une transition d'appui (relâché → enfoncé) : c'est elle qui lève le
    /// drapeau d'interruption Joypad (bit 4 du registre IF $FF0F — PanDocs « Interrupt Sources »).
    pub fn set_key(&mut self, index: u8, pressed: bool) -> bool {
        let Some(slot) = self.keys.get_mut(index as usize) else {
            return false; // index hors des huit boutons : ignoré
        };
        if !*slot && pressed {
            *slot = true;
            return true; // transition 1 → 0 (relâché → enfoncé) : interruption Joypad levée
        }
        *slot = pressed;
        false
    }

    /// État d'un bouton par index ([`KEY_A`]…[`KEY_DOWN`]) ; `false` pour un index hors plage.
    pub fn is_key_pressed(&self, index: u8) -> bool {
        self.keys.get(index as usize).copied().unwrap_or(false)
    }

    /// Boutons enfoncés de la ligne de quatre boutons commençant à l'index `base` :
    /// position du bit = index - base (PanDocs « Joypad Input »).
    fn line(&self, base: u8) -> u8 {
        let mut line = 0u8;
        for i in base..base + 4 {
            if self.keys[i as usize] {
                line |= 1 << (i - base);
            }
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_on_reads_cf() {
        let j = Joypad::new();
        assert_eq!(j.read(), 0xCF); // P1 = $CF (post-boot ROM), aucun bouton enfoncé → lecture $CF
    }

    #[test]
    fn write_keeps_only_selection_bits() {
        let mut j = Joypad::new();
        j.write(0x5A); // seuls les bits 4-5 sont écriturables (PanDocs) : P1 = $C0 | $10 = $D0
        assert_eq!(j.read(), 0xDF); // ligne actions seule, tout relâché → nibble bas lu $F ($C0 | $10 | $F)
    }

    #[test]
    fn buttons_line_reads_active_low() {
        let mut j = Joypad::new();
        j.write(0xD0); // P1.5 = 0, P1.4 = 1 → ligne actions sélectionnée seule
        assert_eq!(j.read(), 0xDF); // tout relâché : $C0 | $10 | $F
        j.set_key(KEY_A, true);
        assert_eq!(j.read() & 0x0F, 0x0E); // A enfoncé → bit 0 lu à 0 (actif bas)
        j.set_key(KEY_START, true);
        assert_eq!(j.read() & 0x0F, 0x06); // A + Start enfoncés → bits 3 et 0 lus à 0
        j.set_key(KEY_A, false);
        assert_eq!(j.read() & 0x0F, 0x07); // seul Start enfoncé → bit 3 lu à 0
    }

    #[test]
    fn dpad_line_reads_active_low() {
        let mut j = Joypad::new();
        j.write(0xE0); // P1.4 = 0, P1.5 = 1 → ligne D-Pad sélectionnée seule
        assert_eq!(j.read(), 0xEF); // tout relâché : $C0 | $20 | $F
        j.set_key(KEY_RIGHT, true);
        assert_eq!(j.read() & 0x0F, 0x0E); // Right enfoncé → bit 0 lu à 0 (actif bas)
        j.set_key(KEY_DOWN, true);
        assert_eq!(j.read() & 0x0F, 0x06); // Down + Right enfoncés → bits 3 et 0 lus à 0
        j.set_key(KEY_RIGHT, false);
        assert_eq!(j.read() & 0x0F, 0x07); // seul Down enfoncé → bit 3 lu à 0
    }

    #[test]
    fn no_line_selected_reads_all_released() {
        let mut j = Joypad::new();
        j.write(0xF0); // P1.4 = 1 et P1.5 = 1 → aucune ligne sélectionnée ($30)
        j.set_key(KEY_A, true);
        assert_eq!(j.read(), 0xFF); // le nibble bas se lit $F même avec un bouton enfoncé (PanDocs)
    }

    #[test]
    fn both_lines_selected_or_their_states() {
        let mut j = Joypad::new(); // P1 = $CF : les deux lignes sélectionnées simultanément
        j.set_key(KEY_A, true);
        j.set_key(KEY_RIGHT, true); // A et Right partagent le bit 0 de leurs lignes respectives
        assert_eq!(j.read(), 0xCE); // bit 0 lu à 0 (les deux enfoncés), tout le reste relâché
    }

    #[test]
    fn set_key_raises_interrupt_only_on_press_transition() {
        let mut j = Joypad::new();
        assert!(!j.set_key(KEY_B, false)); // relâché → relâché : pas d'interruption
        assert!(j.set_key(KEY_B, true)); // relâché → enfoncé : interruption levée (1 → 0)
        assert!(!j.set_key(KEY_B, true)); // déjà enfoncé : pas de nouvelle interruption
        assert!(!j.set_key(KEY_B, false)); // enfoncé → relâché : pas d'interruption
        assert!(!j.is_key_pressed(KEY_B));
        assert!(!j.set_key(99, true)); // index hors plage : ignoré
    }

    #[test]
    fn reset_returns_to_power_on_state() {
        let mut j = Joypad::new();
        j.write(0xE0);
        j.set_key(KEY_UP, true);
        j.reset();
        assert_eq!(j.read(), 0xCF);
        assert!(!j.is_key_pressed(KEY_UP));
    }
}
