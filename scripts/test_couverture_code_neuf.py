#!/usr/bin/env python3
"""Tests du contrôle `couverture_code_neuf.py`, dont le test négatif de fond :
une fonction ajoutée, non couverte, sous une feature ou une plateforme inactive,
FAIT ÉCHOUER le contrôle — que le fichier soit absent du rapport, ou présent
(`SF:`) sans que la fonction neuve n'y figure (`FN:`/`DA:`).

Chaque cas fabrique un dépôt git jetable (branche `base`, puis un commit de PR)
et un rapport lcov synthétique, puis lance le vrai script (bibliothèque standard
seule, aucune installation).

Les cas « couvert » et « zéro passage » vérifient le DÉCOMPTE de lignes mesurées
(`5/5`, `0/5`) : un test qui n'asserterait que le code de sortie passerait aussi
quand le contrôle ne voit rien — le défaut même qu'on veut exclure.

    python3 scripts/test_couverture_code_neuf.py

La LIMITE CONNUE est testée, pas cachée : voir
`test_limite_connue_bloc_cfg_dans_fonction_existante`.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "couverture_code_neuf.py")
CHEMIN_CYPHER = "crates/obrain-adapters/src/query/cypher/parser.rs"

BASE = textwrap.dedent(
    """\
    pub fn ancienne(x: u32) -> u32 {
        x + 1
    }
    """
)

FONCTION_NEUVE = textwrap.dedent(
    """\

    #[cfg(feature = "cypher")]
    pub fn nouvelle(n: u32) -> u32 {
        let a = n + 1;
        let b = a * 2;
        if b > 10 { b - 10 } else { b + 10 }
    }
    """
)


def sh(cwd: str, *cmd: str) -> None:
    subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True)


def ligne_de(contenu: str, motif: str) -> int:
    """Numéro (à partir de 1) de la première ligne qui contient `motif`."""
    for i, t in enumerate(contenu.splitlines(), 1):
        if motif in t:
            return i
    raise AssertionError(f"motif absent : {motif}")


class Depot:
    """Dépôt git jetable : un fichier de base, puis la PR qui le modifie."""

    def __init__(self, chemin_fichier: str, base: str, apres: str):
        self.dir = os.path.realpath(tempfile.mkdtemp(prefix="couverture-test-"))
        self.fichier = chemin_fichier
        self.apres = apres
        sh(self.dir, "git", "init", "-q", "-b", "base")
        sh(self.dir, "git", "config", "user.email", "t@example.org")
        sh(self.dir, "git", "config", "user.name", "t")
        self._ecrire(base)
        sh(self.dir, "git", "add", "-A")
        sh(self.dir, "git", "commit", "-q", "-m", "base")
        sh(self.dir, "git", "checkout", "-q", "-b", "pr")
        self._ecrire(apres)
        sh(self.dir, "git", "add", "-A")
        sh(self.dir, "git", "commit", "-q", "-m", "pr")

    def _ecrire(self, contenu: str) -> None:
        chemin = os.path.join(self.dir, self.fichier)
        os.makedirs(os.path.dirname(chemin), exist_ok=True)
        with open(chemin, "w", encoding="utf-8") as f:
            f.write(contenu)

    def lcov(self, da: dict[int, int] | None, fn: list[int] | None = None) -> str:
        """Écrit un rapport lcov. `da=None` : le fichier est ABSENT du rapport.
        Sinon le fichier y figure avec ces `DA:` (ligne -> passages) et ces `FN:`."""
        chemin = os.path.join(self.dir, "lcov.info")
        with open(chemin, "w", encoding="utf-8") as f:
            f.write("TN:\n")
            # Un autre fichier mesuré, pour que le rapport ne soit jamais vide.
            f.write(f"SF:{os.path.join(self.dir, 'crates/obrain-core/src/autre.rs')}\n")
            f.write("FN:1,autre\nFNDA:1,autre\nDA:1,1\nend_of_record\n")
            if da is not None:
                f.write(f"SF:{os.path.join(self.dir, self.fichier)}\n")
                for n in fn or []:
                    f.write(f"FN:{n},fn{n}\nFNDA:1,fn{n}\n")
                for n, hits in da.items():
                    f.write(f"DA:{n},{hits}\n")
                f.write("end_of_record\n")
        return chemin

    def controle(self, lcov: str, exemptions: str | None = None) -> subprocess.CompletedProcess:
        cmd = [sys.executable, SCRIPT, lcov, "--base", "base", "--fail-under", "95",
               "--exemptions", exemptions or os.path.join(self.dir, "aucune-exemption.txt")]
        return subprocess.run(cmd, cwd=self.dir, capture_output=True, text=True)

    def nettoyer(self) -> None:
        shutil.rmtree(self.dir, ignore_errors=True)


class ControleCodeNeuf(unittest.TestCase):
    def depot(self, chemin: str = CHEMIN_CYPHER, base: str = BASE, apres: str | None = None) -> Depot:
        d = Depot(chemin, base, base + FONCTION_NEUVE if apres is None else apres)
        self.addCleanup(d.nettoyer)
        return d

    def sortie(self, r: subprocess.CompletedProcess) -> str:
        return r.stdout + r.stderr

    # ── 1. le trou d'origine : fichier absent du rapport ─────────────────────
    def test_fichier_sous_feature_inactive_absent_du_rapport_echoue(self):
        d = self.depot()
        r = d.controle(d.lcov(None))
        self.assertNotEqual(r.returncode, 0, self.sortie(r))
        self.assertIn("ne mesure pas", r.stdout)
        self.assertIn(CHEMIN_CYPHER, r.stdout)

    # ── 2. variante demandée : fichier PRÉSENT, fonction neuve non compilée ──
    def test_fichier_mesure_mais_fonction_neuve_sous_feature_inactive_echoue(self):
        d = self.depot()
        # Le fichier est dans le rapport avec ses lignes anciennes (1-3) ;
        # la fonction neuve (derrière `cfg(feature = "cypher")`) n'a ni FN ni DA.
        r = d.controle(d.lcov({1: 1, 2: 1, 3: 1}, fn=[1]))
        self.assertNotEqual(r.returncode, 0, self.sortie(r))
        self.assertIn("fonctions ajoutées absentes du rapport", r.stdout)
        self.assertIn("nouvelle", r.stdout)

    # ── 3. changement MIXTE : du mesuré couvert ne masque pas du non mesuré ──
    def test_changement_mixte_ligne_mesuree_couverte_et_fonction_non_mesuree_echoue(self):
        apres = BASE.replace("    x + 1\n", "    let y = x + 1;\n    y\n") + FONCTION_NEUVE
        d = self.depot(apres=apres)
        # Les lignes modifiées de `ancienne` (2-3) sont couvertes ; `nouvelle` n'est nulle part.
        r = d.controle(d.lcov({1: 5, 2: 5, 3: 5, 4: 5}, fn=[1]))
        self.assertNotEqual(r.returncode, 0, self.sortie(r))
        self.assertIn("nouvelle", r.stdout)

    def test_changement_mixte_tout_mesure_et_couvert_passe(self):
        apres = BASE.replace("    x + 1\n", "    let y = x + 1;\n    y\n") + FONCTION_NEUVE
        d = self.depot(apres=apres)
        debut = ligne_de(apres, "pub fn nouvelle")
        da = {1: 5, 2: 5, 3: 5, 4: 5, **{n: 3 for n in range(debut, debut + 5)}}
        r = d.controle(d.lcov(da, fn=[1, debut]))
        self.assertEqual(r.returncode, 0, self.sortie(r))

    # ── 4. mesuré mais jamais exécuté : le seuil de diff-cover ───────────────
    def test_fonction_neuve_mesuree_a_zero_passage_echoue(self):
        d = self.depot()
        debut = ligne_de(d.apres, "pub fn nouvelle")
        da = {1: 1, 2: 1, 3: 1, **{n: 0 for n in range(debut, debut + 5)}}
        r = d.controle(d.lcov(da, fn=[1, debut]))
        self.assertNotEqual(r.returncode, 0, self.sortie(r))
        self.assertIn("0/5 lignes couvertes", r.stdout)
        self.assertIn("sous 95", r.stdout)

    def test_fonction_neuve_mesuree_et_couverte_passe(self):
        d = self.depot()
        debut = ligne_de(d.apres, "pub fn nouvelle")
        da = {1: 1, 2: 1, 3: 1, **{n: 4 for n in range(debut, debut + 5)}}
        r = d.controle(d.lcov(da, fn=[1, debut]))
        self.assertEqual(r.returncode, 0, self.sortie(r))
        self.assertIn("5/5 lignes couvertes", r.stdout)

    # ── 5. ce qui ne doit PAS faire échouer à tort ───────────────────────────
    def test_lignes_non_executables_sans_donnee_ne_font_pas_echouer(self):
        # Dans un fichier mesuré : import, constante, type, champs, variantes, `}`.
        apres = BASE + textwrap.dedent(
            """\

            use std::fmt::Debug;
            pub const LIMITE: u32 = 10;
            pub struct Reglage {
                pub nom: String,
                pub valeur: u32,
            }
            pub enum Mode {
                Rapide,
                Lent(u32),
            }
            pub trait Ouvre {
                fn ouvre(&self) -> u32;
            }
            """
        )
        d = self.depot(apres=apres)
        r = d.controle(d.lcov({1: 1, 2: 1, 3: 1}, fn=[1]))
        self.assertEqual(r.returncode, 0, self.sortie(r))
        self.assertIn("sans donnée de couverture : 0", r.stdout)

    def test_commentaires_seuls_dans_un_fichier_non_mesure_passent(self):
        d = self.depot(apres=BASE + "\n// une note\n/// un doc\n")
        r = d.controle(d.lcov(None))
        self.assertEqual(r.returncode, 0, self.sortie(r))

    def test_fichier_sans_fonction_absent_du_rapport_passe(self):
        d = self.depot(
            base="pub const A: u32 = 1;\n",
            apres="pub const A: u32 = 1;\npub const B: u32 = 2;\npub struct S { pub x: u32 }\n",
        )
        r = d.controle(d.lcov(None))
        self.assertEqual(r.returncode, 0, self.sortie(r))

    def test_fichier_exempte_avec_raison_passe(self):
        d = self.depot()
        ex = os.path.join(d.dir, "exemptions.txt")
        with open(ex, "w", encoding="utf-8") as f:
            f.write(f"{CHEMIN_CYPHER}  # code compilé seulement pour wasm\n")
        for lcov in (d.lcov(None), d.lcov({1: 1, 2: 1, 3: 1}, fn=[1])):
            r = d.controle(lcov, exemptions=ex)
            self.assertEqual(r.returncode, 0, self.sortie(r))

    def test_fichier_hors_perimetre_passe(self):
        for chemin in (
            "crates/obrain-engine/tests/nouveau.rs",
            "crates/bindings/c/src/lib.rs",
            "crates/obrain-web/src/lib.rs",
            "crates/obrain-engine/benches/b.rs",
            "docs/exemple.rs",
        ):
            with self.subTest(chemin=chemin):
                d = self.depot(chemin=chemin)
                r = d.controle(d.lcov(None))
                self.assertEqual(r.returncode, 0, self.sortie(r))

    # ── 6. LIMITE CONNUE, testée et nommée ───────────────────────────────────
    def test_limite_connue_bloc_cfg_dans_fonction_existante(self):
        """Du code exécutable ajouté DANS une fonction déjà mesurée, sous un cfg
        inactif, n'a pas de `DA:` et n'est pas une fonction neuve : le contrôle ne
        peut pas échouer dessus. Il le COMPTE et le signale — et c'est tout."""
        apres = textwrap.dedent(
            """\
            pub fn ancienne(x: u32) -> u32 {
                #[cfg(windows)]
                {
                    let y = x * 7;
                    return y + 3;
                }
                x + 1
            }
            """
        )
        d = self.depot(apres=apres)
        # Seules les lignes de l'ancien chemin sont instrumentées.
        r = d.controle(d.lcov({1: 4, 7: 4, 8: 4}, fn=[1]))
        self.assertEqual(r.returncode, 0, self.sortie(r))
        self.assertIn("sans donnée de couverture : 2", r.stdout)
        self.assertIn("non vérifiées", r.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
