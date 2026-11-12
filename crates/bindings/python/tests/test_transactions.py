"""Transactions : commit persiste, rollback annule, handle terminé refusé."""

import pytest

import obrain


def test_commit_rend_les_ecritures_visibles():
    db = obrain.ObrainDB()
    tx = db.begin_transaction()
    tx.execute("INSERT (:Person {name: 'Alix'})")
    assert tx.is_active
    tx.commit()
    assert not tx.is_active
    assert db.node_count == 1


def test_rollback_annule_les_ecritures():
    db = obrain.ObrainDB()
    tx = db.begin_transaction()
    tx.execute("INSERT (:Person {name: 'Alix'})")
    tx.rollback()
    assert db.node_count == 0


def test_execute_apres_commit_refuse():
    db = obrain.ObrainDB()
    tx = db.begin_transaction()
    tx.commit()
    with pytest.raises(RuntimeError, match="completed transaction"):
        tx.execute("MATCH (n) RETURN n")


def test_isolation_level_explicite():
    db = obrain.ObrainDB()
    tx = db.begin_transaction("serializable")
    assert tx.isolation_level == "serializable"
    tx.rollback()


def test_commit_persiste_dans_une_base_ouverte_en_ecriture(base_path):
    db = obrain.ObrainDB.open_write(base_path)
    tx = db.begin_transaction()
    tx.execute("INSERT (:Person {name: 'Alix'})")
    tx.commit()
    db.close()
    ro = obrain.ObrainDB.open(base_path)
    assert ro.node_count == 1
    ro.close()
