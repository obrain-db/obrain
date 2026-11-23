# WDT — profil de la part accélérable — DÉCISION : prototype non justifié (2026-09-07)

## Filtre utilisateur
Accélération totale = 1/((1−f)+f/s). Baseline 8,13 tok/s → 50 tok/s exige : f ≥ 83,7 % (s=∞)
ou f ≥ 95,7 % (s=8). Profilage AVANT prototype.

## Profil (modèle réel IQ1_M, 6,26 GiB de poids — baseline au mur mémoire)
Baseline 8,13 tok/s × ~6,2 Go ≈ 50,4 Go/s ≈ mur DDR ⇒ **temps ≈ octets** (régime bande passante).

| classe | part des octets/token |
|---|---|
| FFN (up+down+gate) | **59,1 %** |
| attention (qkv+gate) | 20,1 % |
| embeddings/output | 14,3 % |
| **SSM/GDN poids (ssm_out + conv/dt/α/β)** | **6,4 %** |
| état SSM R/W (≈2×150 Mo) | ~4,8 % |
| **f_WDT max (si WDT ne touche que GDN)** | **≈ 11,2 %** |

## Verdict
- Accélération max par WDT-GDN seul : 1/(1−0,112) ≈ **1,13×** → ~9,2 tok/s.
- Seuil 50 tok/s (f ≥ 83,7 %) : **EXCLU** — ~79 % du temps (FFN+attention) est hors de portée d'un
  saut de récurrence GDN.
- Si WDT était étendu au décodage par blocs complet (k positions / passe de poids), il redevient du
  spéculatif/batch (régime déjà mesuré : MTP ×1,7 ; s≈acceptation, plafonné), et le budget
  (f,s) s'applique : s≈2 sur f=100 % ⇒ ≤2×.
- L'identité WDT (Lemme A/B) peut rester exacte : ce serait **une reformulation sans accélération
  suffisante démontrée** — prototype non lancé, conformément au filtre.
