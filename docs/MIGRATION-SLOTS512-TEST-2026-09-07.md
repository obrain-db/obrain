# Test de migration slots 512 — 4 copies réelles, triple vérification

**Date** : 2026-09-07 · binaire = code courant (`feat/zorder`) · outil `mig_slots512`
(example engine) via `scripts/migrate-slots512.sh` · **COPIES uniquement**
(`/mnt/obrain/mig/`, disque monté 935 G). Env `OBRAIN_PROPS_SLOTS_512=1`.

## Méthode (par copie, échec = exit 1)
A. baseline (scan nœuds/arêtes) · U. mises à jour de 150 nœuds + 80 arêtes
EXISTANTS (chaînes legacy) + relecture · C. créations packées (80 nœuds + 40
arêtes, tête slot bit 47 vérifiée) + relecture · D. suppressions (legacy +
pack) vérifiées None · E. 3 cycles de réouverture avec **parité exacte**
(n+80 / e+40) + contrôles persistés (mC_1/mC_2 cohérents).

## Résultats
| Copie (source) | baseline n / e | attendu après | cycles E×3 | verdict |
|---|---|---|---|---|
| runtime-dev.obrain | 166 489 / 156 991 | 166 569 / 157 031 | exacts ×3 | ✅ |
| runtime-prod.obrain | 135 230 / 132 116 | 135 310 / 132 156 | exacts ×3 | ✅ |
| runtime.obrain | 135 173 / 132 223 | 135 253 / 132 263 | exacts ×3 | ✅ |
| runtime-full-v2.obrain | 259 164 / 317 733 | 259 244 / 317 773 | exacts ×3 | ✅ |

Zéro perte, zéro dérive ; coexistence legacy 4K / slots 512 sans corruption.
