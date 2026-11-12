"""Bail d'écriture (décision D4) : un seul écrivain par base, dans un processus.

Le bail est intra-processus : ces tests ne prouvent rien entre deux processus
(voir README, « Limites »).
"""

import gc

import pytest

import obrain

REFUS = "bail d'écriture déjà tenu"


def test_second_open_write_echoue_par_bail(base_path):
    premier = obrain.ObrainDB.open_write(base_path)
    with pytest.raises(RuntimeError, match=REFUS):
        obrain.ObrainDB.open_write(base_path)
    premier.close()


def test_close_rend_le_bail(base_path):
    premier = obrain.ObrainDB.open_write(base_path)
    premier.close()
    second = obrain.ObrainDB.open_write(base_path)
    second.close()


def test_destruction_rend_le_bail(base_path):
    premier = obrain.ObrainDB.open_write(base_path)
    del premier
    gc.collect()
    second = obrain.ObrainDB.open_write(base_path)
    second.close()


def test_transaction_tient_le_bail_jusqua_sa_fin(base_path):
    handle = obrain.ObrainDB.open_write(base_path)
    tx = handle.begin_transaction()
    del handle
    gc.collect()
    with pytest.raises(RuntimeError, match=REFUS):
        obrain.ObrainDB.open_write(base_path)
    tx.rollback()
    del tx
    gc.collect()
    obrain.ObrainDB.open_write(base_path).close()


def test_bail_compare_le_chemin_canonique(base_path, tmp_path, monkeypatch):
    premier = obrain.ObrainDB.open_write(base_path)
    monkeypatch.chdir(tmp_path)
    with pytest.raises(RuntimeError, match=REFUS):
        obrain.ObrainDB.open_write("./base/../base")
    premier.close()
