# Queue spectrale des matrices de Qwen3.8-27B — verdict mathématique définitif (2026-09-07)

## Protocole
Copie Q8_0 pure du modèle (`llama-quantize --pure Q8_0`) → déquant → top-256 valeurs propres de
AᵀA par Lanczos (réorthogonalisation complète) sur les matrices réelles (`ops-gpu16/spectral_tail.cpp`).

## Résultats (énergie cumulée top-k ÷ ‖W‖_F²)
| matrice | dims | top-64 | top-128 | top-256 | queue(>128) |
|---|---|---|---|---|---|
| blk.0.attn_qkv | 10240×5120 | 0,9 % | 1,3 % | 1,3 % | 98,7 % |
| blk.0.ffn_down | 5120×17408 | 1,1 % | 1,6 % | 1,6 % | 98,4 % |
| blk.0.ffn_up | 17408×5120 | 1,4 % | 1,9 % | 1,9 % | 98,1 % |
| blk.30.ffn_up | 17408×5120 | 1,0 % | 1,4 % | 1,4 % | 98,6 % |
| blk.0.ssm_out | 5120×6144 | 1,6 % | 2,2 % | 2,2 % | 97,8 % |

Spectre quasi-plat (σ₁≈…≈σ₆₄≈1,2-2,8, PR≈0,01) : **le rang 256 ne capte que ~1-2 % de l'énergie**.

## Conséquence (clôture mathématique)
Le schéma « streaming bas-rang W≈AB + rafraîchissement périodique » exigeait une queue spectrale
rapide ; elle est **plate** → toute factorisation de rang ≤256 a une erreur relative >98 % :
inutilisable. Combiné aux mesures précédentes (Δ inter-tokens grands et plein-rang ; quantification
bornée ~1-2 bits ; BW ~50 Go/s au mur sur deux modèles), **toutes les voies d'approximation interne au
décodage auto-régressif exact sont fermées par la mesure** :
`tok/s ≤ BW/|W|` est une borne atteinte (5,45 tok/s en Q2) et non contournable pour le 27B dense sur
la 780M. 50 tok/s exige : modèle à ~1-2 G paramètres actifs, paradigme non-autorégressif, ou
bande passante externe (≥ ~600 Go/s).
