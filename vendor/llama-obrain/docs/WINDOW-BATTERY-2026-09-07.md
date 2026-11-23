# Batterie « rang local de fenêtre » — protocole corrigé — RÉSULTAT NÉGATIF (2026-09-07)

## Protocole (conforme à la critique utilisateur)
Sonde `ops-gpu16/window_probe.cpp` : capture h_l(t) sur 165 positions réelles (64 couches),
fenêtres du PASSÉ [t−L..t−1] uniquement, L ∈ {4,8,16,32} ; mesure de **l'erreur de
représentabilité du token suivant** (min_α ‖h_t − H_pastᵀα‖/‖h_t‖, base passée), fréquence de
rafraîchissement (frac err>ε) et dérive par profondeur. (Le rang ≤ L est trivial ; le signal
cherché = capacité à représenter les tokens SUIVANTS.)

## Résultats
| profondeur | L=4 méd | L=32 méd | frac err>10 % (L=32) |
|---|---|---|---|
| 0-15 | 0,273 | 0,190 | 0,82 |
| 16-31 | 0,477 | 0,364 | 1,00 |
| 32-47 | 0,531 | 0,442 | 1,00 |
| 48-63 | 0,685 | 0,540 | 1,00 |

Global : err médiane 0,42-0,50 (L=4-32) ; err > 5 % sur ~99-100 % des fenêtres ; err > 10 % sur
~96-100 % (sauf couches 0-15 : 82 % à L=32).

## Verdict (trois niveaux séparés)
1. Identité algébrique : non testée ici (WDT/GDN — nécessite k/v/g/β, séparé).
2. Approximation exploitable (FTC/IRT/SFM, structure fenêtre sur h_t) : **FALSIFIÉE** — le token
   suivant n'est PAS représentable par la fenêtre passée (erreur 0,2-0,7), rafraîchissement quasi
   à CHAQUE token → aucun gain de fenêtre. Confirme et généralise la mesure Δ (innovation par token
   grande ET hors du cône du passé récent).
3. Accélération modèle complet : aucun gain dérivable de cette famille sur ce modèle.

## Reste ouvert
- WDT (agent C) au niveau de la récurrence GDN : fermeture rang-r avec k/v/g/β réels, comparaison
  FP64 vs récurrence complète (état initial, termes d'écriture, sorties intermédiaires), et
  comparaison de nouveauté à DeltaNet-WY/TreeWY ; budget cible 160 ms / 8 tokens (20 ms/token à
  50 tok/s), coûts de projections/certificats/rafraîchissements/rejeu inclus. Non tranché par cette
  batterie (elle porte sur h_t résiduel, pas sur l'état SSM).
