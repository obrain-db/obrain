"""Erreurs typées : arguments invalides -> TypeError/ValueError, base et requête -> RuntimeError."""

import pytest

import obrain


def test_syntaxe_gql_invalide_leve_runtime_error():
    db = obrain.ObrainDB()
    with pytest.raises(RuntimeError, match="syntax error"):
        db.execute("MATCH (n RETURN")


def test_ecriture_en_lecture_seule_porte_un_code_d_erreur(base_path):
    db = obrain.ObrainDB.open_write(base_path)
    db.close()
    ro = obrain.ObrainDB.open(base_path)
    with pytest.raises(RuntimeError, match="OBRAIN-T003"):
        ro.execute("INSERT (:Person {name: 'Gus'})")
    ro.close()


def test_chemin_non_chaine_leve_type_error():
    with pytest.raises(TypeError):
        obrain.ObrainDB.open_write(123)


def test_base_absente_leve_runtime_error(tmp_path):
    with pytest.raises(RuntimeError):
        obrain.ObrainDB.open(str(tmp_path / "absente"))
