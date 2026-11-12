"""ObrainGraph against the LangChain GraphStore interface. No network, no API key."""

import pytest
from langchain_community.graphs.graph_document import GraphDocument, Node, Relationship
from langchain_core.documents import Document

from langchain_obrain import ObrainGraph


def _alix_gus() -> GraphDocument:
    alix = Node(id="Alix", type="Person", properties={"age": 30})
    gus = Node(id="Gus", type="Person", properties={"age": 41})
    return GraphDocument(
        nodes=[alix, gus],
        relationships=[Relationship(source=alix, target=gus, type="KNOWS", properties={"since": 2019})],
        source=Document(page_content="Alix knows Gus since 2019."),
    )


def test_query_with_named_parameters():
    graph = ObrainGraph()
    graph.add_graph_documents([_alix_gus()])
    rows = graph.query("MATCH (p:Person {id: $name}) RETURN p.age AS age", {"name": "Alix"})
    assert rows == [{"age": 30}]


def test_query_without_parameters_returns_list_of_dicts():
    graph = ObrainGraph()
    graph.add_graph_documents([_alix_gus()])
    rows = graph.query("MATCH (p:Person) RETURN count(p) AS c")
    assert rows == [{"c": 2}]


def test_add_graph_documents_is_idempotent():
    graph = ObrainGraph()
    graph.add_graph_documents([_alix_gus()])
    graph.add_graph_documents([_alix_gus()])
    assert graph.query("MATCH (p:Person) RETURN count(p) AS c") == [{"c": 2}]
    assert graph.query("MATCH ()-[r:KNOWS]->() RETURN count(r) AS c") == [{"c": 1}]


def test_add_graph_documents_with_source_links_mentions():
    graph = ObrainGraph()
    graph.add_graph_documents([_alix_gus()], include_source=True)
    rows = graph.query("MATCH (d:Document)-[:MENTIONS]->(p:Person) RETURN count(p) AS c")
    assert rows == [{"c": 2}]


def test_schema_describes_labels_properties_and_relationships():
    graph = ObrainGraph()
    graph.add_graph_documents([_alix_gus()])
    structured = graph.get_structured_schema
    assert structured["node_props"]["Person"] == [
        {"property": "age", "type": "INTEGER"},
        {"property": "id", "type": "STRING"},
    ]
    assert {"start": "Person", "type": "KNOWS", "end": "Person"} in structured["relationships"]
    assert "Person {age: INTEGER, id: STRING}" in graph.get_schema
    assert "(:Person)-[:KNOWS]->(:Person)" in graph.get_schema


def test_refresh_schema_sees_new_content():
    graph = ObrainGraph()
    assert graph.get_structured_schema["node_props"] == {}
    graph.db.execute("CREATE (:City {name: \"Paris\"})")
    graph.refresh_schema()
    assert "City {name: STRING}" in graph.get_schema


def test_base_entity_label_option():
    graph = ObrainGraph(base_entity_label=True)
    graph.add_graph_documents([_alix_gus()])
    assert graph.query("MATCH (n:__Entity__) RETURN count(n) AS c") == [{"c": 2}]


@pytest.mark.parametrize("bad_label", ["Person) DETACH DELETE (n", "Per son", "1X"])
def test_unsafe_label_is_rejected_before_any_query(bad_label):
    graph = ObrainGraph()
    node = Node(id="x", type=bad_label, properties={})
    with pytest.raises(ValueError, match="invalide pour une requête Cypher"):
        graph.add_graph_documents([GraphDocument(nodes=[node], relationships=[], source=Document(page_content=""))])
    assert graph.query("MATCH (n) RETURN count(n) AS c") == [{"c": 0}]
