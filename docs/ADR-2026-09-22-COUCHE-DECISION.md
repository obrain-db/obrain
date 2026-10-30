# ADR — Couche de décision : faire d'Obrain son propre modèle

**Date** : 2026-09-22. **Statut** : **PROPOSÉ ET PROJETÉ** — le plan est complet,
ancré dans le code et **présent dans le graphe obrain**. Comptes **relevés**, non
déclarés : **134 nœuds** et **187 arêtes** — 30 étapes, 27 contraintes, 10 pièges,
14 tests, 6 risques, 9 portes, 34 ancrages de code (tous résolus), 3 artefacts.

**Ces comptes ne doivent pas être crus sur parole : ils se revérifient.**

```bash
MATCH (x) WHERE x.chantier = 'couche-decision' RETURN count(x)   # 134 nœuds
MATCH (c:Chantier {qualified_name:'chantier:couche-decision'})-[:HAS_STEP]->(t:Todo) RETURN count(t)   # 30
```

*Ce document a dérivé quatre fois de la réalité, toujours dans le même sens :
écrit une fois, jamais reconfronté. Il porte désormais la requête qui le
contredit, plutôt qu'un chiffre à maintenir à la main.*

**Où en est le chantier : nulle part, et c'est le fait principal.** Les 26 étapes
sont `a_faire`, **0 des 6 critères d'acceptation** du § 10 est atteint, et
**0 des 14 tests** est écrit — parce qu'aucune ligne de la couche de décision
n'est implémentée. Ce qui existe est la **préparation** : le plan, ses contraintes
ancrées, ses pièges, ses tests à écrire, et la projection qui permet de les
suivre. **La Phase 0 reste la porte** (`todo:couche-decision:p0s1`, gardée par
`connaissance/gate_verite_avant_registre`) : sans oracle, le reste n'est que de
la comptabilité.

**Ce document a été corrigé cinq fois, par vérification et jamais par relecture.**
(1) Le plan a d'abord été projeté selon des labels inventés ; l'idiome
`:Chantier` / `:Todo` / `:Gate` existait déjà, et il a été adopté. (2) Le document
a affirmé que le code d'obrain n'était pas indexé — vrai à cet instant, faux
depuis l'indexation **dans le store qui porte le plan** (§ 14). (3) Une première
requête « relief de l'absence » n'avait pas filtré (`STARTS WITH`), et ses
chiffres ont été refaits. (4) L'audit a montré que les nœuds n'étaient **pas
namespacés** : impossibles à scoper hors parcours. (5) Le même audit, via
`detect_gaps`, a trouvé une **vraie omission** : l'étape `p0s2` — le verdict
GO/STOP lui-même — n'était pas gardée par la porte de vérité, alors que `p0s1`
l'était. Réparé, et le modèle complété d'une arête `TOUCHE` pour les cibles de
modification (§ 13).

**Nature** : architecture decision record doublé d'un plan d'exécution outillé,
décomposé en unités portant un identifiant stable et un état, pour être suivi —
et modifié — en parallèle par plusieurs agents.

**Régime des affirmations.** **CONSTATÉ** = lu dans un fichier cité.
**MESURÉ** = relevé par exécution, commande reproductible citée.
**DÉDUIT** = conséquence directe de ce qui précède.
**NON VÉRIFIÉ** = à prouver, et le document dit par quoi.

**Deux corrections de ce document, assumées.** (1) Le plan a d'abord été projeté
selon des labels que j'avais inventés ; l'idiome `:Chantier` / `:Todo` / `:Gate`
existait déjà dans le graphe, et il a été adopté à la place. (2) Ce document a
affirmé que le code d'obrain n'était pas indexé ; c'était vrai à cet instant, et
l'indexation a depuis été faite **dans le store qui porte le plan** — les 34
ancrages se résolvent maintenant sur les vrais symboles (§ 14).

**Rapport au graphe.** Ce fichier est la **source de vérité** ; le graphe en est
la **projection outillée**. Chaque identifiant `DEC-*` ci-dessous devient un nœud
du même nom. Le mapping et les requêtes d'avancement sont au § 13. L'ingestion
est `scripts/couche-decision.gql` (idempotente, vérifiée).

---

## 1. Le problème, posé sans détour

Obrain prend déjà des décisions partout — quels poids donner à une recherche,
quel seuil déclenche un réflexe, quel rayon d'affinité déclenche un détecteur
immunitaire, quelle demi-vie appliquer à un oubli. **Aucune de ces décisions
n'est apprise, et aucun des nombres qui les portent n'est une probabilité.**

**CONSTATÉ.** Les poids de `obrain.cognitive.search` sont des constantes :
similarité 0.3, énergie 0.3, topologie 0.3, synapse 0.1
(`crates/obrain-engine/src/cognitive_procedures.rs:445-448`).

**CONSTATÉ.** `crates/obrain-cognitive/src/intent.rs` classifie l'intention par
signaux pondérés, et le fichier l'assume : *« no embeddings or LLM required »*.

**CONSTATÉ.** `obrain.risk` renvoie `churn × pagerank × knowledge_gap ×
betweenness` (`crates/obrain-engine/src/cognitive_udfs.rs:68`) — un produit de
termes hétérogènes, dont personne ne connaît l'échelle.

**CONSTATÉ.** Aucune règle de score propre n'existe dans le dépôt : recherche de
`brier`, `log.?loss`, `isotonic`, `temperature scal`, `ECE`,
`expected calibration` sur `crates/` ne rend **aucun** résultat pertinent.

**DÉDUIT.** On ne peut pas écrire `WHERE risque > 0.7` en sachant ce que ça veut
dire. Ce n'est pas un manque de puissance de calcul : c'est un **manque
d'échelle**.

---

## 2. Ce qui existe déjà et sur quoi cette couche s'appuie

Tout existe sauf deux pièces. Le travail est un assemblage, pas une invention.

| Pièce | Où | État |
|---|---|---|
| Modèle prédictif `P(outcome\|context)` par engramme : moyenne, variance, `observation_count`, précision β | `crates/obrain-cognitive/src/engram/hopfield.rs:435` ; champ `predictive_model` `engram/types.rs:91` | **existe** |
| Mise à jour en ligne (Welford) | `engram/hopfield.rs:519` (`observe`) | **existe** |
| Correction par l'erreur : `lr = precision × pe`, postérieur de précision | `engram/hopfield.rs:633` (`bayesian_update`) | **existe** |
| Récupération = softmax sur motifs, **β par motif** (une température par motif) | `engram/hopfield.rs:1-30`, `softmax_compete` `hopfield.rs:243` | **existe** |
| Recalcul de la représentation du modèle à chaque mutation | `crates/obrain-cognitive/src/kernel.rs:1-50` (`MutationBus → recompute_affected`) | **existe** |
| Registre `Stimulus → Process → Outcome → Validation → Lesson` | `crates/obrain-cognitive/src/episodic.rs:1-8` ; `Outcome` `episodic.rs:76` | **existe** |
| Comptes persistés sur arêtes (précédent direct) | `PROP_SYNAPSE_REINFORCEMENT_COUNT`, `crates/obrain-cognitive/src/store_trait.rs:19-80` | **existe** |
| Write-through DashMap + persistance propriétés | `store_trait.rs:1-9`, `persist_node_f64` `store_trait.rs:129` | **existe** |
| Stockage d'une distribution (vecteur **et** map), survit au redémarrage | MESURÉ, § 7 / PI1 | **existe** |
| Comparaison d'une distribution par seuil | MESURÉ, § 7 / PI1 | **existe**, sous condition |
| Point d'extension des procédures `CALL` | `crates/obrain-engine/src/cognitive_procedures.rs:41` | **existe** |
| Registre d'UDF | `crates/obrain-engine/src/cognitive_udfs.rs:168-170` | **existe** |
| **Interface questions typées / options définies à l'appel** | — | **à construire** |
| **Règle de score propre** | — | **à construire** |

**DÉDUIT.** Le manque n'est pas le moteur, ni la mémoire, ni le retour
d'expérience : **il manque le verdict**. Obrain a tout le mécanisme
d'apprentissage sauf la fonction qui rend un nombre honnête.

---

## 3. Décision

**D1.** Obrain expose une **couche de décision** : des questions typées
(`choice`, `noul`, `score`) posées à un état, une **distribution** en retour,
dont chaque nombre est une **probabilité calibrée par construction** à partir des
issues accumulées.

**D2.** La calibration vient d'un **postérieur Beta** sur des comptes
`(k succès, n essais)` : `p = (k + α) / (n + α + β)`. Pas de descente de
gradient, pas de run d'entraînement, pas de refit de température hors ligne.
L'incertitude est honnête quand `n` est petit — Beta(1,1) rend 0.5, jamais une
certitude.

**D3.** Le registre de décisions vit **dans le substrat**, avec le patron
write-through du dépôt : DashMap en cache chaud, propriétés `_cog_decision_*`
persistées, rechargement paresseux.

**D4.** Les options sont identifiées **par leur nom**, jamais par leur index.
C'est ce qui rend les comptes transférables quand l'ensemble d'options évolue —
l'avantage que Laya revendique, porté par des données plutôt que par une
architecture.

**D5.** La probabilité n'est **pas** stockée comme vérité : elle est recalculée.
Le champ de distribution persisté n'est qu'un cache de lecture, re-dérivable
depuis `n`/`k`. **Un chiffre qu'on ne peut pas re-dériver est un chiffre en qui
on ne peut pas avoir confiance.**

**D6.** `obrain.decide` est une **lecture** prouvée ; `obrain.settle` est une
**écriture différée** par file, drainée au même endroit que les deltas de
synapses.

**Non-objectifs** (voir § 12) : pas d'ONNX, pas de Laya, pas de Jev, pas de
gradient, pas de LLM dans la boucle, pas de nouveau langage.

---

## 4. Modèle de données

Aucune collision : `:Decision` et `_cog_decision_*` sont libres (seul `:Engram`
existe comme label cognitif persisté — CONSTATÉ par inventaire des labels du
store vivant).

```
(:Decision)  _cog_decision_question   "triage.department"
             _cog_decision_type       "choice" | "noul" | "score"
             _cog_decision_options    ["billing","technical","sales","other"]
             _cog_decision_dist       [0.94, 0.04, 0.01, 0.01]
             _cog_decision_epoch      <f64 secondes UNIX>
             _cog_decision_settled    0 | 1
             _cog_decision_outcome    "billing"     -- null tant que non réglée
             _cog_decision_acted      "billing"     -- l'option RÉELLEMENT appliquée
             _cog_decision_source     "modele" | "politique" | "humain" | "exploration"
(:Decision)-[:SUIT_DE]->(:Decision)          -- ordre dans l'épisode
```

**Les trois derniers champs ne sont pas des détails — ils sont la condition de
toute question causale (§ 16).** Un registre qui ne stocke que la distribution et
l'issue est **observationnel** : il ne peut pas distinguer « l'option que j'ai
choisie » de « l'issue que j'ai eue ». Et `acted` peut différer de l'argmax —
parce qu'une politique ou un humain a tranché, ou parce qu'on a **exploré** : cette
différence *est* le signal interventionnel. Les ajouter maintenant coûte trois
propriétés ; les rétro-ajouter coûte **toutes les données accumulées d'ici là**,
c'est-à-dire précisément la période où elles sont les plus rares (DEC-C25).

(:DecisionOption)  _cog_decision_n  <u32 essais>
                   _cog_decision_k  <u32 succès>
(:Decision)-[:DECIDES_ON]->(:DecisionOption)
```

**Pourquoi `:DecisionOption` en nœuds plutôt qu'une map.** `pagerank`, `leiden`,
`spread` et `detect_gaps` s'appliquent alors à la structure de décision sans une
ligne de code supplémentaire. Une map est plus simple à écrire et **ne se
parcourt pas**.

**Contrainte de dimension.** `Value::Vector` accepte une longueur variable — une
distribution sur K options est donc stockable quel que soit K. **L'index HNSW
non** : il exige une dimension fixe (`crates/obrain-core/src/index/vector/config.rs:39`).
Ce n'est pas une gêne : une distribution n'a rien à faire dans un index de
similarité, et l'absence d'indexation est ici la bonne réponse.

---

## 5. Régime d'état — comment on voit l'avancement réel

Chaque unité de travail porte un identifiant stable et quatre propriétés
d'état :

| Propriété | Valeurs | Rôle |
|---|---|---|
| `status` | `a_faire` \| `en_cours` \| `fait` \| `bloque` | l'état courant |
| `owner` | chaîne libre (agent, personne, session) | **qui** travaille dessus |
| `seq` | entier | l'ordre dans la séquence (`NEXT` suit le même ordre) |
| `note` | texte | ce qu'il y a à faire — le livrable |
| `proof` | `CONSTATÉ:<fichier>:<ligne>` \| `MESURÉ:<commande>` | ce qui prouve `fait` (convention) |

**Règle du dépôt conservée** : `done` sans `proof` n'est pas `done`. C'est la
règle du régime des affirmations du projet, appliquée à l'avancement.

**MESURÉ.** Changer un état est une ligne, et la relecture est immédiate :

```bash
obrain query <store> "MATCH (t:Task {id:'DEC-P1-S2-T1'}) SET t.status='doing', t.owner='agent-a'"
obrain query <store> "MATCH (t:Task {id:'DEC-P1-S2-T1'}) RETURN t.status, t.owner"
```

**MESURÉ.** `MERGE` est idempotent (deux passes → un seul nœud) : le script
d'ingestion est rejouable sans doublon.

---

## 6. Contraintes dures

Chacune a une conséquence, et un ancrage vérifié. **C'est cette table qui décide
de la faisabilité**, pas le reste du document.

| Id | Contrainte | Conséquence | Ancrage |
|---|---|---|---|
| **DEC-C1** | Sans la feature `algos`, `plan_call_procedure` n'existe pas : **tout** `CALL` échoue à la planification. | La feature doit être active dans `cognitive-full` ; les tests l'activent explicitement. | CONSTATÉ `crates/obrain-engine/src/query/plan.rs:580-585` ; `query/planner/lpg/mutation.rs:743` |
| **DEC-C2** | `PROCEDURES_DE_LECTURE` classe lecture vs écriture. Tout ce qui n'y est pas est traité comme **écriture** : perte de l'annulation, transaction implicite payée, et **refus en session `open_read_only`**. | `obrain.decide` **doit être prouvé** lecture seule pour y figurer. `obrain.settle` ne doit **pas** y figurer. | CONSTATÉ `plan.rs:320-351`, `plan.rs:579` ; discipline `crates/obrain-engine/tests/procedure_qui_ecrit.rs` |
| **DEC-C3** | Le code dit lui-même pourquoi les cognitives sont exclues de cette liste : *« `CognitiveEngine` est un trait à mutabilité intérieure, sa signature ne prouve rien »*. L'exception `spread` a exigé une preuve (code + bench). | La preuve de DEC-C2 doit être faite, pas déclarée. | CONSTATÉ `plan.rs:335-351` |
| **DEC-C4** | Une procédure ne reçoit qu'un store en **lecture** : *« It is never used to mutate the graph »*. | L'écriture passe par les stores du `CognitiveEngine`, jamais par l'argument. | CONSTATÉ `cognitive_procedures.rs:38-52` ; modèle `execute_reinforce` `cognitive_procedures.rs:228` |
| **DEC-C5** | Écrire en synchrone depuis le chemin chaud a déjà cassé la production : sous 21 threads, sérialisation sur le writer WAL (gotcha 49938809). | `settle` **enfile**, il n'écrit pas. | CONSTATÉ `crates/obrain-cognitive/src/synapse.rs:473,493-502` ; drain `synapse.rs:897` |
| **DEC-C6** | Le câblage est **dédoublé** : `with_config` a sa propre copie inline et ne passe pas par `with_substrate_store`. Les stores cognitifs retombaient sur les propriétés LPG au lieu des colonnes du substrat. **42 tests.** | Un nouveau store doit être câblé dans **tous** les constructeurs, et un test doit le prouver. | CONSTATÉ `crates/obrain-engine/src/database/mod.rs:1100-1120`, « 42 tests » `:1117` |
| **DEC-C7** | La durabilité n'est pas acquise : perte possible des propriétés écrites après le dernier flush. | Une perte asymétrique est un **biais de survivance** du registre. Soit `settle` est durable, soit la perte est **comptée et exposée**. La nier est la seule option interdite. | MESURÉ, § 7 / PI5 |
| **DEC-C8** | Le piège du vecteur est **silencieux** et la documentation l'enseigne. | Coercition `vector()` obligatoire des **deux** côtés ; test dédié ; correction de la doc. | MESURÉ, § 7 / PI1 |
| **DEC-C9** | Le binaire déployé est minimal : `features: gql, cypher`. Ni `vector-index`, ni `embed`. | La couche ne dépend d'**aucune** des deux. Contrainte **favorable** : elle interdit de dériver vers un encodeur. | MESURÉ `obrain version` |
| **DEC-C10** | Lints stricts : `unsafe_code = "warn"`, `missing_docs = "warn"`, clippy `all` + `pedantic` en warn. | Chaque type public documenté, chaque module avec son `//!`. | CONSTATÉ `Cargo.toml:160-169` |
| **DEC-C11** | Éviction LRU des caches cognitifs, rechargement à la demande. | `decide` doit rendre le **même** résultat après éviction. | CONSTATÉ `crates/obrain-cognitive/src/config.rs:411-434` |
| **DEC-C12** | L'en-tête de `cognitive_procedures.rs` interdit de tailler une sortie pour un consommateur particulier. | `decide` rend des lignes neutres ; l'adaptation se fait au-dessus. | CONSTATÉ `cognitive_procedures.rs:21-27` |
| **DEC-C13** | Une règle de score propre a une propriété mathématique testable : l'espérance du score est **minimale à la probabilité vraie**. | La Phase 2 est entièrement vérifiable **sans corpus ni données**. | DÉDUIT (propriété des règles strictement propres) |
| **DEC-C14** | Le parseur GQL **n'accepte qu'une instruction par invocation** : *« the entire query must be consumed »*. | Un fichier `.gql` multi-instructions échoue à la deuxième. La projection passe par un pilote qui joue ligne par ligne. | MESURÉ (3 instructions séparées par `;`) |
| **DEC-C15** | `INSERT` d'arête **n'est pas idempotent** : le rejeu crée un doublon, compté 2. | Utiliser `MERGE` sur motif d'arête. Ne jamais revenir à `INSERT` pour une projection rejouable. | MESURÉ (`count(r)` = 2 après rejeu) |
| **DEC-C16** | `order` est un **mot réservé** : `ORDER BY p.order` échoue, `p.\`order\`` passe. | Nommer la propriété `seq`, jamais `order`. | MESURÉ |
| **DEC-C17** | La **concaténation de chaînes** ne fonctionne pas en Cypher : le préfixe collé au champ rend **0 résultat, sans erreur**. | Mettre les identifiants complets dans les données d'`UNWIND`, jamais les composer dans la requête. | MESURÉ (deux appels rendant 0 au lieu de 6 et 9) |
| **DEC-C18** | `ORDER BY` sur une propriété après `OPTIONAL MATCH` échoue : *« Variable not found for ORDER BY property projection »*. | Trier sur l'alias projeté, ou trier ailleurs. | MESURÉ |
| **DEC-C19** | **`STARTS WITH` est inutilisable** : le filtre rend **0 ligne** là où l'égalité rend des lignes, ou il est **ignoré** et rend le jeu complet. | Ne **jamais** filtrer par préfixe. Utiliser l'égalité sur `qualified_name`, ou une propriété dédiée au périmètre (le chantier). Figé par `DEC-T14`. | MESURÉ |
| **DEC-C20** | **`CONTAINS` est inutilisable de la même façon** : rend 0 pour *tous* les crates, y compris ceux dont un fichier est indexé. | Ne jamais filtrer par sous-chaîne. Égalité, ou correspondance de nom exacte. | MESURÉ |
| **DEC-C21** | **Projeter une propriété absente élimine la ligne.** `RETURN x.file_path` sur un `:Method` rend **0 ligne** là où `RETURN count(x)` sur le même `MATCH` rend **1**. | Un test d'absence ne doit **jamais** projeter la propriété qu'il teste. Utiliser `count()`. C'est ce piège qui a rendu mon premier test de couverture structurellement incapable de trouver quoi que ce soit. | MESURÉ |
| **DEC-C22** | **`count(x)` après un `OPTIONAL MATCH` compte les lignes**, pas les nœuds distincts : 28 au lieu de 21 sur le même jeu. | Compte de nœuds : `MATCH (x:Label) RETURN count(x)` sans expansion. Compte d'arêtes : motif obligatoire `MATCH (a)-[r:T]->(b) RETURN count(r)`. | MESURÉ |
| **DEC-C23** | **Les nœuds d'un chantier ne sont pas namespacés** : `:Todo` et `:Gate` sont partagés entre projets, et le filtre par préfixe étant cassé (C19), scoper ses propres nœuds était **impossible** hors parcours. | Chaque nœud porte une propriété `chantier`, et le périmètre se lit par **égalité**. Réparé sur les 119 nœuds. | MESURÉ |

---

## 7. Pièges relevés — et ce qui les fige

### PI1 — Le vecteur silencieux (**MESURÉ**, le plus dangereux du lot)

Un littéral `[0.9, 0.05, 0.05]` se parse en `Value::List`. `as_vector()`
n'accepte **que** `Value::Vector` (`crates/obrain-common/src/types/value.rs:277`).
Il existe une coercition `vector()` (`crates/obrain-core/src/execution/operators/filter.rs:3371`),
et `cosine_similarity` (`filter.rs:3393`).

Relevé sur une base jetable (`/tmp/dec-test`), binaire `target/release/obrain` :

| Requête | Résultat |
|---|---|
| `RETURN cosine_similarity([0.9,0.05,0.05],[0.88,0.06,0.06])` | `null` |
| `RETURN cosine_similarity(vector([0.9,0.05,0.05]), vector([0.88,0.06,0.06]))` | **0.9998429417610168** |
| `MATCH (d:Decision {name:'d1'}) RETURN cosine_similarity(d.dist, vector([...]))` | `null` |
| `… cosine_similarity(vector(d.dist), vector([...]))` | **0.9998429417610168** |
| `MATCH (d:Decision) WHERE cosine_similarity(vector(d.dist), vector([...])) > 0.99 RETURN d.name` | **`d1`** |

**Le piège n'est pas la coercition, c'est le silence** : `WHERE null > 0.99` est
**faux**, donc la requête rend **une liste vide**, pas une erreur. Un seuil mal
écrit donne un résultat plausible et faux.

**CONSTATÉ, et c'est plus grave que le piège lui-même :**
`docs/user-guide/vector-search/basics.md` montre exactement le geste **sans**
coercition (`WHERE cosine_similarity(d.embedding, $query) > 0.8`). **La
documentation enseigne le bug.**

**Figé par** : `DEC-T5`.

### PI2 — Le câblage dédoublé (**CONSTATÉ**, coût : 42 tests)

Voir DEC-C6. Le mécanisme du dégât : `substrate_handle()` rendait `None`, donc
les stores cognitifs tombaient dans le chemin LPG dont les défauts de trait ont
un corps **vide** — les appels réussissaient en ne faisant rien.

### PI3 — La persistance synchrone qui sérialise le WAL (**CONSTATÉ**)

Voir DEC-C5. Le remède en place : `SegQueue<MetadataDelta>`, drainée par le
consolidateur ou par le gestionnaire SIGTERM. **Un `set_node_property` direct
dans `settle` reproduit l'incident.**

### PI4 — Le catalogue de lecture fait échouer en session de lecture (**CONSTATÉ**)

Voir DEC-C2. Sans inscription dans `PROCEDURES_DE_LECTURE`, une session
`open_read_only` (le mode serveur de lecture) **refuse** la procédure avec
`Transaction(ReadOnly)` — alors que c'est la projection la plus utile en lecture.

### PI5 — La perte asymétrique après flush (**MESURÉ**)

Relevé à chaque ouverture de `/tmp/dec-test` :

```
WARN obrain_substrate::replay: post-replay anchor audit: nodes with labels but
no reachable properties (husk signature — properties written after the last
flush were lost) label_only=1 dangling=0 regime="pages_v2"
```

Sur un registre de calibration, cela signifie qu'on calibrerait sur les décisions
qui ont bien voulu s'écrire. **Figé par** : `DEC-T7` (invariant de conservation).

### PI6 — L'écho auto-référentiel (**CONSTATÉ**)

`crates/obrain-rag/src/feedback.rs` renforce les synapses entre les concepts
**mentionnés dans la réponse du LLM lui-même**. Le graphe apprend de la sortie
qu'il a nourrie. Un renforcement hebbien sans verdict extérieur converge vers
« ce que le modèle a déjà dit ». **C'est le mode d'échec qui tue la couche
entière, pas un défaut de réglage.** Figé par : `DEC-X3`.

### PI7 — Une instruction par invocation (**MESURÉ**)

Voir DEC-C14. Découvert en écrivant la projection : un fichier de trois
instructions séparées par `;` échoue à la deuxième, avec *« Unexpected token
'MERGE' after the end of the statement… the entire query must be consumed »*.

### PI8 — Le filtre par préfixe qui échoue en silence (**MESURÉ**)

`STARTS WITH` rend **0 ligne sans erreur**, ou **ignore le filtre et rend le jeu
complet**. Relevé sur les contraintes de ce chantier :

| Requête | Résultat |
|---|---|
| `MATCH (x:Constraint) RETURN count(x)` | **19** |
| `… WHERE x.qualified_name = 'couche-decision/contrainte-c1'` | la ligne, correctement |
| `… WHERE x.proof_kind = 'MESURÉ'` | **exactement 7** — l'égalité filtre bien |
| `… WHERE x.qualified_name STARTS WITH 'couche-decision/' RETURN count(x)` | **0** |
| `… WHERE x.qualified_name STARTS WITH 'couche-decision/' RETURN x.qualified_name` | **0 lignes** |

**Et c'est exactement le même piège que PI1** : la requête fautive ne lève rien,
elle rend un résultat plausible. Une première version de la requête « relief de
l'absence » de ce document (§ 13) **a été écrite avec `STARTS WITH` et a rendu
les 19 contraintes** : le filtre n'avait pas filtré, et le résultat avait l'air
juste parce qu'il contenait ce qu'on cherchait. Le relevé a été refait sans
préfixe. **Figé par** : `DEC-T14`.

---

## 8. Phases, étapes, tâches

Convention d'identifiant : `DEC-P<n>-S<k>` pour une étape, `DEC-P<n>-S<k>-T<j>`
pour une tâche. **Tout est `todo` à la date de ce document.**

### DEC-P0 — L'oracle (⛔ portail)

| Id | Objet | État | Sortie |
|---|---|---|---|
| DEC-P0-S1 | Inventaire des points de décision | todo | liste ancrée dans le code |
| DEC-P0-S1-T1 | Recenser : poids `cognitive.search`, coefficients `intent.rs`, seuils `reflex.rs`, `affinity_radius` `immune.rs`, constantes `engram/decay.rs`, gate `gap_detection` | todo | tableau |
| DEC-P0-S1-T2 | Pour chacun, nommer la **forme du verdict** (binaire, choix, ordinal) | todo | tableau |
| DEC-P0-S1-T3 | Pour chacun, nommer **ce qui, dehors, dira s'il a eu raison** | todo | tableau |
| DEC-P0-S2 | Verdict de recevabilité | todo | GO / STOP |
| DEC-P0-S2-T1 | **≥ 3 décisions à vérité observable hors du modèle** | todo | GO, sinon STOP (§ 11 X1) |

### DEC-P1 — Le registre

| Id | Objet | État | Livrable |
|---|---|---|---|
| DEC-P1-S1 | Modèle de données (§ 4) | todo | labels, clés, option par nom |
| DEC-P1-S2 | `DecisionStore` : DashMap + write-through | todo | patron `store_trait.rs` |
| DEC-P1-S2-T1 | Clés `_cog_decision_*` + comptes `n`/`k` | todo | constantes |
| DEC-P1-S2-T2 | Récupération d'un postérieur depuis les comptes | todo | fonction pure |
| DEC-P1-S3 | Persistance + rechargement au démarrage | todo | rechargement paresseux |
| DEC-P1-S3-T1 | Câblage dans **tous** les constructeurs (DEC-C6) | todo | aucune dégradation silencieuse |
| DEC-P1-S4 | Test de câblage substrat | todo | `DEC-T2` |

### DEC-P2 — L'estimateur et les règles de score

| Id | Objet | État | Livrable |
|---|---|---|---|
| DEC-P2-S1 | `Beta(α,β)` + primitives `choice`/`noul`/`score` | todo | module pur, sans I/O |
| DEC-P2-S2 | Règles de score propres : log, sphérique, Brier, RPS (ordinal) | todo | module pur |
| DEC-P2-S3 | Tests mathématiques | todo | `DEC-T3`, `DEC-T4` |
| DEC-P2-S4 | Refit de température : **décider après mesure**, pas avant | todo | mesure, puis décision |

### DEC-P3 — `obrain.decide` (lecture)

| Id | Objet | État | Livrable |
|---|---|---|---|
| DEC-P3-S1 | Récupération Hopfield sur les signatures des décisions passées | todo | candidats + β |
| DEC-P3-S2 | Procédure + enregistrement dans le dispatcher | todo | `CALL obrain.decide(...)` |
| DEC-P3-S3 | **Preuve de lecture seule** (code + bench + `open_read_only`) | todo | `DEC-T6` |
| DEC-P3-S4 | Inscription dans `PROCEDURES_DE_LECTURE` — **après** la preuve | todo | mode lecture accepté |

### DEC-P4 — `obrain.settle` (écriture différée)

| Id | Objet | État | Livrable |
|---|---|---|---|
| DEC-P4-S1 | `DecisionDelta` + `SegQueue` (DEC-C5) | todo | file, pas d'écriture directe |
| DEC-P4-S2 | Drain par le consolidateur **et** par SIGTERM | todo | aucun delta perdu |
| DEC-P4-S3 | Compteur de pertes **exposé** (DEC-C7) | todo | `decisions_lost_total` |
| DEC-P4-S4 | Survie au redémarrage + invariant de conservation | todo | `DEC-T7`, `DEC-T8` |

### DEC-P5 — La surface de requête

| Id | Objet | État | Livrable |
|---|---|---|---|
| DEC-P5-S1 | UDF de confiance + seuil sur distribution | todo | `WHERE` utilisable |
| DEC-P5-S2 | Test du piège PI1 | todo | `DEC-T5` |
| DEC-P5-S3 | Correction de `docs/user-guide/vector-search/basics.md` | todo | doc qui n'enseigne plus le bug |
| DEC-P5-S4 | `docs/cognitive/decisions.md` | todo | couche documentée |

### DEC-P6 — La boucle fermée et la mesure

| Id | Objet | État | Livrable |
|---|---|---|---|
| DEC-P6-S1 | Brancher l'oracle de DEC-P0 | todo | vérité qui arrive |
| DEC-P6-S2 | Accumuler | todo | N décisions réglées |
| DEC-P6-S3 | Mesurer la calibration contre témoins | todo | `DEC-T9` |
| DEC-P6-S4 | Publier la courbe d'apprentissage | todo | ECE vs nombre de décisions |

---

## 9. Tests

Discipline du dépôt respectée : chaque test négatif est accompagné d'un
**témoin d'anti-vacuité** qui prouve qu'il n'est pas vert par vide — convention
présente partout (`crates/obrain-engine/tests/procedure_qui_ecrit.rs:56` :
*« Si ce test échouait, les deux suivants ne prouveraient rien »*).

| Id | Test | Ce qu'il fige |
|---|---|---|
| DEC-T1 | `decision_store_roundtrip` | écrire, relire dans un **nouveau process**, comparer (PI5) |
| DEC-T2 | `decision_store_cablage_substrat` | le store utilise le chemin substrat, pas les propriétés LPG (DEC-C6) |
| DEC-T3 | `regle_propre_minimum_a_p_vrai` | balayage de `p` : le minimum d'espérance est à `p_true` (DEC-C13) |
| DEC-T4 | `beta_naif_et_convergence` | Beta(1,1) rend 0.5 ; `k/n → p` converge, l'intervalle se resserre |
| DEC-T5 | `seuil_distribution_exige_coercion` | **PI1** : sans `vector()` → vide silencieux ; avec → le bon nœud |
| DEC-T6 | `decide_est_lecture_seule` | session `open_read_only` : `decide` **passe** ; **et** `settle` y est **refusé** (DEC-C2) |
| DEC-T7 | `settle_survit_au_redemarrage` | invariant `écrites = réglées + en_attente + perdues` (DEC-C7) |
| DEC-T8 | `settle_ne_serialise_pas_le_wal` | micro-bench calqué sur `crates/obrain-cognitive/tests/synapse_reinforce_micro_bench.rs` : pas de régression p99 (DEC-C5) |
| DEC-T9 | `calibration_sur_decisions_reelles` | ECE mesuré, contre classe majoritaire **et** constante |
| DEC-T10 | `decision_dist_eviction_reload` | même résultat après éviction LRU (DEC-C11) |
| DEC-T11 | `decide_absent_de_la_liste_lecture_si_il_ecrit` | garde d'inventaire sur `PROCEDURES_DE_LECTURE` |
| DEC-T12 | `determinisme` | deux exécutions sur le même registre → **distribution identique** |

Commandes :

```bash
cargo test -p obrain-cognitive --features cognitive-full
cargo test -p obrain-engine --features algos,gql,cognitive-full
cargo test -p obrain-engine --features algos,gql --test procedure_qui_ecrit
cargo test --all-features --workspace --release
cargo clippy --workspace --all-targets -- -D warnings
```

---

## 10. Critères d'acceptation

**État à la date de l'audit : 0 sur 6.** Aucun n'est atteint, et aucun ne peut
l'être : les six portent sur du code qui n'existe pas. Ce tableau est le tableau
de bord des phases P3 à P6, pas une liste de courses.

| # | Critère | État | Ce qui manque |
|---|---|---|---|
| 1 | Calibration : N ≥ 500 décisions réglées, ECE < celui d'une constante | **non atteint** | aucun registre (P1), aucune décision |
| 2 | Conservation : `écrites = réglées + en_attente + perdues` après redémarrage | **non atteint** | `settle` n'existe pas (P4) |
| 3 | Sûreté : `decide` accepté en `open_read_only`, `settle` refusé | **non atteint** | ni l'un ni l'autre n'existe (P3, P4) |
| 4 | Coût : p99 de `decide` borné, mutation sans régression | **non atteint** | rien à mesurer |
| 5 | Piège figé : `DEC-T5` échoue si la coercition disparaît | **non atteint** | test non écrit, comme les 13 autres |
| 6 | Déterminisme : même registre → distribution identique | **non atteint** | rien à exécuter |

1. **Calibration** : sur N ≥ 500 décisions réglées, ECE **inférieur** à celui
   d'une constante, courbe de fiabilité publiée. Le chiffre compte moins que le
   fait d'être mesuré **sur les décisions d'Obrain**.
2. **Conservation** : l'invariant de `DEC-T7` tient après redémarrage, `perdues`
   **exposé**.
3. **Sûreté** : `decide` accepté en `open_read_only` ; `settle` refusé et non
   annulable ; aucun `CALL` nouveau n'échoue à la planification.
4. **Coût** : p99 de `decide` mesuré et borné ; chemin de mutation sans
   régression (`DEC-T8`).
5. **Piège figé** : `DEC-T5` échoue si la coercition disparaît.
6. **Déterminisme** : `DEC-T12`.

---

## 11. Risques et critères d'arrêt

| Id | Risque | Critère d'arrêt |
|---|---|---|
| **DEC-X1** | **L'oracle est le risque porteur.** Sans vérité extérieure, tout le reste n'est que de la comptabilité. | DEC-P0 sans 3 décisions à vérité observable → **STOP**. |
| **DEC-X2** | **Plafond de couverture.** Le modèle ne connaît que ce qui est passé par lui. | Si les décisions visées exigent de lire du **texte brut**, la réponse honnête est « il faut un encodeur » — et on retombe sur Laya. Le dire en P0, pas en P6. |
| **DEC-X3** | **L'écho** (PI6). | Tout oracle auto-référentiel est **refusé**, immédiatement. |
| **DEC-X4** | **La lenteur.** Un modèle vivant n'est bon qu'après beaucoup de décisions réglées. | Si après N décisions le modèle est **pire que la classe majoritaire**, le dire. C'est le piège que Laya documente contre lui-même : ses checkpoints de base sont à 0.362 contre 0.461. |
| **DEC-X5** | **La dette de câblage** (DEC-C6). | Le test de câblage existe dès P1, pas en P5. |

---

## 12. Non-objectifs

- Pas d'ONNX, pas de `ort`, pas de `tokenizers` : DEC-C9 l'interdit de fait.
- Pas de Laya, pas de Jev, pas de poids téléchargés.
- Pas de descente de gradient, pas de run d'entraînement, pas de checkpoint.
- Pas de LLM dans la boucle de décision.
- Pas de nouveau langage de requête.
- Pas d'encodeur texte → signature en v1 (c'est DEC-X2, et c'est un choix).

---

## 13. Décomposition en graphe

Le fichier est la source ; le graphe est la projection. **Chaque identifiant de
ce document devient un nœud du même nom** — la correspondance est bijective, donc
vérifiable.

### Modèle réellement projeté

**MESURÉ — et corrigé en cours de route.** J'avais d'abord conçu mes propres
labels (`:DecisionPlan`, `:Phase`, `:Step`, `:Task`). Le graphe en portait
**déjà** un idiome, relevé avant d'écrire : `chantier:dragon-3d-n1`
(`:Chantier`) → `todo:d3d:graphe` (`:Todo`, `status` ∈ `a_faire`/`fait`/
`en_cours`) → `:NEXT`, plus des `:Gate` du type `connaissance/gate_build_a_jour`
(*« avant de déployer, vérifier que le build reflète le code »*). **La
progression suit donc la convention de la maison, pas la mienne.**

| Label | Propriétés | Identifiants |
|---|---|---|
| `:Chantier` | `qualified_name`, `name`, `description`, `status` | `chantier:couche-decision` |
| `:Todo` | `qualified_name`, `name`, `note`, `seq`, `status`, `owner` | `todo:couche-decision:p0s1` … |
| `:Gate` | `qualified_name`, `name`, `description`, `status` | `connaissance/gate_*` (7) |
| `:Constraint` | `qualified_name`, `statement`, `consequence`, `proof_kind`, `anchor` | `couche-decision/contrainte-c1` … `c18` |
| `:Pitfall` | `qualified_name`, `description`, `evidence` | `couche-decision/piege-pi1` … `pi7` |
| `:Test` | `qualified_name`, `name`, `pins`, `status` | `couche-decision/test-t1` … `t13` |
| `:Risk` | `qualified_name`, `statement`, `stop_criterion` | `couche-decision/risque-x1` … `x6` |
| `:CodeRef` | `qualified_name`, `file`, `line`, `symbol` | `couche-decision/ref-*` (34) |
| `:Artefact` | `qualified_name`, `kind`, `fichier`, `description` | les 3 fichiers de ce chantier |

### Modèle d'arêtes (réellement posé)

```
(:Chantier)-[:HAS_STEP]->(:Todo)         26
(:Todo)-[:NEXT]->(:Todo)                 25   -- séquence linéaire, vérifiée : 1 seule tête (seq 1), 25 sortants
(:Chantier)-[:S_APPUIE_SUR]->(:CodeRef)  16   -- ce qui existe déjà (§ 2)
(:Chantier)-[:SOURCE]->(:Artefact)        3   -- ADR + pilote + données
(:CodeRef)-[:ANCRE_DANS]->(:symbole|:File) 34  -- 18 symboles nommés, 16 fichiers
(:Constraint)-[:ANCHORED_AT]->(:CodeRef) 17   -- +1 depuis un :Pitfall (PI6), d'où 18 au total
(:Constraint)-[:ENFORCED_BY]->(:Test)    11
(:Pitfall)-[:PINNED_BY]->(:Test)          7
(:Risk)-[:GATES]->(:Todo)                 6
(:Todo)-[:GARDI_PAR]->(:Gate)             9   -- 9 étapes gardées, sur 7 portes distinctes
                                          ────
                                          155  arêtes
```

**Tous les nœuds du chantier portent `chantier = 'couche-decision'`** (119
nœuds). C'est ce qui rend le périmètre lisible **par égalité** — donc fiable,
puisque le filtre par préfixe est cassé (DEC-C19) et que `:Todo` comme `:Gate`
sont partagés avec d'autres projets :

```bash
# Le périmètre exact, sans parcours et sans préfixe
MATCH (x) WHERE x.chantier = 'couche-decision' RETURN labels(x), count(x)
```

### Ce que ces arêtes rendent possible — et que la prose ne permet pas

**MESURÉ.** Sans ces arêtes, les contraintes ne sont que du texte indexé :
`impact_set` n'a rien à parcourir, `detect_gaps` n'a rien à relier. **Avec**
elles, ces quatre requêtes fonctionnent (vérifiées) :

```bash
# Où en est le chantier ? — 26 lignes, ordonnées, l'état lu d'un coup d'œil
obrain query <store> "MATCH (c:Chantier {qualified_name:'chantier:couche-decision'})\
-[:HAS_STEP]->(t:Todo) RETURN t.seq, t.name, t.status ORDER BY t.seq"
```

```bash
# Qui travaille sur quoi, maintenant ?
obrain query <store> "MATCH (t:Todo) WHERE t.status IN ['en_cours','bloque'] \
  RETURN t.qualified_name, t.owner, t.status"
```

```bash
# Le relief de l'absence : quelles contraintes ne sont garanties par AUCUN test ?
# SANS filtre par préfixe : STARTS WITH est inutilisable (DEC-C19).
obrain query <store> "MATCH (x:Constraint) \
  OPTIONAL MATCH (x)-[:ENFORCED_BY]->(t:Test) RETURN x.qualified_name, count(t) AS tests"
```

*C'est la requête qui a produit le relevé du § 7 / PI8 — après avoir été écrite
une première fois avec `STARTS WITH`, qui n'avait pas filtré. Résultat utile :
**10 des 19 contraintes n'ont aucun test.** C'est la liste de travail, et elle
est sortie du graphe, pas d'une relecture.*

```bash
# Quelles portes gardent quelle étape ?
obrain query <store> "MATCH (t:Todo)-[:GARDI_PAR]->(g:Gate) RETURN t.name, g.name, g.description"
```

### Changer un état

**MESURÉ.** Une ligne, et la relecture est immédiate :

```bash
obrain query <store> "MATCH (t:Todo {qualified_name:'todo:couche-decision:p1s2'}) \
  SET t.status='en_cours', t.owner='agent-a'"
```

États utilisés : `a_faire`, `en_cours`, `fait`, `bloque`. La règle du dépôt
s'applique : **`fait` sans `proof` n'est pas `fait`**.

### Ingestion

`scripts/couche-decision.gql` (données, une instruction par ligne) joué par
`scripts/couche-decision-ingest.sh` (pilote). Deux garde-fous vérifiés :
`MERGE … ON CREATE SET` pour les nœuds — **un `status` déjà changé survit au
rejeu** ; `MERGE` sur motif d'arête — pas de doublon. Les pièges DEC-C14 à
DEC-C18 viennent tous de cette projection.

---

## 14. Chemin d'écriture : d'abord fermé, puis ouvert

**MESURÉ, dans cet ordre.** La première tentative d'écriture a été **refusée** par
le pont `codebase-memory-mcp` : `{"acces":"écriture", "http":401}`, *« aucune
identité présentée »*, avec `OBRAIN_IAM=1` mais **aucun `OBRAIN_IAM_TOKEN`** dans
l'environnement. Le même refus indique que l'administration IAM est close
(*« le magasin d'identités n'a qu'un écrivain, viz serve ; les sessions ne
peuvent ni s'émettre ni se révoquer »*) : impossible donc de forger un jeton. Et
le CLI ne peut pas écrire non plus, `viz serve` tenant le store en écrivain
unique (verrou exclusif, `pid` cité par le moteur) — c'est la raison d'être du
drapeau `--read-only`.

**Puis le pont `parc` a écrit.** Retrait de codebase-memory, passage plein
obrain : `graph_link` → `status: ok`, et surtout **`query_graph` accepte les
écritures** (`MERGE`, `SET`, `DELETE`, `UNWIND`). C'est le canal utilisé pour
toute la projection ci-dessus.

**Trois limites relevées sur ce canal** : les **propriétés d'arête ne sont pas
persistées** (*« API nœud uniquement pour l'instant »*) — tout le savoir est donc
sur les nœuds ; le CLI reste **inutilisable en écriture** tant que le processus
MCP tient le verrou exclusif ; et l'indexation du code est une opération longue
(voir ci-dessous).

### Le code obrain est indexé — dans le même store que le plan

**MESURÉ.** La première version de ce document affirmait que le graphe « ne
contient pas le code d'obrain (aucun `procedure_mute`, aucun `SynapseStore`) » et
que les `:CodeRef` resteraient des ancres autoportantes. **C'était vrai à
l'instant où c'était écrit, et ce ne l'est plus.**

Le dépôt a d'abord été cherché au mauvais endroit : il existe un **monde** obrain
(`mondes/engine_idx.obrain`, preset de `viz serve`) qui contient bien le code —
mais **partiel et figé** : `files_scanned` 244, **le seul crate
`crates/obrain_engine/`**, daté du 8 septembre. Les symboles `SynapseStore`,
`persist_node_f64`, `PredictiveModel`, `as_vector`, `Engram`, `Outcome` y sont
**absents** (vérifié par égalité sur `name`, la seule forme fiable).

Le dépôt complet a donc été indexé **dans le store qui porte le plan**, sous le
projet `obrain` :

| Avant | Après |
|---|---|
| 56 908 nœuds, 722 819 arêtes | **110 917 nœuds, 1 482 090 arêtes** |
| projet `obrain` absent | `files_scanned` **1 885**, 13 partiels, 267 non supportés |
| `SynapseStore` introuvable | trouvé, `crates/obrain-cognitive/src/synapse.rs` |

**Conséquence : les 34 ancrages sont désormais reliés aux vrais symboles** par
`(:CodeRef)-[:ANCRE_DANS]->(:Function|:Method|:Struct|:Variable|:File)` —
18 vers un symbole nommé, 16 vers le fichier quand l'ancre vise une ligne à
l'intérieur d'un corps de fonction. La chaîne
`Contrainte → ANCRAGE → symbole réel` est parcourable :

```bash
# Ce que le chantier touche réellement dans le code
MATCH (c:Constraint)-[:ANCHORED_AT]->(r:CodeRef)-[:ANCRE_DANS]->(s)
RETURN s.file_path, s.name, c.qualified_name
```

**Une réserve d'honnêteté** : l'appel d'indexation a rendu `-32001: Request
timed out` côté MCP alors que le travail a abouti côté serveur. Un timeout n'est
donc **pas** une preuve d'échec — il faut relire le store pour savoir.

**Une leçon de méthode, à garder** : deux de mes conclusions intermédiaires
étaient fausses parce que la requête l'était, pas la donnée — `STARTS WITH` et
`CONTAINS` rendent 0 là où l'égalité rend des lignes, et **projeter une propriété
absente élimine la ligne** (`RETURN x.file_path` sur un `:Method` rend 0 ligne,
alors que `RETURN count(x)` sur le même `MATCH` rend 1). Un test d'absence ne
doit jamais projeter la propriété qu'il teste.


---

## 15. Premier pas

**DEC-P0 seul.** Sortir l'inventaire des points de décision d'`obrain-cognitive`
avec, pour chacun, la forme du verdict et ce qui, dehors, dira s'il a eu raison.
C'est borné, c'est de l'inventaire, et **il n'y a rien à écrire dans le registre
avant que ce tableau existe** — sinon on construit sur du vide. Voir DEC-X1.

---

## 16. Causalité : oui, mais comme couche distincte — et pas tout de suite

**Question posée** : introduit-on des graphes de causalité dans cette architecture ?

**Réponse** : oui, et il y a une raison précise. Mais **jamais en fusionnant la
structure causale dans l'estimateur** — et le prérequis est un enregistrement qui
n'existe pas encore dans ce plan.

### 16.1 Ce qu'obrain a déjà, et ce que ce n'est pas

**CONSTATÉ.** Obrain porte déjà une notion de causalité, et elle est documentée :

| Pièce | Où |
|---|---|
| `EdgeClass::Causal` vs `Associative`, classification par type d'arête | `crates/obrain-cognitive/src/activation.rs:122,227,245` |
| `causal_boost` appliqué à la propagation | `activation.rs:408` |
| `CALL obrain.search_causes` / `search_effects` — BFS inverse/avant, `score(v) = Σ score(u) × γ`, **γ = 0,5** | `crates/obrain-engine/src/database/search_causality.rs:1-27` |

Et la limite est **écrite noir sur blanc** :
*« "causalité" = structurelle dirigée amortie (**pas d'inférence temporelle
formelle**) »* (`docs/APPORT-OBRAIN-USAGES.md:106`).

**Il faut tenir cette frontière.** γ = 0,5 est une **constante d'amortissement**,
pas une stratégie d'identification. Deux parents de même degré reçoivent le même
crédit, causal ou non ; un chemin entièrement confondu propage un score parfaitement
confiant. Promouvoir ça en inférence causale produirait **exactement le « score sans
échelle » que ce plan combat** — un nombre qui a l'air d'une mesure. D'où DEC-C26.

### 16.2 Ce que l'estimateur du § 3 ne peut structurellement pas faire

Quatre questions, quatre réponses :

| Question | L'estimateur Beta y répond-il ? |
|---|---|
| Qu'observe-t-on quand X arrive ? | **oui** — c'est sa fonction |
| Que se passe-t-il si je **fais** X ? | **non** — le registre ne voit que ce qui a été fait, et confond « X est juste » avec « X est choisi quand c'est facile » |
| **Quelle** décision a causé l'issue ? | **non** — une issue qui arrive après cinq décisions ne dit pas laquelle |
| Qu'aurait donné l'option non choisie ? | **non, par construction** |

Les deux lignes du milieu sont **précisément les deux modes d'échec que ce plan
nomme déjà** : l'assignation de crédit est le fond du piège DEC-PI6 (l'écho —
`feedback.rs` renforce ce qui a **co-occured** dans la réponse du LLM, c'est-à-dire
une confusion entre corrélation et efficacité), et `risk = churn × pagerank × gap ×
betweenness` est un produit de termes associatifs qu'on utilise comme une grandeur
causale.

### 16.3 Le prérequis : le registre d'intervention (DEC-C25)

Le modèle de données du § 4 **ne permet aujourd'hui même pas de formuler** une
question causale : il stocke la distribution et l'issue, pas l'option **agie**, ni
l'ordre des décisions. Trois ajouts, à faire **avant** d'accumuler :

- `_cog_decision_acted` — l'option réellement appliquée. Elle peut **différer de
  l'argmax** : une politique a tranché, un humain a corrigé, ou on a exploré.
  **Cette différence est le signal interventionnel.**
- `_cog_decision_source` — `modele` | `politique` | `humain` | **`exploration`**.
- `(:Decision)-[:SUIT_DE]->(:Decision)` — l'ordre dans l'épisode, sans lequel
  l'assignation de crédit est impossible.

C'est **P4-S5**, inséré dans la chaîne (seq 19). Coût : trois propriétés,
maintenant. Coût du rétro-ajout : **toutes les données d'ici là** (DEC-PI10).

### 16.4 La sortie honnête du plafond observationnel (DEC-C27)

Un registre ne voit jamais l'option non choisie. La seule voie honnête vers autre
chose que de l'observationnel est la **randomisation** : choisir délibérément une
option non-argmax à faible taux, et marquer son origine comme `exploration`.

C'est **le même geste que le bruit gaussien centré du RLCD de Laya** — là pour
estimer un gradient, ici pour collecter des données. Ce n'est pas une coïncidence :
dans les deux cas, on paie un peu de performance immédiate pour obtenir de
l'information qu'aucune observation passive ne donnera.

**P7-S2** le met en œuvre et exige de **mesurer son coût en performance** — un taux
d'exploration qui ne se paie pas est un taux qu'on n'assume pas.

### 16.5 L'obligation de falsificateur

Le graphe porte déjà l'idiome : `:Falsification` (13 nœuds, dossier Riemann).
Toute arête causale doit porter **ce qu'on devrait observer pour la retirer**.

D'où `connaissance/gate_falsificateur_obligatoire`, qui garde **P7-S1** et
**P7-S3** : sans falsificateur, une structure causale n'est pas de la connaissance,
c'est une opinion bien rangée. Et **P7-S3** exige de publier le **taux de retrait** —
c'est la seule mesure de la valeur de la couche causale.

### 16.6 L'ordre, et il n'est pas négociable

```
P1-P2  registre + estimateur Beta   ← la calibration est la seule chose honnête ici
P4-S5  registre d'intervention      ← à faire AVANT d'accumuler, sinon perte sèche
P6     la boucle fermée, l'oracle
P7     la couche causale            ← APRÈS, et gardée par le falsificateur
```

**La structure causale ne doit jamais entrer dans le calcul de la probabilité.**
Si on la fusionne, on perd la calibration — la seule chose que ce plan peut
démontrer — pour gagner une structure qu'on ne peut pas valider. La séparation est
ce qui permet de **mesurer chacune des deux** : « avec quelle fiabilité » (le
registre) et « à cause de quoi » (la couche causale). Ce sont deux questions
différentes, et les mélanger rend les deux intestables.
