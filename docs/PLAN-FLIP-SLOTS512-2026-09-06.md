# PLAN — Flip prod du format « slots 512 » (PropsZone) + migration native des bases

**Date** : 2026-09-06 · branche `feat/zorder` · état câblé derrière `OBRAIN_PROPS_SLOTS_512` (défaut OFF), suites **821/821+1 · engine 683 · invariants 9/9**, A/B mesurés (1ʳᵉ prop −59 %, pages ×7, multi mono +48 %).
RFC : `docs/rfc/substrate/propszone-pages-partagees.md` · design : `docs/DESIGN-PROPSZONE-SLOTS-512-2026-09-06.md` · décision : `docs/DECISION-SLOTS512-WIRING-2026-09-06.md`.

## Principe (acté)

Le format est **mixte et lisible** : les chaînes legacy 4K et les chaînes packées (slots 512, 7 par carrier 4K) coexistent, tous les lecteurs sont unité-aware. **Aucune réécriture de données n'est nécessaire** pour activer le format : la « migration » = ouvrir la base sous le nouveau binaire, activer l'écriture packée, **valider la parité** — pas convertir.

## 1. Garde de version (décision)

- Le danger : un **vieux binaire** ouvrant une base contenant des carriers packés compterait le magic `0xC5A2_50C8` comme padding (recherche binaire de frontière) et réallouerait par-dessus → corruption silencieuse. Un vieux binaire ne connaît pas le magic → il ne peut pas se protéger lui-même.
- **Mesure** : au flip, `SUBSTRATE_FORMAT_VERSION` passe **1 → 2** (meta.rs:8, refus à l'ouverture file.rs:939) — imposé par la règle lib.rs:25-27 (« toute évolution de format exige bump + étape `obrain-migrate` »).
- Conséquences opérationnelles :
  - **Rollback interdit après première écriture** d'une base en v2 (le méta est estampillé 2 ; un vieux binaire refuse proprement, « use obrain-migrate ») — c'est la protection recherchée ;
  - Tous les binaires montent en lockstep au flip (services + outils + bridge) ;
  - L'étape `obrain-migrate` du bump est **native et sans réécriture** : ouvrir (v1 → lire), marquer prêt, ré-ouvrir en v2, QC.

## 2. Activation par base + QC parité (bases harness en priorité — exigence utilisateur)

Pour CHAQUE base (ordre : une base de test → bases harness (`code-all3`, `agent`, runtime/dsh, bridge) → reste) :
1. **Snapshot** (copie ou backup binaire, pattern bascules existantes) ;
2. Open en v2 (env `OBRAIN_PROPS_SLOTS_512=1`) **sans aucune écriture** → **QC lecture** : comptes (`count(n)`, `count(e)`, par label/type), valeurs échantillonnées, fenêtres exact==sorted, lookups indexés — comparés au baseline daté (18 contrôles, pattern bascule T17) ;
3. **Écriture de contrôle** : CREATE 1 prop + SET + REMOVE sur une poignée de nœuds neufs → relire (les nouvelles chaînes naissent packées) ;
4. **Kill/crash test** sur la copie (écriture entre flushs → réouverture → parité) ;
5. QC vert → base activée ; QC rouge → rollback (restauration du snapshot), base jamais laissée hybride illisible.

## 3. Rollback

1 commande par base (mv du backup, pattern bascules `brainfix`/M4) — valable tant que le méta n'a pas été estampillé 2 par une écriture ; après, le downgrade est refusé proprement par le vieux binaire (pas de corruption possible).

## 4. Mesures de sortie (avant/pendant le flip)

- Sonde `ab512` : 1ʳᵉ prop µs/op + pages par base type (attendu −59 %/−86 %) ;
- Sonde multi-graphes @1/@8/@16 (machine au calme) : CREATE ops/s ;
- Import bulk avec props : temps + taille de zone (densité) ;
- Ledger : ligne flip (bases activées, QC par base, rollback éventuel).

## 5. Périmètre et hors-périmètre au flip

- **À jour (2026-09-07)** : les **arêtes sont désormais couvertes** par l'env (set/remove/lookup/read_all arêtes en variante brute, test E2E `env_slots_512_aretes_packees`) — l'activation concerne nœuds ET arêtes.
- Hors périmètre : compaction de chaînes (tombstones accumulés), repack des anciennes chaînes legacy (outil optionnel ultérieur, jamais requis), chantier `compact_community` (aucun driver live identifié).

## 6. Critère de « fait »

Toutes les bases harness activées avec QC vert + ledger à jour + 0 rouge ; binaire v2 en place sur les services ; rollback documenté et testé sur une copie ; chiffres de sortie datés.
