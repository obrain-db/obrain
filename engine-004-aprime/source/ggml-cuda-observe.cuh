// ggml-cuda-observe.cuh — observateur de transitions de CUDA Graph (ENGINE-004 / A'')
//
// Objet : nommer CE QUI invalide le graphe, et dater chaque evenement par ORDINAL DE CALCUL
// (jamais par le temps, jamais par une lecture de compteur par token).
//
// Regles dures :
//   - aucune I/O, aucune allocation dynamique dans le chemin chaud (anneau statique prealloue) ;
//   - compteurs atomiques relaxed uniquement, aucun ordonnancement ajoute ;
//   - le vidage se fait APRES la fenetre, en sortie de processus ou explicitement ;
//   - la correspondance compteur <-> trace se fait sur l'ordinal, seul juge recevable.
//
// Inerte sans GGML_OBSERVE=1 : cout nul quand desactive.

#pragma once

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

// ------------------------------------------------------------------ vocabulaire ferme

enum ggml_obrain_obs_field {
    GGML_OBS_FIELD_NONE            = 0,
    GGML_OBS_FIELD_UID             = 1,
    GGML_OBS_FIELD_NODE_COUNT      = 2,
    GGML_OBS_FIELD_NODE_TENSOR_BYTES = 3,
    GGML_OBS_FIELD_SRC_DATA_PTR    = 4,
    GGML_OBS_FIELD_SRC_SHAPE_NE    = 5,
    GGML_OBS_FIELD_SRC_STRIDE_NB   = 6,
    GGML_OBS_FIELD_OTHER           = 7,
};

inline const char * ggml_obrain_obs_field_name(int f) {
    switch (f) {
        case GGML_OBS_FIELD_UID:               return "UID";
        case GGML_OBS_FIELD_NODE_COUNT:        return "NODE_COUNT";
        case GGML_OBS_FIELD_NODE_TENSOR_BYTES: return "NODE_TENSOR_BYTES";
        case GGML_OBS_FIELD_SRC_DATA_PTR:      return "SRC_DATA_PTR";
        case GGML_OBS_FIELD_SRC_SHAPE_NE:      return "SRC_SHAPE_NE";
        case GGML_OBS_FIELD_SRC_STRIDE_NB:     return "SRC_STRIDE_NB";
        case GGML_OBS_FIELD_OTHER:             return "OTHER";
        default:                               return "NONE";
    }
}

// Precedence FIXE et documentee quand plusieurs champs different sur le meme noeud :
// DATA_PTR (le pointeur source a bouge) > SHAPE_NE > STRIDE_NB > NODE_TENSOR_BYTES > OTHER.
// Un seul champ est publie par evenement : le premier de cette liste qui differe.

// ------------------------------------------------------------------ anneau d'evenements

struct ggml_obrain_obs_events {
    static constexpr int MAX = 4096;   // dimensionne STATIQUEMENT (aucune allocation en fenetre)

    struct evt {
        uint64_t ordinal       = 0;      // ordinal de calcul (compte par l'observateur)
        uint64_t uid           = 0;      // cgraph->uid
        uint64_t cached_uid    = 0;      // uid memorise dans le graphe avant l'appel
        uint64_t graph_key     = 0;      // (uintptr_t) premier noeud du graphe = cle de cache
        uint32_t node_index    = 0xFFFFFFFFu;
        uint32_t n_nodes       = 0;
        uint8_t  kind          = 0;      // 1 = properties_changed, 2 = capture_begin, 3 = capture_end,
                                         // 4 = instantiate, 5 = warmup_reset, 6 = launch_no_graph
        uint8_t  field         = 0;      // ggml_obrain_obs_field
        uint8_t  reserved[2]   = {};
        uint64_t old_data_ptr  = 0;
        uint64_t new_data_ptr  = 0;
        int64_t  old_ne[4]     = {};
        int64_t  new_ne[4]     = {};
        int64_t  old_nb[4]     = {};
        int64_t  new_nb[4]     = {};
        uint64_t old_bytes     = 0;      // bytes du noeud lui-meme
        uint64_t new_bytes     = 0;
        char     node_name[48] = {};
    };

    std::atomic<uint64_t> seq { 0 };
    evt                   ring[MAX] = {};

    void push(const evt & e) {
        const uint64_t s = seq.fetch_add(1, std::memory_order_relaxed);
        if (s >= (uint64_t) MAX) {
            return;   // anneau plein : les compteurs restent exacts, l'echantillon est borne et declare
        }
        ring[s] = e;
    }

    int  dump_count() const {
        const uint64_t n = seq.load(std::memory_order_relaxed);
        return (int) (n < (uint64_t) MAX ? n : (uint64_t) MAX);
    }
    uint64_t total() const { return seq.load(std::memory_order_relaxed); }
};

// ------------------------------------------------------------------ etat global

struct ggml_obrain_observe {
    std::atomic<uint64_t> properties_changed_true   { 0 };
    std::atomic<uint64_t> properties_changed_nodes  { 0 };
    std::atomic<uint64_t> update_required_true      { 0 };
    std::atomic<uint64_t> capture_begin             { 0 };
    std::atomic<uint64_t> capture_end               { 0 };
    std::atomic<uint64_t> instantiate               { 0 };
    std::atomic<uint64_t> exec_update               { 0 };
    std::atomic<uint64_t> exec_update_reinstantiate { 0 };
    std::atomic<uint64_t> launch                    { 0 };
    std::atomic<uint64_t> no_update_launch          { 0 };
    std::atomic<uint64_t> launch_no_graph           { 0 };
    std::atomic<uint64_t> warmup_reset              { 0 };
    std::atomic<uint64_t> uid_reused                { 0 };
    std::atomic<uint64_t> frozen_writes             { 0 };

    std::atomic<uint64_t> compute_ordinal { 0 };
    std::atomic<uint64_t> warmup_begin    { 0 };
    std::atomic<uint64_t> warmup_end      { 0 };

    std::atomic<uint64_t> window_begin { 0 };   // 0 = pas encore ouverte
    std::atomic<uint64_t> window_end   { 0 };   // 0 = pas encore fermee

    std::atomic<bool>     frozen { false };

    ggml_obrain_obs_events events;
};

inline ggml_obrain_observe & ggml_obrain_obs() {
    static ggml_obrain_observe s;
    return s;
}

// Ordinal AVANT d'ecrire un evenement : l'evenement porte le numero du calcul en cours.
inline uint64_t ggml_obrain_obs_ordinal() {
    return ggml_obrain_obs().compute_ordinal.load(std::memory_order_relaxed);
}

// ------------------------------------------------------------------ configuration

inline bool ggml_obrain_obs_armed() {
    static const bool on = getenv("GGML_OBSERVE") != nullptr;
    return on;
}

inline const char * ggml_obrain_obs_out_path() {
    static const char * p = getenv("GGML_OBSERVE_OUT");
    return p;
}

inline bool ggml_obrain_obs_verbose() {
    static const bool on = getenv("GGML_OBSERVE_VERBOSE") != nullptr;
    return on;
}

inline void ggml_obrain_obs_freeze(bool on) {
    ggml_obrain_obs().frozen.store(on, std::memory_order_relaxed);
}

// Toute I/O d'observation passe par ce garde : refusee et comptee pendant la fenetre.
inline bool ggml_obrain_obs_write_allowed() {
    ggml_obrain_observe & o = ggml_obrain_obs();
    if (o.frozen.load(std::memory_order_relaxed)) {
        o.frozen_writes.fetch_add(1, std::memory_order_relaxed);
        return false;
    }
    return true;
}

// ------------------------------------------------------------------ points d'observation

// Appele par le harnais au debut de CHAQUE calcul de graphe : incremente l'ordinal.
// Sans cela, les evenements ne sont pas datables et la correspondance compteur/trace est impossible.
inline void ggml_obrain_obs_begin_compute() {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_obs().compute_ordinal.fetch_add(1, std::memory_order_relaxed);
}

// Bornes de la fenetre mesuree, posees par le harnais (aucune lecture de compteur, aucun I/O).
inline void ggml_obrain_obs_window_begin() {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_observe & o = ggml_obrain_obs();
    o.window_begin.store(o.compute_ordinal.load(std::memory_order_relaxed) + 1, std::memory_order_relaxed);
}

inline void ggml_obrain_obs_window_end() {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_observe & o = ggml_obrain_obs();
    o.window_end.store(o.compute_ordinal.load(std::memory_order_relaxed) + 1, std::memory_order_relaxed);
}

inline void ggml_obrain_obs_begin_warmup() {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_obs().warmup_begin.store(ggml_obrain_obs_ordinal(), std::memory_order_relaxed);
}

inline void ggml_obrain_obs_end_warmup() {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_obs().warmup_end.store(ggml_obrain_obs_ordinal(), std::memory_order_relaxed);
}

inline void ggml_obrain_obs_reset_counters() {
    ggml_obrain_observe & o = ggml_obrain_obs();
    o.properties_changed_true.store(0);
    o.properties_changed_nodes.store(0);
    o.update_required_true.store(0);
    o.capture_begin.store(0);
    o.capture_end.store(0);
    o.instantiate.store(0);
    o.exec_update.store(0);
    o.exec_update_reinstantiate.store(0);
    o.launch.store(0);
    o.no_update_launch.store(0);
    o.launch_no_graph.store(0);
    o.warmup_reset.store(0);
    o.uid_reused.store(0);
    o.frozen_writes.store(0);
    o.events.seq.store(0);
}

// ------------------------------------------- evenements riches (appeles UNIQUEMENT sur transition)

struct ggml_obrain_obs_changed_node {
    uint32_t node_index = 0xFFFFFFFFu;
    uint8_t  field      = GGML_OBS_FIELD_NONE;
    uint64_t old_data_ptr = 0, new_data_ptr = 0;
    int64_t  old_ne[4] = {}, new_ne[4] = {};
    int64_t  old_nb[4] = {}, new_nb[4] = {};
    uint64_t old_bytes = 0, new_bytes = 0;
    const char * node_name = "";
};

inline void ggml_obrain_obs_event_transition(const ggml_obrain_obs_changed_node & c,
                                             int field, uint64_t uid, uint64_t cached_uid,
                                             uint64_t graph_key, int n_nodes) {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_observe & o = ggml_obrain_obs();

    o.properties_changed_true.fetch_add(1, std::memory_order_relaxed);
    if (c.node_index != 0xFFFFFFFFu) {
        o.properties_changed_nodes.fetch_add(1, std::memory_order_relaxed);
    }

    ggml_obrain_obs_events::evt e;
    e.ordinal     = ggml_obrain_obs_ordinal();
    e.uid         = uid;
    e.cached_uid  = cached_uid;
    e.graph_key   = graph_key;
    e.node_index  = c.node_index;
    e.n_nodes     = (uint32_t) n_nodes;
    e.kind        = 1;
    e.field       = (uint8_t) field;
    e.old_data_ptr = c.old_data_ptr;
    e.new_data_ptr = c.new_data_ptr;
    for (int i = 0; i < 4; ++i) {
        e.old_ne[i] = c.old_ne[i];
        e.new_ne[i] = c.new_ne[i];
        e.old_nb[i] = c.old_nb[i];
        e.new_nb[i] = c.new_nb[i];
    }
    e.old_bytes = c.old_bytes;
    e.new_bytes = c.new_bytes;
    if (c.node_name) {
        std::snprintf(e.node_name, sizeof(e.node_name), "%s", c.node_name);
    }
    o.events.push(e);
}

inline void ggml_obrain_obs_event_marker(int kind, uint64_t uid, uint64_t graph_key, int n_nodes,
                                         int field, uint64_t old_bytes, uint64_t new_bytes) {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_observe & o = ggml_obrain_obs();
    ggml_obrain_obs_events::evt e;
    e.ordinal   = ggml_obrain_obs_ordinal();
    e.uid       = uid;
    e.graph_key = graph_key;
    e.n_nodes   = (uint32_t) n_nodes;
    e.kind      = (uint8_t) kind;
    e.field     = (uint8_t) field;
    e.old_bytes = old_bytes;
    e.new_bytes = new_bytes;
    o.events.push(e);
}

inline void ggml_obrain_obs_fast_path_reuse(uint64_t uid, uint64_t graph_key, int n_nodes) {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_observe & o = ggml_obrain_obs();
    o.uid_reused.fetch_add(1, std::memory_order_relaxed);
    if (ggml_obrain_obs_verbose() && ggml_obrain_obs_write_allowed()) {
        std::fprintf(stderr, "[obs] ord=%llu uid=%llu key=%llu reused (fast path)\n",
                     (unsigned long long) ggml_obrain_obs_ordinal(), (unsigned long long) uid,
                     (unsigned long long) graph_key);
    }
    (void) n_nodes;
}

inline void ggml_obrain_obs_update_required() {
    if (ggml_obrain_obs_armed()) {
        ggml_obrain_obs().update_required_true.fetch_add(1, std::memory_order_relaxed);
    }
}

inline void ggml_obrain_obs_capture_begin(uint64_t uid, uint64_t graph_key, int n_nodes) {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_obs().capture_begin.fetch_add(1, std::memory_order_relaxed);
    ggml_obrain_obs_event_marker(2, uid, graph_key, n_nodes, GGML_OBS_FIELD_NONE, 0, 0);
}

inline void ggml_obrain_obs_capture_end() {
    if (ggml_obrain_obs_armed()) {
        ggml_obrain_obs().capture_end.fetch_add(1, std::memory_order_relaxed);
    }
}

inline void ggml_obrain_obs_instantiate() {
    if (ggml_obrain_obs_armed()) {
        ggml_obrain_obs().instantiate.fetch_add(1, std::memory_order_relaxed);
    }
}

inline void ggml_obrain_obs_exec_update(bool reinstantiated) {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_observe & o = ggml_obrain_obs();
    o.exec_update.fetch_add(1, std::memory_order_relaxed);
    if (reinstantiated) {
        o.exec_update_reinstantiate.fetch_add(1, std::memory_order_relaxed);
    }
}

inline void ggml_obrain_obs_reinstantiate() {
    if (ggml_obrain_obs_armed()) {
        ggml_obrain_obs().exec_update_reinstantiate.fetch_add(1, std::memory_order_relaxed);
    }
}

// Semantique ALIGNEE sur la telemetrie engine-003 V3 (verifiee sur la source figee) :
//   launch           = nombre d'appels cudaGraphLaunch (lancements VIA graphe)
//   no_update_launch = lancements via graphe SANS update requis (instance presente)
//   launch_no_graph  = chemins d'evaluation directe (aucun appel cudaGraphLaunch)
// Ne PAS confondre les trois : c'est ce melange qui avait produit un no_update_launch=0 faux.
inline void ggml_obrain_obs_launch(bool via_graph, bool instance_present, bool update_required) {
    if (!ggml_obrain_obs_armed()) {
        return;
    }
    ggml_obrain_observe & o = ggml_obrain_obs();
    if (!via_graph) {
        o.launch_no_graph.fetch_add(1, std::memory_order_relaxed);
        return;
    }
    o.launch.fetch_add(1, std::memory_order_relaxed);
    if (instance_present && !update_required) {
        o.no_update_launch.fetch_add(1, std::memory_order_relaxed);
    }
}

inline void ggml_obrain_obs_warmup_reset() {
    if (ggml_obrain_obs_armed()) {
        ggml_obrain_obs().warmup_reset.fetch_add(1, std::memory_order_relaxed);
    }
}

// ------------------------------------------------------------------ restitution

// Compteurs seuls, sans evenements : lisible par le harnais APRES la fenetre.
// o[0..15] = pc, pc_nodes, urT, cb, ce, inst, upd, upd_reinst, launch, nol, launch_no_graph,
//            warmup_reset, uid_reused, frozen_writes, compute_ordinal, uid_reused_events
inline void ggml_obrain_obs_counters(double * o16) {
    ggml_obrain_observe & o = ggml_obrain_obs();
    o16[0]  = (double) o.properties_changed_true.load(std::memory_order_relaxed);
    o16[1]  = (double) o.properties_changed_nodes.load(std::memory_order_relaxed);
    o16[2]  = (double) o.update_required_true.load(std::memory_order_relaxed);
    o16[3]  = (double) o.capture_begin.load(std::memory_order_relaxed);
    o16[4]  = (double) o.capture_end.load(std::memory_order_relaxed);
    o16[5]  = (double) o.instantiate.load(std::memory_order_relaxed);
    o16[6]  = (double) o.exec_update.load(std::memory_order_relaxed);
    o16[7]  = (double) o.exec_update_reinstantiate.load(std::memory_order_relaxed);
    o16[8]  = (double) o.launch.load(std::memory_order_relaxed);
    o16[9]  = (double) o.no_update_launch.load(std::memory_order_relaxed);
    o16[10] = (double) o.launch_no_graph.load(std::memory_order_relaxed);
    o16[11] = (double) o.warmup_reset.load(std::memory_order_relaxed);
    o16[12] = (double) o.uid_reused.load(std::memory_order_relaxed);
    o16[13] = (double) o.frozen_writes.load(std::memory_order_relaxed);
    o16[14] = (double) o.compute_ordinal.load(std::memory_order_relaxed);
    o16[15] = (double) o.events.total();
}

enum { GGML_OBS_PHASE_PREWINDOW = 0, GGML_OBS_PHASE_MEASURED = 1, GGML_OBS_PHASE_POSTWINDOW = 2 };

inline int ggml_obrain_obs_phase_of(uint64_t ordinal, uint64_t wb, uint64_t we) {
    if (wb == 0) {
        return GGML_OBS_PHASE_PREWINDOW;   // pas de fenetre declaree : tout est pre-fenetre
    }
    if (ordinal < wb) {
        return GGML_OBS_PHASE_PREWINDOW;
    }
    if (we != 0 && ordinal >= we) {
        return GGML_OBS_PHASE_POSTWINDOW;
    }
    return GGML_OBS_PHASE_MEASURED;
}

inline void ggml_obrain_obs_dump(FILE * f) {
    ggml_obrain_observe & o = ggml_obrain_obs();

    const uint64_t wb = o.window_begin.load(std::memory_order_relaxed);
    const uint64_t we = o.window_end.load(std::memory_order_relaxed);
    const uint64_t cb = o.capture_begin.load(std::memory_order_relaxed);
    const uint64_t ce = o.capture_end.load(std::memory_order_relaxed);
    const uint64_t pc = o.properties_changed_true.load(std::memory_order_relaxed);

    std::fprintf(f, "OBSERVER_RING_CAPACITY=%d\n", ggml_obrain_obs_events::MAX);
    std::fprintf(f, "OBSERVER_RING_RESERVED_BYTES=%zu\n", sizeof(o.events.ring));
    std::fprintf(f, "OBSERVER_COMPUTE_ORDINAL_TOTAL=%llu\n",
                 (unsigned long long) o.compute_ordinal.load(std::memory_order_relaxed));
    std::fprintf(f, "WINDOW_BEGIN_COMPUTE_ORDINAL=%llu\n", (unsigned long long) wb);
    std::fprintf(f, "WINDOW_END_COMPUTE_ORDINAL=%llu\n", (unsigned long long) we);
    std::fprintf(f, "WARMUP_BEGIN_COMPUTE_ORDINAL=%llu\n",
                 (unsigned long long) o.warmup_begin.load(std::memory_order_relaxed));
    std::fprintf(f, "WARMUP_END_COMPUTE_ORDINAL=%llu\n",
                 (unsigned long long) o.warmup_end.load(std::memory_order_relaxed));

    std::fprintf(f, "properties_changed_true=%llu\n", (unsigned long long) pc);
    std::fprintf(f, "properties_changed_nodes=%llu\n",
                 (unsigned long long) o.properties_changed_nodes.load(std::memory_order_relaxed));
    std::fprintf(f, "update_required_true=%llu\n",
                 (unsigned long long) o.update_required_true.load(std::memory_order_relaxed));
    std::fprintf(f, "capture_begin=%llu\n", (unsigned long long) cb);
    std::fprintf(f, "capture_end=%llu\n", (unsigned long long) ce);
    std::fprintf(f, "instantiate=%llu\n", (unsigned long long) o.instantiate.load(std::memory_order_relaxed));
    std::fprintf(f, "exec_update=%llu\n", (unsigned long long) o.exec_update.load(std::memory_order_relaxed));
    std::fprintf(f, "exec_update_reinstantiate=%llu\n",
                 (unsigned long long) o.exec_update_reinstantiate.load(std::memory_order_relaxed));
    std::fprintf(f, "launch=%llu\n", (unsigned long long) o.launch.load(std::memory_order_relaxed));
    std::fprintf(f, "no_update_launch=%llu\n",
                 (unsigned long long) o.no_update_launch.load(std::memory_order_relaxed));
    std::fprintf(f, "launch_no_graph=%llu\n",
                 (unsigned long long) o.launch_no_graph.load(std::memory_order_relaxed));
    std::fprintf(f, "warmup_reset=%llu\n", (unsigned long long) o.warmup_reset.load(std::memory_order_relaxed));
    std::fprintf(f, "uid_reused=%llu\n", (unsigned long long) o.uid_reused.load(std::memory_order_relaxed));
    std::fprintf(f, "frozen_writes=%llu\n", (unsigned long long) o.frozen_writes.load(std::memory_order_relaxed));

    const int n = o.events.dump_count();
    int n_pre = 0, n_meas = 0, n_post = 0;
    int n_trans_pre = 0, n_trans_meas = 0, n_trans_post = 0;
    int n_capture_pre = 0, n_capture_meas = 0, n_capture_post = 0;

    for (int i = 0; i < n; ++i) {
        const ggml_obrain_obs_events::evt & e = o.events.ring[i];
        const int ph = ggml_obrain_obs_phase_of(e.ordinal, wb, we);
        if (ph == GGML_OBS_PHASE_PREWINDOW) { n_pre++;  if (e.kind == 1) n_trans_pre++;  if (e.kind == 2) n_capture_pre++; }
        if (ph == GGML_OBS_PHASE_MEASURED)  { n_meas++; if (e.kind == 1) n_trans_meas++; if (e.kind == 2) n_capture_meas++; }
        if (ph == GGML_OBS_PHASE_POSTWINDOW){ n_post++; if (e.kind == 1) n_trans_post++; if (e.kind == 2) n_capture_post++; }
    }

    std::fprintf(f, "A2_EVENT_COUNT_PREWINDOW=%d\n", n_pre);
    std::fprintf(f, "A2_EVENT_COUNT_MEASURED_WINDOW=%d\n", n_meas);
    std::fprintf(f, "A2_EVENT_COUNT_POSTWINDOW=%d\n", n_post);
    std::fprintf(f, "A2_TRANSITION_COUNT_PREWINDOW=%d\n", n_trans_pre);
    std::fprintf(f, "A2_TRANSITION_COUNT_MEASURED_WINDOW=%d\n", n_trans_meas);
    std::fprintf(f, "A2_TRANSITION_COUNT_POSTWINDOW=%d\n", n_trans_post);
    std::fprintf(f, "A2_CAPTURE_COUNT_PREWINDOW=%d\n", n_capture_pre);
    std::fprintf(f, "A2_CAPTURE_COUNT_MEASURED_WINDOW=%d\n", n_capture_meas);
    std::fprintf(f, "A2_CAPTURE_COUNT_POSTWINDOW=%d\n", n_capture_post);
    std::fprintf(f, "MEASURED_WINDOW_GRAPH_TRANSITION_EVENT_COUNT=%d\n", n_trans_meas);
    std::fprintf(f, "CUMULATIVE_PREWINDOW_COUNTER_STATE[pc=%llu cb=%llu ce=%llu urT=%llu]\n",
                 (unsigned long long) pc, (unsigned long long) cb, (unsigned long long) ce,
                 (unsigned long long) o.update_required_true.load(std::memory_order_relaxed));

    for (int i = 0; i < n; ++i) {
        const ggml_obrain_obs_events::evt & e = o.events.ring[i];
        if (e.kind == 2) {
            continue;   // les marqueurs de capture sont comptes ci-dessus ; le detail suit en EVENT
        }
        const int ph = ggml_obrain_obs_phase_of(e.ordinal, wb, we);
        const char * phn = ph == GGML_OBS_PHASE_PREWINDOW ? "PREWINDOW"
                         : ph == GGML_OBS_PHASE_MEASURED  ? "MEASURED_WINDOW" : "POSTWINDOW";
        std::fprintf(f, "EVENT_%d_COMPUTE_ORDINAL=%llu\n", i, (unsigned long long) e.ordinal);
        std::fprintf(f, "EVENT_%d_PHASE=%s\n", i, phn);
        std::fprintf(f, "EVENT_%d_KIND=%u\n", i, (unsigned) e.kind);
        std::fprintf(f, "EVENT_%d_GRAPH_KEY=%llu\n", i, (unsigned long long) e.graph_key);
        std::fprintf(f, "EVENT_%d_CGRAPH_UID=%llu\n", i, (unsigned long long) e.uid);
        std::fprintf(f, "EVENT_%d_CACHED_UID=%llu\n", i, (unsigned long long) e.cached_uid);
        std::fprintf(f, "EVENT_%d_N_NODES=%u\n", i, (unsigned) e.n_nodes);
        std::fprintf(f, "EVENT_%d_FIRST_CHANGED_NODE_INDEX=%u\n", i, (unsigned) e.node_index);
        std::fprintf(f, "EVENT_%d_FIRST_CHANGED_NODE=%s\n", i, e.node_name);
        std::fprintf(f, "EVENT_%d_FIRST_CHANGED_FIELD=%s\n", i, ggml_obrain_obs_field_name(e.field));
        if (e.kind == 1) {
            std::fprintf(f, "EVENT_%d_OLD_DATA_PTR=%llu NEW_DATA_PTR=%llu\n", i,
                         (unsigned long long) e.old_data_ptr, (unsigned long long) e.new_data_ptr);
            std::fprintf(f, "EVENT_%d_OLD_NE=[%lld,%lld,%lld,%lld] NEW_NE=[%lld,%lld,%lld,%lld]\n", i,
                         (long long) e.old_ne[0], (long long) e.old_ne[1], (long long) e.old_ne[2], (long long) e.old_ne[3],
                         (long long) e.new_ne[0], (long long) e.new_ne[1], (long long) e.new_ne[2], (long long) e.new_ne[3]);
            std::fprintf(f, "EVENT_%d_OLD_NB=[%lld,%lld,%lld,%lld] NEW_NB=[%lld,%lld,%lld,%lld]\n", i,
                         (long long) e.old_nb[0], (long long) e.old_nb[1], (long long) e.old_nb[2], (long long) e.old_nb[3],
                         (long long) e.new_nb[0], (long long) e.new_nb[1], (long long) e.new_nb[2], (long long) e.new_nb[3]);
            std::fprintf(f, "EVENT_%d_OLD_BYTES=%llu NEW_BYTES=%llu\n", i,
                         (unsigned long long) e.old_bytes, (unsigned long long) e.new_bytes);
        }
    }

    std::fprintf(f, "CUDA_COUNTER_TRANSITION_COUNT=%llu\n", (unsigned long long) pc);
    std::fprintf(f, "OBSERVER_RING_TRANSITION_COUNT=%d\n", n_trans_pre + n_trans_meas + n_trans_post);
    std::fprintf(f, "INVARIANT_CAPTURE_BEGIN_EQ_CAPTURE_END=%s\n", (cb == ce) ? "OK" : "VIOLATED");
    std::fprintf(f, "INVARIANT_NO_IO_DURING_WINDOW=%s\n",
                 o.frozen_writes.load(std::memory_order_relaxed) == 0 ? "OK" : "VIOLATED");
    std::fprintf(f, "INVARIANT_TOTAL_EVENTS_FIT_RING=%s\n",
                 o.events.total() <= (uint64_t) ggml_obrain_obs_events::MAX ? "OK" : "VIOLATED");
}

inline bool ggml_obrain_obs_dump_to(const char * path) {
    if (!ggml_obrain_obs_armed()) {
        return false;
    }
    if (ggml_obrain_obs().frozen.load(std::memory_order_relaxed)) {
        ggml_obrain_obs().frozen_writes.fetch_add(1, std::memory_order_relaxed);
        return false;
    }
    if (path == nullptr) {
        ggml_obrain_obs_dump(stdout);
        return true;
    }
    FILE * f = std::fopen(path, "w");
    if (f == nullptr) {
        return false;
    }
    ggml_obrain_obs_dump(f);
    std::fclose(f);
    return true;
}

struct ggml_obrain_obs_atexit {
    ggml_obrain_obs_atexit() {
        std::atexit([]() {
            if (!ggml_obrain_obs_armed()) {
                return;
            }
            (void) ggml_obrain_obs_dump_to(ggml_obrain_obs_out_path());
        });
    }
};

inline void ggml_obrain_obs_install_atexit() {
    static ggml_obrain_obs_atexit once;
    (void) once;
}

// ------------------------------------------------------------------ surface stable pour le harnais
//
// Les fonctions ci-dessus sont `inline` (en-tete) : l'editeur de liens ne les exporte pas
// forcement depuis une bibliotheque partagee. Le harnais a besoin d'une poignee explicite,
// non inline, donc exportee. Ces enveloppes sont volontairement triviales.

// Les quatre fonctions ci-dessous appellent des fonctions `inline` : sans adresse prise, le
// compilateur les replie et aucun symbole hors-ligne n'apparait dans la bibliotheque (constate :
// references indefinies au link du harnais). On force donc l'emission d'une definition locale.
namespace ggml_obrain_obs_force_emit {
    inline void * volatile force_window_begin  = (void *) &ggml_obrain_obs_window_begin;
    inline void * volatile force_window_end    = (void *) &ggml_obrain_obs_window_end;
    inline void * volatile force_begin_warmup  = (void *) &ggml_obrain_obs_begin_warmup;
    inline void * volatile force_end_warmup    = (void *) &ggml_obrain_obs_end_warmup;
}

void ggml_obrain_obs_driver_window_begin()  { ggml_obrain_obs_window_begin(); }
void ggml_obrain_obs_driver_window_end()    { ggml_obrain_obs_window_end(); }
void ggml_obrain_obs_driver_begin_warmup()  { ggml_obrain_obs_begin_warmup(); }
void ggml_obrain_obs_driver_end_warmup()    { ggml_obrain_obs_end_warmup(); }
void ggml_obrain_obs_driver_freeze(bool on) { ggml_obrain_obs_freeze(on); }
void ggml_obrain_obs_driver_reset()         { ggml_obrain_obs_reset_counters(); }
void ggml_obrain_obs_driver_counters(double * o) { ggml_obrain_obs_counters(o); }
int  ggml_obrain_obs_driver_dump(const char * p) { return ggml_obrain_obs_dump_to(p) ? 1 : 0; }
