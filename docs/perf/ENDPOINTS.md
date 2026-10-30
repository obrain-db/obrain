# Coûts consolidés des endpoints et des opérations du moteur

`1er appel` = cache serveur froid · `suivants` = cache serveur chaud. Le serveur mémorise les
sous-graphes par `(store, centre, sauts, limite)` : le prix se paie une fois par combinaison.

Mesuré le 2026-09-10T20:35:04.192Z sur `runtime-dev.obrain` — 3 tours par mesure, min/médiane/max.
**Aucune valeur n'est en nanosecondes.** Le plancher observé est de l'ordre de la dizaine de
microsecondes (`count(n)` et `count(r)` ne comptent pas assez pour remplir une milliseconde),
et le plafond à 26 s pour un seul appel. L'écart entre les deux est de **six ordres de grandeur**.

| section | mesure | froid | médiane | max | temps moteur |
|---|---|---|---|---|---|
| route | /api/viz/subgraph hops=2 (phase MOTIFS) | 26.21 s | 8.9 ms | 26.21 s | — |
| moteur | agréger par type d'arête | 2.09 s | 1.7 ms | 2.09 s | 2.09 s |
| route | /api/viz/space (sous-ensembles) | 1.62 s | 2.1 ms | 1.62 s | — |
| route | /api/viz/weave (tissage complet) | 1.18 s | 1.18 s | 1.18 s | — |
| route | /api/viz/signatures (toutes les signatures) | 1.08 s | 1.13 s | 1.17 s | — |
| route | /api/viz/subgraph hops=1 (phase RAPIDE) | 757 ms | 22 ms | 757 ms | — |
| moteur | agréger par label | 225 ms | 2.9 ms | 225 ms | 222 ms |
| route | /api/functions (recherche) | 203 ms | 1.6 ms | 203 ms | — |
| route | /api/viz/overview (résumé d'un store) | 151 ms | 1.4 ms | 151 ms | — |
| rejeu | expansion du nœud cyclique 317553 | 143 ms | 1.9 ms | 143 ms | 134 ms |
| rejeu | expansion du nœud cyclique 318414 | 118 ms | 1.5 ms | 118 ms | 114 ms |
| moteur | compter les nœuds | 91 ms | 2.7 ms | 91 ms | 29 µs |
| moteur | compter UN type d'arête | 34 ms | 2.8 ms | 34 ms | 29 ms |
| moteur | étendre 1 saut depuis 1 nœud | 23 ms | 2.9 ms | 23 ms | 18 ms |
| moteur | lire 10 nœuds | 22 ms | 1.3 ms | 22 ms | 21 ms |
| route | /api/viz/modules (ensembles d'un monde) | 6.0 ms | 2.5 ms | 6.0 ms | — |
| route | /api/viz/activation (journal vif) | 5.9 ms | 4.9 ms | 5.9 ms | — |
| route | /api/engine/sante (LLM) | 5.2 ms | 4.6 ms | 5.2 ms | — |
| moteur | compter les arêtes | 3.5 ms | 2.0 ms | 3.5 ms | 33 µs |
| route | /api/worlds (registre) | 2.2 ms | 1.5 ms | 2.2 ms | — |
| moteur | chercher par nom (index) | 1.9 ms | 1.5 ms | 1.9 ms | 47 µs |
| rejeu | expansion du nœud VOISIN 318415 (témoin) | 1.4 ms | 1.2 ms | 1.4 ms | 89 µs |
| rejeu | expansion du nœud VOISIN 317554 (témoin) | 1.3 ms | 902 µs | 1.3 ms | 110 µs |

Point noir : **/api/viz/subgraph hops=2 (phase MOTIFS)** — premier appel 26.21 s, suivants 8.9 ms (290× le coût médian des autres).
Deuxième poste : **agréger par type d'arête** (2.09 s).

## Rejeu du journal du serveur

Journal : `/tmp/vizserve.log` (36564 lignes).
- décodements de propriétés en échec : **36400** sur **241** nœuds distincts ;
- cycles d'expansion détectés : **94** sur 4 nœud(s) ;
- compteurs d'instantané déclarés **faux** puis corrigés : nœuds 371258 → 372019, arêtes 1186790 → 1188294 ;
