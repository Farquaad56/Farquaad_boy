# Consigne de travail (en vigueur)

**Exécuter l'Étape 2, point 3 du plan de refactoring CPU/Bus : porter la famille `(HL)` lecture-modification-écriture en micro-op.**

## Acquis (terminé, committé, ne pas retoucher sans mandat explicite)
- Étape 1 : wrapper `Bus` + `Cpu::tick` + `MicroOp` + latches W/Z + table de cycles depuis `data/Opcodes.json` (`d2701a1`).
- Étape 2 point 1 — `(HL)` écriture (`$70-75, $77, $36, $22, $32`), `$36` corrigé à 3 M-cycles (`12c107b`).
- Étape 2 point 2 — `(HL)` lecture (`$46-7E, $0A, $1A, $F2, $F0, $FA, CB xx,(HL)` lecture seule/`BIT`) (`e6d662c`).
- Chantier Timer : détection de front sur compteur partagé + fenêtre de rechargement `$00`, 13 ROMs Mooneye `acceptance/timer/` comme garde-fou permanent (`ca1aa54`, `0393a24`, `9ec315f`).

## Mandat actuel
Porter la famille `(HL)` lecture-modification-écriture :
- **`INC (HL)`** (`$34`), **`DEC (HL)`** (`$35`) — 3 M-cycles chacun.
- **Tous les `CB xx,(HL)` sauf `BIT`** — rotations/décalages (`RLC/RRC/RL/RR/SLA/SRA/SWAP/SRL (HL)`) et `RES`/`SET b,(HL)` — 4 M-cycles chacun.

Avant tout code : cite le tableau M-cycle GBCTR chapitre 6 pour chaque opcode, croisé avec `Opcodes.json`. Signale toute divergence avec le compte legacy avant de l'implémenter, ne la corrige pas silencieusement (comme `$36`).

Point d'attention : `INC`/`DEC`/rotations modifient les flags en plus de lire-puis-écrire — vérifie à quel M-cycle exact le calcul des flags a lieu.

Méthode imposée, identique aux points précédents :
1. Micro-programme + test d'entrelacement dédié par opcode.
2. A/B `cargo test` (ensembles d'échecs, pas totaux).
3. `cpu_instrs` (11/11) + `mooneye_timer_roms_pass` (13/13, ne doit pas régresser).
4. Relancer `03-modify_timing.gb` de Blargg — sortie brute avant/après, sans attente qu'elle passe forcément au vert.
5. Ne committer qu'après mon feu vert explicite.

## Interdictions explicites
- Ne pas porter d'autre famille au-delà de ce point.
- Ne pas toucher au PPU, ni au rendu des sprites (décalage d'une case connu, noté pour un futur mandat séparé — **pas maintenant**).
- Ne pas retoucher `timer.rs`, `Bus::read`/`Bus::write`, ni la boot ROM sans mandat explicite séparé.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même.
- Ne pas committer sans mon feu vert explicite.