# Chantier NATIF-J6 — l'index ordonne de propriete

But : rendre INSTANTANES `ORDER BY n.<prop> LIMIT k`, `min(n.<prop>)`, `max(n.<prop>)`
sur gros stores, sans jamais degrader une forme deja rapide (eq, voisinage, count,
CONTAINS, range/STARTS WITH) et sans jamais rendre un resultat different de l'existant.

## Le defaut mesure (banc runtime-full-v2, 259164 noeuds, binaire j6, `--slots`)

| requete                                             | slots lus |
|-----------------------------------------------------|-----------|
| `MATCH (n) RETURN n.qualified_name ORDER BY n.qualified_name LIMIT 20`      | 259164 |
| `... ORDER BY n.qualified_name DESC LIMIT 20`        | 259164 |
| `MATCH (n) RETURN min(n.qualified_name)`             | 259164 |
| `MATCH (n) RETURN max(n.qualified_name)`             | 259164 |
| `MATCH (n) RETURN count(n)`                          | 0 (deja rapide) |
| `... WHERE n.qualified_name STARTS WITH "session-"`  | 0 (index ordonne existant) |

Racine : ORDER BY + LIMIT et min/max BALAIENT toute la zone noeuds.

## Constat d'architecture (etat des lieux avant code)

1. Le **top-K heap borne existe deja** : `bounded_top_k` (obrain-core/.../operators/sort.rs),
   cable via `SortOperator::with_limit(k)` <- `plan_sort(sort, Some(k))` <- `plan_limit`
   sur un `Limit -> Sort` direct. Il borne la MEMOIRE (O(N log k), pas O(N log N)) mais
   DRAINE tout l'input : il lit toujours O(N) slots. La phase (a) du cahier des charges
   est donc DEJA livree cote code ; il reste a la PROUVER (non-regression + borne memoire).
   Elle ne suffit PAS a atteindre la cible slots ~= k.
2. Un **index ordonne persistant existe deja** : `substrate.props.idx.v2` (props_index.rs,
   `PropsIndexZone`). Entree = `EntreeIndex { cle: [u8;12], slot: u32 }`, serie TRIEE par
   `(cle, slot)`. La cle est un encodage MONOTONE mais LOSSY (chaines tronquees a 11 octets).
   Il sert le range / STARTS WITH via `candidats_ordonnes` -> `find_nodes_in_range`
   (sur-ensemble re-verifie). Il n'offre NI iteration en ordre de VALEUR exacte, NI
   premiere/derniere cle.

## Pourquoi l'index existant ne suffit pas pour ORDER BY / min-max exacts

La serie de `props.idx.v2` est triee par `(cle12, slot)`, pas par la VRAIE valeur.
Pour `qualified_name`, des dizaines de milliers de noeuds partagent le prefixe de 12
octets (`session-c254...`). Iterer la serie dans l'ordre `(cle, slot)` ne rend donc PAS
l'ordre exact des valeurs a l'interieur d'un groupe de meme cle, et rendre exactement les
k premiers exigerait de lire tout le groupe-frontiere : potentiellement ~65000 slots pour
un LIMIT 20. Cible ratee. Il faut une serie triee par la VRAIE valeur.

## Decision architecturale : nouvelle zone triee par valeur, alimentant les operateurs existants

Alternatives pesees :

- **A. Reutiliser `props.idx.v2` + re-verification du groupe-frontiere.** Zero nouvelle
  persistance. REJETE : pas de borne O(k) garantie (groupes de meme cle enormes sur
  `qualified_name`), viole la doctrine « instantane qu'importe les valeurs ».
- **B. Arbre B persistant par propriete.** Reequilibrage, eclatement de pages, bascule de
  racine atomique. REJETE : le module existant a deja tranche « serie triee reecrite
  plutot qu'arbre B » (reecriture integrale ~5 Mo << un balayage). On garde ce choix.
- **C. RETENUE — nouvelle zone `substrate.props.ord.idx.v2`, serie triee par la VRAIE
  valeur (puis slot).** Reutilise TELS QUELS les en-tetes de page, l'entree 16 octets,
  le CRC par page, l'ecriture atomique temp+rename+fsync, la generation et les ancres de
  couverture de `props_index.rs` (importes, pas dupliques). Seule difference semantique :
  l'ordre de la serie est l'ordre de VALEUR (le meme comparateur que le SORT du moteur),
  et non `(cle, slot)`. La cle est conservee dans l'entree comme accelerateur de recherche
  binaire (elle reste monotone non-decroissante, donc `candidats` reste valide), mais
  l'ITERATION l'ignore.

Consequences :

- `min(prop)` = premiere entree de la serie (valeur du slot). O(1). EXACT (serie triee par
  valeur, et min/max ignorent les NULL — verifie sur le banc : `min(head_sha)` rend le seul
  non-null).
- `max(prop)` = derniere entree. O(1). EXACT.
- `ORDER BY prop ASC LIMIT k` = les k premieres entrees (slots) -> alimentent l'operateur
  SORT EXISTANT (`SortOperator::with_limit(k)`), qui produit l'ordre exact. Comme on reutilise
  le meme comparateur stable, le resultat est IDENTIQUE au tri complet, mais ne lit que ~k
  slots. `DESC` = les k dernieres entrees, meme operateur.

## Sortie du bloc : identite des resultats garantie (fondement de la non-regression)

La route ne SELECTIONNE que le sous-ensemble de k noeuds que le top-K complet aurait garde,
puis le laisse trier par le MEME `SortOperator`. Elle est donc identique au chemin actuel
des que la SELECTION est exacte. La selection est exacte ssi :

- l'index est construit avec le MEME ordre que le comparateur du SORT (obrain-common
  `OrderableValue` / `comparer_ordonnable` — partage entre substrat et moteur, donc pas de
  divergence muette ; cf. lecon `79e9e283`) ;
- les NULL (noeuds sans la propriete) ne tombent pas dans le top-k :
  - ASC : NULL en dernier -> sur (nb entrees index >= k) ;
  - DESC : NULL en premier -> sur seulement si AUCUN NULL, i.e. nb entrees index == nb noeuds
    concernes. (Sur le banc, `qualified_name` est sur les 259164 noeuds : ASC et DESC surs.)

Garde-fou de cout (comme J5B) : hors de ces conditions (propriete non indexee, expression,
filtre concurrent incompatible, plusieurs cles de tri, index non fiable, k > entrees pour
ASC, NULL possibles pour DESC), on RETOMBE proprement sur le chemin actuel (heap borne).
Jamais d'erreur, jamais pire que l'existant.

## Semantique cadree et testee

- Noeuds sans la propriete : exclus de l'index (encoder_cle rend None pour Null) ; le
  comportement ORDER BY (NULL en fin/tete selon direction) est PRESERVE par le garde-fou.
- Egalites : ordre stable par slot (== ordre d'arrivee du NodeScan == ce que fait le heap).
- Types : String au minimum ; Int/Float/temporels ordonnes par `encoder_cle`/`OrderableValue`
  (monotone) ; la SELECTION reste exacte car on trie ensuite par le vrai comparateur.
- Sensibilite : comparaison BINAIRE octet-a-octet UTF-8 des chaines (== code-point), la meme
  que `OrderableValue::String` et que le SORT. Pas de collation locale.

## Persistance

- Nouvelle FormatZone `substrate.props.ord.idx.v2`, magics propres, `ORD_VERSION`.
- Construite par `obrain index rebuild` (etendu) depuis les seules chaines ; reconstructible ;
  jetee au moindre doute (magic/version/CRC/ancres) -> repli balayage. Aucune migration de
  `SUBSTRATE_FORMAT_VERSION` (zone auto-guerissante hors versioning meta, comme eq/tri).
- Le rebuild ne casse ni eq ni trigrammes ni le range (zones distinctes).

## Phasage (anti-enlisement)

- (a) heap borne : DEJA en place -> prouver non-regression + borne memoire.
- (b) zone ordonnee valeur + routes min/max + ORDER BY LIMIT k : le vrai instantane.
  Livrer min/max d'abord (plus petit), puis ORDER BY. Si ORDER BY deborde : livrer min/max
  + la zone prouvee + l'analyse d'ORDER BY.

## Preuves attendues

1. slots : ORDER BY LIMIT 20 ~= k (et non 259164) ; min/max O(1).
2. identite des resultats (memes 20 lignes, meme ordre) avant/apres.
3. min/max identiques.
4. non-regression : eq, voisinage entrant, count, CONTAINS, range/STARTS WITH inchanges.
5. `cargo test --release` engine + substrate verts.
6. idempotence du rebuild + refus croise v3 intact.

## NATIF-J6B — garde NULL relachee pour les placements NULLS LAST

Constat prod : la garde initiale « aucun NULL » (nb_valeurs_ordonnees ==
node_count) ne tient JAMAIS sur un store MIXTE (runtime-dev : des centaines de
milliers de noeuds sans qualified_name) ni sous ecritures (node_count bouge en
continu). Or elle est inutilement forte pour les placements NULLS LAST.

Raisonnement : l'index ne porte que les valeurs NON-NULL, triees par
comparer_total. Quand les NULL sortent en QUEUE du tri (NULLS LAST), les k
premieres lignes du resultat sont les k valeurs non-null extremes — les NULL
sont hors du LIMIT des que l'index rend k ids. La route est donc EXACTE meme en
presence de NULLs. Quand les NULL sortent en TETE (NULLS FIRST), l'index ne
peut pas les servir : la garde forte reste requise (et ne tient pas sur un
store mixte => repli, jamais faux).

Table de garde (placement effectif = clause explicite, sinon defaut par
direction : ASC->LAST, DESC->FIRST — convention PostgreSQL/Neo4j) :

| cas                          | placement NULL | garde                                    |
|------------------------------|----------------|------------------------------------------|
| ASC (defaut)                 | LAST           | index rend k ids (nb non-null >= k)      |
| ASC NULLS LAST explicite     | LAST           | idem                                     |
| DESC NULLS LAST explicite    | LAST           | idem (derniers_k)                        |
| DESC (defaut)                | FIRST          | AUCUN NULL (nb == node_count) => repli sur mixte |
| ASC NULLS FIRST explicite    | FIRST          | idem => repli sur mixte                  |

Fraicheur (store vivant) : toute ecriture posterieure a la construction fait
diverger les ancres => zone non fiable => nb/premiers_k/derniers_k rendent None
=> repli (jamais faux). Les hooks flush/checkpoint REBATISSENT la zone depuis
les chaines (source de verite, valeurs a jour) sous ancres fraiches : si le
processus ecrivain flush/checkpoint a la fermeture, l'index est re-synchronise
et la route re-engage avec des resultats a jour. A prouver sur banc : ecritures
post-rebuild (noeuds AVEC et SANS la propriete), puis mesure slots + identite.

Contrat de preuves J6B (banc MIXTE = banc + noeuds sans qualified_name) :
1. ASC LIMIT 20 avec NULLs presents : 0 slots + IDENTIQUE au temoin.
2. DESC defaut avec NULLs : repli (balayage) + IDENTIQUE.
3. DESC NULLS LAST explicite : 0 slots + IDENTIQUE.
4. ASC NULLS FIRST explicite : repli + IDENTIQUE.
5. min/max sur mixte : 0 slots + IDENTIQUES (NULL ignores des deux cotes).
6. Fraicheur : ecritures post-rebuild -> comportement documente (re-sync par
   hook ou repli), et IDENTITE tenue dans tous les cas.

## Contrat de tests (Rust)

- Substrat (`props_ord_index.rs`, module `tests`) : serie triee par valeur relue exacte,
  premiere/derniere entree = min/max, premiers_slots/derniers_slots bornes, ancres
  divergentes -> non fiable, fusion refuse une serie a cle non-monotone, survie a la
  reouverture, idempotence octet-a-octet de deux fusions du meme contenu.
- Moteur (test d'integration) : sur un store en dur, `min`/`max` == balayage temoin ;
  `ORDER BY prop ASC/DESC LIMIT k` == prefixe du tri complet temoin ; exclusion des NULL ;
  egalites stables. Ces tests passent AVANT l'optimisation (repli) et DOIVENT rester verts
  apres (garantie d'identite).
