// ── L'op WHT sur un VRAI appareil ────────────────────────────────────────────────────────────
//
// Le test précédent (`test-obrain-rotation-ggml`) travaillait avec des tenseurs dont les données
// vivent en mémoire hôte : c'est juste sur CPU, et faux sur GPU. Ici on fait ce qu'un op doit
// subir pour exister vraiment : les tenseurs sont alloués PAR LE BACKEND (`ggml_gallocr`), les
// entrées sont téléversées (`ggml_backend_tensor_set`), le résultat redescend
// (`ggml_backend_tensor_get`).
//
// Deux choses sont mesurées, et la première compte plus que la seconde :
//
//   1. L'OP EST-ELLE SUPPORTÉE ? Un backend qui ne la connaît pas fait échouer l'allocation —
//      c'est le résultat honnête, et c'est ce qu'il faut savoir avant toute promesse de débit
//      (sur GPU, un op non supporté retombe sur le CPU, donc un aller-retour PCIe par rotation).
//   2. Ce que ça coûte, et si le contrat tient sur l'appareil.
//
// Sélection : CPU par défaut ; `OBRAIN_TEST_GPU=1` prend le premier appareil non-CPU.

#include "obrain-rotation.h"

#include "ggml.h"
#include "ggml-alloc.h"
#include "ggml-backend.h"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <random>
#include <string>
#include <vector>

static double now_ms(void) {
    using clk = std::chrono::steady_clock;
    return std::chrono::duration<double, std::milli>(clk::now().time_since_epoch()).count();
}

int main(void) {
    const bool want_gpu = getenv("OBRAIN_TEST_GPU") != nullptr;

    // le sélecteur passe par l'API GÉNÉRIQUE des appareils : aucune dépendance à un backend
    ggml_backend_dev_t dev = nullptr;
    printf("appareils disponibles :\n");
    for (size_t i = 0; i < ggml_backend_dev_count(); ++i) {
        ggml_backend_dev_t d = ggml_backend_dev_get(i);
        const char * name = ggml_backend_dev_name(d);
        const char * desc = ggml_backend_dev_description(d);
        size_t free_mem = 0, total_mem = 0;
        ggml_backend_dev_memory(d, &free_mem, &total_mem);
        printf("  [%zu] %-10s %-40s %.1f/%.1f Go\n", i, name, desc,
               free_mem/1e9, total_mem/1e9);

        const bool is_cpu = std::string(name) == "CPU";
        if (want_gpu ? !is_cpu : is_cpu) {
            if (dev == nullptr) dev = d;
        }
    }
    if (dev == nullptr) {
        printf("aucun appareil correspondant (want_gpu=%d)\n", (int) want_gpu);
        return 1;
    }

    ggml_backend_t backend = ggml_backend_dev_init(dev, nullptr);
    if (backend == nullptr) { printf("initialisation de l'appareil impossible\n"); return 1; }
    const bool on_gpu = std::string(ggml_backend_dev_name(dev)) != "CPU";
    printf("backend retenu : %s\n\n", ggml_backend_dev_name(dev));

    const int64_t ne_in  = 5120;
    const int64_t ne_out = 8;
    const int64_t n_tok  = 2;
    const int32_t block  = 1024;

    std::mt19937 rng(20260920);
    std::normal_distribution<float> gauss(0.0f, 1.0f);

    std::vector<float> W((size_t) ne_in*ne_out), W_folded((size_t) ne_in*ne_out);
    for (auto & e : W) e = gauss(rng) * 0.02f;
    for (int64_t j = 0; j < ne_out; ++j) {
        memcpy(&W_folded[(size_t) j*ne_in], &W[(size_t) j*ne_in], (size_t) ne_in*sizeof(float));
        obrain_rotate(&W_folded[(size_t) j*ne_in], ne_in);
    }
    std::vector<float> X((size_t) ne_in*n_tok);
    for (auto & e : X) e = gauss(rng);

    // référence en clair
    std::vector<double> ref((size_t) ne_out*n_tok, 0.0);
    for (int64_t j = 0; j < ne_out; ++j)
        for (int64_t t = 0; t < n_tok; ++t)
            for (int64_t i = 0; i < ne_in; ++i)
                ref[(size_t) j*n_tok + t] += (double) W[(size_t) j*ne_in + i] * X[(size_t) t*ne_in + i];

    ggml_init_params p = { 16*1024*1024, nullptr, /* no_alloc = */ true };
    ggml_context * ctx = ggml_init(p);

    ggml_tensor * t_x  = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, ne_in, n_tok); ggml_set_input(t_x);
    ggml_tensor * t_Wf = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, ne_in, ne_out); ggml_set_input(t_Wf);

    ggml_tensor * y = ggml_mul_mat(ctx, t_Wf, ggml_wht(ctx, t_x, block));
    ggml_set_output(y);

    // ── L'OP EST-ELLE SUPPORTÉE ICI ? ────────────────────────────────────────────────────────
    //
    // Question posée AVANT d'exécuter, appareil par appareil. C'est la seule façon honnête de
    // savoir : un backend qui accepte un op qu'il ne calcule pas ne produit pas une erreur, il
    // produit un résultat faux — et un résultat faux plausible est pire qu'un plantage.
    //
    // DEUX FORMES, ET ELLES N'ONT PAS LE MÊME VERDICT :
    //   - `ggml_wht(t_x, 1024)` sur [5120, n_tok] : le bloc est PLUS PETIT que la ligne. Le shader
    //     Vulkan transforme UNE ligne de largeur ne[0] et n'implémente donc que ne[0] == bloc : il
    //     doit REFUSER ce cas (l'accepter rendrait un résultat faux en silence). Repli CPU, correct.
    //   - la forme du MODÈLE : reshape en [1024, N] AVANT l'op, donc ne[0] == bloc. Là, l'op est
    //     servie par l'appareil — c'est exactement ce que construit le crochet de `llama-graph.cpp`.
    // Les deux sont éprouvées ci-dessous ; n'éprouver que la première revenait à conclure « pas de
    // noyau Vulkan » alors que le chemin réel du modèle, lui, tourne sur l'appareil.
    ggml_tensor * probe = ggml_wht(ctx, t_x, block);
    printf("l'op WHT est-elle déclarée supportée ?\n");
    printf("  forme du repli hors ligne : ne[0]=%lld > bloc=%d\n", (long long) ne_in, block);
    for (size_t i = 0; i < ggml_backend_dev_count(); ++i) {
        ggml_backend_dev_t d = ggml_backend_dev_get(i);
        printf("  %-10s : %s\n", ggml_backend_dev_name(d),
               ggml_backend_dev_supports_op(d, probe) ? "OUI" : "non");
    }
    printf("\n");

    ggml_cgraph * gf = ggml_new_graph(ctx);
    ggml_build_forward_expand(gf, y);

    // ── LE HARNAIS CORRECT : LE SCHÉDULICATEUR ───────────────────────────────────────────────
    //
    // `ggml_gallocr` alloue et envoie TOUT au backend choisi : il ne consulte pas `supports_op`,
    // et un op non implémenté passe donc silencieusement à la trappe (constaté sur le 780M :
    // « allocation OK », résultat faux, et 475 Go/s — plus rapide que la mémoire de la machine,
    // ce qui trahit un calcul qui n'a pas eu lieu).
    //
    // Le schédulicateur est ce que llama.cpp utilise : il interroge chaque backend et place
    // chaque nœud là où il est supporté, en insérant les copies. C'est le seul harnais dont le
    // verdict veut dire quelque chose sur un GPU.
    ggml_backend_t              backends[2];
    ggml_backend_buffer_type_t  bufts[2];
    int nb = 0;
    if (on_gpu) {
        backends[nb] = backend;
        bufts[nb]    = ggml_backend_get_default_buffer_type(backend);
        nb++;
    }

    ggml_backend_dev_t cpu_dev = nullptr;
    for (size_t i = 0; i < ggml_backend_dev_count(); ++i) {
        ggml_backend_dev_t d = ggml_backend_dev_get(i);
        if (std::string(ggml_backend_dev_name(d)) == "CPU") { cpu_dev = d; break; }
    }
    ggml_backend_t cpu = on_gpu ? ggml_backend_dev_init(cpu_dev, nullptr) : backend;
    backends[nb] = cpu;
    bufts[nb]    = ggml_backend_get_default_buffer_type(cpu);
    nb++;

    ggml_backend_sched_t sched = ggml_backend_sched_new(backends, bufts, nb, /* graph_size */ 8192, /* parallel */ false, /* op_offload */ true);
    if (!ggml_backend_sched_alloc_graph(sched, gf)) {
        printf("allocation impossible\n");
        return 1;
    }

    printf("allocation OK — répartition (forme du repli) : les nœuds WHT vont au CPU, le reste à %s\n",
           on_gpu ? ggml_backend_dev_name(dev) : "CPU");

    ggml_backend_tensor_set(t_x,  X.data(),  0, ggml_nbytes(t_x));
    ggml_backend_tensor_set(t_Wf, W_folded.data(), 0, ggml_nbytes(t_Wf));

    if (ggml_backend_sched_graph_compute(sched, gf) != GGML_STATUS_SUCCESS) {
        printf("échec de l'exécution du graphe\n");
        return 1;
    }

    std::vector<float> Y((size_t) ne_out*n_tok, 0.0f);
    ggml_backend_tensor_get(y, Y.data(), 0, ggml_nbytes(y));

    double err = 0.0, scale = 0.0;
    for (int64_t j = 0; j < ne_out; ++j)
        for (int64_t t = 0; t < n_tok; ++t)
            for (int64_t i = 0; i < ne_in; ++i)
                scale += fabs((double) W[(size_t) j*ne_in + i] * X[(size_t) t*ne_in + i]);
    for (int64_t j = 0; j < ne_out; ++j)
        for (int64_t t = 0; t < n_tok; ++t)
            err = fmax(err, fabs(Y[(size_t) t*ne_out + j] - ref[(size_t) j*n_tok + t]));

    printf("\n=== LE CONTRAT SUR L'APPAREIL ===\n");
    printf("  écart absolu maximal %.3e  (échelle des contributions %.3e  →  relatif %.2e)\n",
           err, scale, err/scale);
    const bool ok_support = err/scale < 1e-4;

    printf("\n=== CHRONOMÉTRAGE sur %s (512 lignes de %lld) ===\n", ggml_backend_dev_name(dev), (long long) ne_in);
    {
        const int64_t rows = 512;
        std::vector<float> buf((size_t) ne_in*rows);
        for (auto & e : buf) e = gauss(rng);

        ggml_context * c2 = ggml_init({ 16*1024*1024, nullptr, true });
        ggml_tensor * big  = ggml_new_tensor_2d(c2, GGML_TYPE_F32, ne_in, rows); ggml_set_input(big);
        ggml_tensor * o    = ggml_wht(c2, big, block); ggml_set_output(o);
        ggml_cgraph * g2   = ggml_new_graph(c2);
        ggml_build_forward_expand(g2, o);

        ggml_backend_sched_t s2 = ggml_backend_sched_new(backends, bufts, nb, 8192, false, true);
        if (ggml_backend_sched_alloc_graph(s2, g2)) {
            ggml_backend_tensor_set(big, buf.data(), 0, ggml_nbytes(big));
            ggml_backend_sched_graph_compute(s2, g2); // chauffe

            const int reps = 30;
            const double t0 = now_ms();
            for (int r = 0; r < reps; ++r) ggml_backend_sched_graph_compute(s2, g2);
            const double dt = (now_ms() - t0)/reps;
            const double bytes = (double) ne_in*rows*4;
            printf("  op WHT : %7.3f ms  →  %5.1f Go/s (aller-retour)\n", dt, 2.0*bytes/(dt/1000.0)/1e9);
            if (on_gpu) {
                printf("  (ne[0]=%lld > bloc=%d : le shader %s ne fait qu'UNE ligne de largeur ne[0] et\n"
                       "   REFUSE donc cette forme — repli CPU, un aller-retour hôte↔périphérique par\n"
                       "   rotation. Ce n'est PAS un noyau manquant : la section suivante sert la même\n"
                       "   op SUR l'appareil, dans la forme que le modèle emploie.)\n",
                       (long long) ne_in, block, ggml_backend_dev_name(dev));
            }
        } else {
            printf("  (allocation du banc impossible)\n");
        }
        ggml_backend_sched_free(s2);
        ggml_free(c2);
    }

    // ── LA FORME QUE LE MODÈLE EMPLOIE : [1024, N], DONC ne[0] == bloc ───────────────────────
    //
    // Le crochet de `llama-graph.cpp` reshape en [OBRAIN_WHT_BLOCK, N] AVANT l'op, précisément pour
    // que ne[0] vaille le bloc — c'est ce qui rend l'op servable par l'appareil. C'est cette forme
    // qui porte le préfill ×2 et la fin du fractionnement du graphe : elle doit donc être éprouvée
    // ici, sinon ce test ne dit rien du chemin réel du modèle.
    printf("\n=== LA FORME DU MODELE ([%d, N], donc ne[0] == bloc) ===\n", block);
    bool ok_model = false;
    {
        ggml_context * c3 = ggml_init({ 16*1024*1024, nullptr, true });
        ggml_tensor * x3 = ggml_new_tensor_2d(c3, GGML_TYPE_F32, ne_in, n_tok); ggml_set_input(x3);
        ggml_tensor * w3 = ggml_new_tensor_2d(c3, GGML_TYPE_F32, ne_in, ne_out); ggml_set_input(w3);
        ggml_tensor * xb = ggml_reshape_2d(c3, x3, block, (ne_in/block)*n_tok);
        ggml_tensor * yb = ggml_wht(c3, xb, block);
        ggml_tensor * xs = ggml_reshape_2d(c3, yb, ne_in, n_tok);
        ggml_tensor * y3 = ggml_mul_mat(c3, w3, xs); ggml_set_output(y3);

        printf("  l'op WHT ([%d, %lld]) est-elle déclarée supportée ?\n", block,
               (long long) (ne_in/block)*n_tok);
        for (size_t i = 0; i < ggml_backend_dev_count(); ++i) {
            ggml_backend_dev_t d = ggml_backend_dev_get(i);
            printf("    %-10s : %s\n", ggml_backend_dev_name(d),
                   ggml_backend_dev_supports_op(d, yb) ? "OUI" : "non");
        }

        ggml_cgraph * g3 = ggml_new_graph(c3);
        ggml_build_forward_expand(g3, y3);
        ggml_backend_sched_t s3 = ggml_backend_sched_new(backends, bufts, nb, 8192, false, true);
        if (ggml_backend_sched_alloc_graph(s3, g3)) {
            ggml_backend_tensor_set(x3, X.data(), 0, ggml_nbytes(x3));
            ggml_backend_tensor_set(w3, W_folded.data(), 0, ggml_nbytes(w3));
            if (ggml_backend_sched_graph_compute(s3, g3) == GGML_STATUS_SUCCESS) {
                std::vector<float> Y3((size_t) ne_out*n_tok, 0.0f);
                ggml_backend_tensor_get(y3, Y3.data(), 0, ggml_nbytes(y3));
                double e3 = 0.0;
                for (int64_t j = 0; j < ne_out; ++j)
                    for (int64_t t = 0; t < n_tok; ++t)
                        e3 = fmax(e3, fabs(Y3[(size_t) t*ne_out + j] - ref[(size_t) j*n_tok + t]));
                printf("  contrat : écart absolu maximal %.3e  (relatif %.2e)\n", e3, e3/scale);
                ok_model = e3/scale < 1e-4;
            }
        } else {
            printf("  allocation du graphe impossible\n");
        }
        ggml_backend_sched_free(s3);
        ggml_free(c3);

        // chronométrage DANS CETTE FORME
        {
            const int64_t rows = 512;
            std::vector<float> buf((size_t) ne_in*rows);
            for (auto & e : buf) e = gauss(rng);

            ggml_context * c4 = ggml_init({ 16*1024*1024, nullptr, true });
            ggml_tensor * big = ggml_new_tensor_2d(c4, GGML_TYPE_F32, ne_in, rows); ggml_set_input(big);
            ggml_tensor * ob  = ggml_wht(c4, ggml_reshape_2d(c4, big, block, (ne_in/block)*rows), block);
            ggml_set_output(ob);
            ggml_cgraph * g4 = ggml_new_graph(c4);
            ggml_build_forward_expand(g4, ob);

            ggml_backend_sched_t s4 = ggml_backend_sched_new(backends, bufts, nb, 8192, false, true);
            if (ggml_backend_sched_alloc_graph(s4, g4)) {
                ggml_backend_tensor_set(big, buf.data(), 0, ggml_nbytes(big));
                ggml_backend_sched_graph_compute(s4, g4); // chauffe
                const int reps = 30;
                const double t0 = now_ms();
                for (int r = 0; r < reps; ++r) ggml_backend_sched_graph_compute(s4, g4);
                const double dt = (now_ms() - t0)/reps;
                const double bytes = (double) ne_in*rows*4;
                printf("  op WHT : %7.3f ms  →  %5.1f Go/s%s\n", dt, 2.0*bytes/(dt/1000.0)/1e9,
                       on_gpu ? " (trafic mémoire du périphérique, aucun transfert dans la boucle)"
                              : " (trafic mémoire)");
            } else {
                printf("  (allocation du banc impossible)\n");
            }
            ggml_backend_sched_free(s4);
            ggml_free(c4);
        }
    }

    const bool ok = ok_support && ok_model;
    if (!ok_model) { printf("\nLA FORME DU MODELE N'A PAS TENU LE CONTRAT\n"); }

    ggml_backend_sched_free(sched);
    if (on_gpu && cpu != nullptr) ggml_backend_free(cpu);
    ggml_free(ctx);
    ggml_backend_free(backend);

    printf("\n%s\n", ok ? "CONTRAT TENU sur cet appareil" : "CONTRAT ROMPU sur cet appareil");
    return ok ? 0 : 1;
}
