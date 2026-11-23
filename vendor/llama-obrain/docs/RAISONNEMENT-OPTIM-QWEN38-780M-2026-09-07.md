# Qwen3.8-27B sur Radeon 780M — raisonnement d'optimisation complet (2026-09-07)

## 1. Murs mesurés (pas théoriques)

| mesure | valeur |
|---|---|
| BW effective iGPU (décodage) | ≈ **48-50 Go/s** (9B : 8,5 tok/s × 5,6 Go ; Q2 27B : 5,45 × 9,15) |
| BW mono-flux Q3 27B (12,24 Go) | plafond ≈ 4,1 → mesuré 1,95 (sans spec), 3,3 (MTP) |
| Q2_K_XL + MTP | **5,45-5,7 tok/s = ~50 Go/s → mur BW atteint** |
| calcul dispo | 8 CU RDNA3 (pas de cœurs tensoriels) : très au-dessus du besoin mono-flux |
| modèle | 27,3 B paramètres, 64 couches hybrides attention(4 KV)+SSM récurrent |

**Loi dérivée** : mono-flux `tok/s = BW ÷ poids_par_token` (12,24 Go Q3 → ≤ 4 ; 9,15 Q2 → ≤ 5,5 ;
~7-8 Go 2 bits extrême → ≤ ~7). Le calcul n'est PAS le goulot ; la lecture des poids depuis la DDR l'est.

## 2. Portes ouvertes et fermées par l'expérience

**Ouvertes (mesurées) :**
- **MTP / décodage spéculatif** (draft Q4_0, `--spec-type draft-mtp`) : Q3 1,95 → **3,2-3,5 (×1,7)** ; stable en in-slot.
- **Quant 2 bits calibré (Q2_K_XL)** : → **5,45-5,7 tok/s** (≈ mur BW).
- Combinaison Q2+MTP = ~5,5 ; un draft/acceptance meilleur (EAGLE-lite, n-gram) → ~×2,2 espéré (~7-8).
- **Préfill GPU** : 9-33 tok/s (vs 12 CPU).

**Fermées (mesurées) :**
- **Batch multi-slots** : np=3 → agrégat 2,4 tok/s (pas de gain : le backend sérialise, la BW n'augmente
  pas) ; np=4/8 → **GPU Hang** (instabilité gfx1103 + ROCm 6.2 sous charge ; llama-bench idem).
- **> ~5,5 tok/s mono-flux en 27B** : impossible sur 50 Go/s, quelle que soit l'astuce logicielle
  (50 tok/s exigerait ≤ 1 Go/token ≈ 0,3 bit/paramètre).

## 3. Pourquoi le « batch = ×B » échoue ici (analyse pure)

Sur un APU, CPU et iGPU partagent le même contrôleur DDR : la BW totale (~50-60 Go/s) est le plafond
physique, pas la taille d'un pool séparé. Le batch ne peut que *mieux utiliser* la BW existante ; or le
mono-flux Q2 l'utilise déjà à ~100 % (49,9 Go/s mesurés) → il ne reste rien à gagner, et la charge
supplémentaire fait dépasser le watchdog du driver (GPU Hang). Le batch ne gagnerait que sur une carte où
le mono-flux est *compute*-borné (GPU discret avec cœurs tensoriels) — pas ici.

## 4. Où sont les vraies optimisations restantes pour Qwen3.8 (par effet)

1. **K4 — quant KV « TurboQuant-like »** (chantier déjà fléché) : état 157 Mo → ~25-50 Mo → chargement
   ~10-30 ms, dizaines d'états mémoire par slot, chemin vers le contexte modèle 256k sur 16 Go → c'est le
   verrou du **×2500 contexte** (pas du tok/s mono-flux).
2. **Tuning spéculatif** : n-gram + MTP hybride, `--spec-draft-n-max` optimisé, KV draft q8 → +10-40 % sur
   le 3,3-5,5 actuel.
3. **Quant mixte par sensibilité** (GGUF per-tensor : Q4 sur couches critiques, Q2 ailleurs) → ~11 Go → +10 %.
4. **Cohérence/qualité à 2 bits** : imatrix déjà inclus ; calibrer greedy/température pour le pont.
5. **Réel saut = dGPU/DGX** (≥ ~600 Go/s → 50-80 tok/s sur la même pile).

## 5. Verdict

Sur la 780M : **le programme complet honnête pour Qwen3.8 = Q2/Q3 + MTP (~3,3-5,7 tok/s mono-flux) + K4
(états KV compressés, multi-états, 256k)**. Le « 50 tok/s » mono-flux est hors d'atteinte sur ce silicium
(mur DDR partagé, démontré par l'expérience de batch) ; il devient atteignable en **agrégat sur GPU
discret**, ou en **changement de paradigme** (modèle à diffusion/non-autorégressif — pas le 3.8 dense).
