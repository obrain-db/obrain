# Contrôle cognitif sous migration slots 512 — couche cognitive intacte

**Date** : 2026-09-07 · copie réelle `runtime-dev.obrain` (fraîche) · outil
`cogcheck_slots512` (example substrate) · env `OBRAIN_PROPS_SLOTS_512=1`.

## Méthode
- **Digest cognitif** des NodeRecords préexistants (label_bitset, community_id,
  energy, scar_util_affinity, centrality_cached, flags — `first_prop_off` exclu :
  la migration slots ne doit toucher QUE les chaînes de propriétés), restreint
  à l'ancien high-water ;
- **Tailles des 31 zones cognitives** : engram_bitset / engram_members /
  community / cdc_journal / cdc_tail / degrees.* / tier* / simplex_* ;
- Migration complète entre les deux relevés : 60 updates d'entités EXISTANTES
  (legacy), 50 nœuds + 25 arêtes packés créés, 20 suppressions, 2 réouvertures.

## Résultat
| Signal | Avant | Après | verdict |
|---|---|---|---|
| Digest cognitif (166 707 nœuds préexistants) | `0x22772bff71f02fff` | identique | ✅ |
| Zones cognitives existantes (31) | — | tailles identiques | ✅ |
| Ajout observé | — | `degrees.node.COG.u32` (type d'arête de contrôle neuf) | ✅ attendu |

Par construction, l'env slots ne branche QUE les chaînes de propriétés
(PropsZone) : engrams, synapses (énergie sur records), communautés, tiers,
CDC, degrés et simpliciaux ne sont pas dans ce chemin — confirmé sur données
réelles. Les invariants cognitifs unitaires restent couverts par la suite
substrate (822 tests).
