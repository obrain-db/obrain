# Proposition — chantier « import de code enrichi (relief) »

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `e7bfb9eb`.
**Statut** : R2 EXÉCUTÉ (round 1, commit du chantier) — `obrain enrich --ricci` livré, testé (substrate 804), bench daté (3 162 ms / 1,67 M) ; R1 portes de contrôle OK ; R4/R5 documentés (CDC = bases vivantes ; communautés à la demande).

---

## 1. Constat

Le re-bench (ledger) confirme l'ingestion native très rapide (0,26 s /
1,67 M) et le store importé sert déjà les requêtes O(1), les degrés typés
(auto-maintenus + auto-guérison) et le lookup par `qualified_name`. MAIS le
projet importé est « plat » vis-à-vis du relief livré depuis : pas de
courbure de Ricci persistée, pas de CDC (import natif zone-direct), pas de
signatures prêtes ; les CALLs riches existent mais à la demande.

## 2. Relief proposé à l'import (v1)

Nouvelle passe d'enrichissement post-import, invocable en CLI de façon
additive et idempotente :

- **R1 — Degrés typés + lookup** : déjà garantis (chemins moteur + auto-heal)
  → porte de contrôle (aucun nouveau code), vérifié par test.
- **R2 — Courbure de Ricci persistée par arête** (batch) : `refresh_all_ricci`
  (Jost–Liu) écrit `ricci_f32` durablement dans l'EdgeRecord (update_edge +
  WAL) — débloque `node_curvature`, CALLs bottleneck/Forman, et l'option
  d'enrichir `search_by_role` à terme. Coût attendu ~1840 ns/arête (doc
  geometric) → ~3 s / 1,67 M à MESURER (bench avant/après daté).
- **R3 — Index texte/vectoriel optionnels** : déjà possibles via `index` —
  porte de contrôle, pas de nouveau code (documenté).
- **R4 — CDC sur import** : le chemin natif zone-direct n'émet pas de CDC
  (bulk) ; le relief CDC (`search_delta`/surprise/conflicts) concerne les
  bases VIVANTES (runtime) — documenté, PAS promis pour l'import code v1.
- **R5 — Communautés** : affectation persistée post-import non supportée
  (ranges contiguës à la création) → calcul à la demande via
  `CALL obrain.leiden/louvain` (existant) ; éventuellement stockage du
  `community_id` en PROPRIÉTÉ de nœud pour segmentation par les clients
  (décision d'implémentation si coût acceptable).

### Forme

- CLI : `obrain enrich <store> [--ricci] [--label-communities]` (nouveau sous-
  commande Rust — pas de script JS, principe « obrain = Rust ») : ouvre le
  store, exécute les passes demandées, rend un rapport (arêtes actualisées,
  durée, RSS) ; idempotent (RICCI_STALE re-scanné) ;
- enchaînement conseillé : `native-import`/`code-index-to-substrate` PUIS
  `obrain enrich --ricci`.

## 3. DoD (8 cases)

1. Sous-commande Rust `obrain enrich` additif (aucune modif des formats, des
   stores existants ni des chemins d'import actuels).
2. Passe `--ricci` : refresh batch + persistance vérifiée (réouverture →
   `ricci_f32` lue), rapport daté (nb arêtes, ms, RSS).
3. Tests : petit graphe synthétique code-like (CALLS/IMPORTS) → enrich →
   ricci ≠ 0 sur arêtes vivantes, idempotence (2e passe = no-op/nb stable),
   store sain intact (counts/lookup inchangés), golden diff ∅.
4. Suites : engine 680/724, core 870, substrate 803 — aucune modif existante.
5. Bench daté : enrich `--ricci` sur code-all3 (1,67 M arêtes) — avant/après
   (coût attendu ~3 s), open inchangé.
6. Commit unique réversible + chiffres datés dans le ledger.
7. Doc : cette proposition (statut exécuté) + entrée ledger + usage dans le
   README bench-verify.
8. Pas de déploiement prod ; store de test = copies/corpus locaux.

## 4. Limites / ouvertures

- CDC pour code import = hors v1 (bulk sans événements) — le relief CDC
  s'applique aux bases vivantes (runtime) où le writer les émet déjà.
- HKS/spectre coûteux → calcul à la demande (déjà possible via CALL by_role),
  pas à l'import.
- Communautés persistées post-hoc = hors moteur v1 (décision : propriété
  `community_id` si utile — étudié à l'implémentation, sinon documenté).

## 5. Ordre d'exécution

1. sous-commande + passes (ricci d'abord) → 2. tests synthétiques →
3. bench code-all3 daté → 4. ledger + commit → 5. bilan « import code enrichi »
   + clôture du goal « rebench + relief + positionnement ».
