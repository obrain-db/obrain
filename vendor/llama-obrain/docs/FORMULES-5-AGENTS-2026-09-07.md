# 5 conjectures mathématiques « n'existant pas encore » pour optimiser le décodage 27B (780M)

Mise en commun (5 agents, 2026-09-07). Toutes attaquent le terme octets/token de `tok/s=BW÷octets`
en contournant les faits mesurés (Δ consécutifs grands/pleins, spectres plats) par une structure
CONDITIONNÉE À LA FENÊTRE/À LA TRAJECTOIRE + rafraîchissement périodique certifié.

| agent | nom | formule (essentiel) | gain si vraie | test falsifiable |
|---|---|---|---|---|
| A | IRT-ε | b(ε) ≥ B_full/T_ε, T_ε=min{ε²/v_esc; ln(ε/δ₀)/ln λ_err} ; 50 tok/s ⟺ T_ε≥5,7 | ~60 tok/s si λ_err≤1,4 | PCA glissante h_t : v_esc, λ_err par injection δ₀ |
| B | FTC | ŷ_t=Σⱼ α_{t,j}·y_{t−j} si δ_t≤ε (fenêtre L, Gram glissante) ; erreur certifiée ≤σ_max(W)ε‖h‖ | ×8-25 (q(ε)≥0,9-0,99) | rang local ρ_loc(L) sur hooks réels |
| C | WDT (fichier banc-obrain/formule-wdt.md) | fermeture rang-r du produit de pas GDN : M=(Πg)(I−KCKᵀ), C←C+βaaᵀ−βaaᵀGC (Lemme B) | k=8 ⇒ ~66 tok/s | hooks GDN : R(k)=‖Ŝ−S‖/‖S‖, ε_ρ, rang K |
| D | SFM | lecture unique de Ŵ̂ par fenêtre + facteurs résidents P=Ŵ̂V_r, C=Ŵ̂U_k + erreur-feedback μ ; borne de concentration | ~45-90 tok/s | sweep (r,k,b₀,T) sur spectral_tail.cpp |
| E | budget logit-ε | L ≥ (n/8a)[b_floor+log₂(ε_floor/ε)] (impossibilité) | clôture : 50 tok/s ⇒ impossible sauf B1-B4 | pente empirique bruit 2^(−b) ; α draft 4B |

Points communs structurants : (1) ancrage périodique + certificat d'erreur a posteriori — dégradation
propre vers baseline si la conjecture est fausse ; (2) tous falsifiables sur le modèle réel via les
hooks existants (capture couches, GDN k/v/g/β) ; (3) risques dominants nommés : hypothèse nulle =
faits mesurés (drift/Δ grands), spectres plats ⇒ interpolation faible rang douteuse, amplification
64 couches, stabilité f16/i8/K4 des produits ordonnés.
Prochaine étape recommandée : batterie de mesures « rang local de fenêtre » (B/A) puis fermeture GDN
régulière (C) — la moins chère et la plus discriminante.
