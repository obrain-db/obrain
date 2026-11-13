"""ObrainPropertyGraphStore against the LlamaIndex PropertyGraphStore interface. No network."""

from llama_index.core.graph_stores.types import EntityNode, Relation
from llama_index.core.schema import TextNode
from llama_index.core.vector_stores.types import VectorStoreQuery

from llama_index.graph_stores.obrain import ObrainPropertyGraphStore


def _store() -> ObrainPropertyGraphStore:
    store = ObrainPropertyGraphStore()
    store.upsert_triplet("Alix", "KNOWS", "Gus")
    store.upsert_triplet("Gus", "LIVES_IN", "Paris")
    return store


def test_upsert_triplet_then_get_triplets():
    store = _store()
    triplets = store.get_triplets(entity_names=["Alix"])
    assert [(s.name, r.label, t.name) for s, r, t in triplets] == [("Alix", "KNOWS", "Gus")]


def test_upsert_triplet_is_idempotent():
    store = _store()
    store.upsert_triplet("Alix", "KNOWS", "Gus")
    assert len(store.get_triplets(relation_names=["KNOWS"])) == 1


def test_get_returns_entities_by_name_and_id():
    store = _store()
    by_id = store.get(ids=["Gus"])
    assert [n.name for n in by_id] == ["Gus"]
    by_prop = store.get(properties={"name": "Paris"})
    assert [n.name for n in by_prop] == ["Paris"]


def test_get_rel_map_walks_depth_and_respects_limit():
    store = _store()
    depth1 = store.get_rel_map([EntityNode(name="Alix")], depth=1)
    assert [(s.name, r.label, t.name) for s, r, t in depth1] == [("Alix", "KNOWS", "Gus")]
    depth2 = store.get_rel_map([EntityNode(name="Alix")], depth=2)
    assert {(s.name, r.label, t.name) for s, r, t in depth2} == {
        ("Alix", "KNOWS", "Gus"),
        ("Gus", "LIVES_IN", "Paris"),
    }
    capped = store.get_rel_map([EntityNode(name="Alix")], depth=2, limit=1)
    assert len(capped) == 1


def test_get_rel_map_ignore_rels():
    store = _store()
    triplets = store.get_rel_map([EntityNode(name="Alix")], depth=2, ignore_rels=["KNOWS"])
    assert triplets == []


def test_get_rel_map_follows_incoming_relations():
    store = _store()
    triplets = store.get_rel_map([EntityNode(name="Gus")], depth=1)
    assert {(s.name, r.label, t.name) for s, r, t in triplets} == {
        ("Alix", "KNOWS", "Gus"),
        ("Gus", "LIVES_IN", "Paris"),
    }


def test_unsafe_llm_label_is_sanitized_and_restored():
    store = ObrainPropertyGraphStore()
    store.upsert_nodes([EntityNode(name="Alix", label="person"), EntityNode(name="Acme", label="company org")])
    store.upsert_relations([Relation(label="works at", source_id="Alix", target_id="Acme")])
    triplets = store.get_triplets(relation_names=["works at"])
    assert [(s.name, r.label, t.name) for s, r, t in triplets] == [("Alix", "works at", "Acme")]
    rows = store.structured_query("MATCH (a:person)-[r:works_at]->(b) RETURN a.id AS a, b.id AS b")
    assert rows == [{"a": "Alix", "b": "Acme"}]


def test_structured_query_passes_parameters():
    store = _store()
    rows = store.structured_query(
        "MATCH (a)-[r:KNOWS]->(b) WHERE a.id = $who RETURN b.id AS friend", {"who": "Alix"}
    )
    assert rows == [{"friend": "Gus"}]


def test_vector_query_returns_most_similar_chunk():
    store = ObrainPropertyGraphStore()
    store.upsert_llama_nodes(
        [
            TextNode(id_="c1", text="graphs", embedding=[1.0, 0.0, 0.0]),
            TextNode(id_="c2", text="vectors", embedding=[0.0, 1.0, 0.0]),
        ]
    )
    nodes, scores = store.vector_query(
        VectorStoreQuery(query_embedding=[0.9, 0.1, 0.0], similarity_top_k=1)
    )
    assert [n.text for n in nodes] == ["graphs"]
    assert len(scores) == 1 and 0.0 < scores[0] <= 1.0


def test_get_llama_nodes_round_trips_chunk_text():
    store = ObrainPropertyGraphStore()
    store.upsert_llama_nodes([TextNode(id_="c1", text="hello", embedding=[1.0, 0.0])])
    nodes = store.get_llama_nodes(["c1"])
    assert [(n.node_id, n.get_content()) for n in nodes] == [("c1", "hello")]


def test_delete_removes_entities_and_relations():
    store = _store()
    store.delete(entity_names=["Paris"])
    assert store.get(ids=["Paris"]) == []
    assert store.get_triplets(relation_names=["LIVES_IN"]) == []
    assert len(store.get_triplets(relation_names=["KNOWS"])) == 1
