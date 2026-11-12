# langchain-obrain

LangChain integration for [Obrain](https://obrain.dev), an embedded graph database
with a Rust core. The package provides:

- `ObrainGraph`, a `GraphStore` from `langchain-community`. It runs GQL/Cypher
  queries with named parameters, reports the schema (`get_schema`,
  `get_structured_schema`, `refresh_schema`) and writes `GraphDocument`s
  (`add_graph_documents`). It works with `GraphCypherQAChain`.
- `ObrainVectorStore`, a `VectorStore` on Obrain's HNSW vector index. Hybrid
  search (BM25 on the text index fused with the vector index) goes through
  `CALL obrain.hybrid_search`.

## Installation

Not published on PyPI yet. Build the `obrain` wheel from the sources first, then
install this package in editable mode:

```bash
python -m pip install "maturin==1.16.0"                    # version pinned in pyproject.toml
maturin build --release --manifest-path crates/bindings/python/Cargo.toml --out dist
python -m pip install dist/obrain-*.whl
python -m pip install -e "crates/bindings/python/integrations/langchain_obrain[test]"
```

Requires Python 3.12 or newer.

### Pinned versions

| Package              | Version  |
| -------------------- | -------- |
| `langchain-core`     | 1.8.0    |
| `langchain-community`| 0.4.2    |
| `obrain`             | 0.0.1    |
| `pytest` (tests)     | 9.2.0    |

`langchain-community` is in maintenance mode upstream and emits a
`DeprecationWarning` on import. `ObrainGraph` follows its `GraphStore` interface
for as long as it is the reference interface.

## Example: RAG

This example runs offline. `DeterministicFakeEmbedding` and `FakeListLLM` stand
in for a real embedding model and a real LLM; swap them out in production.

```python
from langchain_core.embeddings import DeterministicFakeEmbedding  # use a real embedding model
from langchain_core.language_models import FakeListLLM  # use a real LLM
from langchain_core.prompts import PromptTemplate
from langchain_core.runnables import RunnablePassthrough
from langchain_obrain import ObrainVectorStore

store = ObrainVectorStore.from_texts(["Obrain is an embedded graph database written in Rust.", "The Python binding exposes ObrainDB as the obrain module."], DeterministicFakeEmbedding(size=64))
retriever = store.as_retriever(search_kwargs={"k": 2})
context = retriever | (lambda docs: "\n".join(d.page_content for d in docs))
prompt = PromptTemplate.from_template("Context:\n{context}\n\nQuestion: {question}\nAnswer:")
llm = FakeListLLM(responses=["Obrain is an embedded graph database."])
rag = {"context": context, "question": RunnablePassthrough()} | prompt | llm
print(rag.invoke("What is Obrain?"))
```

## Graph queries and GraphCypherQAChain

```python
from langchain_obrain import ObrainGraph

graph = ObrainGraph()                      # in memory; ObrainGraph(path=...) for a file
graph.query("CREATE (:Person {id: $id, age: $age})", {"id": "Alix", "age": 30})
graph.refresh_schema()                     # the schema is cached: refresh after raw writes
print(graph.get_schema)                    # labels, properties and relationships
```

Hybrid search needs a text index on the text property, which `ObrainVectorStore`
creates for you. If the engine lacks `obrain.hybrid_search`, `hybrid_search`
raises `HybridSearchUnavailableError`. It never falls back silently to plain
vector search.

## Tests

```bash
cd crates/bindings/python/integrations/langchain_obrain
python -m pytest -v -rs
```

The tests make no network calls: a fixture blocks sockets for the whole session.
They use `FakeListLLM` and `DeterministicFakeEmbedding`, and need no API key.
`GraphCypherQAChain` runs end to end against a real temporary database. A test
that cannot run is reported as a skip with its name and reason, never as a pass.
