# obrain (Python)

Bindings Python (pyo3) de [Obrain](https://obrain.dev), base de graphes embarquée
au cœur Rust. Module `obrain`, classe `ObrainDB`.

## Installation

Pas encore publié sur PyPI : on construit la wheel depuis les sources.

```bash
python -m pip install "maturin==1.16.0"          # version épinglée (pyproject.toml)
cd crates/bindings/python
maturin build --release                          # wheel dans target/wheels/
python -m pip install ../../../target/wheels/obrain-*.whl
```

Pour un environnement de développement : `maturin develop` (dans un venv).
Python 3.12 ou plus récent.

## Exemple

```python
from obrain import ObrainDB

db = ObrainDB.open_write("./mon-graphe")       # écrivain (crée la base si absente)
db.execute("INSERT (:Person {name: 'Alix', age: 30})")
db.execute("INSERT (:Person {name: 'Gus', age: 25})")
db.execute("MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'}) INSERT (a)-[:KNOWS]->(b)")
db.close()                                     # rend le bail d'écriture

ro = ObrainDB.open("./mon-graphe")             # lecture seule (le défaut)
for row in ro.execute("MATCH (p:Person) RETURN p.name AS name ORDER BY name"):
    print(row["name"])                         # Alix, Gus
ro.close()
```

## Modes d'ouverture (décision D4)

| Appel | Mode | Comportement |
| ----- | ---- | ------------ |
| `ObrainDB()` | mémoire | base éphémère, lecture et écriture ; pas de fichier, pas de bail |
| `ObrainDB(path)` | lecture seule | même comportement que `open(path)` ; sans bail |
| `ObrainDB.open(path)` | lecture seule | erreur si la base n'existe pas ; rien n'est créé ni modifié |
| `ObrainDB.open_read_only(path)` | lecture seule | synonyme de `open` |
| `ObrainDB.open_write(path)` | écriture | crée la base si absente ; prend le **bail d'écriture** |

**Lecture seule par défaut :** tout chemin passé au constructeur ou à `open` ouvre
la base en lecture seule. Une écriture (`execute`, `create_node`, transaction en
écriture…) lève `RuntimeError` (« read-only »). Pour écrire sur une base persistante,
ouvrez-la explicitement avec `open_write`.

Le bail : un second `open_write` sur la même base, dans le même processus, lève
`RuntimeError` (« bail d'écriture déjà tenu ») tant que le premier handle, et
ses transactions, ne sont pas fermés (`close()`) ou détruits.

## Limites

- **Bail intra-processus seulement.** Le registre du bail vit dans un processus :
  deux processus peuvent chacun obtenir un `open_write` sur la même base. Entre
  processus, seul le verrou partagé du substrat s'applique ; un seul processus
  écrivain par base relève du déploiement. Voir `crates/bindings/c/README.md`,
  section « Bail ».
- **Lecture seule par défaut.** `ObrainDB(path)`, `open` et `open_read_only`
  refusent toute écriture (`OBRAIN-T003`).
- **Le bail ne concerne que `open_write`.** `ObrainDB(path)` n'en prend pas : il
  ouvre en lecture seule, comme `open`.
- **Vecteurs.** Une liste GQL `[1.0, 0.0]` n'est pas un vecteur indexable ; écrivez
  les embeddings avec `create_node` ou `set_node_property`.
- **`CALL obrain.hybrid_search` (GQL)** dépend de la PR #125, non fusionnée.
  La méthode Python `db.hybrid_search(...)` est disponible.
- **Pas de moteur cognitif** dans la wheel : elle est compilée avec la feature
  `full` (langages de requête, index, algorithmes, RDF).

## Tests

```bash
cd crates/bindings/python
python -m pip install "pytest>=9" "networkx>=3"
python -m pytest -v
```

Les tests utilisent des répertoires temporaires (`tmp_path`), jamais le dépôt.
