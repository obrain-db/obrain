# Vérification du 2026-09-15 : froid, intégrité, et les tests qui le prouvent

Ce document consigne des vérifications **reproductibles**, pour qu'on n'ait pas à
les refaire ni à les croire sur parole. Chaque chiffre porte sa commande.

Binaire : `target/release/obrain` du worktree `banc-obrain-tronc`, branche
`consolidation/tronc-release-2026-09-15`. Store : `runtime-dev`
(439 688 nœuds / 1 324 105 arêtes).

---

## 1. Le froid d'`/api/viz/arcs` : contre-épreuve A/B

La plainte d'origine était « plus de 3 s » sur
`/api/viz/arcs?store=…&top=16&limit=60`. Le même binaire, le même store, un
répertoire de cache VIDE dans les deux cas — seule la variable d'environnement
change :

```bash
# A — préchauffage ACTIF (défaut)
OBRAIN_VIZ_CACHE_DIR=/tmp/viz-pre \
  target/release/obrain viz serve --host 127.0.0.1 --port 8438 --enrich-ricci 0 \
  --preset-default "Runtime dev (engrams)" \
  --preset "Runtime dev (engrams)=$STORE"

# B — préchauffage DÉSACTIVÉ
OBRAIN_VIZ_CACHE_DIR=/tmp/viz-temoin OBRAIN_PRECHO_ARCS_MS=0 \
  target/release/obrain viz serve --host 127.0.0.1 --port 8439 --enrich-ricci 0 \
  --preset-default "Runtime dev (engrams)" \
  --preset "Runtime dev (engrams)=$STORE"
```

| condition | 1re requête utilisateur | rejouée |
|---|---|---|
| **B** — préchauffage désactivé | **4,131 s** | 0,39 ms |
| **A** — préchauffage activé | **0,0007 s** (0,7 ms) | 0,66 ms |

Le préchauffage paie **4 243 ms en tâche de fond**, journalisé et donc
vérifiable :

```
WARN obrain::viz_serve: agrégat d'arcs préchauffé — le premier visiteur ne paiera
  plus le froid  monde=Runtime dev (engrams) ms=4243 octets=3413
```

Les deux appels rendent **200 / 2716 octets** : la correction ne change pas la
réponse, seulement QUI la paie.

### Ce que le préchauffage ne couvre pas (résidu connu)

`prechauffer_les_arcs` (`viz_serve.rs:645`) réchauffe **un seul palier** :
`arcs_corps(&path, 16, palier_arcs(60), None)`. Comme `palier_arcs(60) = 1024`
et que `top` n'entre pas dans la clé de cache, toute demande dont `limit ≤ 1024`
est servie — c'est le cas du dashboard (`top=16&limit=60`).

En revanche une demande à **`limit > 1024`** (palier 4096) reste froide : elle
recalcule. Rajouter ce palier doublerait le coût de démarrage pour un usage qui
n'est pas celui du dashboard ; c'est un arbitrage à faire sciemment, pas par
défaut.

---

## 2. Intégrité : la couche viz ne peut pas corrompre un store

Vérifié par recherche des chemins d'ÉCRITURE, pas par raisonnement :

| question | réponse | preuve |
|---|---|---|
| la couche viz ouvre-t-elle en écriture ? | non | `cached_ro` / `open_read_only` (`viz_serve.rs:155,220`) |
| `ricci` est-il persisté depuis le json-graph ? | non | `ricci_u8` écrit par `writer.rs:537,911` et `store.rs` |
| un `split(..)[k]` alimente-t-il une écriture ? | aucun | pas de `split(` dans un `SET`/`REMOVE`/`CREATE`/`MERGE` |
| une charge relue revient-elle dans le substrat ? | non | seuls `subgraph_for_stream` (WS) et le CLI (export) relisent |

Le seul défaut d'intégrité trouvé — l'**aller-retour asymétrique** des signaux
f32 — a été corrigé et verrouillé par
`ir::tests::aller_retour_des_signaux_f32_est_exact`, qui **échouait** avant
(écrit `0.56078434`, attendu `0.5607843399047852`).

---

## 3. Tests exécutés

| suite | tests | commande |
|---|---|---|
| `obrain-core --lib` | 870 | `cargo test -p obrain-core --lib` |
| `obrain-viz` (unit + golden) | 84 | `cargo test -p obrain-viz` |
| `gql_05_chaines` (chaînes, `split`) | 56 | `cargo test -p obrain-engine --test gql_05_chaines` |
| `coverage_aggregates` | 24 | `cargo test -p obrain-engine --test coverage_aggregates` |

1 034 tests, 0 échec.

---

## 4. Pièges de mesure à ne pas retomber dedans

1. **Vérifier `%{http_code}`, pas seulement `%{time_total}`.** L'IAM est armé
   depuis le 2026-09-15 : une route gardée sans cookie rend un **401 de 480
   octets en 0,5 ms**. Un temps excellent qui ne mesure rien — c'est arrivé, et
   c'est ce qui a fait croire un instant que la route était déjà réparée.
2. **Un banc sans cookie ne mesure pas le rendu.** Le service 8432 exige une
   session ; les mesures de ce document ont été faites sur des instances
   ISOLÉES, sans IAM, avec un cache neuf et contrôlé.
3. **Distinguer « clé neuve » de « clé rejouée ».** Un cache chaud rend 0,4 ms
   là où le calcul coûte 4 131 ms. Publier un seul des deux est mensonger.
4. **`/tmp` ne survit pas à un reboot.** Le cache d'agrégats vit dans
   `/tmp/obrain-viz-cache` si `OBRAIN_VIZ_CACHE_DIR` n'est pas défini (c'est le
   cas du service 8432). Après un reboot, tout le froid est à repayer — d'où
   l'intérêt du préchauffage, et d'un répertoire de cache persistant.
