#!/usr/bin/env python3
"""Contrôle de couverture du code neuf d'une PR (CI, job « Couverture »).

Le trou que ce script ferme
---------------------------
`diff-cover` ne compte que l'intersection entre les lignes du diff et les lignes
`DA:` du rapport lcov. Ce qui n'est pas dans le rapport n'est ni couvert ni
non couvert : c'est ignoré, et un diff entièrement non mesuré sort à 100 %.
Reproduit le 21/11/2026 : huit lignes jamais exécutées ajoutées à
`obrain-adapters/src/query/cypher/parser.rs`, rapport mesuré sans la feature
`cypher` -> `diff-cover` sort en 0 (« No lines with coverage information »).
Deux variantes du même trou :
  - le FICHIER est absent du rapport (feature inactive pendant toute la mesure) ;
  - le fichier est présent, mais la FONCTION NEUVE est derrière une feature ou
    une plateforme inactive : `SF:` existe, aucune ligne neuve en `DA:`.

Ce que le script vérifie (échec dur)
------------------------------------
Périmètre : `crates/<crate>/src/**/*.rs`, hors bindings, obrain-web, tests,
benches, exemples (mêmes exclusions que la mesure `cargo llvm-cov`).

1. FICHIER NON MESURÉ. Fichier du périmètre modifié, qui ajoute du code (hors
   commentaires et lignes vides) et contient des fonctions, mais absent du
   rapport => échec.
2. FONCTION NEUVE NON MESURÉE. Une ligne ajoutée qui définit une fonction à corps
   (`fn nom`, sans `;` final) sans enregistrement `FN:` du rapport à cette ligne
   => échec. Le rapport lcov de `cargo llvm-cov` porte un `FN:<ligne>,<nom>` pour
   chaque fonction compilée ; absent, la fonction n'a pas été compilée pendant la
   mesure (feature, plateforme) — ou bien elle est générique et jamais
   instanciée, ou `const fn` évaluée seulement à la compilation : dans ces cas
   aussi, aucun test ne l'exécute. Couvre les changements MIXTES : une ligne
   mesurée et couverte à côté d'une fonction non mesurée ne masque pas celle-ci.
3. SEUIL. Parmi les lignes ajoutées que le rapport connaît (une ligne `DA:`),
   la part couverte doit atteindre `--fail-under` (95 par défaut). Même
   définition que `diff-cover`, calculée ici : `diff-cover` rapproche les chemins
   du rapport de ceux du diff et, faute de rapprochement (lien symbolique,
   chemin de runner), répond « No lines with coverage information » et sort en 0
   — le même silence que le trou ci-dessus, observé pendant la mise au point de
   ce script. Un contrôle qui peut se taire sans le dire n'en est pas un.

Exemptions : fichier `.github/couverture-exemptions.txt`, une ligne
`chemin  # raison`, relue en PR comme le reste. Elle lève les contrôles 1 et 2
pour ce fichier ; le seuil 3 s'applique toujours aux lignes mesurées.

Ce que le script NE vérifie PAS (limite assumée, comptée à chaque run)
---------------------------------------------------------------------
Du code exécutable ajouté DANS une fonction déjà mesurée mais sous une
compilation conditionnelle inactive (`#[cfg(windows)] { ... }`, bras de `match`
sous feature) n'a pas de `DA:` et n'est pas rattaché à un `FN:` neuf. Le script
ne peut pas le distinguer d'une ligne non exécutable sans réimplémenter rustc :
il COMPTE ces lignes par fichier (« lignes exécutables sans donnée de
couverture »), les annote en avertissement GitHub, et n'échoue pas dessus. La
revue humaine reste le contrôle de ce cas.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from collections import defaultdict

EXCLUS = ("/bindings/", "/obrain-web/", "/tests/", "/benches/", "/examples/")
FN_DEF = re.compile(r"\bfn\s+\w")
HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")

# Lignes qui n'ont jamais de donnée de couverture : on ne les compte pas comme
# « exécutables sans donnée ».
NON_EXECUTABLE = [
    re.compile(r"^[}\])]+[;,]?$"),  # fermetures
    re.compile(r"^\{$"),  # accolade ouvrante seule
    re.compile(r"^#!?\["),  # attributs
    re.compile(
        r"^(pub(\([^)]*\))?\s+)?(use|mod|const|static|type|struct|enum|trait|union|extern|macro_rules!)\b"
    ),
    re.compile(r"^(pub(\([^)]*\))?\s+)?(unsafe\s+)?(impl|trait)\b.*\{$"),
    re.compile(r"^(\}\s*)?else\s*\{?$"),
    re.compile(r"^(pub(\([^)]*\))?\s+)?[A-Za-z_]\w*\s*:\s*[^=;{}]+,$"),  # champ de struct
    re.compile(r"^[A-Z]\w*(\s*\([^)]*\)|\s*\{[^}]*\})?\s*(=\s*[^,]+)?,$"),  # variante d'enum
    re.compile(r"^(pub(\([^)]*\))?\s+)?(async\s+)?(unsafe\s+)?(const\s+)?fn\b.*;$"),  # déclaration
]


def dans_le_perimetre(chemin: str) -> bool:
    if not chemin.endswith(".rs") or not chemin.startswith("crates/"):
        return False
    avec_barre = "/" + chemin
    if "/src/" not in avec_barre:
        return False
    return not any(x in avec_barre for x in EXCLUS)


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout


class Rapport:
    """Ce que le lcov sait, par fichier : lignes `DA:` et débuts de fonction `FN:`."""

    def __init__(self) -> None:
        self.da: dict[str, dict[int, int]] = defaultdict(dict)
        self.fn: dict[str, set[int]] = defaultdict(set)
        self.fichiers: set[str] = set()

    def lire(self, lcov: str, racine: str) -> None:
        courant = None
        with open(lcov, encoding="utf-8") as f:
            for ligne in f:
                if ligne.startswith("SF:"):
                    sf = ligne[3:].strip()
                    if os.path.isabs(sf):
                        # realpath des deux côtés : /var -> /private/var (macOS), un
                        # workspace sous lien symbolique, etc.
                        sf = os.path.relpath(os.path.realpath(sf), os.path.realpath(racine))
                    courant = sf.replace(os.sep, "/")
                    self.fichiers.add(courant)
                elif courant and ligne.startswith("DA:"):
                    n, hits = ligne[3:].strip().split(",")[:2]
                    # Plusieurs profils fusionnés : une ligne couverte dans l'un l'est.
                    self.da[courant][int(n)] = max(self.da[courant].get(int(n), 0), int(hits))
                elif courant and ligne.startswith("FN:"):
                    self.fn[courant].add(int(ligne[3:].split(",", 1)[0]))


def exemptions(chemin: str) -> set[str]:
    if not chemin or not os.path.exists(chemin):
        return set()
    sortie = set()
    with open(chemin, encoding="utf-8") as f:
        for ligne in f:
            ligne = ligne.split("#", 1)[0].strip()
            if ligne:
                sortie.add(ligne)
    return sortie


def lignes_ajoutees(base: str, fichier: str) -> list[tuple[int, str]]:
    """(numéro dans le fichier neuf, texte) des lignes de code ajoutées :
    ni vides ni commentaires."""
    diff = git("diff", "-U0", "--no-color", f"{base}...HEAD", "--", fichier)
    sortie: list[tuple[int, str]] = []
    n = 0
    for ligne in diff.splitlines():
        m = HUNK.match(ligne)
        if m:
            n = int(m.group(1))
            continue
        if ligne.startswith("+++") or ligne.startswith("---"):
            continue
        if ligne.startswith("+"):
            texte = ligne[1:].strip()
            if texte and not texte.startswith("//"):
                sortie.append((n, texte))
            n += 1
    return sortie


def definit_une_fonction_a_corps(texte: str) -> bool:
    return bool(FN_DEF.search(texte)) and not texte.endswith(";")


def ressemble_a_du_code_executable(texte: str) -> bool:
    return not any(p.match(texte) for p in NON_EXECUTABLE) and not definit_une_fonction_a_corps(texte)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("lcov", nargs="+", help="rapport(s) lcov (profils fusionnés)")
    p.add_argument("--base", required=True, help="branche de base, ex. origin/main")
    p.add_argument("--fail-under", type=float, default=95.0)
    p.add_argument("--exemptions", default=".github/couverture-exemptions.txt")
    p.add_argument("--markdown-report", default="", help="écrit le détail (Markdown) dans ce fichier")
    args = p.parse_args()

    racine = git("rev-parse", "--show-toplevel").strip()
    os.chdir(racine)

    rapport = Rapport()
    for fichier_lcov in args.lcov:
        rapport.lire(fichier_lcov, racine)
    exemptes = exemptions(args.exemptions)

    modifies = [
        f
        for f in git("diff", "--name-only", "--diff-filter=AMR", f"{args.base}...HEAD").splitlines()
        if dans_le_perimetre(f)
    ]

    non_mesures: list[str] = []
    fonctions_non_mesurees: list[tuple[str, int, str]] = []
    sans_donnee: dict[str, list[int]] = {}
    mesurees = 0
    couvertes = 0
    non_couvertes: dict[str, list[int]] = {}

    for f in modifies:
        ajoutees = lignes_ajoutees(args.base, f)
        if not ajoutees:
            continue
        if f not in rapport.fichiers:
            if f in exemptes:
                continue
            try:
                with open(f, encoding="utf-8") as fh:
                    if FN_DEF.search(fh.read()):
                        non_mesures.append(f)
            except OSError:
                pass
            continue
        for n, texte in ajoutees:
            est_fn = definit_une_fonction_a_corps(texte)
            if est_fn and f not in exemptes and n not in rapport.fn[f]:
                fonctions_non_mesurees.append((f, n, texte))
            # La ligne de signature compte aussi dans le seuil quand elle a une donnée.
            if n in rapport.da[f]:
                mesurees += 1
                if rapport.da[f][n] > 0:
                    couvertes += 1
                else:
                    non_couvertes.setdefault(f, []).append(n)
            elif not est_fn and ressemble_a_du_code_executable(texte):
                sans_donnee.setdefault(f, []).append(n)

    echec = False
    print(f"Fichiers du périmètre modifiés : {len(modifies)}")

    if non_mesures:
        echec = True
        print("\nÉCHEC — fichiers modifiés que le rapport de couverture ne mesure pas "
              "(feature non activée pendant la mesure, ou fichier non compilé) :")
        for f in non_mesures:
            print(f"  - {f}")
    if fonctions_non_mesurees:
        echec = True
        print("\nÉCHEC — fonctions ajoutées absentes du rapport (non compilées pendant la "
              "mesure : feature ou plateforme inactive, générique jamais instanciée, "
              "const fn évaluée à la compilation) :")
        for f, n, texte in fonctions_non_mesurees:
            print(f"  - {f}:{n}  {texte[:100]}")
            if os.environ.get("GITHUB_ACTIONS"):
                print(f"::error file={f},line={n}::fonction neuve non mesurée par la couverture")
    if non_mesures or fonctions_non_mesurees:
        print("Soit la mesure doit les couvrir (activer la feature dans le job), soit "
              f"lister le fichier, avec la raison, dans {args.exemptions}. "
              "Une fonction non mesurée n'est jamais « couverte ».")

    total = sum(len(v) for v in sans_donnee.values())
    print(f"\nLignes exécutables ajoutées sans donnée de couverture : {total} "
          "(non vérifiées par ce contrôle — limite documentée en tête du script)")
    for f, lignes in sans_donnee.items():
        print(f"  - {f} : {len(lignes)} (lignes {', '.join(map(str, lignes[:12]))}"
              f"{'…' if len(lignes) > 12 else ''})")
        if os.environ.get("GITHUB_ACTIONS"):
            for n in lignes[:10]:
                print(f"::warning file={f},line={n}::ligne exécutable sans donnée de couverture "
                      "(cfg inactif pendant la mesure ?) — non vérifiée")

    if mesurees:
        pct = couvertes * 100.0 / mesurees
        print(f"\nCode neuf mesuré : {couvertes}/{mesurees} lignes couvertes ({pct:.1f} %), "
              f"seuil {args.fail_under:g} %")
        for f, lignes in non_couvertes.items():
            print(f"  - {f} : {len(lignes)} non couvertes (lignes "
                  f"{', '.join(map(str, lignes[:12]))}{'…' if len(lignes) > 12 else ''})")
            if os.environ.get("GITHUB_ACTIONS"):
                for n in lignes[:10]:
                    print(f"::error file={f},line={n}::ligne neuve non couverte par les tests")
        if pct < args.fail_under:
            echec = True
            print(f"\nÉCHEC — code neuf sous {args.fail_under:g} % de couverture "
                  f"({couvertes}/{mesurees} lignes mesurées).")
    else:
        print("\nCode neuf mesuré : aucune ligne ajoutée n'a de donnée de couverture "
              "(ni couverte ni non couverte) — rien à comparer au seuil.")

    if args.markdown_report:
        with open(args.markdown_report, "w", encoding="utf-8") as out:
            out.write("### Code neuf\n\n")
            if mesurees:
                out.write(f"{couvertes}/{mesurees} lignes mesurées couvertes "
                          f"({couvertes * 100.0 / mesurees:.1f} %), seuil {args.fail_under:g} %.\n\n")
            for f, lignes in non_couvertes.items():
                out.write(f"- `{f}` : non couvertes, lignes {', '.join(map(str, lignes))}\n")
            for f, n, texte in fonctions_non_mesurees:
                out.write(f"- `{f}:{n}` : fonction neuve non mesurée (`{texte[:80]}`)\n")
            for f in non_mesures:
                out.write(f"- `{f}` : fichier modifié non mesuré\n")
            out.write(f"\nLignes exécutables ajoutées sans donnée de couverture (non vérifiées) : {total}.\n")

    return 1 if echec else 0


if __name__ == "__main__":
    sys.exit(main())
