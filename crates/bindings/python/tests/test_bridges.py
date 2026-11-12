"""Bridges networkx et solvOR : la même base vue par les bibliothèques de graphes."""

import networkx as nx

import obrain


def _db_triangle():
    db = obrain.ObrainDB()
    db.execute("INSERT (:City {name: 'Lyon'})")
    db.execute("INSERT (:City {name: 'Paris'})")
    db.execute("INSERT (:City {name: 'Nice'})")
    db.execute(
        "MATCH (a:City {name: 'Lyon'}), (b:City {name: 'Paris'}) INSERT (a)-[:ROAD]->(b)"
    )
    db.execute(
        "MATCH (a:City {name: 'Paris'}), (b:City {name: 'Nice'}) INSERT (a)-[:ROAD]->(b)"
    )
    return db


def test_networkx_to_networkx_exporte_noeuds_et_aretes():
    db = _db_triangle()
    graph = db.as_networkx().to_networkx()
    assert isinstance(graph, nx.DiGraph)
    assert graph.number_of_nodes() == 3
    assert graph.number_of_edges() == 2


def test_networkx_pagerank_sur_le_bridge():
    db = _db_triangle()
    scores = db.as_networkx().pagerank()
    assert len(scores) == 3
    assert abs(sum(scores.values()) - 1.0) < 1e-6


def test_solvor_pagerank_sur_le_bridge():
    db = _db_triangle()
    scores = db.as_solvor().pagerank()
    assert set(scores) == {1, 2, 3}
    assert abs(sum(scores.values()) - 1.0) < 1e-6


def test_solvor_plus_court_chemin_via_le_bridge():
    db = _db_triangle()
    cout, chemin = db.as_solvor().shortest_path(1, 3)
    assert cout == 2.0
    assert chemin == [1, 2, 3]
