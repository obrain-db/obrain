"""Fixtures partagées : bases temporaires, toujours sous `tmp_path` (jamais dans le dépôt)."""

import pytest


@pytest.fixture
def base_path(tmp_path):
    """Chemin d'une base persistante qui n'existe pas encore."""
    return str(tmp_path / "base")
