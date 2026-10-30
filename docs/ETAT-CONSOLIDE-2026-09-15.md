# ÉTAT CONSOLIDÉ — 2026-09-15

*Une seule version, ce qui est clos, ce qui reste ouvert. Écrit après la fusion
des deux branches de consolidation et le déploiement sur les services vivants.*

## 1. Ce qui existe maintenant

**Une ligne**, `fusion/tronc-plus-j6-mes-correctifs`, qui réunit ce qui vivait
séparément :

| Apport | Vient de | Preuve |
|---|---|---|
| Protocole HTTP complet (HEAD, ETag/304, `Date`, keep-alive, 400 sans `Host`, garde de confinement des statiques) | tronc | **TOUT PASSE** (15/15) sur le 8432 |
| Index ordonné par valeur, trigrammes, garde adaptative, cache de requêtes conscient de l'époque | `mes-correctifs` (6 commits) | les mêmes mesures que la ligne d'origine |
| Transport **WebSocket streaming**, client **et** serveur | lignée servie j5 | identité WS == HTTP, cache chaud compris |
| Opérations nommées (compteurs) + `overview` à **0,68 ms** | chantier NANO | 5/5 tests d'invariant |
| UI du tronc (fratrie, natures, crans de vue) + client WS + ancres résolues en qn | tronc + dashboard | `UI SERVIE CONFORME` |

La fusion des deux branches s'est faite **sans aucun conflit** (zones
disjointes : moteur/serveur d'un côté, dashboard et bancs de l'autre), puis trois
commits ont porté le reste :

- `feefb56f` — `label_counts()` + les 5 tests d'invariant NANO ;
- `75e58662` — `overview` en opérations nommées, serveur WS porté, **et deux
  défauts de la voie WS corrigés** (voir §3) ;
- `0981fcac` — la fusion elle-même ;
- `39f8741f` — le client WS et la résolution des ancres par défaut (ils étaient
  encore non commités dans le worktree du tronc).

## 2. Ce qui est déployé

| Service | Version | Vérifié |
|---|---|---|
| `obrain-viz` (8432) | `obrain.consolide-20260915` (md5 `b9d865a1`, 160 982 768 o) | protocole **TOUT PASSE** · endpoints **27/27** · `UI SERVIE CONFORME` · `overview` à froid **0,684 ms** · WS cache chaud **50/50 nœuds, 397/397 arêtes, 1ʳᵉ frame 5 ms** · ancre en nom nu (`e1000_probe`) **200, 18 nœuds** · traversée `/app/../…` **404 de 7 à 20 niveaux** |
| `this-rs-gateway` (8144) | `this-rs-gateway.stats-nano-20260915` | `/stores/{nom}/stats` **200** en public, **401** sans jeton sur un store à tenant, politique armée au journal |
| UI servie | `20260915-115305-51f332c2-consolidee` — **bâtie depuis la lignée consolidée** (`ui-deploy.sh --worktree /tmp/tpc-j6`) | porte `tsc` + `vite` vertes, capacités 5/5, `UI SERVIE CONFORME` |

## 3. Les deux défauts que le portage du WS a mis au jour (préexistants)

Reproduits **à l'identique sur le binaire servi**, donc antérieurs :

1. **Le cache de sous-graphe refusait son propre document.** La voie WS relit le
   corps mis en cache par le HTTP avec la validation stricte, qui refuse un
   graphe **tronqué** — or un graphe tronqué a des arêtes pendantes légitimes
   (mesuré : `limit=50` sur un nœud à 397 arêtes). Résultat, cache chaud :
   `« cache illisible: arête pendante »` au lieu de streamer, et le client
   retombait sur HTTP au bout de son délai. Corrigé par une relecture
   **lénifiante** (`from_json_str_lenient` : schéma vérifié, arêtes pendantes
   tolérées) — c'est notre propre document.

2. **Les trames d'erreur n'étaient pas du JSON.** Les corps d'erreur étaient
   assemblés au `format!` ; dès qu'un message contenait un guillemet
   (`src "100" absent des nœuds`), le JSON était invalide, le client ne pouvait
   pas le lire, et la cause réelle devenait **invisible**. Les 16 constructions
   passent désormais par `serde_json` (`json_erreur`).

## 4. Ce qui reste ouvert

- ~~**L'index de trigrammes sur le store vivant**~~ : **MESURÉ, ET DÉJÀ GAGNÉ
  par la consolidation — aucun réglage à armer.** A/B sur le store vivant,
  cache de requêtes isolé des deux côtés :

  | requête | ancien binaire servi (j5) | binaire consolidé |
  |---|---|---|
  | `CONTAINS 'iam'` (417 lignes) | **61,3 ms** | **1,5 ms** (froid) · 0,0 ms (chaud) |
  | `CONTAINS 'drivers'` (3 lignes) | **62,0 ms** | idem |
  | motif ABSENT | — | 1,5 ms |
  | `CONTAINS 'session-'` (288 424 lignes) | — | 168,4 ms |

  **×41** sur le cas sélectif, et la preuve de route est déterministe :
  `obrain query --read-only --slots` rend **`slots_noeuds_lus = 0`** — la
  réponse est comptée depuis l'index, sans lire un seul nœud.

  Attribution : l'ancien binaire n'a **aucun** des réglages du tronc
  (`OBRAIN_TRIGRAMMES_DES_LA` = 0 occurrence), mais la **zone de trigrammes de
  la lignée j6**, fusionnée à l'écriture, sert la route — c'est elle que la
  consolidation a rendue active en même temps que le protocole et l'IAM. Le
  résidu non sélectif (168 ms pour 288 424 lignes) est la **vérification des
  candidats** (≈ 0,6 µs × 288 424), où les deux routes convergent : rien à
  gagner sans changer la sémantique.
- **Les trois assertions de `fratrie-check.mjs`** qui échouent sur le 8432 pour
  des raisons de données (aucun qn `__file__` dans l'échantillon, phase Cosmos
  sur un store *runtime*) et une assertion instable (cadrage « yml 3 »).
- ~~**`scripts/acceptation-8432.sh` ne peut pas démarrer**~~ : **CORRIGÉ le
  2026-09-15**, deux défauts réparés — `ISOLÉ_PORT` (nom de variable bash
  invalide : le mode par défaut abortait sur « ISOL: unbound variable ») devient
  `ISOLE_PORT`, et la porte **démarre elle-même** le navigateur CDP de la fratrie
  (elle l'appelait sans navigateur : `ECONNREFUSED`, et « la fratrie n'est pas
  prouvée » accusait le produit alors qu'il accusait un harnais incomplet).
  **Résultat : `acceptation-8432.sh --sans-build --vif 8432` rend
  `ACCEPTATION : TOUT PASSE`** — protocole, endpoints, projection, fratrie.
- **Les assertions de la batterie `/api/query`** : `GROUP BY` label 89 ms,
  `GROUP BY` type d'arête 455 ms, `CONTAINS 'session-'` 954 ms — inchangées. Le
  reste du chantier NANO (opérations nommées pour les agrégats arbitraires)
  n'est pas rapatrié : les réécrire est le travail du planificateur.
- **La Phase B du WS** (abonnement vivant, curseur d'époque, deltas) est portée
  mais n'a été vérifiée que par le chemin `live=1` — pas par un test dédié.
- Le reste de la lignée NANO (33 fichiers / 5 130 lignes, zero-copy) reste sur
  sa branche : on a rapatrié le **geste**, pas la branche.

## 5. Retour arrière (une commande par service)

**Binaire obrain** — l'unité pointe sur un binaire **nommé**, donc le retour est
un changement de nom :

```bash
sed -i 's|obrain.consolide-20260915|obrain.avant-consolide-20260915|' \
  ~/.config/systemd/user/obrain-viz.service
systemctl --user daemon-reload && systemctl --user restart obrain-viz.service
```

Sauvegardes empilées et datées, dans `~/sauvegardes-bascule-v3/` :

| binaire | ce qu'il est |
|---|---|
| `obrain.avant-consolide-20260915` | la lignée servie **juste avant ce déploiement** (j5 + garde de confinement, md5 `e169eb0c`) |
| `obrain.avant-garde-confinement-20260915` | avant la garde de confinement (j5 + compteurs, `fccbd880`) |
| `obrain.avant-nano-20260915` | avant tout ce chantier (la copie servie d'hier soir, `1610aad9`) |
| `obrain.consolide-20260915` | la version consolidée déployée (`b9d865a1`) |

Unité sauvegardée : `obrain-viz.service.bak-avant-consolide-20260915`.

**Passerelle** : `sed -i 's|stats-nano-20260915|v3-j4|' ~/.config/systemd/user/this-rs-gateway.service`,
`daemon-reload`, `restart` — sauvegarde `.bak-avant-stats-20260915`, binaire
`v3-j4` conservé intact.

**UI** : `bash scripts/ui-bascule.sh --liste` puis `ui-bascule.sh <version>` (le
magasin de versions garde toutes les versions gelées).

## 6. Ce qui n'est PAS fait, et qui demande ton accord

- **La lignée ne compilait pas** : `StageContinu.tsx` (commité) importe
  `graph/fractale`, qui n'avait jamais été commité — réparé ici (`fractale.ts` et
  `natures.ts` rapatriés). Un `tsc` en porte l'a montré ; sans porte, le défaut
  n'apparaissait qu'à la première construction ailleurs.
- **Committer dans la branche du tronc** (`fusion/tronc-plus-j6`) : la fusion vit
  sur `fusion/tronc-plus-j6-mes-correctifs`. Deux sessions travaillent dans ces
  worktrees ; je n'ai pas touché à leur branche.
- **Armer les réglages de trigrammes** (une fenêtre de redémarrage + une
  mesure avant/après).
- **Le worktree de la lignée consolidée vit dans `/tmp/tpc-j6`** : les commits
  sont dans `.git` (donc durables), mais le répertoire de travail est volatil.
  Si cette lignée devient LA référence, elle mérite un worktree durable.
