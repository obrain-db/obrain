"""Recherche : texte, vectorielle, hybride (méthode Python) et procédure GQL.

Les vecteurs s'écrivent avec `create_node` / `set_node_property` : une liste
GQL `[1.0, 0.0]` n'est pas indexée comme vecteur (voir README, « Limites »).
"""

import pytest

import obrain


def _corpus():
    db = obrain.ObrainDB()
    db.create_node(["Doc"], {"body": "graph database engine", "emb": [1.0, 0.0, 0.0]})
    db.create_node(["Doc"], {"body": "cooking pasta recipes", "emb": [0.0, 1.0, 0.0]})
    db.create_node(["Doc"], {"body": "graph theory and paths", "emb": [0.9, 0.1, 0.0]})
    db.create_text_index("Doc", "body")
    db.create_vector_index("Doc", "emb", 3, "cosine")
    return db


def test_recherche_texte_trouve_les_documents_pertinents():
    db = _corpus()
    hits = db.text_search("Doc", "body", "graph", 5)
    assert {node_id for node_id, _score in hits} == {1, 3}


def test_recherche_vectorielle_classe_le_plus_proche_en_premier():
    db = _corpus()
    hits = db.vector_search("Doc", "emb", [1.0, 0.0, 0.0], 2)
    assert hits[0][0] == 1
    assert len(hits) == 2


def test_hybrid_search_methode_fusionne_texte_et_vecteur():
    db = _corpus()
    hits = db.hybrid_search("Doc", "body", "emb", "graph", 2, [1.0, 0.0, 0.0])
    ids = [node_id for node_id, _score in hits]
    assert ids[0] in {1, 3}
    assert len(ids) == 2


@pytest.mark.skip(
    reason=(
        "CALL obrain.hybrid_search est livré par la PR obrain-db/obrain#125 "
        "(feat/gql-hybrid-search), non fusionnée sur main à la date de cette PR"
    )
)
def test_call_obrain_hybrid_search_en_gql():
    db = _corpus()
    result = db.execute(
        "CALL obrain.hybrid_search('Doc', 'body', 'emb', 'graph', 2, [1.0, 0.0, 0.0]) "
        "YIELD node_id, score RETURN node_id, score"
    )
    assert len(result.to_list()) == 2
