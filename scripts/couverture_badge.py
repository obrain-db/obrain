#!/usr/bin/env python3
"""Badge et page de couverture publiés sur GitHub Pages (CI, job « Couverture »).

Pourquoi ce script
------------------
Le README affichait un badge STATIQUE (« couverture : cible 95 % ») et une
section de chiffres recopiés à la main le 21/11/2026, déjà faux deux jours plus
tard (plancher annoncé 85, CI à 70 ; outil annoncé 0.8.5, CI en 0.9.1). Un
chiffre qu'un humain recopie vieillit. Ce script écrit, à chaque CI de `main`,
ce que la mesure vient de produire, et rien d'autre :

  - `couverture.json` : badge au format endpoint de shields.io
    (https://shields.io/badges/endpoint-badge), lu par le README ;
  - `index.html`      : le total, le commit mesuré et le détail par crate.

Sources (aucune n'est recalculée ici)
-------------------------------------
  - le TOTAL vient de `cargo llvm-cov report --summary-only` (colonne « Lines »
    de la ligne TOTAL), passé par `--total` : c'est le même nombre que le résumé
    du job et que le contrôle `--fail-under-lines` du plancher ;
  - le DÉTAIL PAR CRATE vient du rapport lcov fusionné (`LF:` lignes mesurées,
    `LH:` lignes couvertes, par fichier `SF:`), déjà filtré par
    `COUVERTURE_EXCLUS`. Le total recalculé depuis lcov est affiché à côté, pour
    qu'un écart entre les deux sources se voie au lieu de se cacher.

Refus (sortie 1, rien n'est écrit)
----------------------------------
  - rapport lcov sans aucune ligne mesurée : un badge à « 0 % » ou « 100 % »
    publié sur une mesure vide serait un mensonge ;
  - `--total` illisible (ni « 84.12% » ni « 84.12 »), ou hors de [0, 100].

    python3 scripts/couverture_badge.py lcov.info --total 84.12% \\
        --commit <sha> --date 2026-11-23 --sortie site/

Bibliothèque standard seule. Testé par `scripts/test_couverture_badge.py`.
"""

from __future__ import annotations

import argparse
import html
import json
import os
import re
import sys

CIBLE = 95.0

# Seuils de couleur : rouge sous 70 (plancher actuel de la CI), orange sous 85,
# jaune sous la cible de 95, vert à partir de la cible.
SEUILS = ((70.0, "red"), (85.0, "orange"), (CIBLE, "yellow"))

CRATE = re.compile(r"/crates/((?:bindings/)?[^/]+)/")


def couleur(pourcentage: float) -> str:
    for seuil, nom in SEUILS:
        if pourcentage < seuil:
            return nom
    return "brightgreen"


def lire_total(texte: str) -> float:
    m = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*%?\s*", texte)
    if not m:
        raise ValueError(f"total illisible : « {texte} »")
    valeur = float(m.group(1))
    if not 0.0 <= valeur <= 100.0:
        raise ValueError(f"total hors de [0, 100] : {valeur}")
    return valeur


def lire_lcov(chemin: str) -> dict[str, list[int]]:
    """Rend {crate: [lignes mesurées, lignes couvertes]}."""
    par_crate: dict[str, list[int]] = {}
    crate = None
    with open(chemin, encoding="utf-8") as f:
        for ligne in f:
            ligne = ligne.strip()
            if ligne.startswith("SF:"):
                m = CRATE.search(ligne[3:].replace("\\", "/"))
                crate = m.group(1) if m else "(hors crates/)"
                par_crate.setdefault(crate, [0, 0])
            elif ligne.startswith("LF:") and crate is not None:
                par_crate[crate][0] += int(ligne[3:])
            elif ligne.startswith("LH:") and crate is not None:
                par_crate[crate][1] += int(ligne[3:])
            elif ligne == "end_of_record":
                crate = None
    return par_crate


def fr(pourcentage: float) -> str:
    return f"{pourcentage:.1f}".replace(".", ",") + " %"


def milliers(n: int) -> str:
    """12345 -> « 12 345 » (espace fine insécable, typographie française)."""
    return f"{n:,}".replace(",", " ")


def badge(total: float) -> dict:
    return {
        "schemaVersion": 1,
        "label": "couverture",
        "message": fr(total),
        "color": couleur(total),
    }


def page(total: float, par_crate: dict[str, list[int]], commit: str, date: str) -> str:
    mesurees = sum(v[0] for v in par_crate.values())
    couvertes = sum(v[1] for v in par_crate.values())
    lignes = []
    for crate, (lf, lh) in sorted(par_crate.items(), key=lambda kv: (kv[1][1] / kv[1][0]) if kv[1][0] else 0.0):
        if lf == 0:
            continue
        p = 100.0 * lh / lf
        lignes.append(
            f"<tr><td><code>{html.escape(crate)}</code></td>"
            f"<td class=n>{milliers(lf)}</td><td class=n>{milliers(lh)}</td><td class=n>{fr(p)}</td></tr>"
        )
    lcov_total = 100.0 * couvertes / mesurees
    c = html.escape(commit)
    lien = f"https://github.com/obrain-db/obrain/commit/{c}"
    return f"""<!doctype html>
<html lang="fr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Couverture obrain</title>
<style>
:root {{ --fond: #ffffff; --texte: #1f2328; --pale: #59636e; --trait: #d1d9e0; }}
@media (prefers-color-scheme: dark) {{ :root {{ --fond: #0d1117; --texte: #e6edf3; --pale: #9198a1; --trait: #3d444d; }} }}
body {{ background: var(--fond); color: var(--texte); font: 15px/1.5 system-ui, sans-serif; margin: 0 auto; max-width: 760px; padding: 24px 16px; }}
h1 {{ font-size: 1.4em; margin: 0 0 4px; }}
p {{ color: var(--pale); margin: 4px 0 16px; }}
.total {{ font-size: 2.4em; font-weight: 600; color: var(--texte); }}
table {{ border-collapse: collapse; width: 100%; }}
th, td {{ border-bottom: 1px solid var(--trait); padding: 6px 8px; text-align: left; }}
.n {{ text-align: right; font-variant-numeric: tabular-nums; }}
a {{ color: inherit; }}
</style>
</head>
<body>
<h1>Couverture de code — obrain</h1>
<p><span class="total">{fr(total)}</span> des lignes (cible {fr(CIBLE)}).</p>
<p>Mesuré par le job « Couverture » de la CI sur <a href="{lien}"><code>{c[:8]}</code></a> (<code>main</code>), le {html.escape(date)} : <code>cargo llvm-cov</code>, profils fusionnés (features par défaut + toutes features), hors bindings, <code>obrain-web</code>, tests, benches et exemples.</p>
<table>
<thead><tr><th>Crate</th><th class=n>Lignes mesurées</th><th class=n>Couvertes</th><th class=n>Couverture</th></tr></thead>
<tbody>
{chr(10).join(lignes)}
</tbody>
</table>
<p>Total recalculé depuis le rapport lcov : {fr(lcov_total)} ({milliers(couvertes)} / {milliers(mesurees)} lignes). Le total en tête vient de <code>llvm-cov report --summary-only</code>, comme le plancher de la CI.</p>
</body>
</html>
"""


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("lcov")
    ap.add_argument("--total", required=True, help="ligne TOTAL de llvm-cov, colonne Lines (ex. 84.12%%)")
    ap.add_argument("--commit", required=True)
    ap.add_argument("--date", required=True)
    ap.add_argument("--sortie", required=True)
    a = ap.parse_args(argv)

    try:
        total = lire_total(a.total)
    except ValueError as e:
        print(f"couverture_badge : {e}", file=sys.stderr)
        return 1
    par_crate = lire_lcov(a.lcov)
    if sum(v[0] for v in par_crate.values()) == 0:
        print("couverture_badge : le rapport lcov ne mesure aucune ligne — rien n'est publié", file=sys.stderr)
        return 1

    os.makedirs(a.sortie, exist_ok=True)
    with open(os.path.join(a.sortie, "couverture.json"), "w", encoding="utf-8") as f:
        json.dump(badge(total), f, ensure_ascii=False)
        f.write("\n")
    with open(os.path.join(a.sortie, "index.html"), "w", encoding="utf-8") as f:
        f.write(page(total, par_crate, a.commit, a.date))
    print(f"couverture_badge : {fr(total)} ({couleur(total)}), {len(par_crate)} crates -> {a.sortie}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
