# obrain-web — paquet navigateur (essai)

Base obrain dans le navigateur, via `wasm-bindgen`. Deux modes :

- **en mémoire** : `new Base()`, disparaît au rechargement de la page ;
- **persistant** : `await Base.ouvrir(nom)`, adossé à un journal OPFS, avec `persister()`.

## Construire

```sh
cargo build --target wasm32-unknown-unknown -p obrain-web --release --features complet
wasm-bindgen --target web    --out-dir pkg-web  target/wasm32-unknown-unknown/release/obrain_web.wasm
wasm-bindgen --target nodejs --out-dir pkg      target/wasm32-unknown-unknown/release/obrain_web.wasm
```

Versions : `wasm-bindgen-cli` **0.2.130**, identique à la version verrouillée dans `Cargo.lock`.

## Utiliser

En mémoire :

```js
import init, { Base } from "./obrain_web.js";
await init();
const b = new Base();
b.executer("INSERT (:Personne {nom: 'Alix'})");
const r = JSON.parse(b.executer("MATCH (p:Personne) RETURN p.nom AS nom"));
// r = { colonnes: ["nom"], lignes: [["Alix"]] }
```

Persistant. `Base.ouvrir` doit être appelé dans un **worker dédié** : OPFS n'expose son accès
synchrone que là. Hors worker, la Promise est rejetée par une erreur lisible. Vérifié dans node
(`tests/js/suite.mjs`, « persistance : ouvrir hors worker dédié rejette… ») ; sur la page principale,
ce rejet n'est pas couvert par la batterie automatique.

```js
// worker.js
import init, { Base } from "./obrain_web.js";
await init();
const b = await Base.ouvrir("ma-base.journal");
b.creer_noeud(["Personne"], JSON.stringify({ nom: "Alix" }));
b.persister(); // publie l'instantané : ce qui précède est durable
```

Une requête invalide lève une erreur JavaScript avec le message du moteur. Aucune méthode ne
panique : une panique wasm abattrait toute l'instance.

## Ce qui est persisté, et ce qui ne l'est pas

**Décision C (v0.0.1) : pas de WAL sur OPFS.** La durabilité de ce paquet est celle de
l'instantané publié par `persister()`. Un journal écrit à chaque mutation n'existe pas en
v0.0.1 : il est reporté, et la limite est listée dans `docs/LIMITES-v0.0.1.md`.

**Persisté : la dernière publication réussie.** `persister()` écrit l'état complet de la base
(nœuds, arêtes, propriétés, dictionnaires) dans le journal OPFS, sous la forme de deux
emplacements logiques A/B portant un numéro de génération et un CRC. Quand `persister()` rend
`Ok`, cet état survit à la fermeture et au rechargement. Une publication coupée en cours ouvre
la publication précédente, ou une base vide si aucune n'a jamais abouti : jamais une erreur
d'ouverture, jamais un mélange des deux. Verrou : `obrain-substrate`,
`miroir::tests_journal::un_persist_coupe_ouvre_une_generation_complete` (coupures réparties sur
toute la publication, sur le format de journal réel).

**Non promis : les écritures faites depuis la dernière publication.** Ce n'est pas un journal.
Une écriture non suivie d'un `persister()` n'est pas promise : elle n'atteint pas OPFS avant la
publication suivante, et elle est perdue à la fermeture, au rechargement ou au crash. Pour rendre
une série d'écritures durable, appelez `persister()` après elle. Verrou :
`tests/navigateur.rs::les_ecritures_apres_la_derniere_publication_sont_perdues`.

**Coût et plafond.** Chaque publication ajoute au journal **deux** images complètes de la base
(`obrain-substrate`, `miroir::tests_journal::chaque_publication_ajoute_deux_images_au_journal`),
et le journal n'est jamais compacté. Le plafond de 2 Gio porte sur la taille totale du journal,
pas sur la taille de la base : une base dont l'image pèse S octets ne peut être publiée qu'environ
1 Gio / S fois. Au-delà, la publication est refusée avant toute écriture.

**Non vérifié : les index.** La survie des index vectoriels et plein texte (`creer_index_*`) à la
réouverture n'est pas testée. Ne pas la supposer : recréez-les après `Base.ouvrir` au besoin.

Un seul écrivain par journal : un second `Base.ouvrir(nom)` sur le même fichier est refusé tant
que le premier n'est pas libéré (`free()`, ou ramassage par le GC, non déterministe). Le refus vient
d'OPFS (`createSyncAccessHandle`) ; il ne nomme pas le détenteur. Test :
`tests/navigateur.rs::un_second_ouvreur_du_meme_fichier_est_refuse`.

## Limites

- Essai, non publié : aucune version ni release n'est créée.
- Persistance : instantané seulement (voir ci-dessus).
