// ENGINE-007B-F1A-S2bis-I2 — registre de capture diagnostique, lie au graphe.
//
// INERTE par defaut : f1a_capture_get_ctx() retourne nullptr tant que OBRAIN_F1A_CAPTURE=1
// n'est pas pose. Aucun dup, aucun registre, aucune E/S sur le chemin normal.
//
// Registre scope a une EPOQUE de graphe (paragraphe 7/13) : pas de singleton non scope, donc
// plusieurs contextes et reutilisations de pointeurs ne peuvent pas provoquer de collision.
#pragma once

#include "ggml.h"
#include "ggml-backend.h"   // ggml_backend_t

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
    unsigned long long build_id;                 // R2 : identifiant de construction
    unsigned long long submission_id_at_build;   // R2 : soumissions de calcul deja effectuees
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
    // RUN2F-A / M1 : pas scientifique LATCHE au moment de f1a_capture_note.
    //   -1  = hors pas scientifique (prefill, ou non renseigne)
    //   0..8 = pas scientifique Q0FWD. SEULE signification de -1.
    // Copie faite pendant que la cible appartient sans ambiguite au pas s.
    // Ne JAMAIS la remplacer par une lecture du contexte courant a end_epoch.
    int32_t            decode_step_latched;
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
// R2 : identifie la SOUMISSION DE CALCUL. Appelee juste avant
// ggml_backend_sched_graph_compute_async. Incremente le compteur de soumissions.
void f1a_mark_compute_submission(struct ggml_cgraph * gf, int submission_rc);
bool f1a_ptr_in_submitted_graph(const void * p);
// R5-c : relations structurelles des noeuds du graphe soumis.
int  f1a_submitted_graph_output_count(void);
bool f1a_node_reaches_declared_output(const void * node);
const void * f1a_sub_node_src(const void * node, int k);
int          f1a_sub_node_op(const void * node);
bool         f1a_reaches_from_to(const void * from, const void * to, int max_hops);
// R4-a : PUBLIER les compteurs SANS creer de contexte de capture.
// Appelee par le harnais a la fin du test, capture active ou non. Aucune allocation,
// aucun tenseur, aucune ecriture : uniquement la lecture des compteurs.
void f1a_publish_counters_standalone(void);

// R4-a : nombre de noeuds DUP ajoutes par la greffe F1A. Doit rester 0 capture off.
unsigned long long f1a_dup_nodes_added(void);

void f1a_dump_submitted_snapshots();

// R2 : la greffe d extraction doit savoir combien de soumissions ont eu lieu.
unsigned long long f1a_compute_submission_count();

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
void f1a_capture_end_epoch(f1a_capture_ctx * cc, ggml_backend_t backend,
                           ggml_backend_sched_t sched);

// Vrai si une capture est active ET qu'il y a des cibles enregistrees.
bool f1a_capture_active(const f1a_capture_ctx * cc);

// N3nonies : marque un tenseur de capture avec GGML_TENSOR_FLAG_OUTPUT.
// ggml-alloc.c:690 : sans ce drapeau, ggml_gallocr_free_node LIBERE la region
// des la derniere consommation -- donc avant notre extraction de fin d epoque.
// Le fork fait exactement cela pour ses propres copies de sortie (backend.cpp:1342).
void f1a_mark_output(struct ggml_tensor * t);

// RUN2F-A / M1 : contexte de pas scientifique.
//   -1 = hors pas scientifique (prefill, ou entre deux pas)
//  0..8 = pas scientifique Q0FWD, produisant les logits de OUTPUT_IDS[s]
void f1a_set_decode_step(int32_t step);
int32_t f1a_get_decode_step(void);

// RUN2F-A / M2 : identite de la derniere soumission autoritaire.
// Mise a jour UNIQUEMENT dans f1a_mark_compute_submission, au meme endroit
// que l increment de f1a_ctr_submission. Aucun second compteur.
unsigned long long f1a_last_submission_id(void);
int32_t            f1a_last_submission_decode_step(void);

// RUN2F-A / M3 : ledger evenementiel, ecriture immediate.
// f1a_event_current : lit le contexte courant (boucle scientifique).
// f1a_event_latched : NE LIT AUCUN contexte mutable ; identite fournie
//                     explicitement. Obligatoire pour CAPTURE_*, emis a
//                     end_epoch donc HORS du pas courant.
void f1a_event_current(const char * kind);
void f1a_event_latched(const char * kind,
                       int32_t            decode_step,
                       unsigned long long compute_submission_id,
                       unsigned long long capture_epoch_id,
                       int                eligible_target_count,
                       int                captured_target_count);
void f1a_event_close(void);
// RUN2H / A2 : nombre d echecs d E/S du ledger. Un evenement perdu doit etre
// observable, jamais silencieux.
unsigned long long f1a_event_io_failure_count(void);
