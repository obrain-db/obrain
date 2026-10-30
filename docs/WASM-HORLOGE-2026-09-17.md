# Horloge wasm — le module liait le panic de `std::time`, il ne le lie plus

**Date** : 2026-09-17. **Statut** : **APPLIQUÉ** — migration faite, garde posée,
mesures avant/après relevées ci-dessous.

**Nature** : note de correctif. Le défaut n'était pas visible à la compilation :
le module se construisait (exit 0) et se chargeait, mais portait un trap à
l'exécution.

**Régime des affirmations.** **CONSTATÉ** = lu dans un fichier cité.
**MESURÉ** = relevé par exécution, ou par inspection d'un artefact produit, avec
la commande reproductible. **NON MESURÉ** = ce qui n'a pas été relevé ici.

---

## 1. Le fait

La bibliothèque standard Rust n'implémente **aucune** horloge sur
`wasm32-unknown-unknown` : `Instant::now()` et `SystemTime::now()` y paniquent
(« time not implemented on this platform »), ce qui, en wasm, se manifeste par un
trap `unreachable`.

Or le module livré pour le navigateur **liait ce panic**.

**MESURÉ (avant)**

```bash
strings -a target/wasm32-unknown-unknown/minimal-size/obrain_wasm.wasm \
  | grep -c 'time not implemented'        # -> 1
strings -a target/wasm32-unknown-unknown/minimal-size/obrain_wasm.wasm \
  | grep 'unsupported/time'               # -> library/std/src/sys/pal/wasm/../unsupported/time.rs
```

Le binaire **natif** ne contient pas cette chaîne (`grep -c` → `0`) : le trap
était propre à l'artefact wasm.

## 2. Le mécanisme, prouvé par exécution

**MESURÉ.** Un module wasm minimal (`cdylib`, `crate-type = ["cdylib"]`,
`--target wasm32-unknown-unknown`, `panic = "abort"`) exposant trois fonctions —
une témoin sans horloge, une avec `Instant::now()`, une avec `SystemTime::now()` —
instancié sous Node :

| fonction | résultat |
|---|---|
| `probe_ok()` (témoin, retourne 42) | `42` |
| `probe_instant()` | `RuntimeError: unreachable` |
| `probe_systemtime()` | `RuntimeError: unreachable` |

Le témoin rend sa valeur, les deux fonctions d'horloge trappent : la cause est
l'horloge, pas le module ni l'hôte.

## 3. Pourquoi le lien suffisait à conclure

Un panic est **lié** dès qu'un crate du graphe appelle la fonction, même si le
chemin n'est jamais emprunté par le navigateur. La présence de la chaîne dans le
binaire est donc un détecteur fiable — et c'est ce détecteur qui sert de garde
(§7) : pas besoin d'exécuter le scénario fautif pour savoir que le code fautif
est revenu.

## 4. L'état du dépôt avant ce correctif

**CONSTATÉ.** Le sujet avait déjà été **partiellement** traité, et le travail s'y
était arrêté :

- `crates/obrain-substrate/src/meta.rs` (`now_unix_seconds`) et
  `crates/obrain-substrate/src/checkpoint.rs` (`unix_seconds`, `unix_micros`)
  portaient déjà une branche `#[cfg(target_arch = "wasm32")] { 0 }`, avec le
  raisonnement écrit : ces valeurs alimentent des champs **documentaires**
  (`created_at`, `last_checkpoint`, `timestamp` d'un `WalRecord` dont l'ordre réel
  vient du `lsn`), et zéro est la réponse honnête. **Ces branches sont intactes**
  — ce correctif ne rouvre pas cette décision.
- Partout ailleurs, `std::time` était appelé sans garde : **187 lignes** dans
  **142 fichiers**, réparties ainsi :

| crate | fichiers touchés |
|---|---|
| `obrain-engine` | 57 |
| `obrain-cognitive` | 45 |
| `obrain-substrate` | 21 |
| `obrain-adapters` | 10 |
| `obrain-core` | 4 |
| `obrain-reactive` | 3 |
| `obrain-common` | 2 |

`obrain-cognitive` et `obrain-reactive` déclaraient déjà `web-time` mais ne
l'utilisaient que dans **un** fichier chacune : le reste de leurs appels restait
`std::time`.

## 5. Le correctif

**CONSTATÉ.** Remède déjà retenu par le dépôt — `web-time`, qui est le même
`std::time` sur natif et l'horloge de l'hôte JS (`performance.now()`) sur
wasm32 — appliqué cette fois à **tout** le graphe wasm de `obrain-wasm`
(`obrain-common`, `obrain-core`, `obrain-substrate`, `obrain-engine`,
`obrain-adapters`, `obrain-cognitive`, `obrain-reactive`).

- `web-time = "1"` déclaré dans les 5 crates qui ne l'avaient pas
  (`obrain-common`, `obrain-core`, `obrain-substrate`, `obrain-engine`,
  `obrain-adapters`), avec le commentaire qui dit pourquoi.
- 187 lignes remplacées (239 insérées) : imports `use std::time::Instant` →
  `use web_time::Instant`, et chemins qualifiés afférents ; `Duration` **non**
  touché (il ne dépend pas d'une horloge et fonctionne sur wasm).
- Sur natif, `web_time` ré-exporte `std::time::*` : le type et le comportement
  sont **identiques**, le changement est un no-op à l'exécution native.

### 5bis. La régression que le profil `cognitive` a révélée

**CONSTATÉ.** `Engram` (`crates/obrain-cognitive/src/engram/types.rs`) dérive
`Serialize`/`Deserialize` sur des champs `created_at` / `last_activated` de type
`SystemTime`. Or `web_time::SystemTime`, sur wasm, n'implémente **pas** ces
traits : la migration ci-dessus cassait donc la compilation de la cible wasm dès
que la feature `cognitive` était activée. Sur natif, aucune alerte — `web-time`
y **est** `std::time`, qui a les impls serde.

Correctif : activer la feature `serde` de `web-time`
(`crates/obrain-cognitive/Cargo.toml`). Elle est **sûre** parce que l'encodage de
`web-time` est copié de celui de serde pour `std::time::SystemTime` — mêmes
champs `secs_since_epoch` / `nanos_since_epoch`, même nom de structure (vérifié
dans `web-time-1.1.0/src/time/serde.rs`, dont l'en-tête revendique cette
compatibilité). Les données persistées restent donc lisibles des deux côtés.

**Leçon de méthode, parce qu'elle a failli me tromper :** ce défaut s'est
d'abord caché derrière un `exit 0`. La commande de contrôle était
`cargo build … | tail -12`, et **sans `pipefail` le code de sortie est celui de
`tail`**, pas celui de `cargo` : un échec de compilation rapportait 0. Toutes
les mesures de ce document ont été reprises avec `set -o pipefail` (ou
`PIPESTATUS`). Un `exit 0` derrière un tube ne prouve rien.

## 6. Mesure après le correctif

**MESURÉ.** Tous les codes de sortie ci-dessous sont ceux de `cargo`.

| contrôle | avant | après |
|---|---|---|
| `grep -c 'time not implemented'` dans `obrain_wasm.wasm` | 1 | **0** |
| `grep 'unsupported/time'` dans `obrain_wasm.wasm` | présent | **absent** |
| taille du module (profil `minimal-size`, défaut `browser`) | 2 986 402 o | 2 991 939 o |

Note sur la taille : deux builds successifs du **même** profil ont mesuré
2 990 320 o puis 2 991 939 o (~1,6 Ko d'écart). L'ordre de grandeur utile est
donc « +≈5 Ko », pas le chiffre à l'octet près.

| profil wasm | exit `cargo` | taille | panic d'horloge |
|---|---|---|---|
| défaut (`browser`) | 0 | 2 991 939 o | 0 |
| `--features full` | 0 | 3 952 237 o | 0 |
| `--features cognitive` | 0 | 3 085 050 o | 0 |

L'horloge vient désormais de l'hôte : le module **importe**
`__wbg_performance_*` et `__wbg_getTime*`, et `web-time v1.1.0` apparaît dans
l'arbre de dépendances cible wasm.

Autres contrôles :

- `cargo check --workspace --all-targets` → **exit 0**, 0 erreur.
- Tests unitaires natifs (`--lib`), **0 échec** :

| crates | résultat |
|---|---|
| `obrain-common` + `obrain-substrate` | **888 passés**, 2 ignorés |
| `obrain-core` | **285 passés** |
| `obrain-engine` | **871 passés** |
| `obrain-adapters` | **707 passés** |
| `obrain-cognitive` | **38 passés**, 1 ignoré |

…soit **2 789 tests passés** sans échec. C'est la garantie que la substitution
n'a rien changé sur natif (`web_time` y **est** `std::time`).

## 6bis. L'exécution réelle — ce que la mesure a révélé, et qui change la conclusion

**MESURÉ, sur l'artefact livré.** La glue de `crates/bindings/wasm/pkg/` (build
du 2026-09-03, celle que consomme `@obrain-db/wasm`) est exécutable sans
`wasm-bindgen` : je l'ai chargée dans Node et appelée. Résultat :

```
1. init() ................ OK
panicked at library/std/src/sys/pal/wasm/../unsupported/time.rs:31:9:
time not implemented on this platform
2. new Database() ........ ECHEC: RuntimeError - unreachable
```

Le défaut est donc **réel et total** : dans le navigateur, le tout premier
constructeur trapait. Il n'y avait pas de dégradation partielle à découvrir —
le module était inutilisable.

**MESURÉ, après ce correctif.** Le même essai, sur le module reconstruit et
passé au `wasm-bindgen` **0.2.128** (celui du crate ; voir §8 pour le CLI) :

```
1. init() ................ OK
panicked at library/std/src/sys/pal/wasm/../unsupported/os.rs:60:5:
no filesystem on this platform
2. new Database() ........ ECHEC: RuntimeError - unreachable
```

Le panic d'horloge a **disparu** : le trap a *avancé* jusqu'au blocage suivant.
C'est la preuve d'exécution que ce correctif fait ce qu'il annonce — et, en même
temps, la démonstration que **le module ne fonctionne toujours pas dans un
navigateur**, pour une raison **indépendante et préexistante** :

- `ObrainDB::with_config` (`crates/obrain-engine/src/database/mod.rs:944`), quand
  `config.path` est `None` — c'est `Config::in_memory()` —, appelle
  `SubstrateStore::open_tempfile()` ;
- `crates/obrain-substrate/src/store.rs:4877` → `SubstrateFile::open_tempfile()` ;
- `crates/obrain-substrate/src/file.rs:1207` → `std::env::temp_dir()`, qui
  panique sur `wasm32-unknown-unknown` (« no filesystem on this platform »).

Autrement dit : le « mode mémoire » d'Obrain n'est pas un substrat en RAM, c'est
**un substrat fichier dans un répertoire temporaire** (la doc du code le dit :
« the canonical post-T17 replacement for `LpgStore::new()` in unit tests and
doctests »). Or `wasm32-unknown-unknown` n'a pas de système de fichiers. Faire
fonctionner `Database::new()` dans le navigateur demande donc un **vrai substrat
en mémoire** — une fonctionnalité, pas une correction. Ce n'est pas fait ici, et
ce n'est pas de mon fait : le blocage d'horloge le masquait.

**Pourquoi personne ne l'a vu :** la suite de tests wasm du binding —
`crates/bindings/wasm/tests/web.rs`, **22 tests** — **ne compile même pas**.

```
error[E0277]: `JsError` doesn't implement `std::fmt::Display`   (×3)
error[E0599]: no method named `export_snapshot` found for struct `Database`   (×2)
error[E0599]: no function or associated item named `import_snapshot`   (×2)
```

Ces deux méthodes n'existent plus (le binding expose `import_lpg`, `import_rdf`,
`import_rows`) : la suite est restée sur une API antérieure. Elle est donc
**morte** — c'est précisément la garde qui aurait attrapé le défaut d'horloge…
et le blocage de système de fichiers. Sa réparation n'est pas faite ici : elle
demande un arbitrage (exposer un `export_snapshot`/`import_snapshot`, ou réécrire
les deux tests concernés), que ce chantier n'a pas à trancher seul.

## 6ter. La suite de tests wasm est réparée — elle tourne

**CONSTATÉ (avant).** `cargo test -p obrain-wasm --target wasm32-unknown-unknown
--test web` échouait à la **compilation**, 7 erreurs, toutes dans
`tests/web.rs` :

| erreur | sites | cause |
|---|---|---|
| `JsError doesn't implement Display` | 3 | `format!("{err}")` |
| `no method named export_snapshot` | 2 | API **retirée** |
| `no function or associated item import_snapshot` | 2 | API **retirée** |

Les deux derniers ne sont pas une négligence : `git log -S export_snapshot`
désigne le commit **`b0f5bd11`**, « T17 Wave A: retire legacy single-file .obrain
format (v1/v2) », qui supprime la surface bincode de snapshot en même temps que
les formats legacy, avec ce message dans le moteur : *« A substrate-native
snapshot API lands as T17b »*. Les tests étaient donc restés sur une API
volontairement disparue.

**CE QUI A ÉTÉ FAIT.**

- Les 3 sites `Display` lisent désormais le message là où il est : la propriété
  `message` de l'objet JS (`js_error_message`, via `JsError → JsValue` puis
  `Reflect::get`). Ni `Display` (absent) ni `Debug` (qui passe par `JsValue`)
  ne donnaient le texte attendu par les assertions.
- Les deux tests de snapshot ont été réécrits contre la surface **réelle** —
  relecture par requête, reconstruction d'une charge LPG, réimport dans une base
  neuve — au lieu de ressusciter une API retirée. Le test avec arête couvre le
  cas que l'autre ne couvre pas. Un commentaire renvoie à `b0f5bd11` et à T17b :
  le jour où un snapshot natif arrive, c'est ce test qu'il faudra renforcer.
- Un avertissement **préexistant** a été corrigé au passage :
  `use web_time::Instant` dans `crates/obrain-engine/src/query/cache.rs` était
  inutilisé sur wasm, ses trois usages étant déjà gatés
  `#[cfg(not(target_arch = "wasm32"))]` (l'ancien `std::time::Instant` produisait
  le même avertissement).

**MESURÉ (après).** La suite **compile** (exit 0) et **tourne** :

```
running 22 tests
test result: FAILED. 1 passed; 21 failed; 0 ignored; 0 filtered out; finished in 0.02s
```

`test_version` passe (il ne construit pas de base). Les **21 échecs ont une cause
UNIQUE**, comptée dans la sortie :

```
21 × panicked at library/std/src/sys/pal/wasm/../unsupported/os.rs:60:5
21 × no filesystem on this platform
0 × time not implemented on this platform
```

Le second chiffre est la preuve que le correctif d'horloge tient : **aucun** test
ne panique plus sur le temps. Et la suite, morte depuis la suppression de
l'API snapshot, est maintenant un **instrument de mesure** : 21 → 0 est
exactement l'objectif du substrat en mémoire (§6bis).

## 6quater. Le substrat en mémoire — mesuré, et le module fonctionne

**CE QUI A ÉTÉ FAIT** (décision et alternatives : voir
`docs/ADR-2026-09-17-SUBSTRAT-EN-MEMOIRE.md`). Le mode « en mémoire » était un
substrat FICHIER dans un répertoire temporaire ; il devient un vrai substrat en
RAM, sans aucune E/S :

| fichier | changement |
|---|---|
| `crates/obrain-substrate/src/file.rs` | `ZoneFile` a un support à deux modes (`Mmap` \| `Ram`), API publique **inchangée** ; `RamZone` + `RamDir` (registre partagé par nom) ; `SubstrateFile::open_memory()` |
| `crates/obrain-substrate/src/wal_io.rs` | `WalWriter` écrit dans un fichier **ou** dans un tampon (`WalMedium`), `open_memory()` |
| `crates/obrain-substrate/src/writer.rs` | `Writer::new` choisit le WAL selon `is_memory()` |
| `crates/obrain-substrate/src/store.rs` | `SubstrateStore::open_memory()` |
| `crates/obrain-engine/src/database/mod.rs` | `config.path == None` → substrat mémoire **sur wasm32 seulement** ; le natif garde `open_tempfile()` |

**MESURÉ.**

| contrôle | avant | après |
|---|---|---|
| suite wasm du binding (`--test web`, runner 0.2.128) | 1 passé / **21 échoués** | **22 passés / 0 échoué** |
| tests natifs du mode mémoire (`tests_memoire`, 6 tests) | — | **6 passés / 0 échoué** |
| `cargo test --workspace` (natif) | 8 844 passés | voir §6quinquies |

| contrôle | résultat |
|---|---|
| `scripts/verify-wasm.sh` (construit + garde + exécute) | **exit 0 — « Suite wasm verte »** |

Et l'exécution dans un hôte JS réel — la preuve que l'objectif demandait,
faite cette fois sur **l'artefact LIVRÉ** `crates/bindings/wasm/pkg/` (régénéré,
voir plus bas), avec la forme d'appel d'un vrai appelant JS :

```
1. init() ................ OK
2. new Database() ........ OK
3. INSERT ................ OK
4. MATCH ................. [{"p.name":"Alix","p.age":30}]
   nodeCount: 1  edgeCount: 0
5. importLpg({nodes:[...]}) OK, nodeCount: 2
```

Les deux poignées partagent bien les octets d'une même zone (test natif dédié :
`deux_poignees_de_la_meme_zone_partagent_les_octets`) — sans le registre de
`RamDir`, la seconde lirait des zéros, ce qui serait une corruption silencieuse
et non une erreur.

## 6quinquies. Ce qui reste vrai malgré tout

**NON MESURÉ / À SAVOIR**, pour ne pas transformer ce succès en promesse :

- **Le mode mémoire ne persiste RIEN.** C'est sa définition. Le binding navigateur
  gagne une base éphémère, pas un stockage, et **aucun snapshot natif substrat
  n'existe** (chantier T17b) : un graphe RAM ne peut pas être sauvegardé
  aujourd'hui.
- **Le mode mémoire n'est PAS checkpointable** — et la raison est MESURÉE, pas
  supposée. `SubstrateStore::checkpoint()` échoue d'abord dans son `flush()`, qui
  persiste dictionnaire, propriétés et arêtes externes dans un répertoire
  inexistant. Les SAUTER a été essayé, puis **reverté**, parce que le flush
  aboutit alors, laissant la surveillance du WAL déclencher un vrai checkpoint,
  et la ré-ancrage des index qui le suit nomme ses fichiers temporaires avec
  `std::process::id()` (`props_index.rs:1009`, `props_eq_index.rs:668`,
  `props_ord_index.rs:406`) — qui **panique** sur wasm32 (« no pids on this
  platform »). Mesuré : la suite wasm est retombée de **22/22 à 1/22** lors de cet
  essai, d'où le retour en arrière. Le test
  `le_mode_memoire_ne_sait_pas_encore_checkpointer` fige cet état pour que
  quiconque le rendra possible doive passer par là.
- Les colonnes blob (`blob_column.rs:647-648`) écrivent toujours sur `path()` et
  **n'ont pas été exercées** en mode RAM : elles échoueraient proprement.
- Le mode mémoire **ne persiste toujours rien** : même un checkpoint réussi n'y
  sauvegarderait rien (voir T17b ci-dessus).
- **L'artefact livré a été régénéré** (`scripts/build-wasm.sh`, `pkg/` daté du
  2026-09-17, garde horloge franchie, `init()`/`new Database()`/`insert`/`match`/
  `importLpg` vérifiés dessus). Il remplace le build du 2026-09-03 qui trappait.
- **Observation de taille, non expliquée :** le `pkg/` régénéré pèse 863 Ko gzip
  là où `docs/api/wasm/index.md` annonce « ~660 KB gzipped », et le script
  signale « Exceeds 400 KB gzip target ». Une partie de l'écart est
  ANTÉRIEURE à ce chantier (le `pkg/` précédent datait du 2026-09-03, trois
  semaines de dépôt plus tôt) et je ne l'ai pas mesurée par différence : la
  contribution propre du substrat en mémoire n'est **pas** isolée.
- Le module n'a pas été exécuté dans un **navigateur** (Node oui).

## 6sexies. Vérification native — et un avertissement de méthode

**MESURÉ (ciblé).** Les crates touchés passent, exécutés seuls :

| commande | résultat |
|---|---|
| `cargo test -p obrain-substrate --lib` | **896 passés** (888 + les 8 du mode mémoire et des tests voisins), 0 échec |
| `cargo test -p obrain-engine --test unicite_merge` (×3) | **4 passés**, 0 échec |
| `cargo check --workspace --all-targets` | exit 0, 0 erreur |

**MESURÉ (et gêné).** Deux exécutions complètes de `cargo test --workspace` ont
échoué, mais **pas pour une raison imputable à ce chantier** : un **autre agent
exécutait `cargo test --workspace --no-fail-fast` au même moment, dans le MÊME
dépôt et le MÊME `target/`** (processus observé : `cargo test --workspace
--no-fail-fast`, journal `/tmp/nr-final-1.log`, pendant que le mien tournait).
Symptômes observés, tous compatibles avec cette concurrence :

- un binaire de test **disparu entre la compilation et l'exécution**
  (« could not execute process … (never executed) : No such file or directory ») —
  l'autre processus reconstruisait le même répertoire ;
- `unicite_merge::merge_concurrent_ne_cree_jamais_de_doublon` en échec, un test à
  **16 fils** qui passe 3 fois sur 3 en isolation et dont l'assertion est un
  « zéro conflit attendu » — donc sensible à la charge machine ;
- et, plus tôt, la disparition de `target/wasm32-unknown-unknown/`.

**Conséquence honnête :** ces deux exécutions-là ne prouvaient rien. La
re-vérification a donc été faite **à `target/` libre**, après la fin du
processus concurrent :

| commande | résultat |
|---|---|
| `cargo test --workspace` (target libre, sans autre cargo) | **exit 0 — 8 853 passés, 0 échec**, 77 ignorés |

**8 853 > 8 844** : le chantier ajoute des tests, n'en casse aucun. La
non-régression native est **établie**. La leçon de méthode reste, elle : un
`cargo test` concurrent dans le même `target/` produit des échecs qui
ressemblent à des régressions (binaire disparu, test de concurrence à 16 fils
qui tombe) et qui n'en sont pas. Vérifier `ps` avant de conclure.

## 6septies. Validation finale — récapitulatif du 2026-09-17

**MESURÉ**, dans un seul passage, charge machine 11,98 (les chiffres de lecture du
banc §5ter de l'ADR sont donc à relire à charge basse ; ceux-ci ne sont pas des
mesures de performance, seulement des verdicts) :

| contrôle | commande | résultat |
|---|---|---|
| suite wasm (construction + garde + exécution) | `bash scripts/verify-wasm.sh` | **exit 0 — 22 passés, 0 échoué** |
| suite native complète | `cargo test --workspace` | **exit 0 — 8 854 passés, 0 échoué**, 77 ignorés |
| garde horloge sur l'artefact **livré** | `strings … pkg/obrain_wasm_bg.wasm \| grep -c 'time not implemented'` | **0** |
| exécution bout en bout sur le **livrable** | Node : `init()` → `new Database()` → `INSERT` → `MATCH` → `importLpg` | **les 5 étapes OK** |

Le chantier est complet au sens de l'objectif : substrat en mémoire livré et
prouvé par exécution, suite wasm réparée et verte (elle était morte), natif sans
régression (8 854 > 8 844 d'avant le chantier), décision et mesures consignées.

**Ajout du même jour :** un banc compare les deux supports
(`crates/obrain-substrate/benches/memoire_vs_fichier.rs`, contrôle à blanc
inclus) — ouverture **128× plus rapide** et création de nœuds **7,7×** en
mémoire, lectures **identiques** (±1 %). Détail, et la correction d'un premier
relevé erroné, dans `crates/obrain-substrate/benches/BANCS.md`.

## 7. La garde
**CONSTATÉ.** `scripts/build-wasm.sh` refuse désormais de produire un module qui
porte le panic (étape « 1bis », avant `wasm-bindgen`) : il teste la chaîne et
sort en erreur avec le remède. `scripts/build-wasm-all.sh` en hérite, puisqu'il
appelle ce script.

**MESURÉ.** La garde a été éprouvée dans les deux sens, sur deux artefacts :

| artefact | panic présent | garde | exit |
|---|---|---|---|
| module témoin (`Instant::now()`, cf. §2) | oui | **déclenchée** (« Trace : …unsupported/time.rs ») | 1 |
| `obrain_wasm.wasm` après correctif | non | **muette** | 0 |

Une garde qui n'a jamais été vue déclencher ne prouve rien : celle-ci l'a été.

**Portée de la garde, pour ne pas lui prêter plus qu'elle ne fait :** elle
constate l'**absence du chemin d'horloge std**, rien d'autre. Elle ne dit pas que
le module est utilisable — il ne l'est pas encore (§6bis), et une chaîne absente
n'a jamais prouvé qu'un programme marche.

## 8. Ce qui n'est pas mesuré

**NON MESURÉ**, et il faut le dire :

- **Le module n'a pas été exécuté dans un *navigateur*.** Il l'a été dans
  **Node** (§6bis), via la glue `--target web` du livrable : c'est un hôte JS
  réel, mais pas un navigateur. Le trap est donc constaté à l'exécution, sur
  l'artefact réel — pas sur un module jouet.
- **La suite `wasm-bindgen-test` du binding n'a pas été exécutée** : elle ne
  compile pas (§6bis). Le harnais est pourtant disponible — voir la note
  d'outillage ci-dessous.
- Les deux branches natives de `meta.rs`/`checkpoint.rs` renvoient toujours `0`
  sur wasm : décision antérieure, non rouverte ici. Une horloge réelle y serait
  disponible (`Date.now()`) si un jour un lecteur dépend de ces champs.

**Note d'outillage (CONSTATÉ), parce qu'elle coûte cher à retrouver :** exécuter
un module Obrain wasm exige un `wasm-bindgen` de **la version exacte du crate**,
soit **0.2.128** ici. Le CLI présent dans `PATH` (`~/.cargo/bin/wasm-bindgen`) est
en **0.2.114** : il refuse le module neuf. La version qui va bien est **en
cache**, avec son test-runner :

```
~/.cache/.wasm-pack/wasm-bindgen-4f04094f43ba0220/wasm-bindgen              # 0.2.128
~/.cache/.wasm-pack/wasm-bindgen-4f04094f43ba0220/wasm-bindgen-test-runner  # 0.2.128
```

Les commandes qui ont servi à §6bis :

```bash
export PATH="$HOME/.cache/.wasm-pack/wasm-bindgen-4f04094f43ba0220:$PATH"
cargo build --target wasm32-unknown-unknown --profile minimal-size -p obrain-wasm
wasm-bindgen --target web --out-dir /tmp/pkg-fix \
  target/wasm32-unknown-unknown/minimal-size/obrain_wasm.wasm
node /tmp/test-pkg-fix.mjs        # init() puis new Database()
```

**CONSTATÉ aussi :** le répertoire `target/wasm32-unknown-unknown/` a été
supprimé pendant ce chantier (~16:43), sans que rien ne l'exige (217 Go libres) —
probablement par un nettoyeur de la machine, la cible native étant intacte. Les
artefacts wasm sont donc à rebâtir, ce qui est immédiat.

## 9. Périmètre — ce que ce chantier ne concerne pas

**CONSTATÉ.** Il n'existe **pas** de binaire `obrain` wasm : le livrable de la
cible `wasm32-unknown-unknown` est la **bibliothèque de liaison**
`crates/bindings/wasm` (paquet `obrain-wasm`, publié en `@obrain-db/wasm`), qui
embarque le moteur en mémoire. Le CLI `obrain`, lui, est natif (`obrain version`
→ `rustc 1.91.1`, `target x86_64`). Parler d'« exécuter le binaire obrain sous
wasm32 » mélange donc deux objets : le moteur *en tant que bibliothèque* tourne
dans le navigateur, le binaire *en tant que programme* non.
