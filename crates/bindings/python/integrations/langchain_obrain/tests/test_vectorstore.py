"""ObrainVectorStore against the LangChain VectorStore interface, with a deterministic fake embedding."""

import pytest
from langchain_core.embeddings import DeterministicFakeEmbedding
from obrain import ObrainDB

from langchain_obrain import (
    HybridSearchUnavailableError,
    ObrainVectorStore,
    hybrid_search_available,
)
import langchain_obrain.vectorstore as vectorstore_module

TEXTS = [
    "graph databases store nodes and edges",
    "vector indexes find nearest neighbours",
    "rust makes systems code safe",
]


def _store(**kwargs) -> ObrainVectorStore:
    emb = DeterministicFakeEmbedding(size=16)
    return ObrainVectorStore.from_texts(
        TEXTS,
        emb,
        metadatas=[{"lang": "en"}, {"lang": "en"}, {"lang": "fr"}],
        ids=["g", "v", "r"],
        **kwargs,
    )


def test_similarity_search_returns_exact_match_first():
    store = _store()
    hits = store.similarity_search(TEXTS[1], k=1)
    assert [d.id for d in hits] == ["v"]
    assert hits[0].page_content == TEXTS[1]


def test_similarity_search_with_score_orders_by_distance():
    store = _store()
    scored = store.similarity_search_with_score(TEXTS[0], k=3)
    distances = [s for _, s in scored]
    assert distances == sorted(distances)
    assert scored[0][0].id == "g"


def test_metadata_filter_restricts_results():
    store = _store()
    hits = store.similarity_search(TEXTS[0], k=3, filter={"lang": "fr"})
    assert [d.id for d in hits] == ["r"]
    assert hits[0].metadata == {"lang": "fr"}


def test_get_by_ids_keeps_requested_order():
    store = _store()
    docs = store.get_by_ids(["r", "g"])
    assert [d.id for d in docs] == ["r", "g"]


def test_delete_removes_documents():
    store = _store()
    assert store.delete(["v"]) is True
    assert store.get_by_ids(["v"]) == []
    assert "v" not in [d.id for d in store.similarity_search(TEXTS[1], k=3)]


def test_reserved_metadata_key_is_rejected():
    store = ObrainVectorStore(ObrainDB(), DeterministicFakeEmbedding(size=4))
    with pytest.raises(ValueError, match="réservées"):
        store.add_texts(["x"], [{"doc_id": "forged"}])


def test_hybrid_search_fuses_text_and_vector():
    if not hybrid_search_available(ObrainDB()):
        pytest.skip("obrain.hybrid_search absent de ce binding : test hybride non exécuté")
    store = _store()
    hits = store.search("graph", "hybrid", k=2)
    assert hits[0].id == "g"
    assert "score" in hits[0].metadata


def test_hybrid_search_raises_when_procedure_missing(monkeypatch):
    # The store must never fall back silently to plain vector search.
    monkeypatch.setattr(vectorstore_module, "hybrid_search_available", lambda db: False)
    store = _store()
    with pytest.raises(HybridSearchUnavailableError, match="obrain.hybrid_search"):
        store.hybrid_search("graph", k=2)
