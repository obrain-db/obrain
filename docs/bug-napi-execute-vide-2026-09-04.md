# RÉSOLU — faux positif : « binding NAPI renvoie 0 ligne » (2026-09-04)

**Cause : artefact de TEST, pas un bug moteur.** `ObrainDB.execute()` (binding
NAPI) est **async** : il retourne une `Promise<QueryResult>`. Les scripts de
diagnostic appelaient `db.execute(...)` **sans `await`** puis lisaient
`toArray()` sur la Promise → toujours 0 ligne. Avec `await`, le binding
fonctionne :

```js
const m = await db.execute("MATCH (f:File) RETURN f.path AS path");
// → [{path: 'a.js'}, {path: 'b.js'}]  ✓
```

- Les écritures `createNode` sont bien visibles par `MATCH` (await).
- La lecture sur un store rempli par le CLI fonctionne aussi (await).
- Test moteur ajouté `crates/obrain-engine/tests/session_visibility.rs`
  (vert) : `with_session` ET session tenue ET session neuve voient les
  écritures committées.

**Impact** : aucun sur aletheia (le code de prod `obrain-store.ts` fait
correctement `await`). Le store git obrain tourne sur la voie CLI (choix
valide) ; le binding NAPI est une alternative fonctionnelle.
