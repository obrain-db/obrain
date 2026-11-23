# Chantiers C1/C2/C3 (post-15-agents) — résultats mesurés (2026-09-07, 780M/HIP)

## C1 — Spéculatif MTP (sweep γ) : partiel, instabilité driver sur runs longs
- Q3_K_XL, dgx-HIP, n=64 : **MTP γ=3 → 5,91 tok/s (wall)** — le meilleur mono-flux mesuré à ce jour
  (supérieur à Q2+MTP 5,45). none : 1,65 ; mtp γ=1 : 2,12.
- n=300 : crash/HTTP 500 à γ=2 et forte variance (3,3-5,9) → **le driver gfx1103/ROCm 6.2 ne tient pas
  les runs longs** (watchdog, cf. hangs np≥4 et llama-bench). Mesures fiables seulement en rafales courtes (<128 tok).
- Q2_K_XL sans MTP : crash rocBLAS (pas de TensileLibrary gfx1103) — chemin hipBLAS non couvert.

## C2 — Batch 2 séquences (lecture de poids partagée) : NÉGATIF
- np=2, 2 générations simultanées (Q3+MTP γ3, n=64) : **agrégat 3,7 tok/s < mono 5,9** → le backend
  sérialise (graphe SSM par séquence) : pas de partage de lecture, pénalité au lieu de ×2. Axe
  « agrégat → 50 » **clos sur ce driver** (cohérent avec np=3 : 2,4).

## C3 — IQ2_XXS (7,2 Go, i-quant) : NÉGATIF
- iq2-none : **2,02 tok/s** (~14 Go/s) ; iq2+MTP γ3 : **4,32 tok/s** (~29 Go/s) → kernels i-quant
  ALU-bound sur 8 CU : le fichier plus petit n'atteint pas le mur. Les i-quants sont une régression
  sur 780M (risque signalé par les agents, confirmé).

## Config retenue (mono-flux, ce matériel)
**Q3_K_XL + MTP γ≈3, slot unique, générations courtes (<~128 tok)** → ~5-6 tok/s wall en rafale
(soutenu ~3,3-4 à cause de l'instabilité). Éviter : np>1, runs longs, i-quants (IQ2_XXS), Q2 sans MTP.

## Verdict 15-agents confirmé par la mesure
Mono-flux ~6-8 tok/s = plafond logiciel du 27B sur la 780M ; 50 tok/s non atteignable sur ce
GPU/modèle par le logiciel seul (loi BW + instabilité driver). Les 3 chantiers « positifs » des agents
sont soit clos par la mesure (C2, C3), soit plafonnés par le driver (C1 long).
