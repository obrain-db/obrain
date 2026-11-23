#!/usr/bin/env python3
"""Tests de `couverture_badge.py` (badge et page de couverture sur GitHub Pages).

Chaque cas fabrique un rapport lcov synthétique, lance le vrai script en
sous-processus et lit ce qu'il a écrit. Les cas positifs vérifient les VALEURS
(pourcentage du badge, décompte par crate) : un test qui n'asserterait que le
code de sortie passerait aussi avec un badge faux.

    python3 scripts/test_couverture_badge.py
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "couverture_badge.py")


def enregistrement(chemin: str, lf: int, lh: int) -> str:
    return f"SF:{chemin}\nDA:1,1\nLF:{lf}\nLH:{lh}\nend_of_record\n"


class Badge(unittest.TestCase):
    def lancer(self, lcov: str, total: str) -> tuple[subprocess.CompletedProcess, str]:
        d = tempfile.mkdtemp()
        rapport = os.path.join(d, "lcov.info")
        with open(rapport, "w", encoding="utf-8") as f:
            f.write(lcov)
        sortie = os.path.join(d, "site")
        r = subprocess.run(
            [sys.executable, SCRIPT, rapport, "--total", total, "--commit", "0123456789abcdef", "--date", "2026-11-23", "--sortie", sortie],
            capture_output=True,
            text=True,
        )
        return r, sortie

    def test_badge_porte_le_total_de_llvm_cov(self):
        lcov = enregistrement("/w/crates/obrain-core/src/a.rs", 100, 80)
        r, sortie = self.lancer(lcov, "84.12%")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(os.path.join(sortie, "couverture.json"), encoding="utf-8") as f:
            b = json.load(f)
        self.assertEqual(b, {"schemaVersion": 1, "label": "couverture", "message": "84,1 %", "color": "orange"})

    def test_detail_par_crate_additionne_les_fichiers(self):
        lcov = (
            enregistrement("/w/crates/obrain-core/src/a.rs", 100, 80)
            + enregistrement("/w/crates/obrain-core/src/b.rs", 50, 50)
            + enregistrement("/w/crates/obrain-engine/src/c.rs", 200, 100)
        )
        r, sortie = self.lancer(lcov, "76.9")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(os.path.join(sortie, "index.html"), encoding="utf-8") as f:
            page = f.read()
        # obrain-core : 150 lignes, 130 couvertes ; obrain-engine : 200, 100.
        self.assertIn("<code>obrain-core</code></td><td class=n>150</td><td class=n>130</td><td class=n>86,7 %</td>", page)
        self.assertIn("<code>obrain-engine</code></td><td class=n>200</td><td class=n>100</td><td class=n>50,0 %</td>", page)
        # Tri croissant : la crate la plus basse d'abord.
        self.assertLess(page.index("obrain-engine"), page.index("obrain-core"))
        # Total recalculé depuis lcov : 230 / 350 = 65,7 %, séparateur de milliers absent sous 1000.
        self.assertIn("65,7 % (230 / 350 lignes)", page)

    def test_milliers_et_virgule_decimale_ne_se_melangent_pas(self):
        lcov = enregistrement("/w/crates/obrain-substrate/src/z.rs", 56696, 51480)
        r, sortie = self.lancer(lcov, "90.8")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(os.path.join(sortie, "index.html"), encoding="utf-8") as f:
            page = f.read()
        self.assertIn("56 696", page)
        self.assertIn("90,8 %", page)

    def test_couleurs(self):
        for total, attendu in (("69.9", "red"), ("70", "orange"), ("85", "yellow"), ("94.99", "yellow"), ("95", "brightgreen")):
            r, sortie = self.lancer(enregistrement("/w/crates/x/src/a.rs", 10, 5), total)
            self.assertEqual(r.returncode, 0, r.stderr)
            with open(os.path.join(sortie, "couverture.json"), encoding="utf-8") as f:
                self.assertEqual(json.load(f)["color"], attendu, total)

    def test_refus_rapport_vide(self):
        r, sortie = self.lancer("", "84.12%")
        self.assertEqual(r.returncode, 1)
        self.assertIn("aucune ligne", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(sortie, "couverture.json")))

    def test_refus_total_illisible_ou_hors_bornes(self):
        for total in ("-", "abc", "101", "84,12"):
            r, sortie = self.lancer(enregistrement("/w/crates/x/src/a.rs", 10, 5), total)
            self.assertEqual(r.returncode, 1, total)
            self.assertFalse(os.path.exists(os.path.join(sortie, "couverture.json")), total)

    def test_commit_echappe_dans_la_page(self):
        lcov = enregistrement("/w/crates/x/src/a.rs", 10, 5)
        d = tempfile.mkdtemp()
        rapport = os.path.join(d, "lcov.info")
        with open(rapport, "w", encoding="utf-8") as f:
            f.write(lcov)
        sortie = os.path.join(d, "site")
        r = subprocess.run(
            [sys.executable, SCRIPT, rapport, "--total", "50", "--commit", "<script>", "--date", "x", "--sortie", sortie],
            capture_output=True,
            text=True,
        )
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(os.path.join(sortie, "index.html"), encoding="utf-8") as f:
            self.assertNotIn("<script>", f.read())


if __name__ == "__main__":
    unittest.main(verbosity=2)
