# llama-index-graph-stores-obrain

LlamaIndex `PropertyGraphStore` backed by [Obrain](https://obrain.dev), an embedded
graph database with a Rust core. It plugs into `PropertyGraphIndex`:

- `upsert_triplet`, `upsert_nodes`, `upsert_relations` write the graph;
- `get`, `get_triplets` and `get_rel_map` read it, with depth and limit;
- `structured_query` runs GQL/Cypher with named parameters;
- `vector_query` searches the embedded text chunks (HNSW index);
- `get_schema` returns the database schema.

## Installation

Not published on PyPI yet. Build the `obrain` wheel from the sources first, then
install this package in editable mode:

```bash
python -m pip install "maturin==1.16.0"                    # version pinned in pyproject.toml
maturin build --release --manifest-path crates/bindings/python/Cargo.toml --out dist
python -m pip install dist/obrain-*.whl
python -m pip install -e "crates/bindings/python/integrations/llama_index_obrain[test]"
```

Requires Python 3.12 or newer.

### Pinned versions

| Package            | Version |
| ------------------ | ------- |
| `llama-index-core` | 0.14.25 |
| `obrain`           | 0.0.1   |
| `pytest` (tests)   | 9.2.0   |

## Example: RAG

`MockLLM` and `MockEmbedding` keep this example offline. `MockLLM` only echoes
its prompt, so it extracts no triplets and the answer is `Empty Response`: use a
real LLM (for example `llama_index.llms.openai.OpenAI`) and a real embedding
model for real results.

```python
from llama_index.core import Document, PropertyGraphIndex, Settings
from llama_index.core.embeddings import MockEmbedding  # use a real embedding model
from llama_index.core.indices.property_graph import SimpleLLMPathExtractor
from llama_index.core.llms import MockLLM  # use a real LLM
from llama_index.graph_stores.obrain import ObrainPropertyGraphStore

llm = Settings.llm = MockLLM(max_tokens=None)  # use a real LLM
index = PropertyGraphIndex.from_documents(
    [Document(text="Alix knows Gus. Gus lives in Paris.")],
    kg_extractors=[SimpleLLMPathExtractor(llm=llm)],
    embed_model=MockEmbedding(embed_dim=8),
    property_graph_store=ObrainPropertyGraphStore(),  # path=... for a persistent file
)
print(index.as_query_engine(llm=llm).query("Where does Alix's friend live?"))
```

## Tests

```bash
cd crates/bindings/python/integrations/llama_index_obrain
python -m pytest -v -rs
```

The tests make no network calls: a fixture blocks sockets for the whole session.
They use `CustomLLM` with fixed triplets, `MockLLM` and `MockEmbedding`, and need
no API key. `PropertyGraphIndex` runs end to end against a real temporary
database. A test that cannot run is reported as a skip with its name and reason.
