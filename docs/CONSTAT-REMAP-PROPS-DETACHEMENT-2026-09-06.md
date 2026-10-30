# CONSTAT — détachement de chaîne PropsZone v2 après remap de slots (perte silencieuse)

**Date** : 2026-09-06 · branche `feat/zorder` · HEAD `522d094a` · **aucun fix — mesure du défaut**.

## Symptôme

Après une **permutation de slots** (`slot_remap` : `HilbertRepermuteV2` via
`SubstrateStore::repermuter_par_hilbert`, ou `CompactCommunityV2` — y compris
le **tick warden en ligne**, warden.rs:253), la **première écriture de
propriété** sur un nœud déplacé rend **invisibles toutes ses propriétés
antérieures** : la chaîne v2 est détachée, les anciennes valeurs sortent du
walk. Perte silencieuse (lecture → `None`, aucune erreur).

## Preuve (test `#[ignore]` committé)

`store.rs` (mod tests) `une_prop_ecrite_apres_un_remap_de_slot_ne_perd_pas_la_chaine`
(reproduit l'état post-remap : record copié sur un nouveau slot, pages de la
chaîne toujours stampées à l'ancien slot) :

```
running 1 test
... une_prop_ecrite_apres_un_remap_de_slot_ne_perd_pas_la_chaine ... FAILED
assertion failed: la prop pré-remap 'a' doit survivre à l'écriture post-remap
  left: None
 right: Some(Int64(1))
```

Suite substrate lib : **810/810 + 1 ignoré** (le test est `#[ignore]` avec la
raison, pattern `wal_recovery_reelle` : il échoue sur le code actuel et passe
au vert quand le défaut sera corrigé).

## Cause racine

- `slot_remap` déplace les `NodeRecord` **entiers** vers les nouveaux slots
  (writer.rs:1605-1612) : la tête `first_prop_off` suit le record. Les pages
  de la chaîne v2, elles, ne sont **jamais touchées** par le remap (la zone
  props n'est pas enrôlée, store.rs:4623-4646) et conservent en
  `header.node_id` le **stamp du vieux slot** (écrit à l'append,
  store.rs:7144 ; header page.rs:74).
- Les **lectures** restent correctes : les walkers
  (`walk_chain`/`get_latest_for_key`/`lookup_*_v2`) ne vérifient que le magic
  de page, pas l'owner (props_zone.rs:571-739).
- La **garde d'append** (props_zone.rs ~418-477) compare `header.node_id` au
  slot COURANT : mismatch → « tête d'un autre propriétaire » → détachement sur
  **page neuve sans chaînage** (`allouer_page_neuve(…, None, …)`) →
  l'ancienne chaîne n'est plus référencée → perte à la lecture suivante.

## Portée

- **Nœuds seulement** (les arêtes ne sont pas déplacées par la permutation —
  seuls `src/dst` sont réécrits) ; chaînes nœuds v2 avec props écrites AVANT
  un remap et modifiées APRÈS.
- Déclencheurs réels : warden (maintenance périodique en ligne,
  `repermuter_par_hilbert`), compact communautaire, tri bulk (import/tri zorder)
  sur des bases portant déjà des propriétés.
- Cas « parasite » historique (têtes étrangères pré-T17f, correctif 19/08) :
  la garde d'append restait le seul rempart contre l'épissage croisé — le fix
  doit préserver ce comportement pour les vraies têtes parasites tout en
  réparant les chaînes légitimement déplacées par le remap.

## Directions de fix (à départager — chantier P0 dédié, DoD propre)

1. **Rafraîchir les owner stamps dans la section critique du remap**
   (`repermuter_par_hilbert`/`compact_community` store-level) : avec la
   correspondance `old→new`, patcher `header.node_id` + re-seal des pages des
   chaînes déplacées (uniquement si stamp == ancien slot du mapping — les
   vraies têtes parasites, dont le stamp ne correspond à aucun déplacement,
   restent détectées et détachées comme aujourd'hui). Exige la **sûreté
   crash** du geste (fenêtre de rejeu, WAL HilbertRepermuteV2, ordre par
   rapport à la barrière `première_mutation`) et des tests de rejeu.
2. **Rendre la garde d'append remap-aware** : distinguer « tête étrangère
   parasite » de « chaîne légitime déplacée » (par ex. stamp mémorisé dans le
   record au moment du remap) — plus invasif.
3. Prérequis commun : le futur format « slots 512 » (RFC
   propszone-pages-partagees.md) devra rendre les owner stamps **remap-aware
   par conception** (ou les supprimer au profit d'une identité stable).

**État** : mesure committée (test `#[ignore]` + présent constat) — **fix non
engagé** (chantier P0, machine partagée avec M6 ; suites complètes + rejeu à
couvrir avant tout commit de correctif).

---

## SUIVI — FIX LIVE LIVRÉ (2026-09-06, `d7f1a303`)

**Chemin live corrigé** : `repermuter_par_hilbert` (le driver du tick warden
en ligne et des tests) :
- `Writer` mémoïse la permutation `old→new` (`derniere_permutation_remap`,
  posée dans `bulk_sort_by_hilbert`) ;
- `PropsZone::rafraichir_owners_de_chaine` (props_zone.rs) re-stampe (+
  re-seal CRC) les pages dont `old_to_new[stamp] == slot` courant et **s'arrête
  sur une page étrangère** (vraie tête parasite du correctif 19/08 : la garde
  d'append continue de détacher — comportement préservé) ; garde anti-cycle ;
- `repermuter_par_hilbert` rafraîchit après le sort, avant le flush.

**Tests** : `props_preservees_apres_repermuter_par_hilbert` (remap réel, 320
nœuds communautés alternées → déplacement massif vérifié non-trivial,
écritures post-remap sur chaque nœud → p0/p1/post intacts). Contrôle négatif
fait : hook désactivé → échec « p0 perdu sur NodeId(128) » → le test détecte
bien la régression. Suites : substrate lib **811/811 + 1 ignoré** ·
hilbert_bulk_sort + slot_remap_sidecars + community_local_insert **14/14** ·
engine **683/683** · invariants **9/9**.

**Mesures** (mono, release, machine partagée — indicative) : remap hilbert 20 k
nœuds **0,477 s sans props → 0,581 s avec refresh (~20 k chaînes), soit
+0,10 s (~5 µs/nœud déplacé)** ; chemin d'écriture des props inchangé
(chaude 0,87 µs, neuve ~6,5 µs).

**Résidus P0 (documentés, non traités dans `d7f1a303`) :**
1. `compact_community` (writer) permute aussi les slots — pas de driver
   store-level identifié aujourd'hui ; à couvrir si un chemin productif
   l'atteint (même mécanique de refresh à brancher).
2. **Fenêtre power-loss** : remap committé (WAL) puis crash avant le flush qui
   suit → au rejeu, la permutation est ré-appliquée par
   `HilbertRepermuteV2` mais les stamps ne sont PAS rafraîchis (le replay n'a
   pas accès à la zone props). Suite : hook de rejeu ou refresh à l'ouverture
   quand un remap est présent dans la fenêtre rejouée.
3. Le repro simulé (`une_prop_ecrite_apres_un_remap_de_slot_ne_perd_pas_la_
   chaine`, état brut sans le chemin du fix) reste `#[ignore]` — passerait au
   vert si la GARDE elle-même devenait remap-aware (option 2 du constat).

## SUIVI 2 — RÉSIDU 2 TRAITÉ (`57e185f1`, auto-guérison au rejeu)

La fenêtre power-loss est fermée : `ReplayStats::remaps_appliques` compte les
records de permutation dans la fenêtre rejouée (`HilbertRepermute{V2}` /
`CompactCommunity{V2}`) ; `SubstrateStore::from_substrate` re-stampe alors les
chaînes PropsZone v2 (bases FINALISÉES uniquement — `restamper_apres_rejeu`,
best-effort, jamais fatal à l'ouverture). Test
`auto_guerison_restampe_les_chaines_apres_rejeu_remap`. Aucun coût sur les
ouvertures propres (le heal ne tourne que si une permutation est dans la
fenêtre). Suites : substrate lib **812/812 + 1 ignoré**, intégrations
replay/remap vertes, engine **683/683**, invariants **9/9**.

**Reste ouvert** : résidu 1 (`compact_community` — pas de driver live
identifié, à couvrir si un chemin l'atteint) et le repro simulé (garde
remap-aware — option 2).
