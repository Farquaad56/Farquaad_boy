# Consigne de travail (en vigueur)

**Exécuter l'étape 2 du plan de refactoring CPU/Bus : porter les familles d'instructions vers des programmes micro-op, famille par famille, une famille à la fois.**

## Acquis (terminé, committé, ne pas retoucher sans mandat explicite)
- Étape 1 : wrapper `Bus` + `Cpu::tick(&mut self, bus)` + enum `MicroOp` + latches W/Z + table de cycles générée depuis `data/Opcodes.json` + test exhaustif (`d2701a1`).
- Étape 2, point 1 — famille `(HL) en écriture` (`$70-75, $77, $36, $22, $32`) : portage micro-op terminé, y compris la correction du timing de `$36` (`LD (HL),n8`) à 3 M-cycles / 12 T-cycles — confirmé par `data/Opcodes.json` ET par GBCTR chapitre 5 (tableau M-cycle détaillé de `LD (HL), n`, page 25 : fetch / `Z ← mem` / `mem ← Z`) (`12c107b`).
- Rebranchement de `Emulator::step()` pour boucler `cpu.tick()` jusqu'à ce qu'une instruction portée soit achevée en un seul appel à `step()` : fait, comportementalement neutre pour Blargg (routage déjà en place via `ported_steps()`), confirmé par comparaison A/B (`cargo test`, ensembles d'échecs identiques) (dernier commit de step()).

## Mandat actuel, et rien de plus
Porter le point 2 de l'Étape 2 : **`(HL)` en lecture** — `LD r,(HL)` (`$46,$4E,$56,$5E,$66,$6E,$7E`), `LD A,(BC/DE/a16)` (`$0A,$1A,$FA`), `LDH` en lecture (`$F0,$F2`), et les `CB xx,(HL)` en lecture seule (`BIT`, `$CB 46/4E/56/5E/66/6E/76/7E`).

Pour chaque opcode : consulter le tableau M-cycle détaillé de GBCTR chapitre 6 (déjà utilisé pour `$70-75/$36`) avant d'écrire le micro-programme — ne pas deviner le découpage.

Méthode imposée, identique au point 1 :
1. Écrire le micro-programme + test d'entrelacement dédié (2 `tick()` séparés observables, comme `ported_hl_write_family_interleaves_m_cycles`).
2. Vérifier `cargo test` en A/B (comparaison d'ensembles d'échecs, pas seulement le total).
3. Relancer `cpu_instrs` (doit rester 11/11) et `01-read_timing.gb` de Blargg — coller la sortie brute avant/après.
4. Ne committer qu'après mon feu vert explicite sur le résultat.

## Interdictions explicites tant que ce point n'est pas validé et confirmé par moi
- Ne pas porter d'autre famille au-delà de `(HL)` lecture.
- Ne pas toucher au PPU.
- Ne pas modifier `Bus::write`/`Bus::read` (ordre tick/accès) — chantier séparé, en attente, voir ci-dessous.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même. Toute extension de mandat doit être écrite par moi, dans mon propre commit.
- Ne pas committer sans mon feu vert explicite sur le résultat de chaque étape.

## Chantier en attente (hors scope tant que non explicitement ouvert)
Investigation du couplage tick/accès bus dans `Bus::write`/`Bus::read` : une inversion naïve de l'ordre casse la resynchronisation `tima_64` du ROM Blargg (testé empiriquement). Cause non totalement élucidée. Ne pas y retoucher tant que je n'ai pas ouvert ce chantier explicitement avec son propre mandat.