//! Gestion des flags du registre F (Z, N, H, C) via bitflags.
//!
//! TODO(partie 3) : utiliser ces flags dans les implémentations d'instructions.

use bitflags::bitflags;

bitflags! {
    /// Flags du registre F (bits 7..4).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Flags: u8 {
        const Z = 0b1000_0000; // Zero
        const N = 0b0100_0000; // Subtract
        const H = 0b0010_0000; // Half Carry
        const C = 0b0001_0000; // Carry
    }
}
