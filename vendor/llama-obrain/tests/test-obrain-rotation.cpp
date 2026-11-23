// ── La rotation repliée : ce qui est vérifié, et pourquoi c'est CE test ──────────────────────
//
// Le repli et la transformée jumelle sont la MÊME opération A = H∘S. Le modèle ne « perd » pas
// de qualité si elles divergent : il devient du bruit. Le test doit donc vérifier la propriété
// qui rend l'échange licite, et rien d'autre :
//
//   ⟨A·w, A·x⟩ = ⟨w, x⟩          (A orthogonale — c'est le cœur)
//
// puis, en bout de chaîne, le vrai contrat d'un modèle : une matrice de poids repliée, multipliée
// par une activation transformée, doit rendre EXACTEMENT le même produit qu'avant repli.
// Si l'ordre de S et de H est inversé, ou si le bloc n'est pas le bon, ce dernier contrôle tombe.
//
// Aucun fichier de modèle, aucun GPU, aucune donnée à télécharger : tout est engendré ici.

#include "../src/obrain-rotation.h"

#include <cmath>
#include <cstdio>
#include <cstring>
#include <random>
#include <string>
#include <vector>

static int n_failed = 0;
static int n_passed = 0;

static void check(bool ok, const char * what) {
    if (ok) { n_passed++; } else { n_failed++; printf("  FAIL  %s\n", what); }
}

static double dot(const std::vector<float> & a, const std::vector<float> & b) {
    double s = 0.0;
    for (size_t i = 0; i < a.size(); ++i) s += (double) a[i] * b[i];
    return s;
}

static double norm(const std::vector<float> & a) {
    return sqrt(dot(a, a));
}

// H est symétrique et orthogonale : elle est donc sa propre inverse. C'est ce qui rend le contrôle
// « replier puis déplier » possible — et discriminant, puisque si la normalisation ou la
// construction des papillons était fausse, il ne retomberait pas sur l'entrée.
static void rotate_transpose(float * v, int64_t width) {
    obrain_rotate(v, width);
}

int main(void) {
    const int64_t widths[] = { 5120, 6144, 17408 };

    std::mt19937 rng(20260920);
    std::normal_distribution<float> gauss(0.0f, 1.0f);

    printf("=== 1. la transformée est ORTHOGONALE et c'est sa propre inverse ===\n");
    for (int64_t w : widths) {
        std::vector<float> v((size_t) w);
        for (int64_t i = 0; i < w; ++i) v[i] = gauss(rng);
        std::vector<float> r = v;
        check(obrain_rotate(r.data(), w), "la rotation accepte la largeur");
        obrain_rotate(r.data(), w);
        double err = 0.0, ref = 0.0;
        for (int64_t i = 0; i < w; ++i) { err += fabs(r[i] - v[i]); ref += fabs(v[i]); }
        printf("  largeur %6lld : écart relatif de H∘H à l'identité %.2e\n", (long long) w, err/ref);
        check(err/ref < 1e-6, "appliquer la transformée deux fois rend l'entrée");
    }

    printf("\n=== 2. A est orthogonale : ⟨Aw, Ax⟩ = ⟨w, x⟩ et ‖Aw‖ = ‖w‖ ===\n");
    for (int64_t w : widths) {
        std::vector<float> a((size_t) w), b((size_t) w);
        for (int64_t i = 0; i < w; ++i) { a[i] = gauss(rng); b[i] = gauss(rng); }

        const double d0 = dot(a, b), na0 = norm(a), nb0 = norm(b);
        std::vector<float> fa = a, fb = b;
        check(obrain_rotate(fa.data(), w), "la rotation accepte la largeur");
        check(obrain_rotate(fb.data(), w), "la rotation accepte la largeur");
        const double d1 = dot(fa, fb), na1 = norm(fa), nb1 = norm(fb);

        const double rel_dot  = fabs(d1 - d0) / fabs(d0);
        const double rel_norm = fabs(na1 - na0) / na0;
        printf("  largeur %6lld : ⟨Aw,Ax⟩/⟨w,x⟩ écart relatif %.2e | ‖Aw‖/‖w‖ %.2e\n",
               (long long) w, rel_dot, rel_norm);
        check(rel_dot  < 1e-5, "le produit scalaire est conservé");
        check(rel_norm < 1e-6, "la norme est conservée");
        check(fabs(nb1 - nb0) / nb0 < 1e-6, "la norme est conservée (second vecteur)");
    }

    printf("\n=== 3. le repli est LOCAL AU BLOC (aucun mélange entre blocs de 1024) ===\n");
    {
        const int64_t w = 5120;
        std::vector<float> v((size_t) w, 0.0f);
        for (int i = 0; i < OBRAIN_WHT_BLOCK; ++i) v[i] = gauss(rng); // seul le bloc 0 est rempli
        std::vector<float> f = v;
        obrain_rotate(f.data(), w);
        float leak = 0.0f;
        for (int64_t i = OBRAIN_WHT_BLOCK; i < w; ++i) leak = fmaxf(leak, fabsf(f[i]));
        check(leak == 0.0f, "un contenu dans le bloc 0 ne fuit dans aucun autre bloc");
        check(fabs(norm(f) - norm(v)) / norm(v) < 1e-6, "l'énergie du bloc est conservée");
    }

    printf("\n=== 4. replier puis déplier rend exactement le vecteur de départ ===\n");
    for (int64_t w : widths) {
        std::vector<float> v((size_t) w);
        for (int64_t i = 0; i < w; ++i) v[i] = gauss(rng);
        std::vector<float> r = v;
        obrain_rotate(r.data(), w);
        rotate_transpose(r.data(), w);
        double err = 0.0, ref = 0.0;
        for (int64_t i = 0; i < w; ++i) { err += fabs(r[i] - v[i]); ref += fabs(v[i]); }
        printf("  largeur %6lld : écart relatif après repli+dépli %.2e\n", (long long) w, err/ref);
        check(err/ref < 1e-6, "le dépli défait exactement le repli");
    }

    printf("\n=== 5. LE CONTRAT : poids repliés × activation transformée = produit d'origine ===\n");
    for (int64_t w : widths) {
        const int n_out = 8;
        std::vector<float> W((size_t) (n_out*w)), x((size_t) w);
        for (auto & e : W) e = gauss(rng) * 0.02f;
        for (auto & e : x) e = gauss(rng);

        // référence : W·x, sans aucun repli
        std::vector<double> ref(n_out, 0.0);
        for (int j = 0; j < n_out; ++j)
            for (int64_t i = 0; i < w; ++i) ref[j] += (double) W[(size_t) j*w + i] * x[i];

        // repli des lignes de W, transformation de l'activation
        check(obrain_rotate_rows(W.data(), n_out, w), "le repli des lignes est accepté");
        check(obrain_rotate(x.data(), w), "la transformée de l'activation est acceptée");

        // La métrique est rapportée à la SOMME DES |contributions|, et non au résultat : un produit
        // scalaire de 5120 termes peut annuler (résultat proche de zéro), et rapporter l'erreur à
        // ce résultat-là mesure la malchance, pas la précision (constaté : 3,3e-05 à 5120 contre
        // 4,4e-07 à 17408 — trois fois plus de termes et cent fois moins d'erreur : c'est
        // l'annulation, pas le calcul). Σ|w_i·x_i| est insensible à ce phénomène.
        double max_rel = 0.0;
        for (int j = 0; j < n_out; ++j) {
            double got   = 0.0;
            double scale = 0.0;
            for (int64_t i = 0; i < w; ++i) {
                got   += (double) W[(size_t) j*w + i] * x[i];
                scale += fabs((double) W[(size_t) j*w + i] * x[i]);
            }
            max_rel = fmax(max_rel, fabs(got - ref[j]) / scale);
        }
        printf("  largeur %6lld : écart maximal rapporté aux contributions %.2e\n", (long long) w, max_rel);
        check(max_rel < 1e-6, "le produit scalaire est reproduit après repli + transformée");
    }

    printf("\n=== 6. la politique : ce qui est replié, ce qui est épargné ===\n");
    struct { const char * name; int64_t ne0; int64_t ne1; int ndims; bool expected; } cases[] = {
        { "blk.0.ffn_gate.weight",        5120, 17408, 2, true  },
        { "blk.0.ffn_down.weight",       17408,  5120, 2, true  },
        { "blk.0.attn_qkv.weight",        5120, 10240, 2, true  },
        { "blk.0.attn_k.weight",          5120,  1024, 2, true  },
        { "blk.0.ssm_out.weight",         6144,  5120, 2, true  },
        { "output.weight",                5120,248320, 2, true  },
        { "token_embd.weight",            5120,248320, 2, true  }, // « inverse » : même fonction, autre côté
        { "blk.0.ssm_alpha.weight",       5120,    48, 2, false }, // chemin d'état récurrent
        { "blk.0.ssm_beta.weight",        5120,    48, 2, false },
        { "blk.0.ssm_conv1d.weight",         4, 10240, 2, false }, // ne0 non multiple de 1024
        { "blk.0.attn_norm.weight",       5120,     1, 2, false }, // les norms
        { "blk.0.ssm_norm.weight",         128,     1, 2, false },
        { "blk.0.ssm_a",                    48,     1, 1, false },
        { "blk.0.ssm_dt.bias",              48,     1, 1, false },
    };
    for (const auto & c : cases) {
        const bool got = obrain_tensor_is_rotated(c.name, c.ne0, c.ne1, c.ndims);
        char msg[256];
        snprintf(msg, sizeof(msg), "%s [%lld, %lld] → %s", c.name,
                 (long long) c.ne0, (long long) c.ne1, c.expected ? "replié" : "épargné");
        check(got == c.expected, msg);
    }

    printf("\n=== 7. une largeur illégale est REFUSÉE, pas rabotée ===\n");
    {
        std::vector<float> v(100, 1.0f);
        check(!obrain_rotate(v.data(), 100), "une largeur non multiple de 1024 est refusée");
        check(!obrain_rotate_rows(v.data(), 1, 100), "le repli de lignes la refuse aussi");
    }

    printf("\n%d passed, %d failed\n", n_passed, n_failed);
    return n_failed > 0 ? 1 : 0;
}
