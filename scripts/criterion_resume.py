#!/usr/bin/env python3
"""Résume une sortie Criterion en tableau markdown (résumé de run GitHub).

Criterion écrit, pour chaque bench, `<racine>/<identifiant>/new/estimates.json`
(pente et médiane, en nanosecondes). Le dossier `base/` porte la mesure
précédente du même arbre : la colonne « vs précédent » donne leur rapport.

Usage :
    python3 scripts/criterion_resume.py target/criterion >> "$GITHUB_STEP_SUMMARY"

Un dossier vide n'est pas une erreur : le run affiche alors une ligne qui le dit,
plutôt qu'un tableau muet qu'on prendrait pour un résultat.
"""

from __future__ import annotations

import json
import pathlib
import sys


def duree(ns: float) -> str:
    """Une durée en nanosecondes, dans l'unité lisible la plus proche."""
    for seuil, unite in ((1e9, "s"), (1e6, "ms"), (1e3, "µs")):
        if ns >= seuil:
            return f"{ns / seuil:.3g} {unite}"
    return f"{ns:.0f} ns"


def medianes(racine: pathlib.Path) -> list[tuple[str, float, float | None]]:
    """(identifiant, valeur `new`, valeur `base`) pour chaque bench mesuré."""
    trouves = []
    for fichier in sorted(racine.rglob("estimates.json")):
        # <identifiant>/new/estimates.json ou <identifiant>/base/estimates.json
        if fichier.parent.name not in ("new", "base"):
            continue
        identifiant = fichier.parent.parent.relative_to(racine).as_posix()
        with fichier.open(encoding="utf-8") as fh:
            donnees = json.load(fh)
        # Criterion imprime la PENTE (temps par itération), pas la médiane :
        # sur un bench bruité les deux diffèrent d'un facteur (mesuré le
        # 22/11/2026 : 123,48 ms de pente contre 75,86 ms de médiane).
        valeur = None
        for cle in ("slope", "median", "mean"):
            # Une entrée peut valoir `null` (pente non calculée) : on descend.
            valeur = (donnees.get(cle) or {}).get("point_estimate")
            if valeur is not None:
                break
        if valeur is None:
            continue
        trouves.append((fichier.parent.name, identifiant, float(valeur)))

    par_bench: dict[str, dict[str, float]] = {}
    for genre, identifiant, valeur in trouves:
        par_bench.setdefault(identifiant, {})[genre] = valeur
    lignes = []
    for identifiant, valeurs in sorted(par_bench.items()):
        if "new" not in valeurs:
            continue
        base = valeurs.get("base")
        # Criterion recopie `new` dans `base` quand aucune mesure antérieure
        # n'existe : deux valeurs égales à la neuvième décimale ne sont pas un
        # passage précédent, c'est le même. On n'affiche alors aucune
        # comparaison plutôt qu'un « identique » trompeur.
        if base is not None and base == valeurs["new"]:
            base = None
        lignes.append((identifiant, valeurs["new"], base))
    return lignes


def tableau(lignes: list[tuple[str, float, float | None]]) -> str:
    """Le tableau markdown, avec une colonne de comparaison si `base/` existe."""
    if not lignes:
        return "Aucune mesure Criterion trouvée : le run n'a rien produit à lire.\n"
    avec_base = any(base is not None for _, _, base in lignes)
    entete = ["| Bench | Temps/itération |", "|---|---|"]
    if avec_base:
        entete = ["| Bench | Temps/itération | vs précédent |", "|---|---|---|"]
    sortie = entete[:]
    for identifiant, mediane, base in lignes:
        cellule = f"| `{identifiant}` | {duree(mediane)} |"
        if avec_base:
            if base:
                rapport = mediane / base
                verdict = "identique" if 0.97 <= rapport <= 1.03 else f"×{rapport:.2f}"
                cellule += f" {verdict} |"
            else:
                cellule += " — |"
        sortie.append(cellule)
    sortie.append("")
    sortie.append(
        "Ces chiffres sont ceux de la machine du run : un runner partagé ne "
        "mesure pas. Ils servent de tendance, jamais de référence."
    )
    return "\n".join(sortie) + "\n"


def main(argv: list[str]) -> int:
    racine = pathlib.Path(argv[1] if len(argv) > 1 else "target/criterion")
    if not racine.is_dir():
        print(f"Aucun dossier Criterion à {racine} : rien à résumer.", file=sys.stderr)
        return 0
    print(tableau(medianes(racine)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
