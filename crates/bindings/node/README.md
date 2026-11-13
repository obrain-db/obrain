# @obrain-db/js

Liaison Node.js (napi 3) de [Obrain](https://github.com/obrain-db/obrain), base de données graphe embarquée au cœur Rust.

**Statut : non publié.** Le paquet est construit localement (`npm run build`) et testé en CI ; aucune version n'est publiée sur npm pour l'instant.

## Installation

Depuis les sources du dépôt (Rust stable et Node.js 22 requis) :

```bash
cd crates/bindings/node
npm ci
npm run build      # napi build --platform --release --features full
npm test           # vitest
npm pack --dry-run # contenu du paquet, sans publication
```

## Exemple

```javascript
const { ObrainDB } = require('@obrain-db/js');

const db = ObrainDB.create();                       // base en mémoire
const alix = db.createNode(['Person'], { name: 'Alix', age: 30 });
const gus = db.createNode(['Person'], { name: 'Gus', age: 25 });
db.createEdge(alix.id, gus.id, 'KNOWS', { since: 2024 });

const result = await db.execute('MATCH (p:Person) WHERE p.age > 20 RETURN p.name, p.age');
for (const row of result.toArray()) console.log(row);
// { 'p.name': 'Alix', 'p.age': 30 }
// { 'p.name': 'Gus', 'p.age': 25 }

db.close();
```

## Modes d'ouverture (décision D4)

| Appel | Effet |
|---|---|
| `ObrainDB.create()` | base en mémoire, lecture/écriture |
| `ObrainDB.open(path)` | **lecture seule par défaut** : plusieurs lecteurs peuvent coexister avec un écrivain ; toute mutation lève une erreur |
| `ObrainDB.openWrite(path)` | écriture explicite ; crée la base si elle n'existe pas ; prend le **bail d'écriture** du processus |
| `ObrainDB.create(path)` | même effet que `openWrite(path)` |

Le bail d'écriture refuse un second `openWrite` sur le même store tant que le premier handle n'est pas fermé (`close()`). Il est partagé avec la liaison C (`crates/bindings/common/src/bail.rs`).

## Limites

- **Bail intra-processus seulement.** Il ne protège pas contre deux processus qui ouvrent le même store en écriture : entre processus, c'est le verrou partagé du substrat qui s'applique.
- **Index texte :** un nœud créé après `createTextIndex` n'apparaît pas dans `textSearch` tant que `rebuildTextIndex` n'a pas été appelé (régression moteur signalée, voir la PR).
- **Procédure `CALL` de recherche hybride** absente tant que la PR #125 n'est pas fusionnée ; la méthode `hybridSearch` est disponible.
- **Pas de paquet publié** : les paquets par plateforme (`npm/*`) et les `optionalDependencies` sont ajoutés par `napi prepublish` au moment de publier.
- Les types TypeScript (`index.d.ts`) sont régénérés par `napi build` et vérifiés en CI : ne pas les éditer à la main.
