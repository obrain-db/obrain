#!/usr/bin/env python3
"""Tests de `scripts/criterion_resume.py` (résumé Criterion des runs de mesures).

Chaque cas fixe un comportement qu'un lecteur du résumé peut constater :
l'unité choisie, la valeur retenue (la pente, celle que Criterion imprime), et
les deux pièges du dossier `base/` — une mesure antérieure absente, une base
recopiée à l'identique.
"""

from __future__ import annotations

import json
import pathlib
import sys
import tempfile
import unittest

# `python3 -I` n'ajoute pas le dossier du script à sys.path : on le pose.
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import criterion_resume as cr  # noqa: E402


class Duree(unittest.TestCase):
    def test_choisit_l_unite_lisible(self):
        self.assertEqual(cr.duree(500), "500 ns")
        self.assertEqual(cr.duree(1_500), "1.5 µs")
        self.assertEqual(cr.duree(2_500_000), "2.5 ms")
        self.assertEqual(cr.duree(3e9), "3 s")


class Medianes(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.racine = pathlib.Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def ecrire(self, identifiant: str, genre: str, **estimations) -> None:
        dossier = self.racine / identifiant / genre
        dossier.mkdir(parents=True, exist_ok=True)
        donnees = {}
        for cle, valeur in estimations.items():
            donnees[cle] = None if valeur is None else {"point_estimate": valeur}
        (dossier / "estimates.json").write_text(json.dumps(donnees), encoding="utf-8")

    def test_prend_la_pente_pas_la_mediane(self):
        self.ecrire("query_1hop", "new", slope=5_070.0, median=3_000.0)
        self.assertEqual(cr.medianes(self.racine), [("query_1hop", 5_070.0, None)])

    def test_descend_sur_la_mediane_si_la_pente_est_nulle(self):
        self.ecrire("banc", "new", slope=None, median=42.0)
        self.assertEqual(cr.medianes(self.racine), [("banc", 42.0, None)])

    def test_garde_la_base_quand_elle_differe(self):
        self.ecrire("banc", "new", slope=100.0)
        self.ecrire("banc", "base", slope=50.0)
        self.assertEqual(cr.medianes(self.racine), [("banc", 100.0, 50.0)])

    def test_ignore_une_base_recopiee_a_l_identique(self):
        self.ecrire("banc", "new", slope=100.0)
        self.ecrire("banc", "base", slope=100.0)
        self.assertEqual(cr.medianes(self.racine), [("banc", 100.0, None)])

    def test_un_bench_sans_new_est_ignore(self):
        self.ecrire("ancien", "base", slope=100.0)
        self.assertEqual(cr.medianes(self.racine), [])


class Tableau(unittest.TestCase):
    def test_dossier_vide_dit_qu_il_n_y_a_rien(self):
        self.assertIn("Aucune mesure", cr.tableau([]))

    def test_sans_base_pas_de_colonne_de_comparaison(self):
        sortie = cr.tableau([("banc", 1_500_000.0, None)])
        self.assertIn("| Bench | Temps/itération |", sortie)
        self.assertIn("| `banc` | 1.5 ms |", sortie)
        self.assertNotIn("vs précédent", sortie)

    def test_avec_base_montre_le_rapport(self):
        sortie = cr.tableau([("banc", 200.0, 100.0)])
        self.assertIn("vs précédent", sortie)
        self.assertIn("×2.00", sortie)

    def test_ecart_sous_3_pour_cent_est_identique(self):
        sortie = cr.tableau([("banc", 101.0, 100.0)])
        self.assertIn("identique", sortie)

    def test_ligne_sans_base_mais_tableau_compare_reste_complete(self):
        sortie = cr.tableau([("a", 1.0, None), ("b", 2.0, 1.0)])
        self.assertIn("| `a` | 1 ns | — |", sortie)


class Main(unittest.TestCase):
    def test_dossier_absent_n_echoue_pas(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(cr.main(["prog", str(pathlib.Path(tmp) / "absent")]), 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
