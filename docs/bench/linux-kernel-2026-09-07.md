# Bench M6 — kernel Linux 6.6 (2026-09-07)

Arbre : `/tmp/linux-6.6` (tarball officielle kernel.org, 81 555 fichiers
découverts). Même arbre pour M6 et le C (référence, en cours).

## Résultat M6 (indexeur natif)

```
temps   : 24,691 s
fichiers parses : 70 821  (81 555 découverts ; 146 langages branchés)
noeuds  : 1 188 726
  Function 562 581 | Macro 310 904 | Field 184 071 | File 70 821
  Struct 28 572   | Variable 22 430 | Enum 5 015 | Method 2 846
  Type 719        | Class 658
aretes  : 3 563 190
  CALLS 1 677 844 | DEFINES 1 624 293 | DEFINES_METHOD 190 232
  CONTAINS_FILE 70 821
résolution calls : 1 677 844 résolus / 248 804 non résolus
```

## Scaling vs corpus obrain (1 287 fichiers, 0,67 s)

- 55× plus de fichiers → 37× le temps (24,7 s) — le parallélisme rayon
  compense le surcoût ; scaling sous-linéaire en temps.
- RAM stable (~53 Go dispo après bench) — pas de fuite sur 1,2 M nœuds.
- Machine : AMD Ryzen 7 8845HS (16 threads), 62 Go RAM.

## Composition du kernel (extraits)

C : 32 980 fichiers, headers C : 23 966, assembleur : 1 348, device tree
(.dts) : 2 776, Python : 188, Rust : 60. Le kernel est massivement C —
d'où les 562 k Functions + 310 k Macros (le resolve C kernel est riche).

## Comparaison C (référence)

À compléter quand l'indexation C du même arbre sera terminée (mode full,
similarité incluse — plus long).

## Comparaison M6 vs C — drivers/net (même arbre)

Le C a indexé `/tmp/linux-6.6/drivers/net` (project tmp-linux-6.6-drivers-net,
330 271 nœuds, 1 297 490 arêtes) pour comparaison équitable.

### Fonctions par fichier (multiset, échantillonnage)

| Fichier | C | M6 | Verdict |
|---|---|---|---|
| vxlan/vxlan_core.c | 123 | 123 | = (après fix preproc #if) |
| dummy.c | 11 | 11 | = |
| loopback.c | 12 | 12 | = |
| gtp.c | 63 | 63 | = |
| bonding/bond_main.c | 163 | 163 | = (multiset nom par nom, 0 écart) |
| rtw89/fw.h | 447 | 447 | = |
| mlxsw/reg.h | 243 | 243 | = |
| iwlwifi/mvm/mvm.h | 81 | 81 | = |
| Échantillon 5 .c variés | 418 | 419 | M6 = C (+1 superset) |

### Comptes globaux drivers/net (5 418 fichiers M6)

| Métrique | C | M6 | Ratio |
|---|---|---|---|
| Function | 105 546 | 102 896 | 97,5 % (écart : .S assembleur non branché + ~2,5 % résiduel) |
| CALLS | 177 546 | 341 825 | M6 ×1,9 (résout plus : 341 825 résolus / 26 840 non résolus) |
| Fichiers | 6 001 | 5 418 | C indexe Makefile/Kconfig/.S sans symboles |

### Leçons kernel intégrées

- **preproc #if** (`24fb20da`) : le kernel cache des fonctions entières sous
  `#if IS_ENABLED(CONFIG_...)` — descente ajoutée dans walk_decls. Gain :
  94 407 → 102 896 fonctions sur drivers/net (+8 489).
- Écart résiduel global : fonctions dans les .S assembleurs (93 côté C, pas de
  grammaire assembly branchée) + résolveurs C spécialisés par fichier.

## Bench kernel complet (linux-6.6 racine, 70 821 fichiers)

```
temps   : 24,691 s (mode release, 16 threads rayon)
fichiers parses : 70 821 / 81 555 découverts (146 langages)
noeuds  : 1 188 726 | arêtes : 3 563 190
RAM crête : 4,29 Go (mesurée /usr/bin/time -v)
```

Scaling vs corpus obrain (1 287 fichiers, 0,67 s) : 55× fichiers → 37× temps —
le parallélisme rayon compense ; scaling sous-linéaire.
