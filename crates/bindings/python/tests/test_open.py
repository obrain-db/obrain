"""Modes d'ouverture (décision D4) : lecture seule par défaut, écriture explicite."""

import obrain


def _peupler(path):
    db = obrain.ObrainDB.open_write(path)
    db.execute("INSERT (:Person {name: 'Alix'})")
    db.close()


def test_base_en_memoire_ecrit_et_lit():
    db = obrain.ObrainDB()
    db.execute("INSERT (:Person {name: 'Alix'})")
    assert db.node_count == 1


def test_open_write_cree_puis_open_relit(base_path):
    _peupler(base_path)
    ro = obrain.ObrainDB.open(base_path)
    assert ro.execute("MATCH (p:Person) RETURN p.name AS name").to_list() == [
        {"name": "Alix"}
    ]
    ro.close()


def test_open_est_lecture_seule(base_path):
    import pytest

    _peupler(base_path)
    ro = obrain.ObrainDB.open(base_path)
    with pytest.raises(RuntimeError, match="read-only"):
        ro.execute("INSERT (:Person {name: 'Gus'})")
    ro.close()


def test_open_read_only_est_un_synonyme(base_path):
    _peupler(base_path)
    ro = obrain.ObrainDB.open_read_only(base_path)
    assert ro.node_count == 1
    ro.close()


def test_open_base_absente_echoue_sans_rien_creer(tmp_path):
    import pytest

    cible = tmp_path / "absente"
    with pytest.raises(RuntimeError):
        obrain.ObrainDB.open(str(cible))
    assert not cible.exists()


def test_constructeur_avec_chemin_est_lecture_seule(base_path):
    """D4 : `ObrainDB(path)` ouvre en lecture seule, comme `open`, sans bail."""
    import pytest

    _peupler(base_path)
    db = obrain.ObrainDB(base_path)
    with pytest.raises(RuntimeError, match="read-only"):
        db.execute("INSERT (:Person {name: 'Gus'})")
    with pytest.raises(RuntimeError, match="read-only"):
        db.create_node(["Person"], {"name": "Gus"})
    with pytest.raises(RuntimeError, match="read-only"):
        db.set_node_property(0, "name", "Gus")
    with pytest.raises(RuntimeError, match="read-only"):
        db.delete_node(0)
    assert db.node_count == 1
    db.close()


def test_constructeur_avec_chemin_ne_cree_rien(tmp_path):
    import pytest

    cible = tmp_path / "absente"
    with pytest.raises(RuntimeError):
        obrain.ObrainDB(str(cible))
    assert not cible.exists()


def test_lecteur_coexiste_avec_un_ecrivain_du_meme_processus(base_path):
    _peupler(base_path)
    writer = obrain.ObrainDB.open_write(base_path)
    reader = obrain.ObrainDB.open(base_path)
    assert reader.node_count == 1
    reader.close()
    writer.close()
