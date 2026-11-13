"""PropertyGraphIndex end to end: a fake LLM extracts triplets, the index stores and queries them."""

from llama_index.core import Document
from llama_index.core.embeddings import MockEmbedding
from llama_index.core.graph_stores.types import EntityNode
from llama_index.core.indices.property_graph import (
    CustomPGRetriever,
    PropertyGraphIndex,
    SimpleLLMPathExtractor,
)
from llama_index.core.llms import CompletionResponse, CustomLLM, LLMMetadata, MockLLM
from llama_index.core.llms.callbacks import llm_completion_callback

from llama_index.graph_stores.obrain import ObrainPropertyGraphStore

TRIPLETS = "(Alix, KNOWS, Gus)\n(Gus, LIVES_IN, Paris)"


class FixedTripletLLM(CustomLLM):
    """Fake LLM: always answers with the same triplets, which the extractor parses."""

    @property
    def metadata(self) -> LLMMetadata:
        return LLMMetadata(model_name="fixed-triplets")

    @llm_completion_callback()
    def complete(self, prompt: str, formatted: bool = False, **kwargs) -> CompletionResponse:
        return CompletionResponse(text=TRIPLETS)

    @llm_completion_callback()
    def stream_complete(self, prompt: str, formatted: bool = False, **kwargs):
        raise NotImplementedError("streaming non utilisé dans les tests")


class EntityNeighbourhoodRetriever(CustomPGRetriever):
    """Deterministic retriever: seeds from the names quoted in the question, then walks the graph.

    No LLM is involved, so the test checks the store's graph walk and the
    index's retrieval wiring, not an LLM's keyword extraction.
    """

    def init(self, seed_names: list, depth: int = 2, **kwargs) -> None:
        self._seed_names = list(seed_names)
        self._depth = depth

    def custom_retrieve(self, query_str: str) -> list:
        seeds = [EntityNode(name=n) for n in self._seed_names if n.lower() in query_str.lower()]
        triplets = self.graph_store.get_rel_map(seeds, depth=self._depth)
        return [f"{s.name} -[{r.label}]-> {t.name}" for s, r, t in triplets]


def _build_index() -> tuple[PropertyGraphIndex, ObrainPropertyGraphStore]:
    store = ObrainPropertyGraphStore()
    index = PropertyGraphIndex.from_documents(
        [Document(text="Alix knows Gus. Gus lives in Paris.")],
        kg_extractors=[SimpleLLMPathExtractor(llm=FixedTripletLLM(), max_paths_per_chunk=10)],
        embed_model=MockEmbedding(embed_dim=8),
        property_graph_store=store,
        show_progress=False,
    )
    return index, store


def _retriever(store: ObrainPropertyGraphStore) -> EntityNeighbourhoodRetriever:
    return EntityNeighbourhoodRetriever(store, seed_names=["Alix", "Gus", "Paris"], depth=2)


def test_index_stores_extracted_triplets():
    _, store = _build_index()
    triplets = store.get_triplets()
    # LlamaIndex's extractor normalizes case ("KNOWS" -> "Knows"), so compare upper-cased labels.
    assert {(s.name, r.label.upper(), t.name) for s, r, t in triplets} == {
        ("Alix", "KNOWS", "Gus"),
        ("Gus", "LIVES_IN", "Paris"),
    }


def test_retriever_walks_stored_triplets_from_question_entity():
    _, store = _build_index()
    nodes = _retriever(store).retrieve("What does Alix know?")
    contents = [n.get_content() for n in nodes]
    assert any("Alix" in c and "Gus" in c for c in contents)
    assert any("Gus" in c and "Paris" in c for c in contents)


def test_query_engine_answers_from_graph_context():
    index, store = _build_index()
    engine = index.as_query_engine(
        sub_retrievers=[_retriever(store)], llm=MockLLM(max_tokens=None)
    )
    response = engine.query("Where does Alix's friend live?")
    # MockLLM echoes its prompt, so the retrieved graph context is visible in the answer.
    assert "Paris" in str(response)
