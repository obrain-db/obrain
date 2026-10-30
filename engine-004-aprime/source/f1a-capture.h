// ENGINE-007B-F1A-S2bis-I2 — registre de capture diagnostique, lie au graphe.
//
// INERTE par defaut : f1a_capture_get_ctx() retourne nullptr tant que OBRAIN_F1A_CAPTURE=1
// n'est pas pose. Aucun dup, aucun registre, aucune E/S sur le chemin normal.
//
// Registre scope a une EPOQUE de graphe (paragraphe 7/13) : pas de singleton non scope, donc
// plusieurs contextes et reutilisations de pointeurs ne peuvent pas provoquer de collision.
#pragma once

#include "ggml.h"
#include "ggml-backend.h"   // ggml_backend_t : requis pour le parametre de fin d'epoque

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
#include <unordered_set>
#include <unordered_map>

// ---------------------------------------------------------------- enregistrement

struct f1a_target_rec {
    int                target_id;
    unsigned long long epoch;
    std::string        weight_name;
    int                weight_type;
    int64_t            weight_ne[4];
    const void *       cur_raw_id;      // activation telle qu'elle ARRIVE
    const void *       cur_eff_id;      // activation EFFECTIVEMENT consommee (post-WHT si rotation)
    bool               rotation_applied;
    int64_t            eff_ne[4];
    int64_t            eff_nb[4];
    size_t             eff_bytes;
    int                family;          // 5 = E5, 6 = E6, 0 = inconnu
    int64_t            expected_K;
    // POINTEURS VIVANTS vers les tenseurs du graphe : requis pour ggml_backend_tensor_get
    // apres la synchronisation (paragraphe 4 : ne pas ne stocker que des identifiants).
    ggml_tensor *      t_cur_capture;
    ggml_tensor *      t_mm_raw;
    ggml_tensor *      t_mm_capture;
    // capture d'activation partagee (deduplication)
    int                activation_capture_id;
};

struct f1a_capture_ctx {
    unsigned long long epoch = 0;
    std::vector<f1a_target_rec> targets;
    // cle d'activation : (cur_eff_id, dtype, ne0, nb0)
    std::unordered_map<std::string, int> act_key_to_id;
    int next_target_id = 1;
    int next_act_id    = 1;
};

// ---------------------------------------------------------------- API

f1a_capture_ctx * f1a_capture_get_ctx();
bool f1a_eligible(const ggml_tensor * w);
void f1a_capture_note(f1a_capture_ctx * cc,
                      const ggml_tensor * w,
                      const ggml_tensor * cur_raw,
                      const ggml_tensor * cur_used,
                      const ggml_tensor * cur_capture,
                      const ggml_tensor * mm_raw,
                      const ggml_tensor * mm_capture);

// Fin d'epoque : ecrit le manifeste et termine l'epoque. A appeler APRES synchronisation
// et AVANT tout reset du planificateur.
// Fin d'epoque : synchronise, extrait les valeurs, ecrit les payloads et le manifeste,
// puis invalide le registre. A appeler APRES la soumission du graphe et AVANT toute
// reutilisation de buffer (voir LIFETIME_CONTROL_FLOW_PROOF.txt).
void f1a_capture_end_epoch(f1a_capture_ctx * cc, ggml_backend_t backend);

// Vrai si une capture est active ET qu'il y a des cibles enregistrees.
bool f1a_capture_active(const f1a_capture_ctx * cc);
