# Δ-décodage (sous-espace bas-rang) — protocole de faisabilité — RÉSULTAT NÉGATIF (2026-09-07)

## Idée testée
`y_t = W·x_{t-1} + W·δ_t` avec `W·x_{t-1}` mis en cache et `W·δ_t ≈ (W·P)·(Pᵀδ_t)`
(P : base bas-rang du sous-espace des deltas) + rafraîchissement exact périodique →
réduire les octets DDR lus par token de ~12 Go (Q3) vers ~0,2-1,2 Go → 40-55 tok/s sur la 780M.

## Protocole (sonde `ops-gpu16/delta_probe.cpp`, CPU, Q4_K_M, 195 positions : 85 prompt + 110 générés)
Capture des états cachés par couche (`llama_set_layer_output_capture`) à chaque token,
puis analyse des Δ consécutifs : norme relative, Gram ΔΔᵀ (Jacobi), énergie top-k, PR.

## Résultats (Qwen3.8-27B, couches 0→63)
| couche | rel‖Δ‖/‖h‖ | PR | énergie top-k=8 | k=32 | k=64 | k=128 |
|---|---|---|---|---|---|---|
| 0-3 | 0,31-0,45 | ~0,3 | 0,61-0,72 | 0,82-0,87 | 0,94-0,96 | 0,997+ |
| 16-20 | 0,46-0,60 | ~0,1 | 0,37-0,48 | 0,66-0,71 | 0,84-0,86 | 0,977+ |
| 40-50 | 0,65-0,79 | ~0,0 | 0,30-0,34 | 0,60-0,62 | 0,79-0,81 | 0,962+ |
| 60-63 | 0,85-0,93 | ~0,0 | 0,32-0,44 | 0,60-0,70 | 0,80-0,85 | 0,966+ |

## Conclusion
- Les deltas entre tokens consécutifs sont **grands** (0,3-0,9 × ‖h‖) et de **rang effectif élevé**
  (PR≈0 ; k=64 ne couvre que ~80 % ; ~150-190 directions pour 99 % sur 194 échantillons).
- L'approximation bas-rang du Δ est **invalide** sur ce modèle (erreur résiduelle massive → divergence).
- L'hypothèse « Δ-décodage » est **falsifiée par la mesure** : le mur ~50 Go/s de la 780M ne peut
  pas être contourné par cette voie pour un 27B dense auto-régressif.
- Restent, pour « 50 tok/s » sur CE matériel : paradigmes sans N passes de poids par réponse
  (diffusion/parallèle — checkpoint inexistant pour Qwen3.8), ou bande passante externe (dGPU
  ≥ ~600 Go/s). Sur n3, le plafond réaliste du 27B reste ~5,5-8 tok/s (Q2/Q3 + MTP, mesuré).
