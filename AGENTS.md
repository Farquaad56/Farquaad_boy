# Consigne de travail (en vigueur)

**Exécuter l'Étape 2, point 4 du plan de refactoring CPU/Bus : porter PUSH/POP/CALL/RET/RST et le dispatch d'interruption en micro-op.**

## Acquis (terminé, committé, ne pas retoucher sans mandat explicite)
- Étape 1 : wrapper `Bus` + `Cpu::tick` + `MicroOp` + latches W/Z + table de cycles depuis `data/Opcodes.json` (`d2701a1`).
- Étape 2 point 1 — `(HL)` écriture (`12c107b`).
- Étape 2 point 2 — `(HL)` lecture (`e6d662c`).
- Étape 2 point 3 — `(HL)` lecture-modification-écriture, 26 opcodes (`46f0f8f`).
- Chantier Timer : détection de front + fenêtre de rechargement, 13 ROMs Mooneye `acceptance/timer/` (`ca1aa54`, `0393a24`, `9ec315f`).

## Mandat actuel
Porter :
- **`PUSH r16`** (`$C5, $D5, $E5, $F5`) — 4 M-cycles.
- **`POP r16`** (`$C1, $D1, $E1, $F1`) — 3 M-cycles.
- **`CALL a16`** (`$CD`, 6 M-cycles) et **`CALL cc,a16`** (`$C4, $CC, $D4, $DC` — 6 M-cycles pris / 3 non pris).
- **`RET`** (`$C9`, 4 M-cycles), **`RET cc`** (`$C0, $C8, $D0, $D8` — 5 M-cycles pris / 2 non pris), **`RETI`** (`$D9`, 4 M-cycles).
- **`RST`** (`$C7, $CF, $D7, $DF, $E7, $EF, $F7, $FF`) — 4 M-cycles.
- **Le dispatch d'interruption lui-même** (5 M-cycles : 2 internes, push octet haut de PC, push octet bas, saut vers le vecteur).

Avant tout code : cite le tableau M-cycle GBCTR chapitre 6/7 pour chaque forme (PUSH, POP, CALL pris/non pris, RET/RET cc pris/non pris, RETI, RST, et la séquence de service d'interruption), croisé avec `Opcodes.json`. Attention particulière à l'**ordre exact d'empilement** (octet haut ou bas en premier) — c'est le point le plus facile à inverser silencieusement sur cette famille, et une inversion donnerait le bon compte de cycles avec un comportement faux.

Reprends aussi la question laissée ouverte il y a plusieurs tours : GBCTR confirme-t-il maintenant, à l'examen du dispatch d'interruption réel, si une écriture de registre différée (`pending_complete`) doit s'appliquer même quand ce M-cycle est détourné vers le service d'une interruption plutôt qu'un fetch normal ? Cite si trouvé ; sinon confirme à nouveau l'absence de citation directe.

Méthode imposée, identique aux points précédents :
1. Micro-programme + test d'entrelacement dédié par opcode (assertions individuelles, pas un échantillon).
2. A/B `cargo test` (ensembles d'échecs).
3. `cpu_instrs` (11/11) + `mooneye_timer_roms_pass` (13/13).
4. Relancer les ROMs Blargg `interrupt_time.gb` et `instr_timing.gb` (déjà en échec depuis le début de la conversation) — sortie brute avant/après, sans attente qu'elles passent forcément.
5. Si le temps le permet : chercher et lancer les ROMs Mooneye `acceptance/interrupts/` et `acceptance/instr/` pertinentes (nouvelle source empirique, comme pour le Timer) — rapporter lesquelles passent déjà, lesquelles échouent.
6. Ne committer qu'après mon feu vert explicite.

## Interdictions explicites
- Ne pas porter d'autre famille au-delà de ce point (Étape 2 point 5 reste pour un mandat futur).
- Ne pas toucher au PPU, ni au rendu des sprites.
- Ne pas retoucher `timer.rs`, `Bus::read`/`Bus::write`, ni la boot ROM sans mandat explicite séparé.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même.
- Ne pas committer sans mon feu vert explicite.