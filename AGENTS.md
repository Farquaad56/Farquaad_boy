# Consigne de travail (en vigueur)

**Mini-chantier prioritaire, avant reprise du point 5 : scinder `src/cpu/mod.rs` (3041 lignes) en plusieurs fichiers. Déplacement pur, aucun changement de logique.**

## Contexte
`src/cpu/mod.rs` a atteint 3041 lignes. Une session a planté (`terminated`) en tentant de relire ce fichier en entier de façon répétée. Avant de reprendre le portage (Étape 2 point 5, dont le mandat reste par ailleurs inchangé et sera repris ensuite), on réduit la taille du fichier pour fiabiliser le travail de l'agent.

## Mandat actuel
1. Rapporte d'abord la composition du fichier : combien de lignes sont le module `#[cfg(test)] mod tests { ... }` (ou équivalent), combien sont l'implémentation (`struct CPU`, `MicroOp`, `tick()`, `ported_steps()`, helpers).
2. Découpage minimal recommandé, à ajuster selon la réponse au point 1 :
   - `src/cpu/mod.rs` : struct `CPU`, `tick()`, déclarations de module.
   - `src/cpu/micro_ops.rs` : enum `MicroOp`, `ported_steps()`, helpers de micro-op.
   - `src/cpu/tests.rs` : tout le module de tests.
3. **Aucune ligne de logique ne doit changer** — uniquement déplacement de code entre fichiers + ajustements de visibilité (`pub`/`pub(crate)`) strictement nécessaires pour que ça compile, et mise à jour des déclarations `mod` dans `cpu/mod.rs` ou `main.rs`.
4. Vérification : `cargo build` sans warning nouveau, `cargo test` — **la liste des tests qui passent/échouent doit être strictement identique avant/après** (mêmes noms, même statut). C'est le seul critère de succès : si un seul test change de statut, le split a introduit un bug, à corriger avant de continuer.
5. Ne committer qu'après mon feu vert explicite.

## Interdictions explicites
- Aucun changement de comportement, de timing, ou de logique — uniquement déplacement de code.
- Ne pas reprendre le portage Étape 2 point 5 tant que ce split n'est pas validé et commité.
- Ne pas toucher au PPU, `timer.rs`, `Bus::read`/`Bus::write`, boot ROM.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même.
- Ne pas committer sans mon feu vert explicite.