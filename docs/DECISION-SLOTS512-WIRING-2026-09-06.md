# DÉCISION — Chantier « slots 512 » (PropsZone) : état figé + chemin d'exécution

**Date** : 2026-09-06 · branche principale `feat/zorder` (HEAD `0750783c`, vert) ·
branche de câblage `feat/slots512-wiring` (HEAD `f56772d8`, vert).
**Auteur** : session agent (revue code/graphe + mesures).

## Décisions actées

1. **Problème** : unité 4K par début de chaîne → remplissage mesuré **0,45 %**
   sur le motif CREATE (5 394 pages 4K = 22 Mo pour ~100 Ko) ; RMW 4K par
   append. Objectif densité ×7 (~3 %) + RMW ~×8 (étape 0 mesurée).
2. **Option retenue : B — unités de 512 o (7 par carrier 4K)** (RFC
   `docs/rfc/substrate/propszone-pages-partagees.md`), adressage `UnitéAddr`
   48 bits (bit 47 = slot ; sinon legacy — compat totale), geometry corrigée
   en cours de route : **8×512 = 4096 → l'unité 0 d'un 4K porte l'en-tête de
   carrier, 7 slots utiles**.
3. **Gouvernance de version (choisie)** : **staging par env
   `OBRAIN_PROPS_SLOTS_512` (défaut OFF)** — zéro bump tant que le
   déploiement n'est pas prêt ; au flip prod : garde de version (refus des
   vieux binaires — un magic packé inconnu serait compté comme padding) +
   **QC parité sur les bases harness** (exigence utilisateur, migration
   native et sans erreur).
4. **Sûreté remap (lié)** : fixes P0 livrés (`d7f1a303` live + `57e185f1`
   rejeu) — à étendre aux slots lors du câblage.

## État livré (tout ce qui est committable SANS fenêtre dédiée)

| Livrable | Où | Preuve |
|---|---|---|
| RFC + design + étape 0 mesurée | `feat/zorder` (`10f31733`, `522d094a`, `9f29d3df`, `4ae9fde2`) | docs |
| Étape 1 : `unite.rs` (codec `UnitéAddr`, `SlotPage` 512, `CarrierControl`) | `feat/zorder` (`f370283b`, `8a1a5c5e`) | unite 6/6 |
| Prototype allocateur in-memory (`CarrierPack`) | `feat/zorder` (`86958842`) | unite 6/6 |
| Brique 1 zone réelle : porte env + `ecrire_debut_chainee_pack` | `feat/slots512-wiring` (`f56772d8`) | substrate 819/819 + 1 |

Suites mainline @ `0750783c` : substrate **818/818+1** · engine **683/683** ·
core **870/870** · invariants **9/9** · intégrations vertes. Arbre propre.

## Pourquoi la suite n'est PAS écrite (constat d'ingénierie)

Le câblage réel exige de rendre la chaîne entière « unité-aware » EN UN
TENANT — inventaire fait (sites exacts) : walkers `PropsZone`
(`walk_chain`, `collect_live_props_by_key`, `get_latest_for_key`,
`append_entry_inner`, `allouer_page_neuve`, garde d'owner du 19/08) +
~12 sites store (`lookup_node_property_v2` :7398, jumeau arête :7557,
`read_all_properties_for_node/_edge` :11041/:11122, relevés de têtes
:1680/:7823/:7859, batch :12382, + écritures :7160/:7356/:7485/:7529) +
overflow de slot + remap sur slots + audit d'ancres (replay.rs:234-323).
Toute étape intermédiaire laisserait des têtes de chaîne qu'un lecteur
encore legacy lirait mal : **état hybride = pire qu'aucun code**. La
fenêtre actuelle (worktree partagé M6, plus de sous-agents) ne permet pas
de livrer ce volume avec la garantie « zéro rouge, zéro hybride » exigée
par le repo. Un agent dédié sur la branche, par étapes vertes (une suite
lib complète après chaque commit), est le chemin documenté ci-dessous.

## Chemin d'exécution (ordre imposé par l'absence d'état hybride)

1. Walkers `PropsZone` : itération générique d'unités (legacy 4K | slot 512)
   à partir d'une tête BRUTE (U48) — même comportement legacy ; suites
   vertes avant la suite.
2. Store : passer les têtes brutes partout (sites listés) — suites vertes.
3. Début de chaîne env-gated (`head=None` + `slots_512_actifs()` →
   `ecrire_debut_chainee_pack` + allocation carrier) + overflow slot.
4. Remap (fix P0) étendu aux slots ; audit d'ancres élargi.
5. Tests env=1 E2E (CREATE → lecture → kill/reopen → remap) ; A/B réel.
6. Flip prod : garde de version + QC parité bases harness (ledger).

## Critère de « fait »

Étape 6 atteinte avec : suites vertes complètes, env=1 E2E, A/B daté
(1ʳᵉ prop ~5,7 → ~3,5-4 µs estimé, remplissage ~3 %), ledger + commits
réversibles, 0 rouge — et QC harness passé avant tout déploiement.
