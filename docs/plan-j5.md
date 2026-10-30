# Plan J5 — « les consommateurs parlent v3 » : contrat de tests

Chantier NATIF-J5, lignée feat/principal-natif-j5 (base a8894383).
CE DOCUMENT EST LE CONTRAT : il est commité AVANT toute implémentation et
toute exécution. Les jalons B/C/D ne valent que s'ils satisfont les critères
énoncés ici.

## Objet

Migrer les CONSOMMATEURS obrain vers le format substrat v3 (principal natif)
et répéter la bascule prod SUR COPIES. Aucun déploiement : ce chantier livre
des binaires, des preuves, et un runbook (`docs/plan-j5-bascule.md`).

## Invariants de terrain

- Machine ffs-n3, charge 20-40 (wami teste sur `~/projects/banc-obrain`) :
  toute preuve de performance passe par les COMPTEURS DE SLOTS
  (`obrain_substrate::compteurs`), jamais par chronos seuls. Les chronos
  sont consignés à titre d'information.
- Stores vivants : JAMAIS ouverts par nos binaires (l'ouverture CLI est
  exclusive → migration irréversible). Lecture des fichiers pour COPIE
  uniquement (`cp`), validation de la copie par le binaire v1 déployé.
- Disque : jamais sous 8G libres ; `df -h /` avant chaque copie.
- Ports d'essai : 8460-8470, 127.0.0.1 uniquement. Serveurs tués après usage.

## Adaptation assumée (contrainte disque) — AMENDÉE

AMENDEMENT (11:10) : l orchestrateur a purgé target-verif de J4 — 72G
libres. La contrainte qui motivait le protocole séquentiel est levée : le
jalon C1 se joue sur DEUX copies de runtime-dev, conformément à la lettre
du contrat — runtime-dev.obrain (copie NON migrée, servie par le viz v1
déployé, port 8460) et runtime-dev-v3.obrain (copie froide de la
première, migrée, servie par le viz v3, port 8461). Les deux serveurs
tournent SIMULTANÉMENT, le panel est exécuté sur chacun et comparé
fichier à fichier. Le protocole séquentiel du texte initial (ci-dessous)
reste documenté comme repli si le disque venait à manquer.

Adaptation assumée (contrainte disque) (texte initial, repli)

Le jalon C1 demandait « viz v1 sur la copie NON migrée » EN PARALLÈLE du
viz v3 sur la copie migrée. Deux copies de runtime-dev = 24G : le disque
(29G libres à l'entrée) ne les porte pas en plus du build et de runtime-prod.
Le contrat comparatif est donc SÉQUENTIEL sur la MÊME copie :

1. phase BASELINE : viz **v1** (binaire déployé, read-only) sert la copie
   NON migrée ; le panel complet est exécuté, les réponses normalisées sont
   ARCHIVÉES (`~/tmp/harness-j5/resultats/<tag>/`).
2. migration de la copie (jalon B).
3. phase V3 : viz **v3** sert la copie migrée ; même panel ; comparaison
   fichier à fichier avec la baseline archivée.

Même store, mêmes octets de données, seul le format change : l'égalité des
réponses est exactement le contrat « le consommateur v3 rend ce que le
consommateur v1 rendait ». Le cache de réponses du viz (clé store+mtime+
requête, viz_serve.rs ~1315) est neutralisé de fait : deux PROCESSUS
distincts, et le mtime du store change à la migration ; par ceinture et
bretelles le panel porte un NONCE d'espacement dans chaque requête.

## Jalon A — build des binaires v3

- `cargo build --release` dans `~/projects/banc-obrain-j5` (binaire `obrain`).
- Critères :
  - build VERT ;
  - `strings target/release/obrain` contient les marqueurs `re-ancrage` (ou
    `reancrage`) et `tri.idx` ;
  - `grep SUBSTRATE_FORMAT_VERSION` dans les sources = 3 ;
  - `tenantcat` N'EST PAS attendu dans ce binaire (c'est la passerelle) ;
  - `target/debug` purgé s'il apparaît (disque à 97 %).
- Implémentation autorisée APRÈS commit du présent contrat : un drapeau
  `--slots` sur `obrain query` qui imprime sur STDERR le delta du compteur
  `slots_noeuds_lus` autour de l'exécution. C'est l'instrument des preuves
  C2 sur les copies réelles (les tests Rust `voisinage_entrant.rs` et
  `contains_indexe.rs` prouvent déjà la route sur stores synthétiques).

## Jalon B — copies et migration prouvée

Copies vers `~/tmp/harness-j5/` : `runtime-dev.obrain` (12G),
`runtime-prod.obrain` (7.3G), `chantiers-harness.obrain` (2.4M),
`/tmp/linux-full3.obrain` (119M).

Protocole par store :
1. `df -h /` (≥ 8G après copie estimée, sinon purge de nos artefacts).
2. `cp -r` du store vivant (store CHAUD : writers actifs possibles).
3. VALIDATION DE COPIE par le binaire **v1** déployé :
   `count(n)` + lecture d'un échantillon de propriétés. Copie déchirée →
   on la refait ; 3 échecs → consigné, on continue (copie à froid à la
   charge de l'orchestrateur).
4. CAPTURE BASELINE v1 (comptages CLI : total, par label, arêtes,
   échantillon de props) → fichier archivé.
5. MIGRATION par le binaire **v3** (ouverture exclusive via
   `obrain query <copie> "MATCH (n) RETURN count(n)"`), chrono consigné
   (information, machine chargée).
6. PREUVES (critères de passage) :
   - méta v3 : `format_version` lu = 3 (via `obrain info`/`validate` v3
     ou lecture du header) ;
   - relecture complète : mêmes comptages qu'en 4 (total, par label,
     arêtes), mêmes valeurs sur l'échantillon de props ;
   - IDEMPOTENCE : seconde ouverture v3 → aucun re-passage de migration
     (le méta ne bouge plus : mtime/sha identiques avant/après la seconde
     ouverture, réponse identique) ;
   - REFUS CROISÉ : le binaire **v1** sur la copie migrée échoue PROPREMENT
     (exit ≠ 0, message de version), et NE CORROMPT PAS : sha256 du méta et
     tailles des zones identiques avant/après la tentative, et le binaire
     v3 relit toujours les mêmes comptages.
7. runtime-dev migré : `obrain index rebuild` → `substrate.props.tri.idx.v2`
   apparaît ; `substrate.props.eq.idx.v2` toujours là ; comptages inchangés ;
   procédure de purge des `substrate.props.idx.tmp.*` orphelins consignée
   (fichiers temporaires d'une indexation interrompue : suppression à froid,
   AVANT redémarrage des writers, jamais pendant un rebuild en cours).

## Jalon C — contrat consommateurs

### C1 — viz serve (panel comparatif v1/v3)

Panel de 8 familles, requêtes concrétisées depuis le schéma de la copie
(placeholders résolus par le script — labels et propriétés réels) :

| famille    | forme                                                                 |
|------------|-----------------------------------------------------------------------|
| total      | `MATCH (n) RETURN count(n)`                                           |
| par label  | `MATCH (n:<L>) RETURN count(n)` (5 premiers labels du schéma)          |
| sortant    | `MATCH (a {qualified_name:'<QN>'})-[]->(b) RETURN b.qualified_name ORDER BY b.qualified_name LIMIT 50` |
| ENTRANT    | `MATCH (a)-[]->(b {qualified_name:'<QN>'}) RETURN a.qualified_name ORDER BY a.qualified_name LIMIT 50` |
| filtre eq  | `MATCH (n:<L> {<P>:'<V>'}) RETURN count(n)`                            |
| CONTAINS   | `MATCH (n:<L>) WHERE n.<P> CONTAINS '<frag>' RETURN count(n)`          |
| ORDER BY   | `MATCH (n:<L>) RETURN n.<P> ORDER BY n.<P> LIMIT 20`                   |
| agrégat    | `MATCH (n:<L>) RETURN count(n), min(n.<P>), max(n.<P>)`                |

Exécution via `GET /api/query?store=&query=` (viz v1 : port 8460 ; viz v3 :
port 8461). Critère : réponses normalisées IDENTIQUES baseline/v3 pour
chaque requête du panel. Tout écart → contre-épreuve par le CLI des deux
binaires sur le même store avant de conclure (préexistant ou introduit).

### C2 — preuves ns (compteurs de slots)

Sur la copie runtime-dev MIGRÉE et INDEXÉE, via `obrain query --slots` (v3) :
- voisinage ENTRANT (`MATCH (a)-[]->(b {qualified_name:'<QN>'}) RETURN count(a)`) :
  slots lus = 0 (route re-ancrage) — le TÉMOIN par balayage
  (`MATCH (n) RETURN count(n)`) doit lui compter ~N slots, sinon
  l'instrument ne mesure rien et la preuve est invalide ;
- CONTAINS sur propriété indexée : slots lus = 0 (route trigrammes),
  même témoin ;
- chronos consignés en regard des 4160 ms observés en prod sur
  obrain_voisinage (information, pas preuve).

### C3 — bridge MCP v3 (lecture + ÉCRITURE + kill -9)

`obrain bridge --store <copie migrée>` en JSON-RPC stdio :
- `initialize`, `tools/list` (présence des outils du harness : `query`,
  `search_graph`, `sessions_list`, `import_events`, ...) ;
  NB : `obrain_mondes`/`obrain_voisinage`/`obrain_recherche` sont les outils
  du MOTEUR DE CHAT (outils.rs, servis par l'engine), pas du bridge : au
  bridge on exerce leurs équivalents réels (`query` pour le voisinage,
  `search_graph`/`runtime_search` pour la recherche) — consigné comme
  écart de couverture s'il s'avère que le chat parle à un autre serveur.
- lecture : `query` (count + voisinage), `search_graph`, `sessions_list` ;
- ÉCRITURE (OBRAIN_WRITE=1) : création de nœuds (via `import_events` ou
  `query` CREATE), relecture, modification, relecture ;
- DURABILITÉ : `kill -9` RÉEL du process bridge après écritures NON
  checkpointées, réouverture : les écritures sont là (WAL v3 rejoué, trames
  0x04/0x05 NodeInsertV3/NodeUpdateV3 comprises — correctif J2), et le
  high-water d'allocation survit : un nœud créé APRÈS réouverture reçoit un
  id STRICTEMENT SUPÉRIEUR (pas de réutilisation d'un slot déjà accordé).

### C4 — Ergaster v3

Sur la COPIE migrée de chantiers-harness.obrain, binaire v3 :
`CREATE (e:Event {...})`, `MATCH (c:Chantier {id:...}) RETURN c`,
`MATCH ... SET c.status = ...` — les motifs exacts du protocole Ergaster
rendent les mêmes formes qu'en v1. (Le store Ergaster VIVANT reste servi
par le binaire v1 pendant tout le chantier.)

### C5 — passerelle v3

`~/sauvegardes-bascule-v3/this-rs-gateway.v3-j4` sur un port d'essai
(8462), `--store dev=<copie runtime-dev migrée>,tenant=public`, clés
`~/.config/this-rs-gateway/cles` : une requête ANONYME lit le tenant
public en HTTP 200, comptages cohérents avec le panel. Le service :8144
n'est NI arrêté NI reconfiguré.

### C6 — contre-épreuve BUG-PROPS-TRUNCATE

Deux niveaux :
- test Rust commité `crates/obrain-substrate/tests/contrat_j5_props_truncate.rs` :
  store avec props inline ET blobcol (payload > BLOB_COLUMN_THRESHOLD_BYTES),
  3 cycles open/close : AUCUN fichier props/blobcol ne change de taille,
  toutes les props relues identiques ;
- sur la copie runtime-dev migrée : 3 ouvertures/fermetures v3 successives,
  tailles de `substrate.props*` et `substrate.blobcol.*` STRICTEMENT
  stables, échantillon de props relu identique.

## Jalon D — runbook de bascule

`docs/plan-j5-bascule.md`, commité : séquence exacte orchestrateur (arrêt
writers, copies à froid, migration, index rebuild, purge idx.tmp, échange
des binaires, LimitNOFILE=65536 sur l'unité gateway, redémarrages, batterie
post-bascule = le panel C1 + C2, rollback complet), plus la liste des
consommateurs NON couverts (viz dev wami :8433/:8435/:8448, tmn).

## Traçabilité

Event Ergaster (store vivant, binaire v1, sans accents, retry ×5) à chaque
jalon franchi ; JOURNAL.md (`~/tmp/harness-j5/`) à chaque étape ; UN SEUL
SET final sur l'ancre NATIF-J5 (`status = "livre"` ou `"echec"`).
