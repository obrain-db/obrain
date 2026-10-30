#!/usr/bin/env python3
# ENGINE-004 / A'' : applique l'observateur de transitions a un ggml-cuda.cu (patch ADDITIF).
#
# Usage : python3 apply-observer.py <chemin/ggml-cuda.cu> [--suppress-log]
#
# --suppress-log : supprime l'appel GGML_LOG_DEBUG("CUDA Graph id %zu reused\n") du chemin
#                  rapide, pour que PER_TOKEN_LIBRARY_TEXT_LOGGING=0 soit vrai. Le patch est
#                  declare comme etant CE changement de comportement d'affichage (aucun effet
#                  sur les graphes, les lancements ou les compteurs).
#
# Chaque ancre doit apparaitre exactement une fois, sinon rien n'est ecrit (aucun patch partiel).
# Les emplacements modifies sont marques "// [ENGINE-004-OBS]" pour permettre la restauration.

import sys
import os
import re

MARK = "// [ENGINE-004-OBS]"

# ------------------------------------------------------------------ 1. include

ANCHOR_INCLUDE = """#include "ggml-cuda.h"
#include "ggml-impl.h"
#include "ggml-backend-impl.h"
"""

REPL_INCLUDE = """#include "ggml-cuda.h"
#include "ggml-impl.h"
#include "ggml-backend-impl.h"

// [ENGINE-004-OBS] observateur de transitions (compteurs + anneau d'evenements en memoire).
// Inerte sans GGML_OBSERVE=1. Aucune I/O, aucune allocation dans le chemin chaud.
#include "ggml-cuda/ggml-cuda-observe.cuh"
"""

# L'include doit se poser apres le bloc de telemetrie V3 quand il existe, sinon apres les
# trois includes d'origine : les deux variantes sont equivalentes pour le patch observateur.

ANCHOR_V3_TELEMETRY_END = """// ============================ end ENGINE-002 V3 CUDA TELEMETRY ============================
"""

REPL_V3_TELEMETRY_END = """// ============================ end ENGINE-002 V3 CUDA TELEMETRY ============================

// [ENGINE-004-OBS] observateur de transitions (compteurs + anneau d'evenements en memoire).
// ADDITIF a la telemetrie V3 ci-dessus : aucune ligne V3 n'est modifiee ni retiree.
#include "ggml-cuda/ggml-cuda-observe.cuh"
"""

# ------------------------------------------------------------------ 2. update_required

ANCHOR_UPDATE_REQUIRED = """static bool ggml_cuda_graph_update_required(ggml_backend_cuda_context * cuda_ctx, ggml_cgraph * cgraph) {
    bool res = false;

    const void * graph_key = ggml_cuda_graph_get_key(cgraph);
    ggml_cuda_graph * graph = cuda_ctx->cuda_graph(graph_key);

    if (cgraph->uid != 0 &&
        cgraph->uid == graph->uid) {
        GGML_LOG_DEBUG("CUDA Graph id %zu reused\\n", cgraph->uid);
        GGML_ASSERT((int)graph->node_props.size() == cgraph->n_nodes);
        return false;
    }

    graph->uid = cgraph->uid;
"""

REPL_UPDATE_REQUIRED = """static bool ggml_cuda_graph_update_required(ggml_backend_cuda_context * cuda_ctx, ggml_cgraph * cgraph) {
    bool res = false;

    const void * graph_key = ggml_cuda_graph_get_key(cgraph);
    ggml_cuda_graph * graph = cuda_ctx->cuda_graph(graph_key);

    // [ENGINE-004-OBS] etat avant decision : l'uid memorise, la cle de graphe, les octets du noeud 0.
    const uint64_t obs_cached_uid = graph->uid;
    const uint64_t obs_graph_key  = (uint64_t) (uintptr_t) graph_key;
    const uint64_t obs_bytes0     = cgraph->n_nodes > 0 ? (uint64_t) ggml_nbytes(cgraph->nodes[0]) : 0;

    if (cgraph->uid != 0 &&
        cgraph->uid == graph->uid) {
        GGML_LOG_DEBUG("CUDA Graph id %zu reused\\n", cgraph->uid);
        GGML_ASSERT((int)graph->node_props.size() == cgraph->n_nodes);
        // [ENGINE-004-OBS] chemin rapide UID : compte, et ne consomme PAS d'evenement d'anneau.
        ggml_obrain_obs_fast_path_reuse(cgraph->uid, obs_graph_key, cgraph->n_nodes);
        return false;
    }

    graph->uid = cgraph->uid;

    // [ENGINE-004-OBS] qualification du changement : premier noeud fautif, champ fautif,
    // et valeurs avant/apres. La precedente version du patch ne conservait que l'index.
    int    obs_first_node = -1;
    int    obs_field      = GGML_OBS_FIELD_NONE;
    ggml_obrain_obs_changed_node obs_chg;
"""

# ------------------------------------------------------------------ 3. detection du champ fautif

ANCHOR_MEMCMP = """        if (res || memcmp(&graph->node_props[i], &prop, sizeof(prop)) != 0) {
            graph->node_props[i] = prop;
            res = true;
        }
    }
"""

REPL_MEMCMP = """        if (res || memcmp(&graph->node_props[i], &prop, sizeof(prop)) != 0) {
            // [ENGINE-004-OBS-BEGIN] qualification AVANT ecrasement du snapshot
            if (obs_first_node == -1) {
                obs_first_node = i;
                const ggml_cuda_graph::node_properties & old = graph->node_props[i];
                obs_chg.node_index = (uint32_t) i;
                obs_chg.node_name  = cgraph->nodes[i]->name;
                obs_chg.old_bytes  = (uint64_t) ggml_nbytes(&old.node);
                obs_chg.new_bytes  = (uint64_t) ggml_nbytes(&prop.node);
                // [ENGINE-004-OBS] une seule source de verite : la boucle ci-dessous decide.
                bool obs_any_diff = false;
                for (int j = 0; j < GGML_MAX_SRC; ++j) {
                    if (old.node_src_data_ptrs[j] != prop.node_src_data_ptrs[j]) {
                        obs_any_diff = true;
                        if (obs_field == GGML_OBS_FIELD_NONE) {
                            obs_field = GGML_OBS_FIELD_SRC_DATA_PTR;
                            obs_chg.old_data_ptr = (uint64_t) (uintptr_t) old.node_src_data_ptrs[j];
                            obs_chg.new_data_ptr = (uint64_t) (uintptr_t) prop.node_src_data_ptrs[j];
                        }
                    } else if (memcmp(old.node_src_ne[j], prop.node_src_ne[j], sizeof(prop.node_src_ne[j])) != 0) {
                        obs_any_diff = true;
                        if (obs_field == GGML_OBS_FIELD_NONE) { obs_field = GGML_OBS_FIELD_SRC_SHAPE_NE; }
                    } else if (memcmp(old.node_src_nb[j], prop.node_src_nb[j], sizeof(prop.node_src_nb[j])) != 0) {
                        obs_any_diff = true;
                        if (obs_field == GGML_OBS_FIELD_NONE) { obs_field = GGML_OBS_FIELD_SRC_STRIDE_NB; }
                    }
                }
                (void) obs_any_diff;
                // [ENGINE-004-OBS] ne JAMAIS ecraser un champ deja classe : la premiere
                // difference rencontree dans l'ordre de precedence gagne.
                if (obs_field == GGML_OBS_FIELD_NONE) { obs_field = GGML_OBS_FIELD_OTHER; }
                for (int d = 0; d < 4; ++d) {
                    obs_chg.old_ne[d] = old.node.ne[d];
                    obs_chg.new_ne[d] = prop.node.ne[d];
                    obs_chg.old_nb[d] = old.node.nb[d];
                    obs_chg.new_nb[d] = prop.node.nb[d];
                }
            }
            // [ENGINE-004-OBS-END]
            graph->node_props[i] = prop;
            res = true;
        }
    }
"""

# ------------------------------------------------------------------ 4. emission de l'evenement

ANCHOR_RETURN_RES = """            graph->node_props[i] = prop;
            res = true;
        }
    }

    return res;
}
"""

REPL_RETURN_RES = """            graph->node_props[i] = prop;
            res = true;
        }
    }

    if (res) {
        // [ENGINE-004-OBS] un evenement par transition detectee, date par ordinal de calcul.
        ggml_obrain_obs_changed_node obs_emit = obs_chg;
        if (obs_first_node <= -2) {
            obs_emit.node_index = 0xFFFFFFFFu;   // la TAILLE du graphe a change, pas un noeud
        }
        // [ENGINE-004-OBS] le champ fautif (obs_field) est un etat LOCAL a cette fonction :
        // il doit etre transmis explicitement. Son oubli etait la cause d'un
        // EVENT_0_FIRST_CHANGED_FIELD=NONE alors que le noeud et les valeurs etaient justes.
        ggml_obrain_obs_event_transition(obs_emit, obs_field, cgraph->uid, obs_cached_uid,
                                         obs_graph_key, cgraph->n_nodes);
    }

    return res;
}
"""

# ------------------------------------------------------------------ 5. taille de graphe changee

ANCHOR_NODE_COUNT = """    // Check if the graph size has changed
    if ((int)graph->node_props.size() != cgraph->n_nodes) {
        res = true;
        graph->node_props.resize(cgraph->n_nodes);
    }
"""

REPL_NODE_COUNT = """    // Check if the graph size has changed
    if ((int)graph->node_props.size() != cgraph->n_nodes) {
        res = true;
        // [ENGINE-004-OBS] champ fautif de premier ordre : le nombre de noeuds lui-meme.
        obs_first_node = -2;
        obs_field      = GGML_OBS_FIELD_NODE_COUNT;
        graph->node_props.resize(cgraph->n_nodes);
    }
"""

# ------------------------------------------------------------------ 6. capture / instantiate / launch

# Ancre courte et robuste : la ligne d'appel seule (unique dans le fichier). Insere l'appel
# observateur JUSTE APRES, sans toucher aux lignes V3 eventuellement presentes a cet endroit.
ANCHOR_STREAM_END_CAPTURE = """            CUDA_CHECK(cudaStreamEndCapture(cuda_ctx->stream(), &graph->graph));
"""

REPL_STREAM_END_CAPTURE = """            CUDA_CHECK(cudaStreamEndCapture(cuda_ctx->stream(), &graph->graph));
            ggml_obrain_obs_capture_end();   // [ENGINE-004-OBS]
"""

ANCHOR_INSTANTIATE = """        if (graph->instance == nullptr) { // Create executable graph from captured graph.
            CUDA_CHECK(cudaGraphInstantiate(&graph->instance, graph->graph, NULL, NULL, 0));
"""

REPL_INSTANTIATE = """        if (graph->instance == nullptr) { // Create executable graph from captured graph.
            CUDA_CHECK(cudaGraphInstantiate(&graph->instance, graph->graph, NULL, NULL, 0));
            ggml_obrain_obs_instantiate();   // [ENGINE-004-OBS]
"""

ANCHOR_LAUNCH = """        CUDA_CHECK(cudaGraphLaunch(graph->instance, cuda_ctx->stream()));
"""

REPL_LAUNCH = """        CUDA_CHECK(cudaGraphLaunch(graph->instance, cuda_ctx->stream()));
        // [ENGINE-004-OBS] instance_present et update_required evalues AVANT, comme en V3 :
        // sur ce chemin l'instance vient d'etre creee si elle manquait, et un update a pu etre
        // effectue juste avant. On relit donc l'etat du graphe au moment du lancement.
        ggml_obrain_obs_launch(true,
                               cuda_ctx->cuda_graph(graph_key)->instance != nullptr,
                               cuda_graph_update_required);
"""

ANCHOR_EXEC_UPDATE_CALL = """        if (cuda_graph_update_required) { // Update graph executable
            ggml_cuda_graph_update_executable(cuda_ctx, graph_key);
"""

REPL_EXEC_UPDATE_CALL = """        if (cuda_graph_update_required) { // Update graph executable
            ggml_cuda_graph_update_executable(cuda_ctx, graph_key);
            ggml_obrain_obs_exec_update(false);   // [ENGINE-004-OBS]
"""

ANCHOR_UPDATE_EXECUTABLE_TAIL = """        (void)cudaGetLastError();
        CUDA_CHECK(cudaGraphExecDestroy(graph->instance));
        graph->instance = nullptr;
        CUDA_CHECK(cudaGraphInstantiate(&graph->instance, graph->graph, NULL, NULL, 0));
    } else {
        GGML_ASSERT(stat == cudaSuccess);
    }
}"""

REPL_UPDATE_EXECUTABLE_TAIL = """        (void)cudaGetLastError();
        CUDA_CHECK(cudaGraphExecDestroy(graph->instance));
        graph->instance = nullptr;
        CUDA_CHECK(cudaGraphInstantiate(&graph->instance, graph->graph, NULL, NULL, 0));
        ggml_obrain_obs_reinstantiate();   // [ENGINE-004-OBS] update refuse + recapture
    } else {
        GGML_ASSERT(stat == cudaSuccess);
    }
}"""

ANCHOR_DIRECT_EVAL = """        } else {
            graph_evaluated_or_captured = true; // ggml graph has been directly evaluated
        }
    }
"""

REPL_DIRECT_EVAL = """        } else {
            // [ENGINE-004-OBS] evaluation directe : lancement de chaque noeud, pas de graphe.
            ggml_obrain_obs_launch(false, false, false);
            graph_evaluated_or_captured = true; // ggml graph has been directly evaluated
        }
    }
"""

# ------------------------------------------------------------------ 7. compteurs de haut niveau + ordinal

ANCHOR_COMPUTE_HEAD = """static enum ggml_status ggml_backend_cuda_graph_compute(ggml_backend_t backend, ggml_cgraph * cgraph) {
    ggml_backend_cuda_context * cuda_ctx = (ggml_backend_cuda_context *) backend->context;

    ggml_cuda_set_device(cuda_ctx->device);
"""

REPL_COMPUTE_HEAD = """static enum ggml_status ggml_backend_cuda_graph_compute(ggml_backend_t backend, ggml_cgraph * cgraph) {
    ggml_backend_cuda_context * cuda_ctx = (ggml_backend_cuda_context *) backend->context;

    // [ENGINE-004-OBS] ordinal de calcul : chaque calcul de graphe recoit un numero.
    // C'est lui, et lui seul, qui date les evenements et les classes dans la fenetre.
    ggml_obrain_obs_begin_compute();

    ggml_cuda_set_device(cuda_ctx->device);
"""

# Ancre courte : la ligne d'appel seule, unique dans le fichier. Fonctionne avec ou sans les
# lignes de telemetrie V3 qui l'entourent.
ANCHOR_COMPUTE_UPD_REQ = """            const bool properties_changed = ggml_cuda_graph_update_required(cuda_ctx, cgraph);
"""

REPL_COMPUTE_UPD_REQ = """            const bool properties_changed = ggml_cuda_graph_update_required(cuda_ctx, cgraph);
            if (properties_changed) { ggml_obrain_obs_update_required(); }   // [ENGINE-004-OBS]
"""

# Enregistrement du vidage : pose dans la tete de ggml_backend_cuda_graph_compute, apres l'ordinal.
ANCHOR_ATEXIT = """    // [ENGINE-004-OBS] ordinal de calcul : chaque calcul de graphe recoit un numero.
    // C'est lui, et lui seul, qui date les evenements et les classes dans la fenetre.
    ggml_obrain_obs_begin_compute();
"""

REPL_ATEXIT = """    // [ENGINE-004-OBS] ordinal de calcul : chaque calcul de graphe recoit un numero.
    // C'est lui, et lui seul, qui date les evenements et les classes dans la fenetre.
    ggml_obrain_obs_begin_compute();
    // [ENGINE-004-OBS] enregistre le vidage de fin de processus. Sans cet appel, l'editeur de
    // liens elimine le vidage (inline jamais reference) : compteurs justes et muets.
    ggml_obrain_obs_install_atexit();
"""

ANCHOR_BEGIN_CAPTURE = """        CUDA_CHECK(cudaStreamBeginCapture(cuda_ctx->stream(), cudaStreamCaptureModeRelaxed));"""

REPL_BEGIN_CAPTURE = """        CUDA_CHECK(cudaStreamBeginCapture(cuda_ctx->stream(), cudaStreamCaptureModeRelaxed));
        // [ENGINE-004-OBS] marqueur de capture, avec uid ET cle de graphe (l'ancien log ne donnait que l'uid).
        ggml_obrain_obs_capture_begin(cgraph->uid, (uint64_t) (uintptr_t) graph_key, cgraph->n_nodes);"""

ANCHOR_WARMUP_RESET = """                    graph->warmup_complete = false;
                    GGML_LOG_DEBUG("%s: CUDA graph warmup reset\\n", __func__);"""

REPL_WARMUP_RESET = """                    graph->warmup_complete = false;
                    ggml_obrain_obs_warmup_reset();   // [ENGINE-004-OBS]
                    GGML_LOG_DEBUG("%s: CUDA graph warmup reset\\n", __func__);"""

# ------------------------------------------------------------------ 8. suppression du log par token

ANCHOR_REUSED_LOG = """        GGML_LOG_DEBUG("CUDA Graph id %zu reused\\n", cgraph->uid);
"""

REPL_REUSED_LOG = """        // [ENGINE-004-OBS] PER_TOKEN_LIBRARY_TEXT_LOGGING=0 : cet appel imprimait une ligne
        // par token sur stderr. Il est retire du chemin rapide. Aucun autre effet : le
        // comportement des graphes, des lancements et des compteurs est inchange.
"""


def find_replace(text, anchor, repl, name):
    n = text.count(anchor)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE (%s) : %d occurrence(s)" % (name, n))
    return text.replace(anchor, repl, 1)


def line_replace(text, anchor, repl, name):
    n = text.count(anchor)
    if n != 1:
        raise SystemExit("LIGNE INTROUVABLE OU AMBIGUE (%s) : %d occurrence(s)" % (name, n))
    return text.replace(anchor, repl, 1)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = [a for a in sys.argv[1:] if a.startswith("--")]
    if len(args) != 1:
        raise SystemExit("usage: apply-observer.py <ggml-cuda.cu> [--suppress-log]")
    path = args[0]
    suppress = "--suppress-log" in flags

    with open(path, "r", encoding="utf-8") as f:
        text = f.read()

    if "ggml-cuda-observe.cuh" in text:
        raise SystemExit("DEJA PATCHE : %s" % path)

    steps = [
        (ANCHOR_COMPUTE_HEAD, REPL_COMPUTE_HEAD, "compute_ordinal"),
        (ANCHOR_UPDATE_REQUIRED, REPL_UPDATE_REQUIRED, "update_required (tete + fast path)"),
        (ANCHOR_NODE_COUNT, REPL_NODE_COUNT, "champ NODE_COUNT"),
        (ANCHOR_MEMCMP, REPL_MEMCMP, "qualification du champ fautif"),
        (ANCHOR_RETURN_RES, REPL_RETURN_RES, "emission de l'evenement"),
        (ANCHOR_UPDATE_EXECUTABLE_TAIL, REPL_UPDATE_EXECUTABLE_TAIL, "reinstantiation"),
        (ANCHOR_STREAM_END_CAPTURE, REPL_STREAM_END_CAPTURE, "capture_end"),
        (ANCHOR_BEGIN_CAPTURE, REPL_BEGIN_CAPTURE, "capture_begin"),
        (ANCHOR_INSTANTIATE, REPL_INSTANTIATE, "instantiate"),
        (ANCHOR_EXEC_UPDATE_CALL, REPL_EXEC_UPDATE_CALL, "exec_update"),
        (ANCHOR_LAUNCH, REPL_LAUNCH, "launch"),
        (ANCHOR_DIRECT_EVAL, REPL_DIRECT_EVAL, "direct_eval"),
        (ANCHOR_COMPUTE_UPD_REQ, REPL_COMPUTE_UPD_REQ, "update_required"),
        (ANCHOR_ATEXIT, REPL_ATEXIT, "atexit"),
        (ANCHOR_WARMUP_RESET, REPL_WARMUP_RESET, "warmup_reset"),
    ]

    if ANCHOR_V3_TELEMETRY_END in text:
        text = find_replace(text, ANCHOR_V3_TELEMETRY_END, REPL_V3_TELEMETRY_END,
                            "include (apres telemetrie V3)")
        print("OK  include (apres telemetrie V3, additif)")
    else:
        text = find_replace(text, ANCHOR_INCLUDE, REPL_INCLUDE, "include")
        print("OK  include")

    for anchor, repl, name in steps:
        text = find_replace(text, anchor, repl, name)
        print("OK  %s" % name)

    if suppress:
        text = line_replace(text, ANCHOR_REUSED_LOG, REPL_REUSED_LOG, "suppression du log par token")
        print("OK  suppression du log 'CUDA Graph id ... reused'")
    else:
        print("--  log par token CONSERVE (PER_TOKEN_LIBRARY_TEXT_LOGGING=1)")

    tmp = path + ".engine004-tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(tmp, path)
    print("ECRIT %s (%d octets)" % (path, len(text)))
    print("MARQUEURS=%d" % text.count("[ENGINE-004-OBS]"))


if __name__ == "__main__":
    main()
