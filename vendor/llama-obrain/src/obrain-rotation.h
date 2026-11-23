// ── LA ROTATION REPLIÉE DANS LES POIDS, ET SA JUMELLE ────────────────────────────────────────
//
// Ce fichier porte UNE opération, utilisée à deux endroits qu'il faut tenir ensemble :
//
//   1. hors ligne, au moment de quantifier : chaque LIGNE d'une matrice est repliée par A ;
//   2. au runtime, avant la multiplication : l'ACTIVATION subit la même A.
//
// Si les deux divergent d'un iota, le modèle ne « perd » pas de qualité — il produit du bruit.
// C'est pourquoi elles sont ici, à côté, et pourquoi le test vérifie la seule propriété qui
// compte : A est ORTHOGONALE, donc ⟨A·w, A·x⟩ = ⟨w, x⟩.
//
// A = H, la transformée de Walsh–Hadamard normalisée, par BLOCS DE 1024 le long de la dimension
// d'entrée. Le bloc vaut 1024 et non ne0, parce que la WHT exige une puissance de deux :
// 5120 = 5×1024, 6144 = 6×1024, 17408 = 17×1024. Les trois largeurs du 27B hybride tombent juste.
//
// Le runtime ne duplique pas ce code : il appelle `ggml_wht` (GGML_OP_WHT), qui doit donner
// EXACTEMENT le même résultat. Le test le vérifie sur les trois implémentations (l'op, le chemin
// historique matrice dense + hint, et cette fonction).
//
// Repli des poids : W' = W·Aᵀ (chaque ligne subit A). Pour la table d'embedding, c'est A qui
// s'applique à gauche — et comme le layout GGUF range ne0 en premier, « la ligne » EST le vecteur
// du token : la même fonction convient, sans cas particulier.

#pragma once

#include <cstdint>

// bloc de la transformée : 1024 = 2^10, la plus grande puissance de deux qui divise les trois
// largeurs du modèle (5120, 6144, 17408)
#define OBRAIN_WHT_BLOCK 1024

// A = H sur un vecteur de `width` éléments, EN PLACE. C'est la seule opération du fichier :
//   * repli d'une ligne de poids, hors ligne ;
//   * transformée de l'activation, au runtime (la jumelle).
// Rend false si la largeur n'est pas un multiple du bloc (l'appelant doit alors refuser).
bool obrain_rotate(float * v, int64_t width);

// Replie `nrows` lignes contiguës de `n_per_row` éléments chacune (le layout du quantifieur :
// un tampon [nrows, n_per_row] en clair). Ne fait rien et rend false si la largeur ne convient pas.
bool obrain_rotate_rows(float * data, int64_t nrows, int64_t n_per_row);

// ── La politique : ce qui est replié, et ce qui est épargné ──────────────────────────────────
//
// Elle est MESURÉE, pas recopiée : la rotation ramène le pire groupe de ternarisation à ~32 %
// d'erreur relative partout, alors qu'il monte à 75 % sans elle — mais son gain suit
// l'hétérogénéité des canaux, qui va de 1,9× (attn_qkv) à 15,2× (ssm_out)
// (docs/TERNARISATION-2026-09-20.md §5bis).
//
// Deux exclusions, et chacune a une raison :
//   * `ne1` petit — le chemin d'état récurrent (`ssm_alpha`/`ssm_beta`, forme [5120, 48]) :
//     PrismML le garde en BF16, et ses 0,0976 % de paramètres épargnés sont exactement là ;
//   * `ne0` non multiple de 1024 — `ssm_conv1d` a ne0 = 4, il ne peut pas être replié du tout.
bool obrain_tensor_is_rotated(const char * name, int64_t ne0, int64_t ne1, int ndims);
