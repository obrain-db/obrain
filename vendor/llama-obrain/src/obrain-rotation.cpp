#include "obrain-rotation.h"

#include <cmath>
#include <string>

// ── La transformée, et rien d'autre ──────────────────────────────────────────────────────────
//
// Un vecteur de `width` éléments, découpé en blocs de 1024, chacun transformé par la
// Walsh–Hadamard normalisée. C'est la MÊME opération que `GGML_OP_WHT` (ggml_wht), et c'est
// délibéré : le repli hors ligne et la transformée du runtime doivent être la même fonction, ou
// le modèle devient du bruit.
//
// POURQUOI PAS DE SIGNES ±1. PrismML embarque un vecteur de signes explicite
// (`prism.hadamard.sign_mode = explicit`, 28 672 valeurs). Je les ai mesurés : sur 11 tenseurs du
// 27B et ~450 000 poids (docs/TERNARISATION-2026-09-20.md §5bis), l'erreur moyenne de
// ternarisation est 20,672 % avec la WHT seule contre 20,677 % avec les signes — un écart de
// 5 centièmes de millième, et le meilleur alterne 6 fois sur 11 selon le tenseur. C'est du bruit.
// La rotation, elle, est indispensable (`token_embd` passe de 97,5 % de pire groupe à 31 %).
//
// Conséquence : l'op runtime est une WHT NUE — aucun tenseur de signes, aucune table, aucune
// plomberie d'allocation. Si une mesure future (perplexité) montre que les signes manquent, les
// rajouter est un `ggml_mul` devant l'op, pas une décoration à deviner.

bool obrain_rotate(float * v, int64_t width) {
    if (v == nullptr || width <= 0 || width % OBRAIN_WHT_BLOCK != 0) {
        return false;
    }

    const float norm = 1.0f / sqrtf((float) OBRAIN_WHT_BLOCK);

    for (int64_t base = 0; base < width; base += OBRAIN_WHT_BLOCK) {
        float * x = v + base;

        // H : Walsh–Hadamard de Sylvester, papillons en place, non normalisée
        for (int h = 1; h < OBRAIN_WHT_BLOCK; h *= 2) {
            for (int i = 0; i < OBRAIN_WHT_BLOCK; i += 2*h) {
                for (int j = i; j < i + h; ++j) {
                    const float a = x[j];
                    const float b = x[j + h];
                    x[j]     = a + b;
                    x[j + h] = a - b;
                }
            }
        }

        // normalisation 1/√n — c'est elle qui rend la transformée orthogonale, donc le repli et
        // la transformée jumelle interchangeables sans changer une seule valeur du produit scalaire
        for (int i = 0; i < OBRAIN_WHT_BLOCK; ++i) {
            x[i] *= norm;
        }
    }

    return true;
}

bool obrain_rotate_rows(float * data, int64_t nrows, int64_t n_per_row) {
    if (data == nullptr || nrows <= 0 || n_per_row <= 0) {
        return false;
    }
    if (n_per_row % OBRAIN_WHT_BLOCK != 0) {
        return false;
    }

    for (int64_t r = 0; r < nrows; ++r) {
        if (!obrain_rotate(data + r*n_per_row, n_per_row)) {
            return false;
        }
    }

    return true;
}

// ── La politique ─────────────────────────────────────────────────────────────────────────────

bool obrain_tensor_is_rotated(const char * name, int64_t ne0, int64_t ne1, int ndims) {
    if (name == nullptr || ndims != 2) {
        return false; // les tenseurs d'experts (3D) ne sont pas dans ce modèle ; on ne présume pas
    }
    if (ne0 <= 0 || ne0 % OBRAIN_WHT_BLOCK != 0) {
        return false; // `ssm_conv1d.weight` a ne0 = 4 : il ne PEUT pas être replié
    }
    if (ne1 < 512) {
        return false; // `ssm_alpha`/`ssm_beta` [5120, 48] : le chemin d'état récurrent, épargné
    }

    const std::string n(name);

    if (n.size() < 6 || n.compare(n.size() - 6, 6, "weight") != 0) {
        return false;
    }
    if (n.find("_norm.weight") != std::string::npos) {
        return false;
    }
    if (n.find("ssm_conv1d") != std::string::npos) {
        return false;
    }

    return true;
}
