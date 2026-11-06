# obrain-web — paquet navigateur (essai)

Base obrain **en mémoire**, dans le navigateur, via `wasm-bindgen`.

## Construire

```sh
cargo build --target wasm32-unknown-unknown -p obrain-web --release
wasm-bindgen --target web    --out-dir pkg-web  target/wasm32-unknown-unknown/release/obrain_web.wasm
wasm-bindgen --target nodejs --out-dir pkg      target/wasm32-unknown-unknown/release/obrain_web.wasm
```

Versions : `wasm-bindgen-cli` **0.2.130**, identique à la version verrouillée dans `Cargo.lock`.

## Utiliser

```js
import init, { Base } from "./obrain_web.js";
await init();
const b = new Base();
b.executer("INSERT (:Personne {nom: 'Alix'})");
const r = JSON.parse(b.executer("MATCH (p:Personne) RETURN p.nom AS nom"));
// r = { colonnes: ["nom"], lignes: [["Alix"]] }
```

Une requête invalide lève une erreur JavaScript avec le message du moteur.

## Limites

- Pas de persistance : la base disparaît au rechargement de la page.
- Essai, non publié : aucune version ni release n'est créée.
