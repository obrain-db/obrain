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

**Persisté : l'instantané publié.** `persister()` écrit l'état complet de la base (nœuds, arêtes,
propriétés, dictionnaires) dans le journal OPFS, en deux emplacements alternés avec numéro de
génération et CRC. Une publication interrompue ne rend jamais l'état précédent illisible.

**Non persisté : les écritures faites depuis la dernière publication.** Ce n'est pas un journal.
Une écriture non suivie d'un `persister()` est **perdue à la coupure** (fermeture d'onglet,
rechargement, crash). Pour rendre une série d'écritures durable, appelez `persister()` après elle.
Le coût est proportionnel à la taille de la base, et la publication est refusée au-delà de 2 Gio.

**Non vérifié : les index.** La survie des index vectoriels et plein texte (`creer_index_*`) à la
réouverture n'est pas testée. Ne pas la supposer : recréez-les après `Base.ouvrir` au besoin.

**Pas encore fait : le WAL sur OPFS** (décision C). Un journal écrit à chaque mutation, qui
rendrait chaque écriture durable sans `persister()`, n'existe pas encore. Il n'est pas dans ce
paquet.

Un seul écrivain par journal : un second `Base.ouvrir(nom)` sur le même fichier est refusé tant
que le premier n'est pas libéré (`free()`, ou ramassage par le GC, non déterministe). Le refus vient
d'OPFS (`createSyncAccessHandle`) ; il ne nomme pas le détenteur. Test :
`tests/navigateur.rs::un_second_ouvreur_du_meme_fichier_est_refuse`.

## Limites

- Essai, non publié : aucune version ni release n'est créée.
- Persistance : instantané seulement (voir ci-dessus).
