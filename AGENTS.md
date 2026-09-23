# Consigne de travail (en vigueur)

**Exécuter UNIQUEMENT l'étape 1 du plan de refactoring CPU/Bus. Ne rien faire d'autre que l'étape 1.**

## Périmètre autorisé (étape 1 uniquement)

- Créer `src/bus.rs` : wrapper minimal `Bus` autour du MMU existant (Deref), API D1 :
  `read(addr)` = accès + tick, `write(addr, val)` = accès + tick, `tick_m_cycle()`, `idle_m_cycle()`.
  L'avancement PPU/Timer/Série/DMA et la pose des bits IF quittent `Emulator::step` pour entrer dans `Bus`.
- Renommer/routé `Cpu::step(&mut MMU)` vers `Cpu::tick(&mut self, bus: &mut Bus)`.
- Définir l'enum `MicroOp { FetchOpcode, ReadPcByte, ReadMem(AddrSrc), WriteMem(AddrSrc, ValSrc), Internal }`
  (D2) + latches W/Z sur le CPU + mécanisme d'exécution pas-à-pas dans `tick`.
  Toutes les instructions réelles restent sur le chemin atomique legacy (aucune famille portée).
- Vendre `data/Opcodes.json` (gbdev.io/gb-opcodes) + `scripts/gen_cycle_table.py`
  qui génère `src/cpu/expected_cycles.rs` : `EXPECTED_UNPREFIXED: [(u8, Option<u8>); 256]`,
  `EXPECTED_CBPREFIXED: [(u8, Option<u8>); 256]` en M-cycles (taken/not-taken).
- Ajouter le test générique exhaustif : pour chacun des 512 opcodes (les deux branches des conditionnelles),
  exécuter une instruction isolée dans un ROM minimal et vérifier que les M-cycles consommés
  correspondent exactement au tableau. Tout écart code/JSON est listé et soumis avant correction.

## Interdit (hors étape 1)

- Ne PAS porter aucune instruction family vers micro-op programs (étape 2).
- Ne PAS toucher le PPU, Timer, Série, DMA behavior beyond moving their advancement into `Bus`.
- Ne PAS change any observable CPU/PPU/memory behavior. All existing tests must stay green.
- No new dependencies in Cargo.toml.

## Vérification requise à chaque étape

`cargo test` complet vert + `cargo build --release` (GUI compile). Rapporter la progression après chaque checkpoint.
