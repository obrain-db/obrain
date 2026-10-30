# Formule WDT — Window Delta Transfer (saut de fenêtre de la récurrence GDN hybride)

Contexte : couche récurrente type Gated DeltaNet du fork `llama-obrain` (état S ∈ R^{d×d} par tête, d = S_v = 128, gates scalaires), récurrence lue dans `ggml/src/ggml-sycl/gated_delta_net.cpp` (branche non-KDA), par token t et tête h :

S_t = g_t (I − β_t k_t k_t^T) S_{t−1} + β_t k_t v_t^T ,   g_t = e^{ρ_t} ,   o_t^T = q_t^T S_t   (sortie post-écriture).

## FORMULE :

**Lemme A (transfert exact de fenêtre).** Soit P_t := g_t (I − β_t k_t k_t^T) et M_{a:b} := P_b P_{b−1}⋯P_a (produit ordonné, M_∅ = I). Pour toute fenêtre 𝒲 = {τ+1,…,τ+k}, par télescopage exact (linéarité en S) :

S_{τ+k} = M_{τ+1:τ+k} S_τ + Σ_{j=1..k} β_{τ+j} M_{τ+j+1:τ+k} k_{τ+j} v_{τ+j}^T ,   o_{τ+i}^T = q_{τ+i}^T S_{τ+i}.

**Lemme B (fermeture rang-r — l'identité nouvelle).** Si les clés de la fenêtre vivent dans un sous-espace partagé de rang r, k_t = K a_t (K ∈ R^{d×r}, a_t ∈ R^r, r ≤ 4), alors avec G := K^T K ∈ R^{r×r} le produit se ferme exactement en UN super-opérateur de rang r :

M_{a:b} = (Π_t g_t) (I − K C_{a:b} K^T),   C_{a:b} = C_{a:b−1} + β_b a_b a_b^T − β_b a_b a_b^T G C_{a:b−1},   C_∅ = 0_{r×r}

et le forçage Σ β M k v^T se factorise : chaque terme vaut β_j (Π_{>j} g) K (a_j − C G a_j) v_j^T — toute la fenêtre agit comme UN « super-pas » (décroissance effective Πg_t, clé de rang r). C'est le pendant delta-règle (avec retour d'erreur, produit d'opérateurs non commutants) de la formule de transfert des systèmes linéaires : à ma connaissance, inédit pour GDN — la littérature (chunked scans DeltaNet/GDN) ne ferme le produit que quand TOUTES les entrées de la fenêtre sont déjà connues (scan parallèle), jamais en opérateur agrégé pour un saut.

**Conjecture (WDT renormalisé).** Soit 𝒲 une fenêtre « calme » vérifiant les conditions mesurables : (C1) portes lentes : max_t |ρ_t − ρ̄| ≤ ε_ρ (ρ est une projection post-conv4, lissée par la moyenne mobile conv=4) ; (C2) clés quasi-rang-r : min_{K} max_t ‖k_t − K a_t‖ ≤ ε_k ‖k̄‖ ; (C3) prolongement fiable : le draft trivial (répétition/extrapolation linéaire de l'état caché) satisfait (C1)-(C2) sur la vraie suite avec probabilité 1−δ. Alors

‖ô_{τ+1..τ+k} − o_{τ+1..τ+k}‖ / ‖S_τ‖ ≤ Φ_k (ε_ρ, ε_k², termes croisés),   Φ_k croissant polynomialement en k,

où ô est calculé par Lemme A + B depuis un seul état S_τ lu une fois. Énoncé falsifiable : le résidu R(k) = ‖Ŝ − S‖/‖S_τ‖ est une grandeur mesurable, pas une croyance.

## MECANISME :

Le bloc k se traite comme une mini-prefill : (1) on lit les poids UNE fois pour projeter les k tokens du draft (k_t, q_t, v_t, ρ_t, β_t) ; (2) on lit S_τ une fois ; (3) Lemme B précalcule C_{r×r}, Πg (O(k r²)) ; (4) les k sorties o_{τ+i} s'obtiennent par ~2r GEMM r×d + accumulations — coût bande-passante d'UN token, pas de k. Les 64 couches enchaînent bloc par bloc ; l'attention 4 têtes KV et le conv4 restent dans le flux batch (non sautés). Exact si le draft est vrai ; sinon résidu borné par Φ_k, mesuré couche par couche.

## GAIN :

Aujourd'hui : 8.7 tok/s ⇔ ~6.3 GB lus/token (poids K4 ≈ 3.4 GB relus à CHAQUE token + états SSM ≈ 157 MB f32 et KV). Par bloc : tok/s(k) = k·BW / (B_poids + k·B_état), B_poids ≈ 3.4 GB, B_état ≈ 0.3-0.5 GB. k = 8 ⇒ ~55e9·8/(3.4+3.2)e9 ≈ 66 tok/s, soit ×7.6 ; avec corrections/fallback réaliste ×3-5. La formule rejoint bien « tok/s = k·BW÷octets » quand l'état domine.

## TEST :

Empirique-1 falsifiable sur le fork réel : activer `llama_set_gdn_input_capture(couches)` et lire `llama_get_gdn_captured(layer, what, dims)` (what ∈ {0=k, 1=v, 2=g, 3=β}, `gdn_data`/`t_gdn_in`, ancrages `qwen35.cpp` / `delta-net-base.cpp:435`) + `set_layer_output_capture` (o vrai). Protocole : (1) générer N tokens séquentiels réels ; (2) fenêtres glissantes k = 2..16, par couche : mesurer ε_ρ max, SVD du bloc K (dist. sous-espace r=1..4), et R(k) de Lemme B vs scan séquentiel exact ; (3) seuil de réfutation : la conjecture est statistiquement validée ssi médiane R(8) < η (ex. 1e-3) sur >90 % des fenêtres d'un corpus de texte courant ; sinon identifier quelle condition (C1/C2/C3) casse et où (changement de régime sémantique, chiffres, ponctuation). Contrôle : Lemme A exact doit donner R ≈ zéro machine (valide le câblage des captures).

## RISQUE :

(1) Causalité : le saut exige de connaître les entrées de la fenêtre avant de les produire → toujours draft + résidu ; le saut n'est exact que « étant données les entrées », jamais gratuit pour la génération générale — le gain repose entièrement sur (C3), qui peut être précisément mauvais aux changements de régime (tokens informatifs = fenêtres dures) ; il faut un détecteur de fenêtre calme + fallback séquentiel. (2) Amplification sur 64 couches : l'erreur couche ℓ alimente ℓ+1 et traverse l'attention 4 têtes et le conv4 ; R(k) doit être mesuré par couche, pas seulement en sortie SSM. (3) Les spectres plats annoncés suggèrent une information qui ne décroît pas vite → l'interpolation de rang faible peut être systématiquement mauvaise ; c'est le risque dominant, exactement ce que le TEST tranche. (4) Stabilité numérique des produits ordonnés en f16/i8/K4 (g près de 1 ⇒ P_t proche de l'identité, non contractif) ; le Lemme B réduit la profondeur de produit, ce qui aide.

---

FORMULE: S_{τ+k} = M S_τ + Σ β_j M_{j+1:k} k_j v_j^T (exact) ; si k_t = K a_t : M = (Πg)(I − K C K^T), C ← C + β a a^T − β a a^T G C (fermeture rang-r) ; conjecture : ‖Ŝ − S‖ ≤ Φ_k(ε_ρ, ε_k²)·‖S_τ‖ sous C1-C3.
MECANISME: mini-prefill de k tokens de draft : poids lus une fois, S_τ lu une fois, super-opérateur (Πg, C, K) précalculé en O(k r²), k sorties en ~2r GEMM — coût bande d'un seul token.
GAIN: tok/s(k) = k·BW/(B_poids + k·B_état) ; k=8 ⇒ ~66 tok/s (×7.6 théorique, ×3-5 réaliste) vs 8.7 mesurés.
TEST: hooks `llama_set_gdn_input_capture`/`get_gdn_captured({k,v,g,β})` sur qwen35/delta-net-base : fenêtres glissantes k=2..16, mesurer ε_ρ, dist. sous-espace r≤4, R(k)=‖Ŝ−S‖/‖S_τ‖ ; validée ssi médiane R(8)<1e-3 sur >90 % des fenêtres, sinon réfutée (la condition qui casse est identifiée).
RISQUE: causalité (draft obligatoire, échec aux changements de régime), amplification inter-couches (attention+conv), spectres plats ⇒ rang faible douteux (risque dominant), stabilité f16/i8/K4 des produits ordonnés.
