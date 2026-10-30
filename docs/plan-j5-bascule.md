# Runbook de bascule v3 — séquence orchestrateur (NATIF-J5, jalon D)

Répété sur copies le 14/09 (chantier NATIF-J5, worktree `~/projects/banc-obrain-j5`,
preuves dans `~/tmp/harness-j5/`). La bascule est un ÉVÉNEMENT UNIQUE COORDONNÉ :
la migration v1→v3 est irréversible sur place, tout consommateur v1 refuse un
store v3 et réciproquement. AUCUNE étape de ce runbook n'est optionnelle.

## Binaires livrés (banc NATIF-J5)

| rôle | chemin |
|------|--------|
| obrain v3 (viz serve + bridge + index rebuild + query + repair-degrees) | `~/projects/banc-obrain-j5/target/release/obrain` |
| passerelle v3 | `~/sauvegardes-bascule-v3/this-rs-gateway.v3-j4` (archivée J4, validée C5) |
| obrain v1 (rollback) | `~/.local/share/obrain-harness/bin/obrain` (l'actuel — à ARCHIVER avant échange) |
| passerelle v1 (rollback) | `~/sauvegardes-bascule-v3/this-rs-gateway.v1-avant-bascule` |

Marqueurs vérifiés sur le binaire j5 : `re-ancrage` (×4), `tri.idx` (×1),
`SUBSTRATE_FORMAT_VERSION = 3` ; le binaire bridge déployé du 13/09 13:29 n'a
PAS le re-ancrage (0 occurrence strings) — c'est lui qui rend obrain_voisinage
en 4160 ms ; le j5 rend la même requête en ~0,2 ms serveur (slots = 0).

## Timings mesurés sur copies (information — machine chargée 20+)

| étape | mesuré |
|-------|--------|
| migration chantiers-harness (2,4M) | < 1 s |
| migration runtime-prod (7,3G, 316 511 nœuds) | ~1 s |
| migration runtime-dev (12G, 424 305 nœuds) | < 1 s |
| `index rebuild` runtime-dev (eq + trigrammes, 469M de tri.idx) | 3,8 s |
| copie à froid 12G (disque local) | ~2-3 min |

La migration ne réécrit que la zone nodes (32→40 octets, rename atomique,
méta bumpée en dernier = point de commit, idempotente — prouvée : seconde
ouverture sans réécriture, inode/mtime de substrate.nodes inchangés).

## Séquence

### 0. Préconditions
- `df -h /` : au moins 2× la taille des stores vivants à sauvegarder de libre.
- Binaire j5 présent et marqué (strings ci-dessus) ; `obrain version` répond.
- Fenêtre annoncée aux utilisateurs du chat (bridges MCP coupés pendant la fenêtre).

### 1. Arrêt des WRITERS puis des lecteurs (l'ordre compte)
```
systemctl --user stop dsh-dev-3083        # le writer principal (graph-writer)
systemctl --user stop <gateway :8144>     # this-rs-gateway
systemctl --user stop <viz :8432>         # viz prod
systemctl --user stop <sentinelle continuity>
# + tout timer/cron qui lance obrain (continuity, enrich)
```
Vérifier qu'aucun process ne tient les stores :
`fuser ~/.local/share/obrain-harness/data/*.obrain/substrate.meta` → rien.

### 2. Copies à froid de sauvegarde (rollback)
```
for S in runtime-dev runtime-prod chantiers-harness(~/projects/obrain/) ...; do
  cp -r <store> <store>.bak-avant-v3-$(date +%Y%m%d)
done
```
Valider CHAQUE copie par le binaire v1 : `obrain query <copie> 'MATCH (n) RETURN count(n)'`
+ une lecture de props. C'est la SEULE fenêtre où la copie est garantie saine :
les copies à chaud faites pendant la répétition portaient des artefacts
(husk signature label_only=21/22 — props écrites après le dernier flush perdues).

### 3. Migration des stores vivants (binaire j5, ouverture exclusive)
```
J5=~/projects/banc-obrain-j5/target/release/obrain
$J5 query <store> 'MATCH (n) RETURN count(n)'   # déclenche la migration, rend le count
```
Ordre : chantiers-harness, runtime-prod, runtime-dev, (autres stores du data/ :
atlas, iam, agent-synapse, runtime, runtime-full* — au choix de l'orchestrateur,
même procédure). ATTENTION : après cette étape, plus AUCUN binaire v1 ne lit ces
stores (refus propre vérifié : exit ≠ 0, message de version, empreinte sha256
du store INTACTE après la tentative — prouvé sur copies).

### 4. Vérité des comptages — écarts ATTENDUS (prouvés sur copies)
Les counts v3 peuvent différer des counts v1 d'avant : c'est le v1 qui mentait.
- count() v1 lisait des compteurs de manifeste rassis (sur la copie dev :
  424 258 annoncés contre 424 320 énumérés par lui-même) ;
- v3 est auto-cohérent (count = énumération = schéma : 424 305 sur la copie) ;
- les records VIDES que le prédicat v1 énumérait à tort (15 slots contigus à
  la frontière de page 0x60000 sur la copie dev) deviennent invisibles en v3
  (trous sans ALLOCATED) — c'est une CORRECTION, pas une perte ;
- store linux-full3 : manifeste-seul (zones à 0 octet) — v1 annonçait
  1 553 071 (faux), v3 dit 0 (vrai).
Preuve d'absence de perte à rejouer : énumération `MATCH (n) RETURN id(n)`
avant (v1)/après (v3), les ids v3 ⊂ ids v1 et les exclus sont vides.

### 5. Index et réparations
```
$J5 index rebuild <store>       # eq + trigrammes (tri.idx.v2 apparaît)
$J5 repair-degrees <store>      # sidecars de degrés RASSIS constatés sur
                                # runtime-dev : le v1 déployé rend 0 sortant
                                # sur dsh-runtime — à réparer À FROID
rm <store>/substrate.props.idx.tmp.*   # orphelins d'indexations interrompues
                                # (7 constatés sur runtime-dev) — à froid
                                # UNIQUEMENT, jamais pendant un rebuild
```
Vérifier après rebuild : `substrate.props.tri.idx.v2` présent, comptages inchangés.

### 6. RÉSERVE MAJEURE — adjacence de 15 nœuds sur runtime-dev (voir §Réserves)
Avant redémarrage, exécuter la sonde :
```
$J5 query <runtime-dev> 'MATCH (a)-[r]->(b) RETURN count(*)'   # compteur
$J5 query <runtime-dev> 'MATCH (a)-[r]->(b) RETURN count(a)'   # traversée
```
Si la traversée EXCÈDE le compteur (sur la copie chaude : 20,6M contre 1,29M,
15 nœuds sources émettant chacun ~la totalité de la zone), le store vivant
porte le même artefact d'adjacence — appliquer la procédure de la réserve R1
avant d'ouvrir le service. Sur un arrêt propre + copie à froid, l'artefact
peut ne pas exister : la sonde TRANCHE.

### 7. Échange des binaires
```
cp ~/.local/share/obrain-harness/bin/obrain ~/sauvegardes-bascule-v3/obrain.v1-avant-bascule
cp ~/projects/banc-obrain-j5/target/release/obrain ~/.local/share/obrain-harness/bin/obrain
# unité viz : ExecStart → le même binaire (ou chemin j5)
# gateway : ExecStart → ~/sauvegardes-bascule-v3/this-rs-gateway.v3-j4
```

### 8. Unité gateway : LimitNOFILE
Dans l'unité systemd de la gateway : `LimitNOFILE=65536`, puis
`systemctl --user daemon-reload`.

### 9. Redémarrages (ordre inverse de l'arrêt)
sentinelle → viz :8432 → gateway :8144 → dsh-dev-3083 (writer en dernier :
il ne doit écrire que dans un monde déjà servi et vérifié).

### 10. Batterie post-bascule (le contrat C1/C2 rejoué sur la prod)
```
SC=~/projects/banc-obrain-j5/scripts/contrat-j5-consommateurs.sh
# comptages sur chaque store migré + panel HTTP sur le viz :8432
# preuves slots (routes indexées : 0 slot) :
bash $SC slots $J5 <store> "MATCH (a)-[]->(b {qualified_name: '<ancre existante>'}) RETURN count(a)" 100
bash $SC slots $J5 <store> "MATCH (n) WHERE n.qualified_name CONTAINS 'session-' RETURN count(n)" 1000
# bridge : initialize/tools/list + un query + une écriture import_events + relecture
# gateway : GET anonyme tenant public → HTTP 200, comptages cohérents
# ergaster : CREATE Event + MATCH ancre (motifs du protocole)
```

## Rollback complet
1. Arrêt de tous les services (même liste qu'au §1).
2. Binaires : restaurer `obrain.v1-avant-bascule` → `~/.local/share/obrain-harness/bin/obrain` ;
   gateway → `this-rs-gateway.v1-avant-bascule` ; unité viz → binaire v1.
3. Stores : les stores MIGRÉS ne redeviennent JAMAIS v1 — restaurer les
   copies `.bak-avant-v3-*` du §2 (rename atomique dir par dir).
4. Redémarrages §9 ; les écritures faites entre bascule et rollback sont
   PERDUES (fenêtre courte, writers arrêtés en premier — perte bornée à zéro
   si le rollback précède la réouverture du writer).

## Résultats du contrat rejoué sur copies (référence pour le §10)

- Panel viz v1/v3 (15 requêtes, deux serveurs simultanés 8460/8461) :
  IDENTIQUES sur toutes les familles de données — sortant, entrant (3 lignes),
  entrant volumineux (384), sortant volumineux (52), filtre eq, CONTAINS
  (274 401), ORDER BY, agrégats, label Function ; DIFFÈRENT sur les counts
  par compteurs (total, arêtes, 4 labels de session) — écarts intégralement
  imputés aux compteurs v1 rassis (§4) et prouvés par énumération nominale.
- Slots (copie dev migrée+indexée) : témoin balayage 424 320 ; voisinage
  entrant par qualified_name : 0 slot ; filtre eq : 0 slot ; CONTAINS sur
  propriété au catalogue (qualified_name) : 0 slot. CONTAINS sur une
  propriété d'index TEXT (Message.text) balaie : c'est la route BM25
  (search_graph), pas la route trigrammes — attendu.
- Chronos serveur (information) : entrant volumineux 0,23 ms (v3) contre
  1 824 ms (v1) sur la même copie ; en prod le bridge du 13/09 mesurait
  obrain_voisinage à 4 160 ms.
- Bridge : initialize/tools/list (35 outils), query lecture, CREATE via
  query (RW), SET + relecture, kill -9 réel → les écritures survivent
  (316512/316513), high-water survit (nœud suivant : 316514),
  search_graph et sessions_list opérationnels. Nœuds d'essai laissés dans
  la copie prod du banc, étiquetés EssaiJ5.
- Ergaster v3 (copie chantiers migrée) : CREATE Event, MATCH ancre,
  SET + relecture — motifs du protocole conformes.
- Passerelle v3 (copie dev migrée, :8462) : GET anonyme du relief tenant
  public → 200, nodes = 424 305, edges = 1 288 455 ; /stores sans jeton
  → 401 ; POST anonyme → 401.
- BUG-PROPS-TRUNCATE : test Rust commité VERT (blobcol + inline, 3 cycles) ;
  sur copie réelle 12G : 3 cycles open/close, zones props/blobcol figées à
  l'octet, props relues identiques.

## Addendum NATIF-J5B (correctifs planificateur, 14/09 après-midi)

Deux formes relevées par l'orchestrateur sur le banc, traitées contrat
d'abord (commit 965ef203), correctif ensuite (8105b739). Le binaire livré
`target/release/obrain` INCLUT ces correctifs.

1. **CONTAINS basse sélectivité — CORRIGÉ.** Garde-fou de coût sur la
   route trigrammes : comptes de postings estimés AVANT le choix de route
   (O(log n)), et re-garde sur le total des candidats avant la
   revérification ; au-delà du seuil (`OBRAIN_CONTAINS_GARDE`, défaut
   max(4096, filigrane/8)) la route se désiste et le balayage existant
   reprend. Mesuré : `session-` (274 401/424 305) passait par l'index en
   1 292 ms serveur ; après correctif ~180 ms (balayage), ratio CLI total
   1,32× v1 ; le CONTAINS RARE reste 0-slot ; contains_indexe 8/8 inchangé.
2. **ORDER BY par label — ANALYSE consignée, parité mesurée.** Il n'existe
   d'index par label dans AUCUNE lignée : `nodes_by_label` est un balayage
   de zone au bitset, IDENTIQUE en v1 — parité serveur chaud mesurée
   (scan-sans-props : v1 3,4-4,3 ms, v3 3,6-4,1 ms) ; parité CLI orderby
   1,00×. Les 21,7 ms v1 / 679 ms v3 de la contre-épreuve initiale
   étaient le cache de réponses du viz v1 contre un premier hit froid v3.

### R3 — surcoût résiduel de l'étage LECTURE DE PROPRIÉTÉS (~2× serveur)
Sur les requêtes qui hydratent une propriété PAR nœud en masse (orderby :
39 ms v3 / 23 ms v1 serveur chaud ; contains-balayage : 180 ms / 86 ms),
le v3 paie ~0,7 µs de plus par lecture. Le chemin batch est déjà propre
(portée tenant hissée, un verrou par lot de 16) ; la cause fine n'est pas
tranchée (candidats : stride 40 o + pression de cache, prédicat
`est_visible_pour` par record, vintage du binaire v1 déployé). AUCUNE
route indexée n'est affectée (0 slot partout). Chantier de profilage à
part entière ; les bornes contractuelles tenues : scan ≤ 1,5×,
props-lourds ≤ 2,5× (clause `parite` de la batterie).

## Réserves et consommateurs NON couverts

### R1 — adjacence pathologique de 15 nœuds (runtime-dev)
15 nœuds anciens à données réelles (SelfState v312-314 ids 175745-747,
Messages/divers ids ~317-326k : 175745 175746 175747 317553 318414 319094
325167 325447 325819 326114 326140 326241 326349 326438 326679) émettent
chacun ~1,29M de lignes en expansion non typée v3 (20 601 954 lignes totales
contre 1 288 455 arêtes au compteur). MÉCANISME ÉTABLI : chaîne d'arêtes
CYCLIQUE — le nœud 175745 émet 1 288 464 fois LA MÊME cible (401737) plus
ses 2 vraies (415800, 416102). Le moteur v3 est sain sur stores propres
(synthétique 2=2 ; chantiers migré 23=23 ; melange-v3 2=2) ; le même store
lu par v1 rend 0 sortant sur ces nœuds (traversée v1 bornée par des degrés
rassis — v1 est AVEUGLE, pas correct : il manque aussi ~6 800 arêtes à sa
traversée globale, 1 281 563 contre 1 288 455). Antérieur à la bascule
(données anciennes ; l'incident dedupe du 08/09 est le suspect).
REMÈDES ESSAYÉS SUR COPIE, SANS EFFET : `repair-degrees` (reconstruit les
degrés, l'anomalie persiste) ; `compact` (ne fait qu'un checkpoint WAL).
À préparer AVANT la fenêtre : excision chirurgicale des chaînes des 15 nœuds
ou reconstruction de la zone edges ; la sonde du §6 tranche la présence sur
le vivant. Les requêtes TYPÉES ne sont PAS affectées (CALLS : 618 776 des
deux côtés) ; le panel C1 complet (voisinages par ancre, eq, CONTAINS,
ORDER BY, agrégats) est identique v1/v3 — seule l'expansion non typée
GLOBALE traverse les chaînes malades.

### R2 — v1 count()/traversées non fiables sur runtime-dev
Le binaire v1 déployé rend des counts rassis ET des traversées sortantes
vides (dsh-runtime : 0 sortant en v1, 2 en v3) sur ce store. Toute
« vérification » post-bascule qui comparerait au v1 d'avant DOIT utiliser
les énumérations (id(n)), pas les counts v1.

### Consommateurs non couverts (à la charge de l'orchestrateur)
- les 3 viz dev de wami : :8433, :8435, :8448 (arbre vivant ~/projects/banc-obrain
  — interdits au chantier J5) : même échange de binaire que le viz prod,
  COORDONNÉ avec wami (ses stores dev devront être migrés en même temps) ;
- `tmn` : non trouvé/non exercé pendant la répétition ;
- les 3 bridges MCP du chat : le BINAIRE est couvert (obrain bridge j5,
  contrat C3), mais la CONFIGURATION de chaque bridge (chemins de store dans
  la conf MCP du chat) est à basculer à la main ;
- outils moteur de chat (obrain_mondes/voisinage/recherche via engine) :
  exercés au niveau des équivalents bridge (query/search_graph) — la boucle
  chat complète (LLM) n'a pas été exercée.

## POST-MORTEM bascule prod (14/09 ~12:28-12:35, exécutée par l'orchestrateur)

15 stores migrés v3 en < 11 s cumulées (runtime-dev 12G : 10,6 s ; le reste
< 1,6 s), sauvegardes .bak-avant-v3 validées AVANT, idempotence et fail-closed
confirmés en prod. Résultats sur store vivant servi par le viz :8432 :
voisinage entrant 4160 ms → **74 ms**, CONTAINS large **237 ms** (garde-fou
J5B actif), ORDER BY 36 ms, écriture v3 OK (count monte via le graph-writer).

### PIÈGE dist du viz (30 min de 404 sur /app) — À CONNAÎTRE
`viz_serve.rs::app_dir()` résout le dashboard via
`env!("CARGO_MANIFEST_DIR").join("../../packages/obrain-dashboard/dist")` —
chemin figé À LA COMPILATION, ni le CWD ni current_exe. Le binaire j5 ayant
été bâti dans `~/projects/banc-obrain-j5/crates/obrain-cli`, il sert le dist
depuis `~/projects/banc-obrain-j5/packages/obrain-dashboard/dist`. Après
échange du binaire du viz, il FAUT que le dist bâti (pnpm build, 23 assets)
soit présent à CE chemin (le worktree de build du binaire déployé). L'API
(`/api/stores`, `/api/query`) marchait déjà ; seul le statique `/app` était
404. Remède appliqué : copie du dist bâti à ce chemin. Pour l'avenir : soit
livrer le dist DANS le worktree du binaire, soit ajouter un flag `--ui-dir`.

### Réserve R1 confirmée présente sur le vivant (NON bloquante)
Sonde §6 sur runtime-dev migré : compteur 1 292 944 arêtes vs traversée non
typée 20 673 778 → les 15 nœuds à chaîne cyclique sont bien là (données
anciennes). Requêtes TYPÉES et voisinages ANCRÉS intacts (le voisinage
entrant du chat mesuré à 74 ms passe par une ancre). Excision chirurgicale
des chaînes = chantier de suivi, pas une urgence de service.
