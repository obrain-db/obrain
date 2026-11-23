#include "server-context.h"
#include "obrain-kv-cache.h"   // [obrain K1/K2] cache d'etats, partage avec les tests
#include "obrain-client-seq.h" // [obrain P1] registre client -> sequence, partage avec les tests

static int g_obrain_capacity_b = 0;  // [OBRAIN_CAPACITY] 0 = non fourni -> comportement existant (n_max)
#include "obrain-proposer.h"   // [obrain R3-A] proposeur certifie
#include "server-chat.h"
#include "server-common.h"
#include "server-http.h"
#include "server-task.h"
#include "server-queue.h"
#include "server-schema.h"
#include "server-stream.h"

#include "base64.hpp" // [obrain] J-space endpoints — encode float buffers compactly
#include "build-info.h"
#include "common.h"

// [obrain] horodatages d instrumentation du verify live (mesure seule, aucun effet runtime)
static int64_t g_obrain_t_verify0 = 0;
static int64_t g_obrain_t_dec1    = 0;
static int64_t g_obrain_t_post0   = 0;
static int      g_obrain_b        = 0;
static int      g_obrain_sampled  = 0;
#include "fit.h"
#include "llama.h"
#include "log.h"
#include "sampling.h"
#include "speculative.h"
#include "mtmd.h"
#include "mtmd-helper.h"

#include <algorithm>
#include <cstddef>
#include <cinttypes>
#include <exception>
#include <memory>
#include <filesystem>
#include <utility>
#include <fstream>

// fix problem with std::min and std::max
#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#ifndef NOMINMAX
#   define NOMINMAX
#endif
#include <windows.h>
#endif

using json = nlohmann::ordered_json;

constexpr int HTTP_POLLING_SECONDS = 1;

// ═══════════════════════════════════════════════════════════════════════════
// [obrain P1] RESERVATION DE SEQUENCE — UNE SEULE DEFINITION DE LA REGLE.
//
// POURQUOI CES DEUX FONCTIONS SONT LIBRES ET NON DES METHODES : la route
// `/props` doit publier le nombre de sequences REELLEMENT offertes aux
// clients, et elle n'a -- deliberement -- pas acces a l'etat de
// l'implementation (cf. le garde-fou `bool ctx_server;` dans get_props, qui
// interdit de lire un etat vivant depuis un thread HTTP). Le calcul depend
// uniquement de `n_parallel`, donc il vit ici, en UN exemplaire : la classe
// et la route ne peuvent pas diverger sur la taille du pool offert.
// ═══════════════════════════════════════════════════════════════════════════
static int32_t jspace_reserved_slot_id_of(int32_t n_parallel) {
    return n_parallel >= 3 ? n_parallel - 1 : -1;
}

// Nombre de sequences offertes aux clients : le total MOINS la reservee.
static int32_t jspace_slots_offerts_of(int32_t n_parallel) {
    const int32_t r = jspace_reserved_slot_id_of(n_parallel);
    return n_parallel - (r >= 0 ? 1 : 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// J-space geometric analysis helpers [obrain] — PO plan 4749fb72.
// C++ port of the same math already validated in llm-engine's jspace.rs /
// codazzi.rs (see PO notes 5f511bbe, 49299642). Kept deliberately close to
// the Rust originals (same derivation, same rank-2-update trick) rather than
// re-deriving from scratch, to minimize the risk of a subtle divergence.
// ═══════════════════════════════════════════════════════════════════════════

// RMSNorm scale s(x) = sqrt(mean(x^2) + eps).
// [obrain K1-MANIFEST] LE CACHE D'ETATS VIT DESORMAIS DANS obrain-kv-cache.h.
// Ce fichier et les TESTS incluent le MEME code : les tests n'exercent pas une
// copie simplifiee des regles de compatibilite, d'integrite et d'eviction.
static std::atomic<float> g_kv_ms_model_sha{-2.0f};   // mesure SEPAREE du hachage modele
static std::atomic<float> g_kv_ms_blob_sha{-2.0f};    // mesure SEPAREE du hachage du blob
using kv_state_manifest = obrain_kv::Manifest;
using kv_state_blob     = obrain_kv::Blob;

static obrain_kv::Cache  g_kv_cache(obrain_kv::Cache::default_max_bytes());
static std::mutex &      g_kv_states_mtx = g_kv_cache.mtx;

// SHA256 de la libllama REELLEMENT chargee (une fois par processus).
static const std::string & kv_q1_sha256() {
    static const std::string v = []() -> std::string {
        std::ifstream f("/proc/self/maps"); std::string line, path;
        while (std::getline(f, line)) {
            auto q = line.find("libllama.so");
            if (q != std::string::npos) { auto sl = line.find('/'); if (sl != std::string::npos) path = line.substr(sl); }
        }
        while (!path.empty() && (path.back()=='\n'||path.back()==' ')) path.pop_back();
        if (path.empty()) return std::string();
        return obrain_kv::sha256_hex_file(path);
    }();
    return v;
}

// [obrain K1-MANIFEST] SHA256 EXACT du fichier de modele, calcule UNE FOIS par
// processus. La description, la taille et la topologie NE REMPLACENT PAS cette
// identite : deux modeles differents peuvent partager description et topologie.
// [obrain R3] IDENTITE MODELE LIEe A UNE INSTANCE CHARGEE.
// Une NOUVELLE instance de modele INVALIDE l'identite precedente : elle ne la
// reutilise jamais aveuglement. `instance_token` est un identifiant monotone
// fourni par le chargement.
static std::atomic<uint64_t> g_kv_model_instance{0};
static std::mutex            g_kv_model_mtx;
static std::string           g_kv_model_path;
static std::string           g_kv_model_sha;
static uint64_t              g_kv_model_token = 0;

static uint64_t kv_model_instance_new(const std::string & path) {
    std::lock_guard<std::mutex> lk(g_kv_model_mtx);
    g_kv_model_path  = path;
    g_kv_model_sha.clear();                       // identite INVALIDEe
    g_kv_model_token = g_kv_model_instance.fetch_add(1) + 1;
    return g_kv_model_token;
}

// Rend le SHA du modele de CETTE instance. Recalcule si l'instance a change.
static std::string kv_model_sha256(const std::string & path, uint64_t token) {
    {
        std::lock_guard<std::mutex> lk(g_kv_model_mtx);
        if (token == g_kv_model_token && path == g_kv_model_path && !g_kv_model_sha.empty())
            return g_kv_model_sha;               // reutilisation CONTRACTUELLE
    }
    // Hachage HORS du verrou global : une longue operation ne doit pas bloquer
    // les autres threads du cache.
    const std::string sha = obrain_kv::sha256_hex_file(path);
    {
        std::lock_guard<std::mutex> lk(g_kv_model_mtx);
        g_kv_model_path = path; g_kv_model_sha = sha; g_kv_model_token = token;
    }
    return sha;
}

static uint64_t kv_model_token_now() {
    std::lock_guard<std::mutex> lk(g_kv_model_mtx);
    return g_kv_model_token;
}

// Manifeste COURANT, derive du contexte et de la configuration effectifs.
static kv_state_manifest kv_manifest_of(llama_context * ctx, const std::string & model_path,
                                        int32_t seq_id) {
    const llama_model * model = llama_get_model(ctx);
    kv_state_manifest m;
    { char dbuf[512] = {0};
      const int dn = llama_model_desc(model, dbuf, sizeof dbuf);
      m.model_desc = (dn > 0) ? std::string(dbuf) : std::string("<indisponible>"); }
    m.model_bytes  = (size_t) llama_model_size(model);
    { auto _t0 = ggml_time_us();
      m.model_sha256 = kv_model_sha256(model_path, kv_model_token_now());
      g_kv_ms_model_sha.store((float)(ggml_time_us() - _t0) / 1000.0f); }
    m.model_params = (uint64_t) llama_model_n_params(model);
    m.n_vocab      = (int32_t) llama_vocab_n_tokens(llama_model_get_vocab(model));
    m.n_layer      = (int32_t) llama_model_n_layer(model);
    m.n_embd       = (int32_t) llama_model_n_embd(model);
    m.q1_sha256    = kv_q1_sha256();
    m.type_k       = (int32_t) llama_kv_cache_type_k(ctx);
    m.type_v       = (int32_t) llama_kv_cache_type_v(ctx);
    m.n_ctx = (uint32_t) llama_n_ctx(ctx);
    m.n_batch = (uint32_t) llama_n_batch(ctx);
    m.n_ubatch = (uint32_t) llama_n_ubatch(ctx);
    m.n_seq_max = 1;
    { const char * e = std::getenv("OBRAIN_STATE_TYPE");
      const char * f16 = std::getenv("OBRAIN_STATE_F16");
      m.state_mode = (e && e[0]) ? (int32_t) e[0] : ((f16 && f16[0]=='1') ? 1 : 0); }
    m.seq_id = seq_id;
    return m;
}

static std::string kv_ref_of(const std::vector<uint8_t> & data) { return obrain_kv::ref_of(data); }
static std::string kv_sha256_hex(const uint8_t * d, size_t n)  { return obrain_kv::sha256_hex(d, n); }
static bool kv_states_insert(const std::string & ref, kv_state_blob && b) {
    std::lock_guard<std::mutex> lk(g_kv_cache.mtx);
    return g_kv_cache.insert(ref, std::move(b));
}
static bool kv_states_get(const std::string & ref, kv_state_blob & out) {
    std::lock_guard<std::mutex> lk(g_kv_cache.mtx);
    return g_kv_cache.get(ref, out);
}
static std::string kv_manifest_mismatch(const kv_state_manifest & a, const kv_state_manifest & b) {
    return obrain_kv::manifest_mismatch(a, b);
}

// [obrain P1] ETAT OBSERVABLE DU CACHE D'ETATS (octets, entrees, evictions).
//
// POURQUOI IL EXISTE : une borne qu'on ne peut pas LIRE ne se verifie pas.
// Le budget (`OBRAIN_KV_STATES_MAX_BYTES`) et l'eviction LRU etaient deja
// ecrits dans obrain-kv-cache.h ; ce qui manquait, c'est de quoi constater
// sur un serveur VIVANT que le cache evince au lieu de croitre. Ces
// compteurs partent dans la ligne de journal de chaque extraction, ou la
// mesure les lit.
//
// NOTE DE VERROU : on prend le verrou du cache, jamais un verrou externe --
// `g_kv_states_mtx` EST `g_kv_cache.mtx` (non recursif).
struct kv_cache_etat_t {
    size_t   octets      = 0;
    size_t   entrees     = 0;
    uint64_t evictions   = 0;
    uint64_t remplace    = 0;
    uint64_t refuse_trop = 0;
    size_t   budget      = 0;
};
static kv_cache_etat_t kv_cache_etat() {
    std::lock_guard<std::mutex> lk(g_kv_cache.mtx);
    kv_cache_etat_t e;
    e.octets      = g_kv_cache.accounted_bytes();
    e.entrees     = g_kv_cache.size();
    e.evictions   = g_kv_cache.n_evict;
    e.remplace    = g_kv_cache.n_replace;
    e.refuse_trop = g_kv_cache.n_refuse_too_big;
    e.budget      = g_kv_cache.max_bytes;
    return e;
}

// ═══════════════════════════════════════════════════════════════════════════
// [obrain SPLICE-CHAT] RESOLUTION D'UN ETAT PAR `state_ref` — marche 2.
//
// SOURCE UNIQUE, PARTAGEE AVEC /jspace/inject_knowledge : cache memoire borne
// (`g_kv_cache`, via kv_states_get/kv_states_insert) PUIS fichier
// `<dir>/<ref>.bin` au format OBKV. On ne recopie QUE le transport (cache ->
// fichier), jamais les regles : la CONFORMITE et l'INTEGRITE restent jugees par
// obrain_kv::decide_import, la fonction de decision de production deja exercee
// par les tests (obrain-kv-cache-tests.cpp). Il n'existe donc pas deux jeux de
// regles de refus a maintenir.
//
// POURQUOI LA BRANCHE K2 DE jspace N'EST PAS REWIREE ICI : elle porte sa propre
// instrumentation (`ms_lookup`, `ms_blob_sha256`, `ms_state_import`) et ses
// messages d'erreur, qui font partie du contrat observable de la route ; la
// reecrire pour un helper commun ferait courir un risque a une route qui
// fonctionne pour un gain de lisibilite. La logique KV (cache, format disque,
// integrite, decision d'import) est en revanche bien la MEME dans les deux
// chemins.
//
// `why` porte toujours un motif exploitable tel quel dans un HTTP 400. Le cas
// `state_ref` inconnu reprend MOT POUR MOT le message de la route jspace.
// ═══════════════════════════════════════════════════════════════════════════
static bool kv_state_resolve(const std::string & ref, const std::string & kv_dir,
                             std::shared_ptr<const kv_state_blob> & out, std::string & why) {
    if (ref.empty()) {
        why = "state_ref vide";
        return false;
    }

    kv_state_blob blob;
    // [obrain FIX-DEADLOCK] aucun verrou externe : kv_states_get prend DEJA
    // g_kv_cache.mtx (g_kv_states_mtx EST ce meme mutex, non recursif).
    kv_states_get(ref, blob);
    if (blob.data.empty()) {
        // K1b : fallback disque (etat extrait lors d'un run precedent, ou evince
        // du cache memoire). Meme format que la persistance jspace.
        const std::string path = kv_dir + "/" + ref + ".bin";
        std::ifstream f(path, std::ios::binary);
        if (!f) {
            why = "unknown state_ref '" + ref + "' (extract d'abord)";
            return false;
        }
        f.close();
        const obrain_kv::DiskLoad DL = obrain_kv::load_state_file(path);
        if (!DL.ok) {
            why = "disk state unusable: " + DL.why;
            return false;
        }
        // INTEGRITE : le nom de fichier EST la cle ; on VERIFIE le contenu.
        const std::string recomputed = kv_ref_of(DL.blob.data);
        if (recomputed != ref) {
            why = "state integrity mismatch on disk load (refusal)";
            return false;
        }
        blob = DL.blob;
        // Meme mise en cache que jspace : la seconde lecture d'un meme ref (ou
        // l'import par le slot, juste apres) ne repaie pas l'I/O disque.
        kv_states_insert(ref, kv_state_blob{blob.n_tokens, blob.data, blob.manifest});
    }

    if (!blob.manifest) {
        why = "state has NO compatibility manifest (refusal before import)";
        return false;
    }
    out = std::make_shared<kv_state_blob>(std::move(blob));
    return true;
}

// Une ref valide est exactement la sortie de kv_ref_of : 16 hex minuscules.
// Toute autre valeur est refusée AVANT de construire un chemin disque
// (sinon "state_ref": "../../x" lisait un fichier arbitraire).
static bool kv_ref_is_valid(const std::string & ref) {
    if (ref.size() != 16) {
        return false;
    }
    for (char c : ref) {
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) {
            return false;
        }
    }
    return true;
}

static float jspace_rms_scale(const std::vector<float> & x, float eps) {
    double sum_sq = 0.0;
    for (float v : x) sum_sq += (double) v * v;
    return (float) std::sqrt(sum_sq / x.size() + (double) eps);
}

// gamma_gram_{ij} = gamma_i * gamma_j * gram_{ij} — model constant, computed
// once per (model, gamma, gram) triple.
static std::vector<float> jspace_precompute_gamma_gram(const std::vector<float> & gram, const std::vector<float> & gamma) {
    const int n = (int) gamma.size();
    std::vector<float> out((size_t) n * n);
    for (int i = 0; i < n; i++) {
        const float gi = gamma[i];
        for (int j = 0; j < n; j++) {
            out[(size_t) i * n + j] = gi * gamma[j] * gram[(size_t) i * n + j];
        }
    }
    return out;
}

// g_out(x) = P^T @ (gamma_gram / s^2) @ P via the rank-2-update trick — see
// jspace.rs module docs for the full derivation. O(n^2), not O(n^3).
static std::vector<float> jspace_g_out(const std::vector<float> & x, const std::vector<float> & gamma_gram, float eps) {
    const int n = (int) x.size();
    const float s = jspace_rms_scale(x, eps);
    const float inv_s2 = 1.0f / (s * s);
    const float c = 1.0f / ((float) n * s * s);

    std::vector<float> u(n);
    for (int i = 0; i < n; i++) {
        const float * row = &gamma_gram[(size_t) i * n];
        float acc = 0.0f;
        for (int j = 0; j < n; j++) acc += row[j] * x[j];
        u[i] = acc * inv_s2;
    }
    float a = 0.0f;
    for (int i = 0; i < n; i++) a += x[i] * u[i];

    std::vector<float> g((size_t) n * n);
    for (int i = 0; i < n; i++) {
        const float * gram_row = &gamma_gram[(size_t) i * n];
        float * g_row = &g[(size_t) i * n];
        const float xi = x[i], ui = u[i];
        for (int j = 0; j < n; j++) {
            const float m_ij = gram_row[j] * inv_s2;
            g_row[j] = m_ij - c * (xi * u[j] + ui * x[j]) + c * c * a * xi * x[j];
        }
    }
    return g;
}

// Top eigenvalue/eigenvector of a symmetric PSD matrix via power iteration —
// O(n^2) per iteration, no dense linalg dependency needed.
static std::pair<float, std::vector<float>> jspace_top_eigenpair_psd(const std::vector<float> & mat, int n, int n_iters, uint32_t seed) {
    std::vector<float> v(n);
    for (int i = 0; i < n; i++) {
        uint32_t h = (uint32_t) i * 2654435761u + seed * 999983u + 7u;
        v[i] = (float) (h % 1000) / 500.0f - 1.0f;
    }
    float norm = 0.0f;
    for (float x : v) norm += x * x;
    norm = std::sqrt(norm);
    if (norm < 1e-12f) norm = 1e-12f;
    for (float & x : v) x /= norm;

    float lambda = 0.0f;
    std::vector<float> w(n);
    for (int it = 0; it < n_iters; it++) {
        for (int i = 0; i < n; i++) {
            const float * row = &mat[(size_t) i * n];
            float acc = 0.0f;
            for (int j = 0; j < n; j++) acc += row[j] * v[j];
            w[i] = acc;
        }
        float wn = 0.0f;
        for (float x : w) wn += x * x;
        wn = std::sqrt(wn);
        if (wn < 1e-20f) break;
        for (int i = 0; i < n; i++) v[i] = w[i] / wn;
        lambda = wn;
    }
    return { lambda, v };
}

// Deterministic pseudo-random unit direction (not necessarily orthogonal to
// anything in particular — used as a generic "random direction" baseline
// for comparison against the eigenvector direction).
static std::vector<float> jspace_random_direction(int n, uint32_t seed) {
    std::vector<float> v(n);
    for (int i = 0; i < n; i++) {
        uint32_t h = (uint32_t) i * 2654435761u + seed * 83492791u + 13u;
        v[i] = (float) (h % 1000) / 500.0f - 1.0f;
    }
    float norm = 0.0f;
    for (float x : v) norm += x * x;
    norm = std::sqrt(norm);
    if (norm < 1e-12f) norm = 1e-12f;
    for (float & x : v) x /= norm;
    return v;
}

// [OBRAIN_CAPACITY] definition unique du contrat : 2..49, invalide = FATAL, absent = 0 (chemin inchange).
static int obrain_capacity_from_env() {
    const char * cb = getenv("OBRAIN_CAPACITY_B");
    if (cb == nullptr) { return 0; }
    char * fin = nullptr;
    const long v = strtol(cb, &fin, 10);
    if (fin == cb || *fin != 0 || v < 2 || v > 49) {
        fprintf(stderr, "FATAL OBRAIN_CAPACITY_B invalide : %s (domaine 2..49)\n", cb);
        exit(2);
    }
    return (int) v;
}

static uint32_t server_n_outputs_max(const common_params & params) {
    const uint32_t n_batch  = params.n_batch;

    if (params.embedding ||
            (params.pooling_type != LLAMA_POOLING_TYPE_UNSPECIFIED && params.pooling_type != LLAMA_POOLING_TYPE_NONE)) {
        return n_batch;
    }

    uint32_t n_outputs_per_seq = 1 + common_speculative_n_max(&params.speculative);   // MTP conserve
    const int ob_cap = obrain_capacity_from_env();   // [OBRAIN] helper unique du contrat (patch 1)
    if (ob_cap >= 2) { n_outputs_per_seq = std::max<uint32_t>(n_outputs_per_seq, (uint32_t) ob_cap); }

    const uint64_t n_outputs = (uint64_t) params.n_parallel * n_outputs_per_seq;

    return std::max<uint32_t>(1, std::min<uint64_t>(n_batch, n_outputs));
}

// state diagram: https://github.com/ggml-org/llama.cpp/pull/9283
enum slot_state {
    SLOT_STATE_IDLE,
    SLOT_STATE_WAIT_OTHER, // after assigning a task, but waiting for parent slot to process prompt
    SLOT_STATE_STARTED,    // after assigning a task and about to process prompt
    SLOT_STATE_PROCESSING_PROMPT,
    SLOT_STATE_DONE_PROMPT,
    SLOT_STATE_GENERATING,
};

struct server_slot; // forward declaration

struct server_batch {
    llama_batch batch;
    bool batch_rendered = false;

    struct token {
        int32_t id_slot;
        llama_token token;
        llama_pos pos;
        bool output;
    };
    std::vector<token> tokens;
    int32_t n_tokens_alloc = 0;

    // track if given slot can be batched with slots already in the batch
    server_slot * slot_batched = nullptr;

    float  alora_scale       = -1.0f;
    size_t alora_disabled_id = 0;

    server_batch() {
        batch.token = nullptr; // sentinel: uninitialized batch
    }

    ~server_batch() {
        if (batch.token != nullptr) {
            llama_batch_free(batch);
        }
    }

    void init(int32_t n_tokens_alloc) {
        this->n_tokens_alloc = n_tokens_alloc;
        batch = llama_batch_init(n_tokens_alloc, 0, 1);
        tokens.reserve(n_tokens_alloc);
    }

    bool add(int32_t id_slot, llama_token token, llama_pos pos, bool output) {
        GGML_ASSERT(batch.token != nullptr);
        if ((int32_t)tokens.size() >= n_tokens_alloc) {
            return false;
        }
        tokens.push_back({ id_slot, token, pos, output });
        return true;
    }

    void clear() {
        tokens.clear();
        common_batch_clear(batch);
        slot_batched      = nullptr;
        alora_scale       = -1.0f;
        alora_disabled_id = 0;
        batch_rendered    = false;
    }

    int32_t size() const {
        return (int32_t)tokens.size();
    }

    void set_output(int32_t idx, bool output) {
        GGML_ASSERT(idx >= 0 && idx < (int32_t)tokens.size());
        tokens[idx].output = output;
    }

    void render() {
        GGML_ASSERT(batch.token != nullptr);
        common_batch_clear(batch);
        for (int32_t i = 0; i < size(); i++) {
            const auto & t = tokens[i];
            common_batch_add(batch, t.token, t.pos, { t.id_slot }, t.output);
        }
        batch_rendered = true;
    }

    llama_batch get_view(int32_t off, int32_t n_tokens) const {
        GGML_ASSERT(batch.token != nullptr);
        GGML_ASSERT(batch_rendered);
        GGML_ASSERT(off >= 0 && off < size());
        GGML_ASSERT(n_tokens > 0 && off + n_tokens <= size());

        llama_batch view = {
            n_tokens,
            batch.token    + off,
            nullptr,
            batch.pos      + off,
            batch.n_seq_id + off,
            batch.seq_id   + off,
            batch.logits   + off,
        };

        return view;
    }
};

struct server_slot {
    int id;

    llama_context * ctx_tgt = nullptr;
    llama_context * ctx_dft = nullptr;

    // multimodal
    mtmd_context * mctx = nullptr;
    mtmd::batch_ptr mbatch = nullptr;

    // speculative decoding
    common_speculative * spec;

    llama_tokens spec_draft;
    llama_tokens spec_prompt;
    std::vector<int32_t> spec_i_batch;
    common_prompt_checkpoint spec_ckpt;

    // TODO: move members that belong to the task (such as `generated_text`, `has_new_line`) to task_results_state
    //       see https://github.com/ggml-org/llama.cpp/pull/18283#issuecomment-3710175837
    std::unique_ptr<const server_task> task;
    std::unique_ptr<const server_task> task_prev; // used for debugging

    // used to determine the slot that has been used the longest
    int64_t t_last_used = -1;

    // generation props
    int32_t n_ctx       = 0;  // context size per slot
    int32_t n_keep      = 0;
    int32_t n_decoded   = 0;
    int32_t n_remaining = -1;
    int32_t i_batch     = -1;

    int32_t n_prompt_tokens_cache     = 0;
    int32_t n_prompt_tokens_processed = 0;

    // [obrain SPLICE-CHAT] nombre de tokens du prefixe KV importe pour la tache
    // COURANTE (0 = aucun splice). Renseigne au tout debut du prefill, remis a
    // 0 par reset() : il ne peut donc pas fuir vers la tache suivante du slot.
    int32_t n_state_prefix = 0;

    size_t last_nl_pos = 0;

    std::string  generated_text;
    std::string  debug_generated_text;
    llama_tokens generated_tokens;

    std::vector<completion_token_output> generated_token_probs;

    bool has_next_token = true;
    bool has_new_line   = false;
    bool truncated      = false;

    stop_type stop;

    std::string stopping_word;

    // state
    slot_state state = SLOT_STATE_IDLE;

    server_prompt prompt;

    bool prompt_save(server_prompt_cache & prompt_cache) const {
        if (prompt.tokens.size() == 0) {
            return false;
        }

        GGML_ASSERT(prompt.data.size() == 0);

        const size_t cur_size_tgt =           llama_state_seq_get_size_ext(ctx_tgt, id, LLAMA_STATE_SEQ_FLAGS_NONE);
        const size_t cur_size_dft = ctx_dft ? llama_state_seq_get_size_ext(ctx_dft, id, LLAMA_STATE_SEQ_FLAGS_NONE) : 0;

        const size_t cur_size = cur_size_tgt + cur_size_dft;

        SRV_TRC(" - saving prompt with length %d, total state size = %.3f MiB (draft: %.3f MiB)\n",
                (int) prompt.tokens.size(), cur_size / (1024.0 * 1024.0), cur_size_dft / (1024.0 * 1024.0));

        auto * cur = prompt_cache.alloc(prompt, cur_size_tgt, cur_size_dft);
        if (cur == nullptr) {
            return false;
        }

        llama_state_seq_get_data_ext(ctx_tgt, cur->data.main.data(), cur_size_tgt, id, LLAMA_STATE_SEQ_FLAGS_NONE);
        if (ctx_dft) {
            llama_state_seq_get_data_ext(ctx_dft, cur->data.drft.data(), cur_size_dft, id, LLAMA_STATE_SEQ_FLAGS_NONE);
        }

        return true;
    }

    bool prompt_load(server_prompt_cache & prompt_cache, const server_tokens & tokens) {
        bool res = prompt_cache.load(prompt, tokens, ctx_tgt, ctx_dft, id);
        if (!res) {
            SLT_WRN(*this, "%s", "failed to load prompt from cache\n");
        }

        return res;
    }

    void prompt_clear(bool allow_processing) {
        if (!allow_processing) {
            GGML_ASSERT(!is_processing());
        }

        SLT_TRC(*this, "clearing prompt with %zu tokens\n", prompt.tokens.size());

        common_context_seq_rm(ctx_tgt, id, -1, -1);
        if (ctx_dft) {
            common_context_seq_rm(ctx_dft, id, -1, -1);
        }

        prompt.tokens.clear();
    }

    std::vector<common_adapter_lora_info> lora;
    int32_t alora_invocation_start = -1;

    // sampling
    json json_schema;

    common_sampler_ptr smpl;

    llama_token sampled; // in speculative mode, this is the last accepted token

    // stats
    size_t n_sent_text = 0; // number of sent text character

    // TODO @ngxson : move all metrics to a sub-struct for clarity
    int64_t t_start_process_prompt;
    int64_t t_start_generation;
    int64_t t_print_last = 0;
    int32_t n_decoded_last = 0;

    double t_prompt_processing = 0.0; // ms
    double t_token_generation = 0.0;  // ms

    std::function<void(int /* id_slot */)> callback_on_release;

    // Speculative decoding stats
    int32_t n_draft_total = 0;      // Total draft tokens generated
    int32_t n_draft_accepted = 0;   // Draft tokens actually accepted
    int32_t n_draft_verif_steps = 0; // Total draft token verification steps by the target model
    std::vector<int32_t> n_accepted_per_pos; // Accepted tokens per draft position

    // [obrain SPEC-METRICS] décomposition du coût, PASSE PAR PASSE.
    //
    // Le débit final ne dit pas OÙ passe le temps. Une passe de spéculation paie un BROUILLON
    // (forward du modèle brouillon, qui lit ses propres poids) PUIS une VÉRIFICATION (decode cible
    // sur n_prop + 1 jetons). Ces deux postes ne se réduisent pas de la même façon, donc on les
    // mesure séparément — et on garde la longueur acceptée de CHAQUE passe, pas seulement sa
    // moyenne : c'est la distribution (p50/p90/p95, taux de passes vides) qui décide de la valeur
    // d'un `--draft-max`, pas la moyenne.
    struct spec_pass_stat {
        int32_t n_prop    = 0;    // jetons proposés par le brouillon
        int32_t n_acc     = 0;    // jetons acceptés par la vérification
        float   draft_ms  = 0.0f; // brouillon (proposition)
        float   verify_ms = 0.0f; // decode cible de vérification
        float   round_ms  = 0.0f; // passe entière (brouillon + vérification + reste)
    };

    std::vector<spec_pass_stat> spec_passes;
    // [obrain R3-A] ETAT DU PROPOSEUR CERTIFIE -- UN PAR SLOT, initialise UNE fois par requete.
    // Contrat gele (§77) : Init(prompt) une seule fois ; site 1 = Propose(State_t) ; site 2 =
    // Update(State_t, accepted) sur les SEULS jetons COMMITTED. L'etat n'est jamais reconstruit
    // depuis slot.prompt.tokens apres l'Init.
    obrain_proposer_state     obrain_st;
    bool                      obrain_enabled   = false;
    bool                      obrain_need_init = false;
    std::vector<llama_token>  obrain_draft;
    int64_t                   obrain_output_tokens = 0;
    int64_t                   obrain_generation = 0;   // [R3-A7] cle de contexte
    int32_t spec_n_prop_pending    = 0;    // proposition en cours de vérification
    int64_t spec_t_round_pending   = 0;    // début de la passe en cours (µs)
    int64_t spec_t_verify_pending  = 0;    // début de la vérification (µs, clos à l'échantillonnage)
    float   spec_draft_ms_pending  = 0.0f;
    float   spec_verify_ms_pending = 0.0f;

    // [obrain J2] confidence probe accumulators (read-only signal, filled per sampled
    // token at the main sampling site; draft-accepted tokens are NOT probed — their
    // candidate sets are consumed inside common_sampler_sample_and_accept_n)
    std::vector<int32_t>     probe_idx;     // generation index of each probed token
    std::vector<llama_token> probe_tok;     // sampled token id
    std::vector<float>       probe_entropy; // top-k entropy (nats)
    std::vector<float>       probe_top1;    // renormalized top-1 probability

    void reset() {
        SLT_DBG(*this, "%s", "\n");

        obrain_enabled = (getenv("OBRAIN_PROPOSER") != nullptr && atoi(getenv("OBRAIN_PROPOSER")) == 1);

        // [OBRAIN_CAPACITY] contrat de capacite aval, DECOUPLE de n_max (le MTP garde le sien).
        // Domaine 2..49 ; valeur invalide = FATAL (aucun clipping silencieux).
        if (obrain_enabled) {
            const char * cb = getenv("OBRAIN_CAPACITY_B");
            if (cb != nullptr) {
                char * fin = nullptr;
                const long v = strtol(cb, &fin, 10);
                if (fin == cb || *fin != 0 || v < 2 || v > 49) {
                    fprintf(stderr, "FATAL OBRAIN_CAPACITY_B invalide : %s (domaine 2..49)\n", cb);
                    exit(2);
                }
                g_obrain_capacity_b = (int) v;
            }
            fprintf(stderr, "OBRAIN_CAPACITY init obrain_enabled=1 obrain_capacity_b=%d\n", g_obrain_capacity_b);
        }
    n_prompt_tokens_cache = 0;
        // [obrain SPLICE-CHAT] un prefixe importe appartient a UNE tache : il
        // est oublie des que le slot est libere (nouvelle tache = nouveau
        // splice explicite, jamais un report implicite).
        n_state_prefix = 0;

        last_nl_pos    = 0;
        generated_text = "";
        has_new_line   = false;
        truncated      = false;
        stop           = STOP_TYPE_NONE;
        stopping_word  = "";
        n_sent_text    = 0;

        if (can_speculate()) {
            spec_draft.clear();
            spec_i_batch.clear();
            spec_ckpt.clear();
        }
        // [obrain R3-A] nouvelle requete logique : l'etat du proposeur sera initialise UNE fois,
        // au premier DECIDE (site 1), puis alimente par les seuls jetons COMMITTED.
        obrain_need_init   = true;
        obrain_output_tokens = 0;
        obrain_draft.clear();
        obrain_generation++;      // [R3-A7] nouvelle requete logique -> nouveau CONTEXTE causal
        generated_tokens.clear();
        generated_token_probs.clear();
        json_schema = json();

        // clear speculative decoding stats
        n_draft_total = 0;
        n_draft_accepted = 0;
        n_draft_verif_steps = 0;
        n_accepted_per_pos.clear();

        // [obrain SPEC-METRICS]
        spec_passes.clear();
        spec_n_prop_pending    = 0;
        spec_t_round_pending   = 0;
        spec_t_verify_pending  = 0;
        spec_draft_ms_pending  = 0.0f;
        spec_verify_ms_pending = 0.0f;

        // [obrain J2] clear confidence probe accumulators
        probe_idx.clear();
        probe_tok.clear();
        probe_entropy.clear();
        probe_top1.clear();

        task_prev = std::move(task);
        task.reset();

        llama_set_sampler(ctx_tgt, id, nullptr);

        // clear alora start
        alora_invocation_start = -1;

        // clear multimodal state
        mbatch.reset();
    }

    void init_sampler() const {
        common_sampler_reset(smpl.get());

        if (!task->need_sampling()) {
            return;
        }

        const int64_t t_start = ggml_time_us();

        int n_text = 0;

        for (int i = 0; i < (int) prompt.tokens.size(); i++) {
            const llama_token id = prompt.tokens[i];

            if (id != LLAMA_TOKEN_NULL) {
                common_sampler_accept(smpl.get(), id, false);
                n_text++;
            }
        }

        SLT_TRC(*this, "init sampler, took %0.2f ms, tokens: text = %d, total = %d\n",
                (ggml_time_us() - t_start) / 1000.0, n_text, (int) prompt.tokens.size());
    }

    bool need_embd() const {
        GGML_ASSERT(task);
        return task->need_embd() || (spec && common_speculative_need_embd(spec));
    }

    bool need_embd_nextn() const {
        GGML_ASSERT(task);
        return spec && common_speculative_need_embd_nextn(spec);
    }

    // if the context does not have a memory module then all embeddings have to be computed within a single ubatch
    // also we cannot split if the pooling would require any past tokens
    // (MTP supports splitting — uses task->need_embd() not need_embd())
    bool can_split() const {
        GGML_ASSERT(task);

        return
            !task->need_embd() ||
            (llama_get_memory(ctx_tgt) && llama_pooling_type(ctx_tgt) == LLAMA_POOLING_TYPE_LAST);
    }

    bool can_batch_with(server_slot & other_slot) const {
        GGML_ASSERT(task);

        return task->type == other_slot.task->type && are_lora_equal(lora, other_slot.lora);
    }

    bool has_budget(const common_params & global_params) {
        GGML_ASSERT(task);

        if (task->params.n_predict == -1 && global_params.n_predict == -1) {
            return true; // limitless
        }

        n_remaining = -1;

        if (task->params.n_predict != -1) {
            n_remaining = task->params.n_predict - n_decoded;
        } else if (global_params.n_predict != -1) {
            n_remaining = global_params.n_predict - n_decoded;
        }

        return n_remaining > 0; // no budget
    }

    bool is_processing() const {
        return state != SLOT_STATE_IDLE;
    }

    bool can_speculate() const {
        return !!spec;
    }

    // [obrain SPLICE-CHAT] LA SPECULATION EST DESACTIVEE POUR UNE TACHE SPLICEE.
    //
    // POURQUOI : `llama_state_seq_set_data` n'ecrit que dans la sequence du
    // contexte CIBLE (ctx_tgt). Le contexte BROUILLON (ctx_dft, ici un MTP) ne
    // recoit JAMAIS le prefixe importe : ses brouillons seraient donc proposes
    // sur un contexte tronque. La sortie resterait juste (chaque brouillon est
    // verifie par le modele cible), mais on paierait un taux d'acceptation
    // proche de zero ET on ferait tourner le rollback speculatif sur un KV non
    // homogene (checkpoints du brouillon, seq_rm partiel) : deux effets dont
    // aucun n'est necessaire pour rendre le splice correct.
    //
    // On prefere une generation NUE, exacte, un peu plus lente, plutot qu'une
    // combinaison non verifiee. Ce garde-fou n'existe que sur le chemin
    // `state_ref` : sans splice, `spec_allowed() == can_speculate()`.
    bool spec_allowed() const {
        return can_speculate() && (task == nullptr || !task->spliced());
    }

    void add_token(const completion_token_output & token) {
        if (!is_processing()) {
            SLT_WRN(*this, "%s", "slot is not processing\n");
            return;
        }

        generated_token_probs.push_back(token);
    }

    int get_n_draft_max() const {
        GGML_ASSERT(task);

        if (!can_speculate()) {
            return 0;
        }

        // determine the max draft that fits the current slot state
        // note: slot.prompt is not yet expanded with the `id` token sampled above
        //       also, need to leave space for 1 extra token to allow context shifts
        int n_draft_max = n_ctx - prompt.n_tokens() - 2;

        if (n_remaining > 0) {
            n_draft_max = std::min(n_draft_max, n_remaining - 1);
        }

        SLT_DBG(*this, "max possible draft: %d\n", n_draft_max);

        return n_draft_max;
    }

    // add sampled token of this slot to the batch, optionally add the speculative draft tokens if any
    void handle_last_sampled_token(server_batch & batch) {
        bool add_ok = true;
        if (spec_draft.empty()) {
            // no speculative decoding
            i_batch = batch.size();

            add_ok &= batch.add(id, sampled, prompt.tokens.pos_next(), true);

            SLT_DBG(*this, "slot decode token, id=%d, n_ctx = %d, n_tokens = %d, truncated = %d\n",
                    sampled, n_ctx, prompt.n_tokens(), truncated);
        } else {
            SLT_DBG(*this, "generate_draft: id=%d, #tokens=%zu, #draft=%zu, pos_next=%d\n",
                    sampled, prompt.tokens.size(), spec_draft.size(), prompt.tokens.pos_next());

            GGML_ASSERT(spec_i_batch.empty());

            spec_i_batch.push_back(batch.size());
            for (size_t i = 0; i < spec_draft.size(); i++) {
                spec_i_batch.push_back(batch.size() + i + 1);
            }

            auto pos0 = prompt.tokens.pos_next();

            add_ok &= batch.add(id, sampled, pos0++, true);
            for (auto token : spec_draft) {
                add_ok &= batch.add(this->id, token, pos0++, true);
            }
        }

        GGML_ASSERT(add_ok && "batch must be large enough to hold the sampled and draft tokens");

        prompt.tokens.push_back(sampled);
        prompt.tokens.insert(spec_draft);
    }

    void release() {
        if (is_processing()) {
            GGML_ASSERT(task);

            SLT_INF(*this, "stop processing: n_tokens = %d, truncated = %d\n", prompt.n_tokens(), truncated);

            t_last_used        =  ggml_time_us();
            t_token_generation = (ggml_time_us() - t_start_generation) / 1e3;

            state = SLOT_STATE_IDLE;

            // do not keep context of the child slots - the parent's context is enough
            if (task->is_child()) {
                prompt_clear(false);
            }

            reset();

            callback_on_release(id);
        }
    }

    result_timings get_timings() const {
        result_timings timings;
        timings.cache_n = n_prompt_tokens_cache;

        timings.prompt_n            = n_prompt_tokens_processed;
        timings.prompt_ms           = t_prompt_processing;
        timings.prompt_per_token_ms = t_prompt_processing / n_prompt_tokens_processed;
        timings.prompt_per_second   = 1e3 / t_prompt_processing * n_prompt_tokens_processed;

        timings.predicted_n            = n_decoded;
        timings.predicted_ms           = t_token_generation;
        timings.predicted_per_token_ms = t_token_generation / n_decoded;
        timings.predicted_per_second   = 1e3 / t_token_generation * n_decoded;

        // Add speculative metrics
        // [obrain ATTRIB] ETAPE 0 : sans speculation, tout est autoregressif. Cette attribution
        // doit etre emise QUAND MEME -- elle etait sous if (n_draft_total > 0) et donc muette avec
        // --spec-type none, ce qui rendait l invariant inverifiable sur les bras "plain".
        if (n_draft_total == 0) {
            SLT_INF(*this, "OBRAIN_ATTR source=plain committed=%d rounds=%d len_mean=1.00 coverage=100.00\n",
                    n_decoded, n_decoded);
            SLT_INF(*this, "OBRAIN_ATTR_TOTAL committed=%d somme_sources=%d invariant=OK\n",
                    n_decoded, n_decoded);
        }
        if (n_draft_total > 0) {
            timings.draft_n          = n_draft_total;
            timings.draft_n_accepted = n_draft_accepted;
        }

        return timings;
    }

    size_t find_stopping_strings(const std::string & text, const size_t last_token_size, bool is_full_stop) {
        GGML_ASSERT(task);

        size_t stop_pos = std::string::npos;

        for (const std::string & word : task->params.antiprompt) {
            size_t pos;

            if (is_full_stop) {
                const size_t tmp      = word.size() + last_token_size;
                const size_t from_pos = text.size() > tmp ? text.size() - tmp : 0;

                pos = text.find(word, from_pos);
            } else {
                // otherwise, partial stop
                pos = string_find_partial_stop(text, word);
            }

            if (pos != std::string::npos && (stop_pos == std::string::npos || pos < stop_pos)) {
                if (is_full_stop) {
                    stop           = STOP_TYPE_WORD;
                    stopping_word  = word;
                    has_next_token = false;
                }
                stop_pos = pos;
            }
        }

        return stop_pos;
    }

    void print_timings_tg() {
        if (n_decoded < 100) {
            return;
        }

        const int64_t t_now = ggml_time_us();

        if (t_now - t_print_last < 3*1000*1000) {
            return;
        }

        const double n_gen_second     = 1e3 / (t_token_generation)   * (n_decoded);
        const double n_gen_second_win = 1e6 / (t_now - t_print_last) * (n_decoded - n_decoded_last);

        t_print_last = t_now;
        n_decoded_last = n_decoded;

        SLT_INF(*this, "n_decoded = %6d, tg = %6.2f t/s, tg_3s = %6.2f t/s\n", n_decoded, n_gen_second, n_gen_second_win);
    }

    void print_timings_pp() const {
        const double n_prompt_second = 1e3 / t_prompt_processing * n_prompt_tokens_processed;
        const double f_progress = (float) prompt.n_tokens() / task->n_tokens();

        if (t_prompt_processing < 3000.0) {
            return;
        }

        SLT_INF(*this, "prompt processing, n_tokens = %6d, progress = %.2f, t = %6.2f s / %.2f tokens per second\n",
                n_prompt_tokens_processed, f_progress, t_prompt_processing / 1e3, n_prompt_second);
    }

    void print_timings() const {
        const double t_prompt        =       t_prompt_processing / n_prompt_tokens_processed;
        const double n_prompt_second = 1e3 / t_prompt_processing * n_prompt_tokens_processed;

        const double t_gen        =       t_token_generation / n_decoded;
        const double n_gen_second = 1e3 / t_token_generation * n_decoded;

        SLT_INF(*this,
                "prompt eval time = %10.2f ms / %5d tokens (%8.2f ms per token, %8.2f tokens per second)\n",
                t_prompt_processing, n_prompt_tokens_processed, t_prompt, n_prompt_second);

        SLT_INF(*this,
                "       eval time = %10.2f ms / %5d tokens (%8.2f ms per token, %8.2f tokens per second)\n",
                t_token_generation, n_decoded, t_gen, n_gen_second);

        SLT_INF(*this,
                "      total time = %10.2f ms / %5d tokens\n",
                t_prompt_processing + t_token_generation, n_prompt_tokens_processed + n_decoded);

        SLT_INF(*this,
                "   graphs reused = %10d\n",
                llama_perf_context(ctx_tgt).n_reused);

        if (n_draft_total > 0) {
            const float  draft_ratio  = (float) n_draft_accepted / n_draft_total;
            const double mean_acc_len = n_draft_verif_steps > 0 ? 1.0 + (double) n_draft_accepted / (double) n_draft_verif_steps : 1.0;

            std::string acceptance_rates_per_pos;
            if (n_draft_verif_steps > 0) {
                for (size_t i = 0; i < n_accepted_per_pos.size(); ++i) {
                    if (i > 0) {
                        acceptance_rates_per_pos += ", ";
                    }
                    acceptance_rates_per_pos += string_format("%.3f", (double) n_accepted_per_pos[i] / (double) n_draft_verif_steps);
                }
            }

            SLT_INF(*this,
                    "draft acceptance = %0.5f (%5d accepted / %5d generated), mean len = %5.2f\n",
                    draft_ratio, n_draft_accepted, n_draft_total, mean_acc_len);
            SLT_TRC(*this,
                    "     acc per pos = (%s)\n", acceptance_rates_per_pos.c_str());

            // [obrain SPEC-METRICS] la distribution des longueurs acceptées, et la décomposition
            // brouillon / vérification. C'est ce que la moyenne cache : deux politiques de draft-max
            // peuvent avoir la même moyenne et des distributions opposées.
            //
            // [obrain SPEC-COST] LES COMPTEURS, nommés sans ambiguïté. C'est une erreur de
            // dénominateur qui a fait dire au modèle de coût que la bande passante impliquée
            // dépassait le plafond de la machine :
            //
            //   proposed_draft_tokens     : jetons proposés par le brouillon
            //   accepted_draft_tokens     : jetons du brouillon acceptés par la vérification
            //   verify_rounds             : passes de vérification
            //   verifier_committed_tokens : jetons produits par le VÉRIFICATEUR = 1 par passe (le
            //                               jeton de la position 0 est toujours commis, quelle que
            //                               soit l'issue du brouillon)
            //   committed_output_tokens   : jetons FINAUX = accepted_draft + verifier_committed
            //
            // Le débit et le temps se rapportent aux jetons COMMIS. Un « jeton accepté » n'est pas un
            // « jeton produit », et les confondre sous-estime le dénominateur d'environ 40 % — assez
            // pour inventer une bande passante au-dessus du plafond. La vérification compte UN passage
            // par passe : les poids du modèle principal sont lus une fois pour n_prop + 1 jetons.
            if (spec_passes.empty()) {
                // [obrain ATTRIB] aucune speculation : tout est autoregressif. On emet quand meme
                // l attribution, sinon l invariant n est pas verifiable sur les bras "plain".
                const int64_t commis_nu = (int64_t) std::max<int64_t>(0, n_draft_total);
                SLT_INF(*this, "OBRAIN_ATTR source=plain committed=%lld rounds=%lld len_mean=1.00 coverage=100.00\n",
                        (long long) commis_nu, (long long) commis_nu);
                SLT_INF(*this, "OBRAIN_ATTR_TOTAL committed=%lld somme_sources=%lld invariant=%s\n",
                        (long long) commis_nu, (long long) commis_nu, "OK");
            }
            if (!spec_passes.empty()) {
                std::vector<int32_t> lens;
                lens.reserve(spec_passes.size());

                double  sum_draft   = 0.0;
                double  sum_verify  = 0.0;
                double  sum_round   = 0.0;
                int64_t n_prop      = 0;
                int64_t n_acc       = 0;
                int64_t n_batch_sum = 0;
                int64_t n_batch_max = 0;
                int64_t by_count[5] = { 0, 0, 0, 0, 0 }; // passes à 0, 1, 2, 3, 4+ acceptés

                for (const auto & p : spec_passes) {
                    lens.push_back(p.n_acc);
                    sum_draft   += p.draft_ms;
                    sum_verify  += p.verify_ms;
                    sum_round   += p.round_ms;
                    n_prop      += p.n_prop;
                    n_acc       += p.n_acc;
                    n_batch_sum += p.n_prop + 1;   // le lot de vérification
                    n_batch_max  = std::max<int64_t>(n_batch_max, p.n_prop + 1);
                    by_count[std::min<int32_t>(p.n_acc, 4)]++;
                }
                std::sort(lens.begin(), lens.end());

                const auto pct = [&lens](double q) -> int32_t {
                    if (lens.empty()) {
                        return 0;
                    }
                    const size_t i = (size_t) std::min<double>((double) (lens.size() - 1), q * (double) (lens.size() - 1));
                    return lens[i];
                };

                const int64_t rounds        = (int64_t) spec_passes.size();
                const int64_t committed     = n_acc + rounds; // accepted_draft + verifier_committed
                const double  n_pass        = (double) rounds;
                const double  n_committed_d = (double) std::max<int64_t>(1, committed);
                const double  n_prop_d      = (double) std::max<int64_t>(1, n_prop);

                SLT_INF(*this,
                        "spec counts : proposed_draft %lld | accepted_draft %lld (%.1f%%) | verify_rounds %lld"
                        " | verifier_committed %lld | committed_output %lld\n",
                        (long long) n_prop, (long long) n_acc, 100.0 * (double) n_acc / n_prop_d,
                        (long long) rounds, (long long) rounds, (long long) committed);
                SLT_INF(*this,
                        "spec len (brouillon accepte par passe) : moyenne %.2f  p50 %d  p90 %d  p95 %d  max %d\n",
                        (double) n_acc / n_pass, pct(0.50), pct(0.90), pct(0.95), lens.empty() ? 0 : lens.back());
                SLT_INF(*this,
                        "spec lots de verification (proposed+1) : moyenne %.2f  max %lld  | passerelle MMVQ a 8\n",
                        (double) n_batch_sum / n_pass, (long long) n_batch_max);
                // [obrain COVERAGE] HISTOGRAMME DES LOTS DE VERIFICATION. Le noyau vectoriel du
                // verificateur s'arrete a 16 : au-dela, MMQ reprend, et il est 2,4 a 2,5 fois plus
                // lent sur ces formes. C'est donc CETTE distribution qui decide si le domaine doit
                // etre etendu, et jusqu'ou -- pas une intuition sur la taille des brouillons.
                {
                    static const char * bornes[8] = { "<=16", "17-20", "21-24", "25-32",
                                                      "33-40", "41-49", "50-64", "65+" };
                    uint64_t bins[8] = { 0, 0, 0, 0, 0, 0, 0, 0 };
                    for (const auto & p : spec_passes) {
                        const int batch = p.n_prop + 1;
                        const int b = batch <= 16 ? 0 : batch <= 20 ? 1 : batch <= 24 ? 2 : batch <= 32 ? 3
                                    : batch <= 40 ? 4 : batch <= 49 ? 5 : batch <= 64 ? 6 : 7;
                        bins[b]++;
                    }
                    uint64_t tot = 0;
                    for (int i = 0; i < 8; ++i) { tot += bins[i]; }
                    std::string ligne;
                    for (int i = 0; i < 8; ++i) {
                        char tampon[96];
                        snprintf(tampon, sizeof(tampon), "%s%s=%llu(%.1f%%)", i == 0 ? "" : " | ", bornes[i],
                                 (unsigned long long) bins[i], tot ? 100.0 * (double) bins[i] / (double) tot : 0.0);
                        ligne += tampon;
                    }
                    uint64_t depasse = tot - bins[0];
                    SLT_INF(*this, "spec lots (bins) : %s\n", ligne.c_str());
                    SLT_INF(*this,
                            "spec lots >16 : %llu passes sur %llu (%.1f%%) hors du domaine vectoriel (16)\n",
                            (unsigned long long) depasse, (unsigned long long) tot,
                            tot ? 100.0 * (double) depasse / (double) tot : 0.0);

                    // [obrain COVERAGE] Repartition par SOURCE
                    common_speculative_coverage cov;
                    if (common_speculative_get_coverage(spec, cov)) {
                        cov.committed = (size_t) std::max<int64_t>(0, committed);
                        const size_t plain = cov.committed > cov.acc_tokens ? cov.committed - cov.acc_tokens : 0;
                        // [obrain ATTRIB] UNE LIGNE MACHINE PAR SOURCE : c'est la reponse a des
                        // parseurs qui se trompaient de nombre. Invariant verifie a la ligne suivante :
                        // somme des jetons acceptes par source + plain = jetons commis.
                        size_t somme = plain;
                        for (const auto & e : cov.entries) {
                            std::vector<uint32_t> lens = e.acc_lens;
                            std::sort(lens.begin(), lens.end());
                            const auto q = [&lens](double pp) -> uint32_t {
                                if (lens.empty()) { return 0; }
                                const size_t i = (size_t) std::min<double>((double) (lens.size() - 1),
                                                                           pp * (double) (lens.size() - 1));
                                return lens[i];
                            };
                            double len_moy = 0.0;
                            for (uint32_t x : lens) { len_moy += (double) x; }
                            if (!lens.empty()) { len_moy /= (double) lens.size(); }
                            uint64_t lot_total = 0, lot_depasse = 0;
                            for (int i = 0; i < 8; ++i) { lot_total += e.hist_batch[i]; }
                            for (int i = 1; i < 8; ++i) { lot_depasse += e.hist_batch[i]; }
                            const double lot_moy = e.n_batch ? (double) e.sum_batch / (double) e.n_batch : 0.0;
                            SLT_INF(*this,
                                    "OBRAIN_ATTR source=%s committed=%zu rounds=%zu len_mean=%.2f len_p50=%u"
                                    " len_p95=%u len_max=%u proposal_ms=%.2f proposal_ms_per_token=%.4f"
                                    " batch_mean=%.2f batch_gt16=%llu/%llu coverage=%.2f\n",
                                    e.name.c_str(), e.acc_tokens, e.n_rounds, len_moy, q(0.50), q(0.95),
                                    lens.empty() ? 0u : lens.back(), e.t_draft_ms,
                                    e.acc_tokens ? e.t_draft_ms / (double) e.acc_tokens : 0.0,
                                    lot_moy, (unsigned long long) lot_depasse, (unsigned long long) lot_total,
                                    cov.committed ? 100.0 * (double) e.acc_tokens / (double) cov.committed : 0.0);
                            somme += e.acc_tokens;
                        }
                        SLT_INF(*this,
                                "OBRAIN_ATTR source=plain committed=%zu rounds=%zu len_mean=1.00 coverage=%.2f\n",
                                plain, plain,
                                cov.committed ? 100.0 * (double) plain / (double) cov.committed : 0.0);
                        SLT_INF(*this,
                                "OBRAIN_ATTR_TOTAL committed=%zu somme_sources=%zu invariant=%s\n",
                                cov.committed, somme,
                                (somme == cov.committed) ? "OK" : "*** INCOHERENT ***");
                    }
                }
                SLT_INF(*this,
                        "spec passes a 0/1/2/3/4+ acceptes = %lld / %lld / %lld / %lld / %lld\n",
                        (long long) by_count[0], (long long) by_count[1], (long long) by_count[2],
                        (long long) by_count[3], (long long) by_count[4]);
                SLT_INF(*this,
                        "spec temps par passe : brouillon %8.2f ms | verification %8.2f ms | total %8.2f ms\n",
                        sum_draft / n_pass, sum_verify / n_pass, sum_round / n_pass);
                SLT_INF(*this,
                        "spec temps par JETON COMMIS : brouillon %8.2f ms | verification %8.2f ms | total %8.2f ms"
                        " -> %.2f jetons commis/s\n",
                        sum_draft / n_committed_d, sum_verify / n_committed_d, sum_round / n_committed_d,
                        committed * 1000.0 / std::max<double>(1.0, sum_round));
            }
        }

        common_speculative_print_stats(spec);
    }

    json to_json(bool only_metrics = false) const {
        json res;

        res = {
            {"id",            id},
            {"n_ctx",         n_ctx},
            {"speculative",   can_speculate()},
            {"is_processing", is_processing()},
        };

        const auto & ptask = task ? task : task_prev;

        if (ptask) {
            res["id_task"] = ptask->id;
            res["n_prompt_tokens"]           = (int32_t) prompt.tokens.size();
            res["n_prompt_tokens_processed"] = n_prompt_tokens_processed;
            res["n_prompt_tokens_cache"]     = n_prompt_tokens_cache;
            res["params"] = ptask->params.to_json(only_metrics);
            res["next_token"] = {
                {
                    {"has_next_token", has_next_token},
                    {"has_new_line",   has_new_line},
                    {"n_remain",       n_remaining},
                    {"n_decoded",      n_decoded},
                }
            };

            if (!only_metrics) {
                res["prompt"] = ptask->tokens.detokenize(ctx_tgt, true);
                res["generated"] = generated_text.empty() ? debug_generated_text : generated_text;
            }
        }

        return res;
    }

    void copy_state_to(server_slot & other) const {
        GGML_ASSERT(state == SLOT_STATE_DONE_PROMPT);

        common_context_seq_rm(ctx_tgt, other.id,     -1, -1);
        common_context_seq_cp(ctx_tgt, id, other.id, -1, -1);

        if (ctx_dft) {
            common_context_seq_rm(ctx_dft, other.id,     -1, -1);
            common_context_seq_cp(ctx_dft, id, other.id, -1, -1);
        }

        other.n_decoded   = n_decoded;
        other.n_remaining = n_remaining;
        other.i_batch     = i_batch;

        other.t_start_process_prompt    = t_start_process_prompt;
        other.t_prompt_processing       = t_prompt_processing;
        other.n_prompt_tokens_cache     = n_prompt_tokens_cache;
        other.n_prompt_tokens_processed = n_prompt_tokens_processed;

        other.prompt = prompt.clone();
        other.init_sampler();
    }

    // returns 0 on success
    // caller need to update prompt.tokens after a successful call to keep track of the processing progress
    int process_mtmd_chunk(size_t idx, size_t & n_tokens_out) {
        GGML_ASSERT(mctx);
        const auto & input_tokens = task->tokens;
        const auto & chunk = input_tokens.find_chunk(idx);
        int32_t res = 0;

        auto try_decode = [&]() -> int32_t {
            if (mbatch) {
                float * embd = mtmd_batch_get_output_embd(mbatch.get(), chunk.get());
                if (embd) {
                    void * cb_data = spec;
                    static auto cb = [](llama_batch batch, void * user_data) {
                        common_speculative * spec = static_cast<common_speculative *>(user_data);
                        if (!common_speculative_process(spec, batch)) {
                            return 1;
                        }
                        return 0;
                    };

                    llama_pos new_n_past; // unused for now
                    res = mtmd_helper_decode_image_chunk(
                        mctx,
                        ctx_tgt,
                        chunk.get(),
                        embd,
                        prompt.tokens.pos_next(),
                        id,
                        llama_n_batch(ctx_tgt),
                        &new_n_past,
                        cb,
                        cb_data
                    );
                    if (res != 0) {
                        SLT_ERR(*this, "failed to decode mtmd chunk, idx = %zu, res = %d\n", idx, res);
                        return -1;
                    }
                    n_tokens_out = mtmd_input_chunk_get_n_tokens(chunk.get());
                    return 0; // success
                }
            }
            return 1; // (non-error) need to create & encode batch
        };

        // if the batch is already exist, try searching & encode
        res = try_decode();
        if (res == 0) {
            return 0;
        }
        if (res < 0) {
            // fatal error
            return res;
        }

        // otherwise, the batch is either uninitialized or is used up
        // we need to create & encode a new batch
        mbatch.reset(mtmd_batch_init(mctx));
        res = mtmd_batch_add_chunk(mbatch.get(), chunk.get());
        GGML_ASSERT(res == 0); // we should never have an empty batch

        // try batching as much as possible
        int n_added = 1;
        size_t idx_cur = idx;
        while (res == 0) {
            auto [next_chunk, next_idx] = input_tokens.find_next_media_chunk(idx_cur);
            if (next_chunk == nullptr) {
                break;
            }
            res = mtmd_batch_add_chunk(mbatch.get(), next_chunk->get());
            n_added += (res == 0 ? 1 : 0);
            idx_cur = next_idx;
            SLT_DBG(*this, "try adding media chunk idx = %zu to batch, res = %d\n", next_idx, res);
            // if res != 0, batch is full or chunk is not compatible -> this loop breaks
        }

        // TODO @ngxson : move this log line to debug when it become more stable
        SLT_TRC(*this, "encoding mtmd batch from idx = %zu, n_chunks = %d\n", idx, n_added);

        res = mtmd_batch_encode(mbatch.get());
        if (res != 0) {
            SLT_ERR(*this, "failed to encode mtmd batch for chunk idx = %zu, res = %d\n", idx, res);
            return -1;
        }

        return try_decode();
    }
};



//
// server_metrics
//

struct server_metrics {
    int64_t t_start = 0;

    uint64_t n_prompt_tokens_processed_total = 0;
    uint64_t t_prompt_processing_total       = 0;
    uint64_t n_tokens_predicted_total        = 0;
    uint64_t t_tokens_generation_total       = 0;

    uint64_t n_tokens_max = 0;

    uint64_t n_prompt_tokens_processed = 0;
    uint64_t t_prompt_processing       = 0;

    uint64_t n_tokens_predicted  = 0;
    uint64_t t_tokens_generation = 0;

    uint64_t n_decode_total     = 0;
    uint64_t n_busy_slots_total = 0;

    void init() {
        t_start = ggml_time_us();
    }

    void on_prompt_eval(const server_slot & slot) {
        n_prompt_tokens_processed_total += slot.n_prompt_tokens_processed;
        n_prompt_tokens_processed       += slot.n_prompt_tokens_processed;
        t_prompt_processing             += slot.t_prompt_processing;
        t_prompt_processing_total       += slot.t_prompt_processing;

        n_tokens_max = std::max(n_tokens_max, (uint64_t) slot.prompt.n_tokens());
    }

    void on_prediction(const server_slot & slot) {
        n_tokens_predicted_total   += slot.n_decoded;
        n_tokens_predicted         += slot.n_decoded;
        t_tokens_generation        += slot.t_token_generation;
        t_tokens_generation_total  += slot.t_token_generation;
    }

    void on_decoded(const std::vector<server_slot> & slots) {
        n_decode_total++;
        for (const auto & slot : slots) {
            if (slot.is_processing()) {
                n_busy_slots_total++;
            }
            n_tokens_max = std::max(n_tokens_max, (uint64_t) slot.prompt.n_tokens());
        }
    }

    void reset_bucket() {
        n_prompt_tokens_processed = 0;
        t_prompt_processing       = 0;
        n_tokens_predicted        = 0;
        t_tokens_generation       = 0;
    }
};


//
// server_context_impl (private implementation)
//

struct server_context_impl {
    friend struct server_context;

public:
    // only use these pointers outside of this class:
    //  - when not in sleeping state
    //  - and, with thread-safe APIs (e.g., tokenizer calls)
    llama_model * model_tgt = nullptr;

    mtmd_context * mctx = nullptr;
    const llama_vocab * vocab = nullptr;

    server_queue    queue_tasks;
    server_response queue_results;

    // note: chat_params must not be refreshed upon existing sleeping state
    server_chat_params chat_params;

    server_state_callback_t callback_state = [](server_state, json) -> void {};

    server_context_impl() {
        mtmd_helper_log_set(common_log_default_callback, nullptr);
    }

    ~server_context_impl() {
        if (!sleeping) {
            // destroy() is already called when entering sleeping state
            // we don't call it again here to avoid double free
            destroy();
        }
    }

private:
    // note: accessing these fields outside of this class is not thread-safe
    // use server_context methods instead

    common_params params_base;

    // note: keep these alive - they determine the lifetime of the model, context, etc.
    common_init_result_ptr llama_init;

    llama_context * ctx_tgt = nullptr;

    // J-space [obrain] — Gram matrix cache for SERVER_TASK_TYPE_JSPACE_COMPARE_GENERATION,
    // lazily computed once (~15-30s since the parallel gram, was ~300s) and reused across all such tasks
    // processed by this worker. Separate from server_routes::jspace_gram_b64_cache
    // (the /jspace/gram HTTP handler's own cache) since this one lives on
    // the worker-thread side and is consumed as raw floats, not base64 JSON.
    std::vector<float> jspace_gram_cache;
    bool                jspace_gram_ready = false;

    // [obrain P1] REGISTRE CLIENT -> SEQUENCE (cf. obrain-client-seq.h).
    // Il vit ICI, dans l'implementation, et n'est PAS consulte par /props :
    // seul le thread de boucle (qui choisit les slots) l'attribue et le lit.
    // Ses accesseurs d'observation prennent malgre tout leur verrou, parce
    // qu'un endpoint peut un jour le publier (et que /slots fait deja ce
    // genre de lecture).
    obrain_seq::Registre registre_client;

    server_batch batch;

    llama_model   * model_dft = nullptr;
    llama_context * ctx_dft   = nullptr;

    common_speculative_init_result_ptr spec_init;

    common_context_seq_rm_type ctx_tgt_seq_rm_type = COMMON_CONTEXT_SEQ_RM_TYPE_NO;
    common_context_seq_rm_type ctx_dft_seq_rm_type = COMMON_CONTEXT_SEQ_RM_TYPE_NO;

    common_speculative_ptr spec;

    bool add_bos_token = true;

    int32_t n_ctx; // total context for all clients / slots

    // set to llama_model_n_swa(model)
    // if swa_full is enabled, this is set to 0 to simulate a non-SWA model
    int32_t n_swa;

    // slots / clients
    std::vector<server_slot> slots;

    int trace = 0;
    int slots_debug = 0;
    int n_empty_consecutive = 0;

    std::unique_ptr<server_prompt_cache> prompt_cache;

    server_metrics metrics;

    json json_ui_settings = json::object();

    // Necessary similarity of prompt for slot selection
    float slot_prompt_similarity = 0.0f;

    std::string model_name; // name of the loaded model, to be used by API
    std::set<std::string> model_aliases; // additional names for the model
    std::set<std::string> model_tags;    // informational tags

    bool sleeping = false;

    int64_t t_last_load_progress_ms = 0;

    void destroy() {
        spec.reset();
        spec_init.reset();

        ctx_dft   = nullptr;
        model_dft = nullptr;

        llama_init.reset();

        ctx_tgt = nullptr;
        model_tgt = nullptr;

        mtmd_free(mctx);
        mctx = nullptr;
    }

    void handle_sleeping_state(bool new_state) {
        GGML_ASSERT(sleeping != new_state);
        if (new_state) {
            SRV_INF("%s", "server is entering sleeping state\n");
            destroy();
        } else {
            SRV_INF("%s", "server is exiting sleeping state\n");
            if (!load_model(params_base)) {
                GGML_ABORT("failed to reload model after sleeping");
            }
        }
        sleeping = new_state;
    }

    struct load_progress_data {
        server_context_impl * ctx;
        std::string stage;
        std::vector<std::string> stages;
        int64_t t_last_load_progress_ms = 0;
        load_progress_data(server_context_impl * ctx, const std::string & stage) : ctx(ctx), stage(stage) {}
    };
    static bool load_progress_callback(float progress, void * user_data) {
        auto * d = static_cast<load_progress_data *>(user_data);
        GGML_ASSERT(d);
        // always emit the first and final sample; throttle the rest to one per 200ms
        {
            auto & t_last = d->t_last_load_progress_ms;
            const int64_t t_now = ggml_time_ms();
            const bool first = t_last == 0;
            const bool done  = progress >= 1.0f;
            const bool throttled = !first && !done && (t_now - t_last) < 200;
            if (throttled) {
                return true;
            }
            t_last = t_now;
        }
        if (d->ctx->callback_state) {
            d->ctx->callback_state(SERVER_STATE_LOADING, {
                {"stages", d->stages},
                {"current", d->stage},
                {"value", progress},
            });
        }
        return true;
    }

    // load the model and initialize llama_context
    // this may also be called to resume from sleeping state
    bool load_model(common_params & params) {
        load_progress_data load_progress_text  (this, "text_model");
        load_progress_data load_progress_mmproj(this, "mmproj_model");
        load_progress_data load_progress_spec  (this, "spec_model");

        const bool is_resume = sleeping;

        params_base = params;
        params_base.n_outputs_max = server_n_outputs_max(params_base);

        const bool has_mmproj = !params.mmproj.path.empty();
        const bool has_draft = params.speculative.has_dft();
        const bool spec_mtp = std::find(params_base.speculative.types.begin(),
                                        params_base.speculative.types.end(),
                                        COMMON_SPECULATIVE_TYPE_DRAFT_MTP) != params_base.speculative.types.end();
        const bool has_spec = has_draft || spec_mtp;

        if (callback_state) {
            std::vector<std::string> stages = {"text_model"};
            if (has_spec) {
                stages.push_back("spec_model");
            }
            if (has_mmproj) {
                stages.push_back("mmproj_model");
            }
            load_progress_text.stages   = stages;
            load_progress_mmproj.stages = stages;
            load_progress_spec.stages   = stages;

            // trigger 0% progress
            load_progress_callback(0.0f, &load_progress_text);
        }


        SRV_INF("loading model '%s'\n", params.model.get_name().c_str());
        SRV_TRC("local path '%s'\n", params.model.path.c_str());

        std::string & mmproj_path = params_base.mmproj.path;
        mtmd_context_params mparams = mtmd_context_params_default();
        if (has_mmproj) {
            mparams.use_gpu          = params_base.mmproj_use_gpu;
            mparams.print_timings    = false;
            mparams.n_threads        = params_base.cpuparams.n_threads;
            mparams.flash_attn_type  = params_base.flash_attn_type;
            mparams.warmup           = params_base.warmup;
            mparams.image_min_tokens = params_base.image_min_tokens;
            mparams.image_max_tokens = params_base.image_max_tokens;
            mparams.batch_max_tokens = params_base.mtmd_batch_max_tokens;
            mparams.media_marker     = get_media_marker();
            // progress callback
            mparams.progress_callback           = load_progress_callback;
            mparams.progress_callback_user_data = &load_progress_mmproj;
        }

        // optionally get the memory usage of mmproj
        if (has_mmproj && params_base.fit_params) {
            int64_t t_start = ggml_time_us();
            auto mmproj_mem = mtmd_get_memory_usage(mmproj_path.c_str(), mparams);
            int64_t t_elapsed = ggml_time_us() - t_start;
            if (!mmproj_mem.empty()) {
                size_t total = 0;
                for (auto & [dev, size] : mmproj_mem) {
                    total += size;
                }
                SRV_TRC("[mtmd] estimated worst-case memory usage of mmproj is %.2f MiB (took %.2f ms)\n", total / (1024.0 * 1024.0), t_elapsed / 1000.0);
                GGML_ASSERT(!params_base.fit_params_target.empty());
                for (auto & [dev, size] : mmproj_mem) {
                    for (size_t i = 0; i < ggml_backend_dev_count(); i++) {
                        if (ggml_backend_dev_get(i) == dev) {
                            if (i < params_base.fit_params_target.size()) {
                                SRV_DBG("[mtmd] adding %.2f MiB to fit_params_target for device %s\n", size / (1024.0 * 1024.0), ggml_backend_dev_name(dev));
                                params_base.fit_params_target[i] += size;
                            }
                            break;
                        }
                    }
                }
            } else {
                SRV_ERR("%s", "[mtmd] failed to get memory usage of mmproj\n");
            }
        }

        // optionally reserve VRAM for the draft / MTP context before fitting the target model
        if (params_base.fit_params) {
            if (has_spec) {
                // MTP draft context lives on the target model, only context+compute are new
                bool measure_model_bytes = has_draft;

                common_params params_dft = common_base_params_to_speculative(params_base);

                auto mparams_dft = common_model_params_to_llama(params_dft);
                auto cparams_dft = common_context_params_to_llama(params_dft);
                if (spec_mtp) {
                    cparams_dft.ctx_type = LLAMA_CONTEXT_TYPE_MTP;
                }
                cparams_dft.n_rs_seq = 0;

                std::vector<ggml_backend_dev_t> devs;
                uint32_t hp_ngl = 0;
                uint32_t hp_nct = 0;
                uint32_t hp_nex = 0;
                try {
                    auto dmd = common_get_device_memory_data(
                        params_dft.model.path.c_str(), &mparams_dft, &cparams_dft,
                        devs, hp_ngl, hp_nct, hp_nex, GGML_LOG_LEVEL_ERROR);

                    GGML_ASSERT(!params_base.fit_params_target.empty());
                    size_t total = 0;

                    std::vector<ggml_backend_dev_t> tgt_devices = params.devices;

                    if (tgt_devices.empty()) {
                        for(size_t i = 0; i < ggml_backend_dev_count(); ++i) {
                           tgt_devices.push_back(ggml_backend_dev_get(i));
                        }
                    }

                    for (size_t j = 0; j < devs.size(); ++j) {
                        const size_t bytes = (measure_model_bytes ? dmd[j].model : 0) + dmd[j].context + dmd[j].compute;
                        total += bytes;
                        for (size_t i = 0; i < tgt_devices.size(); i++) {
                            if (tgt_devices[i] == devs[j]) {
                                SRV_DBG("[spec] adding %.2f MiB to fit_params_target for device %s\n",
                                        bytes / (1024.0 * 1024.0), ggml_backend_dev_name(devs[j]));
                                params_base.fit_params_target[i] += bytes;
                                break;
                            }
                        }
                    }
                    SRV_TRC("[spec] estimated memory usage of %s is %.2f MiB\n",
                            has_draft ? "draft model" : "MTP context",
                            total / (1024.0 * 1024.0));
                } catch (const std::exception & e) {
                    SRV_WRN("[spec] failed to measure %s memory: %s\n",
                            has_draft ? "draft model" : "MTP context", e.what());
                }
            }
        }

        // attach a progress callback
        {
            params_base.load_progress_callback = load_progress_callback;
            params_base.load_progress_callback_user_data = &load_progress_text;
        }

        llama_init = common_init_from_params(params_base);

        model_tgt = llama_init->model();
        // [obrain R3] IDENTITE COMPLETE ETABLIE AU CHARGEMENT, avant les requetes,
        // et liee a CETTE instance. Une nouvelle instance invalide la precedente :
        // l'identite n'est jamais reutilisee aveuglement.
        {
            const uint64_t _tok = kv_model_instance_new(params_base.model.path);
            const auto _t0 = ggml_time_us();
            const std::string _sha = obrain_kv::sha256_hex_file(params_base.model.path);
            g_kv_ms_model_sha.store((float)(ggml_time_us() - _t0) / 1000.0f);
            { std::lock_guard<std::mutex> lk(g_kv_model_mtx);
              g_kv_model_path = params_base.model.path; g_kv_model_sha = _sha; g_kv_model_token = _tok; }
            SRV_INF("obrain model identity: instance=%llu sha256=%s (%.1f ms)\n",
                    (unsigned long long) _tok, _sha.c_str(), (double) g_kv_ms_model_sha.load());
        }
        ctx_tgt   = llama_init->context();

        if (model_tgt == nullptr) {
            SRV_ERR("failed to load model, '%s'\n", params_base.model.path.c_str());
            return false;
        }

        vocab = llama_model_get_vocab(model_tgt);

        n_ctx = llama_n_ctx(ctx_tgt);

        add_bos_token = llama_vocab_get_add_bos(vocab);

        if (has_spec) {
            // spec_mtp doesn't use load a model internally, so we report 0.0 and 1.0 manually
            load_progress_callback(0.0f, &load_progress_spec);
            load_progress_spec.t_last_load_progress_ms = 0;  // reset so internal cbs aren't delayed

            {
                common_params params_dft = common_base_params_to_speculative(params_base);

                // progress callback
                params_dft.load_progress_callback           = load_progress_callback;
                params_dft.load_progress_callback_user_data = &load_progress_spec;

                spec_init = common_speculative_init_from_params(params_dft, model_tgt, ctx_tgt);
                model_dft = spec_init->model();
                ctx_dft   = spec_init->context();

                if (has_draft && model_dft == nullptr) {
                    SRV_ERR("failed to load draft model, '%s'\n", params_dft.model.path.c_str());
                    return false;
                }

                if (ctx_dft == nullptr) {
                    SRV_ERR("%s", "failed to create MTP context\n");
                    return false;
                }

                params_base.speculative.draft.ctx_tgt = ctx_tgt;
                params_base.speculative.draft.ctx_dft = ctx_dft;
            }

            load_progress_callback(1.0f, &load_progress_spec);
        }

        if (has_mmproj) {
            if (callback_state) {
                callback_state(SERVER_STATE_LOADING, {{"stage", "mmproj_model"}});
            }

            if (!is_resume) {
                mtmd_helper_log_set(common_log_default_callback, nullptr);
            }

            mctx = mtmd_init_from_file(mmproj_path.c_str(), model_tgt, mparams);
            if (mctx == nullptr) {
                SRV_ERR("failed to load multimodal model, '%s'\n", mmproj_path.c_str());
                return false;
            }
            SRV_INF("loaded multimodal model, '%s'\n", mmproj_path.c_str());

            if (params_base.ctx_shift) {
                params_base.ctx_shift = false;
                SRV_WRN("%s\n", "ctx_shift is not supported by multimodal, it will be disabled");
            }

            if (params_base.n_cache_reuse) {
                params_base.n_cache_reuse = 0;
                SRV_WRN("%s\n", "cache_reuse is not supported by multimodal, it will be disabled");
            }
        }

        if (!llama_memory_can_shift(llama_get_memory(ctx_tgt))) {
            if (params_base.ctx_shift) {
                params_base.ctx_shift = false;
                SRV_WRN("%s\n", "ctx_shift is not supported by this context, it will be disabled");
            }

            if (params_base.n_cache_reuse) {
                params_base.n_cache_reuse = 0;
                SRV_WRN("%s\n", "cache_reuse is not supported by this context, it will be disabled");
            }
        }

        if (llama_model_n_swa(model_tgt) == 0) {
            if (params_base.swa_full) {
                params_base.swa_full = false;
                SRV_WRN("%s\n", "swa_full is not supported by this model, it will be disabled");
            }
        }

        n_swa = params_base.swa_full ? 0 : llama_model_n_swa(model_tgt);

        // Necessary similarity of prompt for slot selection
        slot_prompt_similarity = params_base.slot_prompt_similarity;

        const int n_ctx_train = llama_model_n_ctx_train(model_tgt);

        int n_ctx_slot = llama_n_ctx_seq(ctx_tgt);
        if (n_ctx_slot > n_ctx_train) {
            SRV_WRN("the slot context (%d) exceeds the training context of the model (%d) - using rope scaling to extend\n", n_ctx_slot, n_ctx_train);
            // Do not cap: caller has configured rope scaling (--rope-scale / --rope-scaling yarn) to handle extended context.
        }

        slots.clear();

        ctx_tgt_seq_rm_type = common_context_can_seq_rm(ctx_tgt);
        if (ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_NO) {
            SRV_WRN("%s", "speculative decoding not supported by this context\n");
        }

        if (ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_FULL) {
            SRV_TRC("%s", "speculative decoding will use checkpoints\n");
        }

        // setup slots
        SRV_INF("initializing, n_slots = %d, n_ctx_slot = %d, kv_unified = '%s'\n",
                params_base.n_parallel, n_ctx_slot, params_base.kv_unified ? "true" : "false");

        // initialize slots
        for (int i = 0; i < params_base.n_parallel; i++) {
            slots.emplace_back();
        }

        // [obrain P1] La politique d'attribution a besoin de connaitre le
        // nombre de sequences ET laquelle est reservee : elle est donc posee
        // APRES la creation des slots, jamais avant (avant, le registre
        // croirait que toutes les sequences sont offertes).
        registre_client.configurer((int32_t) slots.size(), jspace_reserved_slot_id());
        SRV_INF("[obrain-p1] registre client -> sequence : %d sequence(s) offerte(s), "
                "reservee = %d (slots = %d)\n",
                registre_client.slots_offerts(), registre_client.reserve(), (int) slots.size());

        // try speculative decoding
        if (ctx_tgt_seq_rm_type != COMMON_CONTEXT_SEQ_RM_TYPE_NO) {
            try {
                spec.reset(common_speculative_init(params_base.speculative, params_base.n_parallel));
            } catch (const std::exception & e) {
                SRV_ERR("failed to initialize speculative decoding context: %s\n", e.what());
            }
        }

        if (ctx_dft) {
            ctx_dft_seq_rm_type = common_context_can_seq_rm(ctx_dft);
        }

        if (spec) {
            SRV_TRC("%s", "speculative decoding context initialized\n");
        } else {
            spec_init.reset();
            ctx_dft   = nullptr;
            model_dft = nullptr;
        }

        for (int i = 0; i < params_base.n_parallel; i++) {
            server_slot & slot = slots[i];

            slot.id      = i;
            slot.ctx_tgt = ctx_tgt;
            slot.ctx_dft = ctx_dft;
            slot.spec    = spec.get();
            slot.n_ctx   = n_ctx_slot;

            slot.mctx                   = mctx;
            slot.prompt.tokens.has_mtmd = mctx != nullptr;

            SLT_TRC(slot, "new slot, n_ctx = %d\n", slot.n_ctx);

            slot.callback_on_release = [this](int id_slot) {
                queue_tasks.pop_deferred_task(id_slot);
            };

            slot.reset();
        }

        {
            const char * LLAMA_TRACE = getenv("LLAMA_TRACE");
            trace = LLAMA_TRACE ? atoi(LLAMA_TRACE) : 0;

            if (trace) {
                SRV_WRN("LLAMA_TRACE = %d\n", trace);
            }
        }

        {
            const char * LLAMA_SERVER_SLOTS_DEBUG = getenv("LLAMA_SERVER_SLOTS_DEBUG");
            slots_debug = LLAMA_SERVER_SLOTS_DEBUG ? atoi(LLAMA_SERVER_SLOTS_DEBUG) : 0;

            if (slots_debug) {
                SRV_WRN("LLAMA_SERVER_SLOTS_DEBUG = %d\n", slots_debug);
            }
        }

        // the update_slots() logic will always submit a maximum of n_batch or n_parallel tokens
        // note that n_batch can be > n_ctx (e.g. for non-causal attention models such as BERT where the KV cache is not used)
        {
            const int32_t n_batch = llama_n_batch(ctx_tgt);
            batch.init(std::max(n_batch, params_base.n_parallel));
        }

        if (params_base.cache_ram_mib != 0) {
            if (params_base.cache_ram_mib < 0) {
                SRV_TRC("prompt cache is enabled, size limit: %s\n", "no limit");
            } else {
                SRV_TRC("prompt cache is enabled, size limit: %d MiB\n", params_base.cache_ram_mib);
            }
            SRV_TRC("%s", "use `--cache-ram 0` to disable the prompt cache\n");

            prompt_cache = std::make_unique<server_prompt_cache>(params_base.cache_ram_mib, n_ctx);
        } else {
            SRV_TRC("%s", "prompt cache is disabled - use `--cache-ram N` to enable it\n");
        }
        SRV_TRC("%s", "for more info see https://github.com/ggml-org/llama.cpp/pull/16391\n");

        if (params_base.n_ctx_checkpoints > 0) {
            SRV_TRC("context checkpoints enabled, max = %d, min spacing = %d\n",
                    params_base.n_ctx_checkpoints, params_base.checkpoint_min_step);
        } else {
            SRV_TRC("%s", "context checkpoints disabled\n");
        }

        if (!params_base.model_alias.empty()) {
            // backward compat: use first alias as model name
            model_name = *params_base.model_alias.begin();
        } else if (!params_base.model.get_name().empty()) {
            model_name = params_base.model.get_name();
        } else {
            // fallback: derive model name from file name
            auto model_path = std::filesystem::path(params_base.model.path);
            model_name = model_path.filename().string();
        }

        model_aliases = params_base.model_alias;
        model_tags    = params_base.model_tags;

        // propagate new defaults back to caller
        params = params_base;

        if (!is_resume) {
            return init();
        }

        if (callback_state) {
            callback_state(SERVER_STATE_READY, {});
        }

        return true;
    }

    // unlike load_model(), this is only called once during initialization
    bool init() {
        GGML_ASSERT(ctx_tgt   != nullptr);
        GGML_ASSERT(model_tgt != nullptr);

        GGML_ASSERT(!sleeping);

        // wiring up server queues
        queue_tasks.on_new_task([this](server_task && task) {
            process_single_task(std::move(task));
        });
        queue_tasks.on_update_slots([this]() {
            update_slots();
        });
        queue_tasks.on_sleeping_state([this](bool sleeping) {
            handle_sleeping_state(sleeping);
        });

        metrics.init();

        if (params_base.cache_idle_slots) {
            if (params_base.cache_ram_mib == 0) {
                SRV_WRN("%s", "--cache-idle-slots requires --cache-ram, disabling\n");
                params_base.cache_idle_slots = false;
            } else {
                if (params_base.kv_unified) {
                    SRV_TRC("%s", "idle slots will be saved to prompt cache and cleared upon starting a new task\n");
                } else {
                    // without a unified KV cache, clearing a slot frees no reusable room, so we only
                    // publish a RAM-cache copy of idle slots (their KV stays in VRAM) [TAG_IDLE_SLOT_CLEAR]
                    SRV_TRC("%s", "idle slots will be saved to prompt cache upon starting a new task\n");
                }
                SRV_DBG("%s", "__TEST_TAG_CACHE_IDLE_SLOTS_ENABLED__\n");
            }
        }

        {
            const std::string & cfg = params_base.ui_config_json;
            if (!cfg.empty()) {
                try {
                    json json_settings = json::parse(cfg);
                    json_ui_settings = json_settings;
                } catch (const std::exception & e) {
                    SRV_ERR("%s: failed to parse UI config: %s\n", __func__, e.what());
                    return false;
                }
            }
        }

        // populate chat template params
        {
            common_chat_templates_ptr chat_templates;
            bool enable_thinking = false;

            try {
                chat_templates = common_chat_templates_init(model_tgt, params_base.chat_template);

                SRV_TRC("%s: chat template, example_format: '%s'\n", __func__,
                    common_chat_format_example(chat_templates.get(), params_base.use_jinja, params_base.default_template_kwargs).c_str());

                // thinking is enabled if:
                // 1. It's not explicitly disabled via --reasoning off
                // 2. The chat template supports it
                const bool template_supports_thinking = params_base.use_jinja && common_chat_templates_support_enable_thinking(chat_templates.get());
                enable_thinking = params_base.enable_reasoning != 0 && template_supports_thinking;
                SRV_TRC("%s: chat template, thinking = %d\n", __func__, enable_thinking);
            } catch (const std::exception & e) {
                SRV_ERR("%s: chat template parsing error: %s\n", __func__, e.what());
                SRV_ERR("%s: please consider disabling jinja via --no-jinja, or use a custom chat template via --chat-template\n", __func__);
                SRV_ERR("%s: for example: --no-jinja --chat-template chatml\n", __func__);
                return false;
            }

            // IMPORTANT: chat_params is reused across sleeping / resuming states,
            //            never store llama_context/llama_model pointers in chat_params,
            //            as they may be invalidated after sleeping
            chat_params = {
                /* use_jinja             */ params_base.use_jinja,
                /* prefill_assistant     */ params_base.prefill_assistant,
                /* reasoning_format      */ params_base.reasoning_format,
                /* chat_template_kwargs  */ params_base.default_template_kwargs,
                /* tmpls                 */ std::move(chat_templates),
                /* allow_image           */ mctx ? mtmd_support_vision(mctx) : false,
                /* allow_audio           */ mctx ? mtmd_support_audio (mctx) : false,
                /* allow_video           */ mctx ? mtmd_helper_support_video(mctx) : false,
                /* enable_thinking       */ enable_thinking,
                /* reasoning_budget      */ params_base.sampling.reasoning_budget_tokens,
                /* reasoning_budget_msg  */ params_base.sampling.reasoning_budget_message,
                /* media_path            */ params_base.media_path,
                /* force_pure_content    */ params_base.force_pure_content_parser
            };

            {
                auto caps = common_chat_templates_get_caps(chat_params.tmpls.get());
                auto it = params_base.default_template_kwargs.find("preserve_reasoning");
                bool supported = caps.at("supports_preserve_reasoning");
                bool enabled = it != params_base.default_template_kwargs.end();
                if (supported && !enabled) {
                    SRV_INF("%s", "chat template supports preserving reasoning, consider enabling it via --reasoning-preserve\n");
                }
                if (!supported && enabled) {
                    SRV_WRN("%s", "chat template does NOT support preserving reasoning, --reasoning-preserve has no effect\n");
                }
            }
        }

        return true;
    }

    server_slot * get_slot_by_id(int id_slot) {
        // note: allow id_slot to be out of bounds (wrap around)
        id_slot = id_slot % slots.size();

        for (server_slot & slot : slots) {
            if (slot.id == id_slot) {
                return &slot;
            }
        }

        return nullptr;
    }

    // J-space [obrain] — the LAST slot is reserved exclusively for
    // llama_set_layer_output_override-based debug/research tasks
    // (SERVER_TASK_TYPE_JSPACE_COMPARE_GENERATION) when the server is
    // provisioned with >= 3 parallel slots (`--parallel 3` or more).
    // Normal completion routing NEVER selects this slot (see the
    // `slot.id == jspace_reserved_slot_id()` skip below in both
    // get_available_slot() loops) — this is what makes it safe for the
    // J-space task to call llama_decode/llama_set_layer_output_override
    // directly on `ctx` with this slot's seq_id, without any risk of
    // racing with concurrent production traffic on the other slots.
    // Returns -1 (no reservation, no slot excluded) if n_parallel < 3, so
    // deployments that haven't opted in keep their full normal capacity.
    int32_t jspace_reserved_slot_id() const {
        return jspace_reserved_slot_id_of((int32_t) params_base.n_parallel);
    }

    // Nombre de sequences offertes aux clients (total moins la reservee).
    int32_t jspace_slots_offerts() const {
        return jspace_slots_offerts_of((int32_t) params_base.n_parallel);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // [obrain P1] IDENTITE DU CLIENT ET ATTRIBUTION DE SA SEQUENCE.
    //
    // POURQUOI L'IDENTITE EST PRISE DANS CET ORDRE : `client_id` est une
    // identite EXPLICITE (le chat peut envoyer son cid, et deux conversations
    // distinctes recoivent alors deux sequences distinctes meme si leurs
    // hachages tombent sur le meme slot). A defaut, le serveur n'a QUE le
    // `id_slot` demande -- c'est deja une identite stable par conversation,
    // puisque le client le derive de son cid (`hash(cid) % total_slots`).
    // Les deux cles sont prefixees pour qu'un cid ne puisse pas entrer en
    // collision avec un slot numerote.
    //
    // POURQUOI LE NOM DE LA CLE EST JOURNALISE : le mapping client -> sequence
    // est le contrat que ce correctif ajoute ; il doit etre lisible dans le
    // journal au moment ou il est etabli, pas seulement deduit.
    // ═══════════════════════════════════════════════════════════════════════
    static std::string cle_client_de(const server_task & task) {
        if (!task.client_id.empty()) return "cid:" + task.client_id;
        if (task.id_slot >= 0)       return "slot:" + std::to_string(task.id_slot);
        return std::string();   // aucune identite : selection automatique du serveur
    }

    // Rend la sequence SERVEUR a utiliser pour cette tache (-1 = laisser le
    // serveur choisir). Elle n'est JAMAIS la sequence reservee.
    int32_t seq_client(const server_task & task) {
        const std::string cle = cle_client_de(task);
        // L'ORACLE D'OCCUPATION : le slot EST la sequence (cf.
        // `batch.seq_id[i][0] == slot.id` et `common_context_seq_rm(ctx, slot.id, ...)`),
        // donc « libre MAINTENANT » se lit sur le slot lui-meme.
        //
        // POURQUOI LE REGISTRE EN A BESOIN : sans cette information, il
        // reprendrait la sequence la plus froide MEME SI SON CLIENT EST EN
        // TRAIN DE GENERER. Avec N clients actifs sur N-1 sequences, la plus
        // froide est toujours celle qui vient de demander le moins recemment :
        // la reprise faisait donc ROTATION, et TOUS les clients -- y compris
        // ceux qui n'avaient rien demande -- re-prefillaient leur contexte a
        // chaque etape. C'est exactement le cout que `id_slot` existe pour
        // eviter, et c'est pire que de n'epingler personne.
        const obrain_seq::Attribution a = registre_client.attribuer(
            cle, task.id_slot,
            [this](int32_t s) {
                return s >= 0 && s < (int32_t) slots.size() && !slots[(size_t) s].is_processing();
            });
        if (a.nouvelle) {
            SRV_INF("[obrain-p1] client '%s' -> sequence %d (demande %d, reservee %d, offertes %d)%s%s\n",
                    cle.c_str(), a.seq_id, task.id_slot, jspace_reserved_slot_id(), jspace_slots_offerts(),
                    a.remap ? " [RE-MAPPE : la sequence demandee etait INUTILISABLE]" : "",
                    a.reaffecte ? " [reprise de la sequence d'un fil INACTIF]" : "");
        } else if (a.sature) {
            // POOL SATURE ET TOUTES LES SEQUENCES ACTIVES : ce fil n'aura
            // pas de sequence stable. On le DIT (c'est un
            // sous-dimensionnement de `--parallel`, et l'operateur doit le
            // voir), et on laisse la selection automatique du serveur
            // decider -- elle exclut deja la sequence reservee et ne prend
            // qu'un slot libre.
            SRV_WRN("[obrain-p1] client '%s' : pool sature (%d sequence(s) offerte(s), toutes "
                    "ACTIVES) — ce fil n'est PAS epingle (selection automatique du serveur) ; "
                    "augmenter --parallel si ce fil doit garder son contexte\n",
                    cle.c_str(), jspace_slots_offerts());
        }
        return a.seq_id;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // [obrain P1] GARDE-FOU DE LA SEQUENCE RESERVEE, APPELE PAR LES DEUX
    // TACHES J-SPACE AVANT LEUR PREMIER `llama_memory_seq_rm`.
    //
    // POURQUOI IL EXISTE ALORS QUE LE REGISTRE REND DEJA LE CAS IMPOSSIBLE :
    // « l'extraction n'efface jamais le KV d'un client » est une garantie
    // VERIFIEE, pas une deduction. Ici, on la verifie sur les deux seules
    // choses qui peuvent la contredire -- un slot en cours de generation sur
    // cette sequence, et une attribution residuelle dans le registre. Si l'une
    // des deux est vraie, on REFUSE l'extraction (aucun octet d'etat ecrit)
    // au lieu d'effacer silencieusement le travail d'un client.
    //
    // ET LE PREFIXE RESIDUEL EST EFFACE AVANT L'EXTRACTION : sur un serveur
    // demarre AVANT ce correctif, un client a pu installer son prefixe sur
    // cette sequence. La sequence va etre purgee -- c'est son role --, et si
    // on laissait la COMPTABILITE de ce prefixe en place (`slots[r].prompt`),
    // la requete suivante de ce client ne re-decoderait rien et lirait un KV
    // vide : exactement le defaut qu'on ferme. On efface donc les deux
    // ensemble : le client re-decodera son prefixe (du temps), il ne lira
    // jamais un cache qu'il croit plein et qui est vide (faux).
    // ═══════════════════════════════════════════════════════════════════════
    bool reserver_sequence_pour_jspace(std::string & pourquoi) {
        const int32_t r = jspace_reserved_slot_id();
        if (r < 0) {
            pourquoi = "aucune sequence reservee : demarrer le serveur avec --parallel 3 (ou plus)";
            return false;
        }
        if (r >= (int32_t) slots.size()) {
            pourquoi = "sequence reservee " + std::to_string(r) +
                       " hors des slots existants (" + std::to_string(slots.size()) + ")";
            return false;
        }
        server_slot & s = slots[r];   // le slot EST la sequence : batch.seq_id[i][0] == slot.id

        if (s.is_processing()) {
            pourquoi = "sequence reservee " + std::to_string(r) +
                       " OCCUPEE par une generation en cours : l'effacer detruirait le KV de ce client "
                       "(extraction refusee, aucun etat n'a ete ecrit)";
            SRV_ERR("[obrain-p1] %s\n", pourquoi.c_str());
            return false;
        }

        const std::string occupant = registre_client.occupant(r);
        if (!occupant.empty()) {
            pourquoi = "sequence reservee " + std::to_string(r) + " attribuee au client '" + occupant +
                       "' : incoherence du registre, extraction refusee (le KV de ce client serait efface)";
            SRV_ERR("[obrain-p1] %s\n", pourquoi.c_str());
            return false;
        }

        if (s.prompt.n_tokens() > 0) {
            SRV_WRN("[obrain-p1] sequence reservee %d : prefixe RESIDUEL de %zu jetons (serveur demarre "
                    "avant le correctif, ou restauration de slot) — comptabilite effacee AVANT l'extraction "
                    "(le client re-decoder, il ne lira pas un KV vide)\n",
                    r, s.prompt.tokens.size());
            s.prompt_clear(false);
        }
        return true;
    }

    // [obrain FIX-inject-dos] Decodage FRACTIONNE des sequences J-space sur la
    // seq reservee.
    //
    // BUG CORRIGE ICI (deni de service, preexistant, pas une regression) : les
    // deux chemins J-space (JSPACE_COMPARE_GENERATION et
    // JSPACE_INJECT_KNOWLEDGE) remplissaient UNE SEULE llama_batch avec
    // l'integralite des tokens (fact_text / prompt). Au-dela de n_batch (2048
    // par defaut), llama_decode() declenche
    //     GGML_ASSERT(n_tokens_all <= cparams.n_batch)   [src/llama-context.cpp:2161]
    // qui appelle ggml_abort() : le PROCESS SERVEUR MEURT, sans meme renvoyer
    // de reponse HTTP au client. Injecter un document d'un peu plus de ~2048
    // tokens suffisait donc a tuer :9910.
    //
    // Le decoupage suit la discipline de update_slots()/decode() dans ce meme
    // fichier (la reference du projet) : tranches de std::min(n_batch, reste),
    // avancement seulement sur decode reussi, et retry a n_batch/2 quand
    // llama_decode renvoie 1 (plus de place dans le KV cache).
    //
    // Deux invariants preserves par rapport a la version mono-lot :
    //  - les positions restent absolues et contigues (pos_start + off + i), ce
    //    qui est indispensable pour que le splice KV reste coherent ;
    //  - `want_logits` ne marque QUE le tout dernier token de la sequence
    //    complete, pas le dernier de chaque tranche (les tranches
    //    intermediaires sont du pur prefill, exactement comme un prompt normal).
    int jspace_decode_chunked(llama_context * ctx,
                              int32_t seq_id,
                              const std::vector<llama_token> & toks,
                              int32_t pos_start,
                              bool want_logits) {
        const int32_t n_toks = (int32_t) toks.size();
        if (n_toks == 0) {
            return 0;
        }

        int32_t n_batch = std::max<int32_t>(1, (int32_t) llama_n_batch(ctx));

        // capacite allouee une seule fois ; les retries ne font que reduire
        // n_batch, donc cette capacite reste toujours suffisante.
        llama_batch b = llama_batch_init(std::min(n_batch, n_toks), 0, 1);

        int rc = 0;
        for (int32_t off = 0; off < n_toks; /* avance uniquement si la tranche passe */) {
            const int32_t n_cur = std::min(n_batch, n_toks - off);

            for (int32_t i = 0; i < n_cur; i++) {
                b.token[i]     = toks[off + i];
                b.pos[i]       = pos_start + off + i;
                b.n_seq_id[i]  = 1;
                b.seq_id[i][0] = seq_id;
                b.logits[i]    = want_logits && (off + i == n_toks - 1);
            }
            b.n_tokens = n_cur;

            rc = llama_decode(ctx, b);

            if (rc == 0) {
                off += n_cur;
                continue;
            }

            // rc == 1 : pas de place KV pour cette tranche -> on retente plus
            // petit, comme server_context_impl::decode(). En dessous de 1 token
            // il n'y a plus rien a tenter : on remonte l'erreur.
            if (rc == 1 && n_batch > 1) {
                n_batch /= 2;
                SRV_WRN("[jspace] plus de place dans le KV cache, nouvelle tentative avec n_batch = %d (off = %d / %d)\n",
                        n_batch, off, n_toks);
                continue;
            }

            break;
        }

        llama_batch_free(b);
        return rc;
    }

    server_slot * get_slot_by_cmpl_id(const std::string & cmpl_id) {
        if (cmpl_id.empty()) {
            return nullptr;
        }

        for (server_slot & slot : slots) {
            if (slot.is_processing() && slot.task && slot.task->params.oaicompat_cmpl_id == cmpl_id) {
                return &slot;
            }
        }

        return nullptr;
    }

    server_slot * get_available_slot(const server_task & task) {
        server_slot * ret = nullptr;

        bool update_cache = false;
        const int32_t reserved_id = jspace_reserved_slot_id();

        // [obrain P1] LE DEFAUT ETAIT ICI, ET NULLE PART AILLEURS.
        //
        // Les deux boucles de SELECTION AUTOMATIQUE ci-dessous excluent la
        // sequence reservee (`slot.id == reserved_id`), mais ce chemin-ci --
        // celui d'un client qui epingle sa conversation -- appelait
        // `get_slot_by_id(task.id_slot)` directement : il rendait donc la
        // sequence RESERVEE a qui la demandait. Le chat obrain la demande
        // (`id_slot = hash(cid) % total_slots`) et un fil sur `n_parallel`
        // tombait dessus, puis l'extraction J-space effacait son KV.
        //
        // `seq_client` rend une sequence NON RESERVEE, STABLE par client. Un
        // client qui visait la reservee est RE-MAPPE (il garde son epinglage),
        // il n'est pas renvoye vers la selection LRU -- cela lui ferait perdre
        // son prefixe a chaque etape, donc couterait exactement le benefice
        // pour lequel `id_slot` existe.
        const int32_t id_slot_effectif = seq_client(task);

        // if a specific slot is requested, use it (still goes through cache update logic below)
        if (id_slot_effectif != -1) {
            ret = get_slot_by_id(id_slot_effectif);
            if (ret) {
                SLT_INF(*ret, "selected slot by id (%d)%s\n", id_slot_effectif,
                        (id_slot_effectif != task.id_slot) ? " [sequence re-mappee par le registre client]" : "");
            }
        }

        // find the slot that has at least n% prompt similarity
        if (slot_prompt_similarity != 0.0f) {
            float sim_best = 0;

            for (server_slot & slot : slots) {
                // [obrain] reserved J-space slot — never selectable by
                // normal completion traffic (see jspace_reserved_slot_id).
                if (slot.id == reserved_id) {
                    continue;
                }
                if (id_slot_effectif != -1 && slot.id != id_slot_effectif) {
                    continue;
                }

                // skip the slot if it is not available
                if (slot.is_processing()) {
                    continue;
                }

                const auto & tokens = slot.prompt.tokens;

                // skip the slot if it does not contains cached tokens
                if (tokens.empty()) {
                    continue;
                }

                // fraction of the Longest Common Prefix length with respect to the input prompt length
                const float sim_cur = float(tokens.get_common_prefix(task.tokens)) / task.tokens.size();

                // select the current slot if the criteria match
                if (sim_cur > sim_best && sim_cur > slot_prompt_similarity) {
                    sim_best = sim_cur;

                    ret = &slot;
                }
            }

            if (ret != nullptr) {
                const float f_keep = (sim_best*task.tokens.size()) / ret->prompt.tokens.size();

                if (id_slot_effectif == -1) {
                    SLT_INF(*ret, "selected slot by LCP similarity, sim_best = %.3f (> %.3f thold), f_keep = %.3f\n",
                            sim_best, slot_prompt_similarity, f_keep);
                }

                // if we are about to lose a large portion of the existing context - save it in the prompt cache
                if (f_keep < 0.5f) {
                    update_cache = true;
                }
            }
        }

        // find the slot that has been least recently used
        if (ret == nullptr) {
            int64_t t_last = -1;

            for (server_slot & slot : slots) {
                if (slot.id == reserved_id) {
                    continue;
                }
                // skip the slot if it is not available
                if (slot.is_processing()) {
                    continue;
                }

                // select the current slot if the criteria match
                if (!ret || slot.t_last_used <= t_last) {
                    t_last = slot.t_last_used;
                    ret = &slot;
                }
            }

            if (ret != nullptr) {
                SLT_INF(*ret, "selected slot by LRU, t_last = %" PRId64 "\n", t_last);

                update_cache = true;
            }
        }

        if (ret) {
            update_cache = update_cache && prompt_cache;

            // cache prompts only for completion tasks
            update_cache = update_cache && task.type == SERVER_TASK_TYPE_COMPLETION;

            // [obrain SPLICE-CHAT] une tache a prefixe importe ne participe PAS au
            // cache de prompts : (a) elle n'y LIT rien — son prefixe vient de
            // l'etat KV importe, pas d'un prompt anterieur, donc la laisser
            // charger un prefixe ressemble a une optimisation mais ne fait que
            // recouvrir ce qu'on vient d'importer ; (b) elle n'y ECRIT rien — un
            // prefixe spliCe associe a un contexte brouillon non homogene (cf.
            // server_slot::spec_allowed) ne doit pas etre rejoue plus tard par une
            // requete qui, elle, aurait la speculation active. Sans `state_ref`,
            // cette ligne est inerte.
            update_cache = update_cache && !task.spliced();

            if (update_cache) {
                SRV_TRC("%s", "updating prompt cache\n");

                const int64_t t_start = ggml_time_us();

                ret->prompt_save(*prompt_cache);

                if (!ret->prompt_load(*prompt_cache, task.tokens)) {
                    ret->prompt_clear(false);
                }

                prompt_cache->update();

                SRV_TRC("prompt cache update took %.2f ms\n", (ggml_time_us() - t_start) / 1000.0);
            }
        }

        return ret;
    }

    // return true if at least one slot has been cleared
    // TODO: improve logic
    //       - smarter decision which slot to clear (LRU or longest prompt?)
    //       - move slot to level 2 cache instead of removing?
    //       - instead of purging, try to store and resume later?
    bool try_clear_idle_slots() {
        bool res = false;

        if (!params_base.kv_unified) {
            return res;
        }

        for (auto & slot : slots) {
            if (slot.is_processing()) {
                continue;
            }

            if (slot.prompt.n_tokens() > 0) {
                SRV_WRN("purging slot %d with %zu tokens\n", slot.id, slot.prompt.tokens.size());

                slot.prompt_clear(false);

                res = true;

                // clear slots one by one
                break;
            }
        }

        return res;
    }

    std::vector<common_adapter_lora_info> construct_lora_list(const std::map<int, float> & config) const {
        std::vector<common_adapter_lora_info> output = params_base.lora_adapters; // copy
        for (size_t i = 0; i < output.size(); ++i) {
            auto it = config.find(i);
            if (it != config.end()) {
                output[i].scale = it->second;
            } else {
                output[i].scale = 0.0f;
            }
        }
        return output;
    }

    bool launch_slot_with_task(server_slot & slot, server_task && task) {
        // process per-request lora adapters
        if (!task.params.lora.empty()) {
            auto task_loras = construct_lora_list(task.params.lora);
            if (!are_lora_equal(task_loras, slot.lora)) {
                // if lora has changed, check to see if the cache should be cleared
                if (lora_should_clear_cache(slot.lora, task_loras)) {
                    SLT_TRC(slot, "clearing cache for lora change. %zu loras -> %zu loras\n", slot.lora.size(), task.params.lora.size());
                    slot.prompt.tokens.clear();
                } else {
                    SLT_TRC(slot, "keeping cache for alora. %zu target loras\n", task_loras.size());
                }
                slot.lora = task_loras;
            }
        } else {
            slot.lora = params_base.lora_adapters;
        }

        // if using alora, make sure it's only a single one requested and active
        size_t alora_invocation_start = task.tokens.size();
        if (lora_all_alora(slot.lora)) {
            const auto & enabled_ids = lora_get_enabled_ids(slot.lora);
            // TODO: This will error out if a user requests two aloras, but only
            // provides the activation string for one. We could, instead search
            // for all requested alora activation strings and then either keep
            // only the last one, or reject if multiple are found.
            if (enabled_ids.size() != 1) {
                send_error(task, "Cannot run multiple aLoRAs in a single request", ERROR_TYPE_INVALID_REQUEST);
                return false;
            }
            const auto & lora = slot.lora[enabled_ids[0]].ptr;

            // get the pointer and count for the invocation tokens
            const uint64_t      n_invocation_tokens = llama_adapter_get_alora_n_invocation_tokens(lora);
            const llama_token * invocation_tokens   = llama_adapter_get_alora_invocation_tokens  (lora);

            // scan backwards through the prompt tokens to find the last
            // occurrence of the invocation sequence
            int match_idx = static_cast<int>(n_invocation_tokens) - 1;
            for (int i = task.tokens.size() - 1; i >= 0; --i) {
                // the token in this position matches the next token to find in
                // the invocation sequence
                if (task.tokens[i] == invocation_tokens[match_idx]) {
                    // if it's a full match, we've found the start
                    if (match_idx == 0) {
                        alora_invocation_start = i;
                        break;
                    }
                    // otherwise, check the next token in the sequence
                    --match_idx;
                } else {
                    // no match in this position, so start looking over again
                    match_idx = static_cast<int>(n_invocation_tokens) - 1;
                }
            }

            // if the activation string is not found, disable the alora
            if (alora_invocation_start == task.tokens.size()) {
                SLT_DBG(slot, "alora %zu requested, but not found. deactivating\n", enabled_ids[0]);
                slot.lora[enabled_ids[0]].scale = 0.0f;
            } else {
                SLT_DBG(slot, "alora %zu activated starting at %zu\n", enabled_ids[0], alora_invocation_start);
                slot.alora_invocation_start = alora_invocation_start;
            }
        }

        if (!task.tokens.validate(ctx_tgt)) {
            send_error(task, "Prompt contains invalid tokens", ERROR_TYPE_INVALID_REQUEST);
            return false;
        }

        SLT_DBG(slot, "launching slot : %s\n", safe_json_to_str(slot.to_json()).c_str());

        // initialize samplers
        if (task.need_sampling()) {
            try {
                slot.smpl.reset(common_sampler_init(model_tgt, task.params.sampling));
            } catch (std::exception & e) {
                std::string err_msg = std::string("Failed to initialize samplers: ") + e.what();
                send_error(task, err_msg, ERROR_TYPE_INVALID_REQUEST);
                return false;
            }

            const bool need_pre_sample_logits = task.params.sampling.n_probs > 0 && !task.params.post_sampling_probs;

            bool backend_sampling = true;

            backend_sampling &= task.params.sampling.backend_sampling;

            // TODO: speculative decoding requires multiple samples per batch - not supported yet
            backend_sampling &= !(slot.can_speculate());

            // TODO: getting pre sampling logits is not yet supported with backend sampling
            backend_sampling &= !need_pre_sample_logits;

            // TODO: tmp until backend sampling is fully implemented
            if (backend_sampling) {
                llama_set_sampler(ctx_tgt, slot.id, common_sampler_get(slot.smpl.get()));
            } else {
                llama_set_sampler(ctx_tgt, slot.id, nullptr);
            }

            SLT_TRC(slot, "sampler chain: %s\n", common_sampler_print(slot.smpl.get()).c_str());
            SLT_TRC(slot, "sampler params: \n%s\n", task.params.sampling.print().c_str());
        } else {
            slot.smpl.reset();
        }

        slot.task = std::make_unique<const server_task>(std::move(task));

        slot.state = slot.task->is_child()
            ? SLOT_STATE_WAIT_OTHER // wait for the parent to process prompt
            : SLOT_STATE_STARTED;

        // reset server kill-switch counter
        n_empty_consecutive = 0;

        SLT_INF(slot, "processing task, is_child = %d\n", slot.task->is_child());
        return true;
    }

    bool process_token(completion_token_output & result, server_slot & slot) {
        // remember which tokens were sampled - used for repetition penalties during sampling
        const std::string token_str = result.text_to_send;
        slot.sampled = result.tok;

        // [obrain R3-A] VOIE NORMALE -- c'est ICI que le jeton genere devient COMMITTED (couvre le
        // decodage sur ABSTAIN *et* le jeton bonus de la voie speculative). Un seul commit par jeton.
        if (slot.obrain_enabled) {
            const int32_t tk = (int32_t) slot.sampled;
            const int is_stop = (slot.stop != STOP_TYPE_NONE) ? 1 : 0;
            int is_eog = -1;
            if (slot.ctx_tgt) { is_eog = llama_vocab_is_eog(llama_model_get_vocab(llama_get_model(slot.ctx_tgt)), tk) ? 1 : 0; }
            slot.obrain_st.obrain_commit(&tk, 1, obrain_proposer_state::OBRAIN_SRC_NORMAL);
            // [R3-A5] plus de compteur disperse : obrain_count_output inscrit l'evenement ET incremente
            // output_tokens_counted, qui est le resume du ledger.
            slot.obrain_st.obrain_count_output((int) tk, "OUTPUT_NORMAL", is_stop, is_eog,
                                                     (int64_t) slot.id, slot.obrain_generation);
            if (!slot.obrain_st.check_invariants("commit normal")) {
                GGML_ABORT("OBRAIN_INVARIANT_KO commit normal");
            }
            fprintf(stderr, "OBRAIN_R3A commit_normal slot=%d tok=%d state=%lld committed=%lld "
                            "normal=%lld spec=%lld output_tokens=%lld\n",
                    slot.id, (int) tk, (long long) slot.obrain_st.state_token_count(),
                    (long long) slot.obrain_st.committed_updates,
                    (long long) slot.obrain_st.normal_commit_tokens,
                    (long long) slot.obrain_st.spec_commit_tokens,
                    (long long) slot.obrain_output_tokens);
            fflush(stderr);
        }

        slot.generated_text += token_str;
        if (slot.task->params.return_tokens) {
            slot.generated_tokens.push_back(result.tok);
        }
        slot.has_next_token = true;

        // check if there is incomplete UTF-8 character at the end
        bool incomplete = validate_utf8(slot.generated_text) < slot.generated_text.size();

        // search stop word and delete it
        if (!incomplete) {
            size_t pos = std::min(slot.n_sent_text, slot.generated_text.size());

            const std::string str_test = slot.generated_text.substr(pos);
            bool send_text = true;

            size_t stop_pos = slot.find_stopping_strings(str_test, token_str.size(), true);
            if (stop_pos != std::string::npos) {
                slot.generated_text.erase(
                    slot.generated_text.begin() + pos + stop_pos,
                    slot.generated_text.end());
                pos = std::min(slot.n_sent_text, slot.generated_text.size());
            } else if (slot.has_next_token && !llama_vocab_is_eog(vocab, result.tok) ) {
                stop_pos = slot.find_stopping_strings(str_test, token_str.size(), false);
                send_text = stop_pos == std::string::npos;
            }

            // check if there is any token to predict
            if (send_text) {
                // no send the stop word in the response
                result.text_to_send = slot.generated_text.substr(pos, std::string::npos);
                slot.n_sent_text += result.text_to_send.size();
                // add the token to slot queue and cache
            } else {
                result.text_to_send = "";
            }

            slot.add_token(result);
            if (slot.task->params.stream) {
                send_partial_response(slot, result, false);
            }
        }

        if (incomplete) {
            slot.has_next_token = true;
        }

        // if context shifting is disabled, make sure that we don't run out of context
        if (!params_base.ctx_shift && slot.prompt.n_tokens() + 1 >= slot.n_ctx) {
            slot.truncated      = true;
            slot.stop           = STOP_TYPE_LIMIT;
            slot.has_next_token = false;

            SLT_DBG(slot, "stopped due to running out of context capacity, prompt.n_tokens() = %d, task.n_tokens = %d, n_decoded = %d, n_ctx = %d\n",
                    slot.prompt.n_tokens(), slot.task->n_tokens(), slot.n_decoded, slot.n_ctx);
        }

        // check the limits
        if (slot.n_decoded > 0 && slot.has_next_token && !slot.has_budget(params_base)) {
            slot.stop           = STOP_TYPE_LIMIT;
            slot.has_next_token = false;

            SLT_DBG(slot, "stopped by limit, n_decoded = %d, n_predict = %d\n", slot.n_decoded, slot.task->params.n_predict);
        }

        if (slot.has_new_line) {
            // require that each new line has a whitespace prefix (i.e. indentation) of at least slot.params.n_indent
            if (slot.task->params.n_indent > 0) {
                // check the current indentation
                // TODO: improve by not doing it more than once for each new line
                if (slot.last_nl_pos > 0) {
                    size_t pos = slot.last_nl_pos;

                    int n_indent = 0;
                    while (pos < slot.generated_text.size() && (slot.generated_text[pos] == ' ' || slot.generated_text[pos] == '\t')) {
                        n_indent++;
                        pos++;
                    }

                    if (pos < slot.generated_text.size() && n_indent < slot.task->params.n_indent) {
                        slot.stop           = STOP_TYPE_LIMIT;
                        slot.has_next_token = false;

                        // cut the last line
                        slot.generated_text.erase(pos, std::string::npos);

                        SLT_DBG(slot, "stopped by indentation limit, n_decoded = %d, n_indent = %d\n", slot.n_decoded, n_indent);
                    }
                }

                // find the next new line
                {
                    const size_t pos = slot.generated_text.find('\n', slot.last_nl_pos);

                    if (pos != std::string::npos) {
                        slot.last_nl_pos = pos + 1;
                    }
                }
            }
        }

        // check if there is a new line in the generated text
        if (result.text_to_send.find('\n') != std::string::npos) {
            slot.has_new_line = true;

            // if we have seen a new line, we stop after a certain time limit, but only upon another new line
            if (slot.task->params.t_max_predict_ms > 0 && (ggml_time_us() - slot.t_start_generation > 1000.0f*slot.task->params.t_max_predict_ms)) {
                slot.stop           = STOP_TYPE_LIMIT;
                slot.has_next_token = false;

                SLT_DBG(slot, "stopped by time limit, n_decoded = %d, t_max_predict_ms = %d ms\n", slot.n_decoded, (int) slot.task->params.t_max_predict_ms);
            }
        }

        if (llama_vocab_is_eog(vocab, result.tok)) {
            slot.stop           = STOP_TYPE_EOS;
            slot.has_next_token = false;

            SLT_DBG(slot, "%s", "stopped by EOS\n");
        }

        SLT_DBG(slot, "n_decoded = %d, n_remaining = %d, next token: %5d '%s'\n", slot.n_decoded, slot.n_remaining, result.tok, token_str.c_str());

        return slot.has_next_token; // continue
    }

    void populate_token_probs(const server_slot & slot, completion_token_output & result, bool post_sampling, bool special, int idx) const {
        const size_t n_probs_request = slot.task->params.sampling.n_probs;

        if (post_sampling) {
            const auto * cur_p = common_sampler_get_candidates(slot.smpl.get(), true);
            const size_t max_probs = cur_p->size;
            const size_t n_probs = std::min(max_probs, n_probs_request);

            // set probability for sampled token
            for (size_t i = 0; i < max_probs; i++) {
                if (cur_p->data[i].id == result.tok) {
                    result.prob = cur_p->data[i].p;
                    break;
                }
            }

            // set probability for top n_probs tokens
            result.probs.reserve(n_probs);
            for (size_t i = 0; i < n_probs; i++) {
                // Some samplers do return 0.0 probabilities, others don't.
                // Filter 0.0 probailities, to ensure the behavior is consistent.
                if (cur_p->data[i].p == 0.0) {
                    break;
                }

                result.probs.push_back({
                    cur_p->data[i].id,
                    common_token_to_piece(ctx_tgt, cur_p->data[i].id, special),
                    cur_p->data[i].p
                });
            }
        } else {
            std::vector<llama_token_data> cur = get_token_probabilities(ctx_tgt, idx, n_probs_request);
            const size_t max_probs = cur.size();
            const size_t n_probs = std::min(max_probs, n_probs_request);

            // set probability for sampled token
            for (size_t i = 0; i < max_probs; i++) {
                // set probability for sampled token
                if (cur[i].id == result.tok) {
                    result.prob = cur[i].p;
                    break;
                }
            }

            // set probability for top n_probs tokens
            result.probs.reserve(n_probs);
            for (size_t i = 0; i < n_probs; i++) {
                result.probs.push_back({
                    cur[i].id,
                    common_token_to_piece(ctx_tgt, cur[i].id, special),
                    cur[i].p
                });
            }
        }
    }

    // [obrain J2] read-only confidence probe: top-k entropy of the sampler candidates
    // for the token just sampled. reads cur_p only — NEVER alters sampling or
    // generation (reactive re-injection designs are documented failures in this
    // repo ; this is a pure signal, summarized once in the final response).
    static constexpr int    PROBE_TOP_K       = 16;
    static constexpr float  PROBE_CONF_THOLD  = 0.95f; // top-1 >= 0.95 => confident (F2)
    static constexpr size_t PROBE_N_PEAKS     = 5;

    void probe_confidence(server_slot & slot, llama_token id) const {
        const auto * cur_p = common_sampler_get_candidates(slot.smpl.get(), false);
        if (cur_p == nullptr || cur_p->size == 0) {
            return;
        }

        // single-pass top-k selection by logit (cur_p may be unsorted and large)
        float top[PROBE_TOP_K];
        int n_top = 0;
        for (size_t i = 0; i < cur_p->size; ++i) {
            const float l = cur_p->data[i].logit;
            if (n_top == PROBE_TOP_K && l <= top[PROBE_TOP_K - 1]) {
                continue;
            }
            int j = std::min(n_top, PROBE_TOP_K - 1);
            while (j > 0 && top[j - 1] < l) {
                top[j] = top[j - 1];
                --j;
            }
            top[j] = l;
            if (n_top < PROBE_TOP_K) {
                ++n_top;
            }
        }

        // softmax over the top-k logits, entropy in nats
        const float l_max = top[0];
        float sum = 0.0f;
        for (int i = 0; i < n_top; ++i) {
            top[i] = expf(top[i] - l_max);
            sum += top[i];
        }
        float entropy = 0.0f;
        for (int i = 0; i < n_top; ++i) {
            const float q = top[i] / sum;
            if (q > 0.0f) {
                entropy -= q * logf(q);
            }
        }

        slot.probe_idx    .push_back(slot.n_decoded); // index of this token (n_decoded not yet incremented)
        slot.probe_tok    .push_back(id);
        slot.probe_entropy.push_back(entropy);
        slot.probe_top1   .push_back(top[0] / sum);
    }

    json probe_summary(const server_slot & slot) const {
        const size_t n = slot.probe_entropy.size();

        json res = {
            {"n_probed",  n},
            {"n_decoded", slot.n_decoded},
            {"top_k",     PROBE_TOP_K},
            {"conf_thold", PROBE_CONF_THOLD},
        };

        if (n == 0) {
            return res;
        }

        double e_sum = 0.0;
        float  e_max = 0.0f;
        size_t n_low = 0;
        int32_t streak = 0, streak_max = 0, streak_max_beg = -1, streak_beg = -1;
        for (size_t i = 0; i < n; ++i) {
            const float e = slot.probe_entropy[i];
            e_sum += e;
            e_max  = std::max(e_max, e);
            if (slot.probe_top1[i] < PROBE_CONF_THOLD) {
                if (streak == 0) {
                    streak_beg = slot.probe_idx[i];
                }
                ++streak;
                ++n_low;
                if (streak > streak_max) {
                    streak_max = streak;
                    streak_max_beg = streak_beg;
                }
            } else {
                streak = 0;
            }
        }

        // doubt peaks: top-N probed positions by entropy
        std::vector<size_t> order(n);
        for (size_t i = 0; i < n; ++i) {
            order[i] = i;
        }
        const size_t n_peaks = std::min(PROBE_N_PEAKS, n);
        std::partial_sort(order.begin(), order.begin() + n_peaks, order.end(),
                [&](size_t a, size_t b) { return slot.probe_entropy[a] > slot.probe_entropy[b]; });

        json peaks = json::array();
        for (size_t k = 0; k < n_peaks; ++k) {
            const size_t i = order[k];
            peaks.push_back({
                {"i",       slot.probe_idx[i]},
                {"entropy", slot.probe_entropy[i]},
                {"top1",    slot.probe_top1[i]},
                {"token",   common_token_to_piece(ctx_tgt, slot.probe_tok[i], true)},
            });
        }

        res["entropy_mean"]         = (float) (e_sum / n);
        res["entropy_max"]          = e_max;
        res["low_conf_frac"]        = (float) n_low / (float) n;
        res["low_conf_streak_max"]  = streak_max;
        res["low_conf_streak_beg"]  = streak_max_beg;
        res["peaks"]                = peaks;

        return res;
    }

    void send_error(const server_task & task, const std::string & error, const enum error_type type = ERROR_TYPE_SERVER) {
        send_error(task.id, error, type);
    }

    void send_error(const server_slot & slot, const std::string & error, const enum error_type type = ERROR_TYPE_SERVER) {
        send_error(slot.task->id, error, type, slot.task->n_tokens(), slot.n_ctx);
    }

    void send_error(const int id_task, const std::string & error, const enum error_type type = ERROR_TYPE_SERVER, const int32_t n_prompt_tokens = 0, const int32_t n_ctx = 0) {
        SRV_ERR("task id = %d, error: %s\n", id_task, error.c_str());

        if (type == ERROR_TYPE_EXCEED_CONTEXT_SIZE) {
            GGML_ASSERT(n_ctx > 0 && n_prompt_tokens > 0);
        }

        auto res = std::make_unique<server_task_result_error>();
        res->id              = id_task;
        res->err_type        = type;
        res->err_msg         = error;
        res->n_prompt_tokens = n_prompt_tokens;
        res->n_ctx           = n_ctx;

        queue_results.send(std::move(res));
    }

    // if multimodal is enabled, send an error and return false
    bool check_no_mtmd(const int id_task) {
        if (mctx) {
            send_error(id_task, "This feature is not supported by multimodal", ERROR_TYPE_NOT_SUPPORTED);
            return false;
        }
        return true;
    }

    void send_partial_response(server_slot & slot, const completion_token_output & tkn, bool is_progress, bool is_begin = false) {
        auto res = std::make_unique<server_task_result_cmpl_partial>();

        res->id    = slot.task->id;
        res->index = slot.task->index;

        if (is_progress) {
            res->is_progress        = true;
            res->progress.total     = slot.task->n_tokens();
            res->progress.cache     = slot.n_prompt_tokens_cache;
            res->progress.processed = slot.prompt.tokens.size();
            res->progress.time_ms   = (ggml_time_us() - slot.t_start_process_prompt) / 1000;
        }
        if (is_begin) {
            res->is_begin = true;
        } else {
            res->content = tkn.text_to_send;
            res->tokens  = { tkn.tok };
        }

        res->n_decoded             = slot.n_decoded;
        res->n_prompt_tokens       = slot.task->n_tokens();
        res->n_prompt_tokens_cache = slot.n_prompt_tokens_cache;
        res->post_sampling_probs   = slot.task->params.post_sampling_probs;

        res->verbose           = slot.task->params.verbose;
        res->res_type          = slot.task->params.res_type;
        res->oaicompat_model   = slot.task->params.oaicompat_model;
        res->oaicompat_cmpl_id = slot.task->params.oaicompat_cmpl_id;

        // populate res.probs_output
        if (slot.task->params.sampling.n_probs > 0) {
            res->prob_output = tkn; // copy the token probs
        }

        // populate timings if this is final response or timings_per_token is enabled
        if (slot.stop != STOP_TYPE_NONE || slot.task->params.timings_per_token) {
            res->timings = slot.get_timings();
        }

        queue_results.send(std::move(res));
    }

    void send_final_response(server_slot & slot) {
        auto res = std::make_unique<server_task_result_cmpl_final>();

        res->id      = slot.task->id;
        res->id_slot = slot.id;

        res->index = slot.task->index;

        // keep copy of last generated text for debugging purposes
        if (slots_debug) {
            slot.debug_generated_text = slot.generated_text;
        }

        // in stream mode, content and tokens are already in last partial chunk
        if (slot.task->params.stream) {
            res->content     = "";
            res->tokens      = llama_tokens{};
        } else {
            res->content     = std::move(slot.generated_text);
            res->tokens      = std::move(slot.generated_tokens);
        }
        res->timings         = slot.get_timings();
        res->prompt          = slot.task->tokens.detokenize(ctx_tgt, true);
        res->response_fields = std::move(slot.task->params.response_fields);

        res->truncated             = slot.truncated;
        res->n_decoded             = slot.n_decoded;
        res->n_prompt_tokens       = slot.task->n_tokens();
        res->n_prompt_tokens_cache = slot.n_prompt_tokens_cache;
        res->n_tokens_cached       = slot.prompt.n_tokens();
        res->has_new_line          = slot.has_new_line;
        res->stopping_word         = slot.stopping_word;
        res->stop                  = slot.stop;
        res->post_sampling_probs   = slot.task->params.post_sampling_probs;

        res->verbose           = slot.task->params.verbose;
        res->stream            = slot.task->params.stream;
        res->include_usage     = slot.task->params.include_usage;
        res->res_type          = slot.task->params.res_type;
        res->oaicompat_model   = slot.task->params.oaicompat_model;
        res->oaicompat_cmpl_id = slot.task->params.oaicompat_cmpl_id;

        // populate res.probs_output
        if (slot.task->params.sampling.n_probs > 0) {
            if (!slot.task->params.stream && slot.stop == STOP_TYPE_WORD) {
                const llama_tokens stop_word_toks = common_tokenize(ctx_tgt, slot.stopping_word, false);

                size_t safe_offset = std::min(slot.generated_token_probs.size(), stop_word_toks.size());
                res->probs_output = std::vector<completion_token_output>(
                        slot.generated_token_probs.begin(),
                        slot.generated_token_probs.end() - safe_offset);
            } else {
                res->probs_output = std::vector<completion_token_output>(
                        slot.generated_token_probs.begin(),
                        slot.generated_token_probs.end());
            }
        }

        // [obrain J2] confidence probe summary
        if (slot.task->params.confidence_probe) {
            res->confidence = probe_summary(slot);
        }

        res->generation_params = slot.task->params; // copy the parameters

        queue_results.send(std::move(res));
    }

    void send_embedding(const server_slot & slot, const llama_batch & batch) {
        auto res = std::make_unique<server_task_result_embd>();
        res->id        = slot.task->id;
        res->index     = slot.task->index;
        res->n_tokens  = slot.task->n_tokens();
        res->res_type  = slot.task->params.res_type;

        const int n_embd_out = llama_model_n_embd_out(model_tgt);

        std::vector<float> embd_res(n_embd_out, 0.0f);

        for (int i = 0; i < batch.n_tokens; ++i) {
            if (!batch.logits[i] || batch.seq_id[i][0] != slot.id) {
                continue;
            }

            const float * embd = nullptr;
            if (llama_pooling_type(slot.ctx_tgt) == LLAMA_POOLING_TYPE_NONE) {
                embd = llama_get_embeddings_ith(slot.ctx_tgt, i);
            } else {
                embd = llama_get_embeddings_seq(slot.ctx_tgt, batch.seq_id[i][0]);
            }

            if (embd == nullptr) {
                SLT_ERR(slot, "failed to get embeddings, token = %d, seq_id = %d\n", batch.token[i], batch.seq_id[i][0]);

                res->embedding.push_back(std::vector<float>(n_embd_out, 0.0f));
                continue;
            }

            // normalize only when there is pooling
            if (llama_pooling_type(slot.ctx_tgt) != LLAMA_POOLING_TYPE_NONE) {
                common_embd_normalize(embd, embd_res.data(), n_embd_out, slot.task->params.embd_normalize);
                res->embedding.push_back(embd_res);
                break;
            }

            res->embedding.emplace_back(embd, embd + n_embd_out);
        }

        SLT_DBG(slot, "%s", "sending embeddings\n");

        queue_results.send(std::move(res));
    }

    void send_rerank(const server_slot & slot, const llama_batch & batch) {
        auto res = std::make_unique<server_task_result_rerank>();
        res->id       = slot.task->id;
        res->index    = slot.task->index;
        res->n_tokens = slot.task->n_tokens();

        for (int i = 0; i < batch.n_tokens; ++i) {
            if (!batch.logits[i] || batch.seq_id[i][0] != slot.id) {
                continue;
            }

            const float * embd = llama_get_embeddings_seq(ctx_tgt, batch.seq_id[i][0]);
            if (embd == NULL) {
                embd = llama_get_embeddings_ith(ctx_tgt, i);
            }

            if (embd == NULL) {
                SLT_ERR(slot, "failed to get embeddings, token = %d, seq_id = %d\n", batch.token[i], batch.seq_id[i][0]);

                res->score = -1e6;
                continue;
            }

            res->score = embd[0];
        }

        SLT_DBG(slot, "sending rerank result, res.score = %f\n", res->score);

        queue_results.send(std::move(res));
    }

    //
    // Functions to process the task
    //

    // tokenize the input if it's set by CLI, return false on error
    bool tokenize_cli_input(server_task & task) {
        try {
            auto & prompt = task.cli_prompt;
            if (mctx != nullptr) {
                task.tokens = process_mtmd_prompt(mctx, prompt, task.cli_files);
            } else {
                task.tokens = std::move(tokenize_input_prompts(vocab, mctx, prompt, true, true)[0]);
            }
            task.cli_prompt.clear();
            task.cli_files.clear();
        } catch (const std::exception & e) {
            send_error(task, std::string("Failed to format input: ") + e.what(), ERROR_TYPE_INVALID_REQUEST);
            return false;
        }
        return true;
    }

    std::vector<server_slot *> get_free_slots(size_t n_slots_needed, int exclude_id_slot) {
        std::vector<server_slot *> free_slots;
        // [obrain P1] LA SEQUENCE RESERVEE N'EST PAS DAVANTAGE OFFERTE ICI.
        // Ce chemin sert les taches FILLES (n_cmpl > 1, speculation) : elles
        // portent le prefixe du parent et l'ecrivent dans le KV de la
        // sequence choisie. Une tache fille installee sur la sequence
        // reservee serait effacee par l'extraction suivante -- le meme
        // defaut que le chemin `id_slot`, par une autre porte.
        const int32_t reserve_p1 = jspace_reserved_slot_id();
        for (auto & slot : slots) {
            if (slot.id == reserve_p1) {
                continue;
            }
            if (!slot.is_processing() && slot.id != exclude_id_slot) {
                free_slots.push_back(&slot);
            }
            if (free_slots.size() >= n_slots_needed) {
                break;
            }
        }
        return free_slots;
    }

    // launch multiple slots for parent + child tasks
    bool launch_slots_with_parent_task(server_slot & parent_slot, std::vector<server_slot *> & child_slots, server_task && parent_task) {
        GGML_ASSERT(!parent_slot.is_processing());
        GGML_ASSERT(parent_task.is_parent());
        GGML_ASSERT(child_slots.size() == parent_task.child_tasks.size());

        int id_parent = parent_task.id;

        SRV_TRC("launching slots for parent task id_task = %d with %zu child tasks\n", id_parent, parent_task.child_tasks.size());

        // to be called in case of failure to release all launched slots
        auto release_slots = [this, id_parent]() {
            for (auto & slot : slots) {
                if (slot.is_processing() && (
                        slot.task->id == id_parent ||
                        slot.task->id_parent == id_parent
                )) {
                    slot.release();
                }
            }
        };

        // launch all child tasks first
        size_t idx = 0;
        GGML_ASSERT(child_slots.size() == parent_task.child_tasks.size());
        for (auto * slot : child_slots) {
            int id_child = parent_task.child_tasks[idx].id;
            if (!launch_slot_with_task(*slot, std::move(parent_task.child_tasks[idx]))) {
                SRV_ERR("failed to launch slot with child task, id_task = %d\n", id_child);
                release_slots();
                return false;
            }
            idx++;
        }

        // finally, launch the parent task
        if (!launch_slot_with_task(parent_slot, std::move(parent_task))) {
            SRV_ERR("failed to launch slot with task, id_task = %d\n", id_parent);
            release_slots();
            return false;
        }

        return true;
    }

    // n_tokens_cur: the number of tokens added to the batch for the current slot
    void create_checkpoint(server_slot & slot, const int64_t n_tokens_cur, llama_pos pos_min, llama_pos pos_max) {
        while (slot.prompt.checkpoints.size() >= (size_t) params_base.n_ctx_checkpoints) {
            // make room for the new checkpoint, if needed
            const auto & cur = slot.prompt.checkpoints.front();

            SLT_WRN(slot, "erasing old context checkpoint (pos_min = %d, pos_max = %d, n_tokens = %" PRId64 ", size = %.3f MiB)\n",
                    cur.pos_min, cur.pos_max, cur.n_tokens, (float) cur.size() / 1024 / 1024);

            slot.prompt.checkpoints.erase(slot.prompt.checkpoints.begin());
        }

        auto & cur = slot.prompt.checkpoints.emplace_back();

        // [TAG_CHECKPOINTS_FIX_POS_MIN]
        // TODO: here we incorrectly deterimne that the saved checkpoint data covers the [pos_min, pos_max] range
        //       this is not true for SWA models: https://github.com/ggml-org/llama.cpp/pull/24411#issuecomment-4677983225
        cur.update_pos(slot.prompt.n_tokens() - n_tokens_cur, pos_min, pos_max);

        cur.update_tgt(ctx_tgt, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
        cur.update_dft(ctx_dft, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
        // stash the draft's speculative state with the checkpoint
        common_speculative_get_state(spec.get(), slot.id, cur.data_spec);

        SLT_TRC(slot,
                "created context checkpoint %d of %d (pos_min = %d, pos_max = %d, n_tokens = %" PRId64 ", size = %.3f MiB)\n",
                (int) slot.prompt.checkpoints.size(), params_base.n_ctx_checkpoints, cur.pos_min,
                cur.pos_max, cur.n_tokens, (float) cur.size() / 1024 / 1024);
    }

    void process_single_task(server_task && task) {
        switch (task.type) {
            case SERVER_TASK_TYPE_COMPLETION:
            case SERVER_TASK_TYPE_INFILL:
            case SERVER_TASK_TYPE_EMBEDDING:
            case SERVER_TASK_TYPE_RERANK:
                {
                    // special case: if input is provided via CLI, tokenize it first
                    // otherwise, no need to tokenize as it's already done inside the HTTP thread
                    if (task.cli) {
                        if (!tokenize_cli_input(task)) {
                            break;
                        }
                    }

                    const int id_task = task.id;

                    server_slot * slot = get_available_slot(task);

                    //
                    // slot scheduling logic
                    //

                    if (slot == nullptr) {
                        // if no slot is available, we defer this task for processing later
                        SRV_DBG("no slot is available, defer task, id_task = %d\n", id_task);
                        queue_tasks.defer(std::move(task));
                        break;
                    }

                    if (slot->is_processing()) {
                        // if requested slot is unavailable, we defer this task for processing later
                        SRV_DBG("requested slot is unavailable, defer task, id_task = %d\n", id_task);
                        queue_tasks.defer(std::move(task));
                        break;
                    }

                    if (task.is_parent()) {
                        // try getting free slots for all child tasks
                        size_t n_child_tasks = task.child_tasks.size();
                        std::vector<server_slot *> child_slots = get_free_slots(n_child_tasks, slot->id);
                        if (child_slots.size() < n_child_tasks) {
                            SRV_DBG("not enough free slots for child tasks, n_free = %zu, n_children = %zu, defer task, id_task = %d\n", child_slots.size(), n_child_tasks, id_task);
                            queue_tasks.defer(std::move(task));
                            break;
                        }
                        if (!launch_slots_with_parent_task(*slot, child_slots, std::move(task))) {
                            SRV_ERR("failed to launch slot with parent task, id_task = %d\n", id_task);
                            break; // drop the task
                        }
                    } else if (!launch_slot_with_task(*slot, std::move(task))) {
                        SRV_ERR("failed to launch slot with task, id_task = %d\n", id_task);
                        break; // drop the task
                    }

                    if (params_base.cache_idle_slots) {
                        for (auto & slot : slots) {
                            if (!slot.is_processing()) {
                                SLT_TRC(slot, "%s", "saving idle slot to prompt cache\n");

                                if (slot.prompt_save(*prompt_cache)) {
                                    SLT_DBG(slot, "%s", "__TEST_TAG_CACHE_IDLE_SLOT__\n");
                                    prompt_cache->update();
                                }

                                if (params_base.kv_unified) {
                                    // [TAG_IDLE_SLOT_CLEAR]
                                    slot.prompt_clear(false);
                                }
                            }
                        }
                    }
                } break;
            case SERVER_TASK_TYPE_CANCEL:
                {
                    // release slot linked with the task id
                    for (auto & slot : slots) {
                        if (slot.task && slot.task->id == task.id_target) {
                            slot.release();
                            break;
                        }
                    }
                } break;
            case SERVER_TASK_TYPE_CONTROL:
                {
                    auto res = std::make_unique<server_task_result_control>();
                    res->id = task.id;

                    server_slot * slot = get_slot_by_cmpl_id(task.params.control_cmpl_id);
                    if (slot == nullptr) {
                        SRV_WRN("control %s on unknown completion id=%s, no live slot\n",
                                task.params.control_action.c_str(), task.params.control_cmpl_id.c_str());
                        res->success = false;
                        res->message = "no active completion for this id";
                        queue_results.send(std::move(res));
                        break;
                    }

                    if (task.params.control_action == "reasoning_end") {
                        // the budget sampler only exists when reasoning control was armed
                        if (!slot->task->params.sampling.reasoning_control) {
                            res->success = false;
                            res->message = "reasoning control not enabled for this completion";
                            queue_results.send(std::move(res));
                            break;
                        }
                        // act on the live slot mid generation, never defer
                        common_sampler_reasoning_budget_force(slot->smpl.get());
                        res->success = true;
                    } else {
                        res->success = false;
                        res->message = "unknown control action";
                    }

                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_NEXT_RESPONSE:
                {
                    // do nothing
                } break;
            case SERVER_TASK_TYPE_METRICS:
                {
                    json slots_data = json::array();

                    int n_idle_slots       = 0;
                    int n_processing_slots = 0;

                    for (server_slot & slot : slots) {
                        json slot_data = slot.to_json(slots_debug == 0);

                        if (slot.is_processing()) {
                            n_processing_slots++;
                        } else {
                            n_idle_slots++;
                        }

                        slots_data.push_back(slot_data);
                    }
                    SRV_DBG("n_idle_slots = %d, n_processing_slots = %d\n", n_idle_slots, n_processing_slots);

                    auto res = std::make_unique<server_task_result_metrics>();
                    res->id                  = task.id;
                    res->slots_data          = std::move(slots_data);
                    res->n_idle_slots        = n_idle_slots;
                    res->n_processing_slots  = n_processing_slots;
                    res->n_tasks_deferred    = queue_tasks.queue_tasks_deferred_size();
                    res->t_start             = metrics.t_start;

                    res->n_prompt_tokens_processed_total = metrics.n_prompt_tokens_processed_total;
                    res->t_prompt_processing_total       = metrics.t_prompt_processing_total;
                    res->n_tokens_predicted_total        = metrics.n_tokens_predicted_total;
                    res->t_tokens_generation_total       = metrics.t_tokens_generation_total;

                    res->n_tokens_max = metrics.n_tokens_max;

                    res->n_prompt_tokens_processed = metrics.n_prompt_tokens_processed;
                    res->t_prompt_processing       = metrics.t_prompt_processing;
                    res->n_tokens_predicted        = metrics.n_tokens_predicted;
                    res->t_tokens_generation       = metrics.t_tokens_generation;

                    res->n_decode_total          = metrics.n_decode_total;
                    res->n_busy_slots_total      = metrics.n_busy_slots_total;

                    if (task.metrics_reset_bucket) {
                        metrics.reset_bucket();
                    }
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_SLOT_SAVE:
                {
                    if (!check_no_mtmd(task.id)) {
                        break;
                    }

                    const int id_slot = task.slot_action.id_slot;
                    server_slot * slot = get_slot_by_id(id_slot);
                    if (slot == nullptr) {
                        send_error(task, "Invalid slot ID", ERROR_TYPE_INVALID_REQUEST);
                        break;
                    }
                    if (slot->is_processing()) {
                        // if requested slot is unavailable, we defer this task for processing later
                        SRV_DBG("requested slot is unavailable, defer task, id_task = %d\n", task.id);
                        queue_tasks.defer(std::move(task));
                        break;
                    }

                    const size_t token_count = slot->prompt.tokens.size();
                    const int64_t t_start = ggml_time_us();

                    std::string filename = task.slot_action.filename;
                    std::string filepath = task.slot_action.filepath;

                    const llama_tokens & tokens = slot->prompt.tokens.get_tokens();
                    const size_t nwrite = llama_state_seq_save_file(ctx_tgt, filepath.c_str(), slot->id, tokens.data(), token_count);

                    const int64_t t_end = ggml_time_us();
                    const double t_save_ms = (t_end - t_start) / 1000.0;

                    auto res = std::make_unique<server_task_result_slot_save_load>();
                    res->id       = task.id;
                    res->id_slot  = id_slot;
                    res->filename = filename;
                    res->is_save  = true;
                    res->n_tokens = token_count;
                    res->n_bytes  = nwrite;
                    res->t_ms     = t_save_ms;
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_SLOT_RESTORE:
                {
                    if (!check_no_mtmd(task.id)) break;
                    const int id_slot = task.slot_action.id_slot;
                    server_slot * slot = get_slot_by_id(id_slot);
                    if (slot == nullptr) {
                        send_error(task, "Invalid slot ID", ERROR_TYPE_INVALID_REQUEST);
                        break;
                    }
                    if (slot->is_processing()) {
                        // if requested slot is unavailable, we defer this task for processing later
                        SRV_DBG("requested slot is unavailable, defer task, id_task = %d\n", task.id);
                        queue_tasks.defer(std::move(task));
                        break;
                    }

                    const int64_t t_start = ggml_time_us();

                    std::string filename = task.slot_action.filename;
                    std::string filepath = task.slot_action.filepath;

                    llama_tokens tokens;
                    tokens.resize(slot->n_ctx);
                    size_t token_count = 0;
                    size_t nread = llama_state_seq_load_file(ctx_tgt, filepath.c_str(), slot->id, tokens.data(), tokens.size(), &token_count);
                    if (nread == 0) {
                        slot->prompt.tokens.clear(); // KV may already been invalidated?
                        send_error(task, "Unable to restore slot, no available space in KV cache or invalid slot save file", ERROR_TYPE_INVALID_REQUEST);
                        break;
                    }
                    tokens.resize(token_count);
                    slot->prompt.tokens.clear();
                    slot->prompt.tokens.insert(tokens);

                    const int64_t t_end = ggml_time_us();
                    const double t_restore_ms = (t_end - t_start) / 1000.0;

                    auto res = std::make_unique<server_task_result_slot_save_load>();
                    res->id       = task.id;
                    res->id_slot  = id_slot;
                    res->filename = filename;
                    res->is_save  = false;
                    res->n_tokens = token_count;
                    res->n_bytes  = nread;
                    res->t_ms     = t_restore_ms;
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_SLOT_COMPACT:
                {
                    // [obrain LONG-RESUME] phantom-prefix compaction: free the attention
                    // KV cells of the prefix, keep the last tail_m cells + the recurrent
                    // state, and deliberately LEAVE slot->prompt.tokens intact so future
                    // requests still prompt-match the full history (no re-prefill; the
                    // model attends over tail + recurrent only). tail_m < 0 = info only.
                    if (!check_no_mtmd(task.id)) {
                        break;
                    }
                    const int id_slot = task.slot_action.id_slot;
                    server_slot * slot = get_slot_by_id(id_slot);
                    if (slot == nullptr) {
                        send_error(task, "Invalid slot ID", ERROR_TYPE_INVALID_REQUEST);
                        break;
                    }
                    if (slot->is_processing()) {
                        SRV_DBG("requested slot is unavailable, defer task, id_task = %d\n", task.id);
                        queue_tasks.defer(std::move(task));
                        break;
                    }

                    llama_memory_t mem = llama_get_memory(ctx_tgt);
                    // ATTENTION-only view: on hybrid models the combined pos_min/max
                    // collapse to the recurrent cell ([last,last]) and would both hide
                    // the real cell window and wrongly refuse the compaction.
                    const llama_pos pos_min_before = llama_memory_seq_pos_min_attn(mem, slot->id);
                    const llama_pos pos_max_before = llama_memory_seq_pos_max_attn(mem, slot->id);
                    const int64_t cells_before = pos_max_before >= 0 ? (int64_t) (pos_max_before - pos_min_before + 1) : 0;

                    const int32_t tail_m = task.slot_action.tail_m;
                    bool compacted = false;
                    if (tail_m >= 0 && pos_max_before >= 0) {
                        const llama_pos p1 = pos_max_before + 1 - tail_m;
                        if (p1 > pos_min_before) {
                            // PREFIX removal [0, p1): frees attention cells; positions are
                            // NOT rebased and the recurrent state survives (validated —
                            // obrain LONG-RESUME phase A). The bool MUST be checked: on
                            // hybrid models a refused partial removal is a silent no-op.
                            if (!llama_memory_seq_rm(mem, slot->id, 0, p1)) {
                                send_error(task, "seq_rm(prefix) refused by model memory (hybrid partial removal)", ERROR_TYPE_SERVER);
                                break;
                            }
                            compacted = true;
                        }
                    }

                    const llama_pos pos_min_after = llama_memory_seq_pos_min_attn(mem, slot->id);
                    const llama_pos pos_max_after = llama_memory_seq_pos_max_attn(mem, slot->id);

                    auto res = std::make_unique<server_task_result_slot_compact>();
                    res->id              = task.id;
                    res->compacted       = compacted;
                    res->tail_m          = tail_m;
                    res->n_ctx           = slot->n_ctx;
                    res->cells_before    = cells_before;
                    res->cells_after     = pos_max_after >= 0 ? (int64_t) (pos_max_after - pos_min_after + 1) : 0;
                    res->pos_min         = pos_min_after;
                    res->pos_max         = pos_max_after;
                    res->n_prompt_tokens = slot->prompt.tokens.size();
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_SLOT_ERASE:
                {
                    if (!check_no_mtmd(task.id)) {
                        break;
                    }
                    const int id_slot = task.slot_action.id_slot;
                    server_slot * slot = get_slot_by_id(id_slot);
                    if (slot == nullptr) {
                        send_error(task, "Invalid slot ID", ERROR_TYPE_INVALID_REQUEST);
                        break;
                    }
                    if (slot->is_processing()) {
                        // if requested slot is unavailable, we defer this task for processing later
                        SRV_DBG("requested slot is unavailable, defer task, id_task = %d\n", task.id);
                        queue_tasks.defer(std::move(task));
                        break;
                    }

                    // Erase token cache
                    const size_t n_erased = slot->prompt.tokens.size();

                    slot->prompt_clear(false);

                    auto res = std::make_unique<server_task_result_slot_erase>();
                    res->id       = task.id;
                    res->id_slot  = id_slot;
                    res->n_erased = n_erased;
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_GET_LORA:
                {
                    // TODO @ngxson : make lora_adapters a dedicated member of server_context
                    auto & loras = params_base.lora_adapters;
                    auto res = std::make_unique<server_task_result_get_lora>();
                    res->id = task.id;
                    for (size_t i = 0; i < loras.size(); ++i) {
                        auto & lora = loras[i];
                        std::string alora_invocation_string = "";
                        const uint64_t n_alora_tokens = llama_adapter_get_alora_n_invocation_tokens(lora.ptr);
                        llama_tokens alora_invocation_tokens;
                        if (n_alora_tokens) {
                            const llama_token * alora_tokens = llama_adapter_get_alora_invocation_tokens(lora.ptr);
                            for (uint64_t j = 0; j < n_alora_tokens; ++j) {
                                alora_invocation_string += common_token_to_piece(vocab, alora_tokens[j]);
                                alora_invocation_tokens.push_back(alora_tokens[j]);
                            }
                        }
                        res->loras.push_back(server_task_result_get_lora::lora{
                            lora,
                            alora_invocation_string,
                            alora_invocation_tokens,
                        });
                    }
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_SET_LORA:
                {
                    auto new_loras = construct_lora_list(task.set_lora);
                    // logging
                    for (size_t i = 0; i < new_loras.size(); ++i) {
                        SRV_TRC("set lora adapter idx=%zu scale=%f\n", i, new_loras[i].scale);
                    }
                    // TODO @ngxson : make lora_adapters a dedicated member of server_context
                    params_base.lora_adapters = new_loras;
                    auto res = std::make_unique<server_task_result_apply_lora>();
                    res->id = task.id;
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_SET_ATTN_MASK:
                {
                    // [obrain] upstream renamed the target context to ctx_tgt
                    // in the 2026-07 server refactor — alias it locally so
                    // the handler bodies below read unchanged.
                    llama_context * ctx = ctx_tgt;
                    if (task.attn_mask.empty()) {
                        llama_set_attn_mask(ctx, nullptr, nullptr, 0, 0, -1);
                        SRV_INF("%s", "custom attention mask cleared\n");
                    } else {
                        const int32_t n_pos = (int32_t) task.attn_mask_pos.size();
                        llama_set_attn_mask(ctx, task.attn_mask.data(), task.attn_mask_pos.data(), n_pos, task.attn_mask_n_head_groups, task.attn_mask_slot_id);
                        SRV_INF("custom attention mask set for %d positions\n", n_pos);
                    }
                    auto res = std::make_unique<server_task_result_set_attn_mask>();
                    res->id = task.id;
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_JSPACE_COMPARE_GENERATION:
                {
                    // [obrain] PO plan 4749fb72 — see jspace_reserved_slot_id()
                    // above for why this is safe to run directly on `ctx`
                    // without going through the normal slot/batching system:
                    // this seq_id is NEVER assigned to normal completions.
                    // (ctx/model/vocab aliased from upstream's ctx_tgt after
                    // the 2026-07 server refactor.)
                    llama_context * ctx = ctx_tgt;
                    const llama_model * model = llama_get_model(ctx);
                    const llama_vocab * vocab = llama_model_get_vocab(model);
                    const int32_t seq_id = jspace_reserved_slot_id();
                    if (seq_id < 0) {
                        auto res = std::make_unique<server_task_result_error>();
                        res->id = task.id;
                        res->err_type = ERROR_TYPE_NOT_SUPPORTED;
                        res->err_msg = "J-space live-testing requires the server to be started with "
                                       "--parallel 3 (or more) to reserve a dedicated slot; current "
                                       "--parallel is too low.";
                        queue_results.send(std::move(res));
                        break;
                    }
                    // [obrain P1] GARDE-FOU AVANT TOUT `seq_rm` (cf.
                    // reserver_sequence_pour_jspace) : cette sequence est
                    // reservee, elle n'appartient a AUCUN client -- et on le
                    // VERIFIE au lieu de le supposer, parce que c'est
                    // exactement la supposition qui etait fausse.
                    {
                        std::string pourquoi_kv;
                        if (!reserver_sequence_pour_jspace(pourquoi_kv)) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_SERVER;
                            res->err_msg = pourquoi_kv;
                            queue_results.send(std::move(res));
                            break;
                        }
                    }

                    const int32_t n_embd  = llama_model_n_embd(model);
                    const int32_t n_layer = llama_model_n_layer(model);
                    const int32_t last_layer = n_layer - 1;

                    // Gram matrix — model constant, computed once, cached.
                    if (!jspace_gram_ready) {
                        SRV_INF("%s", "[jspace] computing readout gram matrix (one-time, ~15-30s, service loop blocked)...\n");
                        jspace_gram_cache.assign((size_t) n_embd * n_embd, 0.0f);
                        const int32_t rc = llama_model_get_output_gram_matrix(model, jspace_gram_cache.data());
                        if (rc != n_embd) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_SERVER;
                            res->err_msg = "failed to compute readout gram matrix";
                            queue_results.send(std::move(res));
                            break;
                        }
                        jspace_gram_ready = true;
                        SRV_INF("%s", "[jspace] gram matrix ready\n");
                    }

                    std::vector<float> gamma((size_t) n_embd);
                    llama_model_get_output_norm_weight(model, gamma.data());
                    std::vector<float> gamma_gram = jspace_precompute_gamma_gram(jspace_gram_cache, gamma);

                    // Tokenize the prompt.
                    std::vector<llama_token> tokens = common_tokenize(ctx, task.jspace_prompt, true, true);
                    if (tokens.size() < 2) {
                        auto res = std::make_unique<server_task_result_error>();
                        res->id = task.id;
                        res->err_type = ERROR_TYPE_INVALID_REQUEST;
                        res->err_msg = "prompt must tokenize to at least 2 tokens";
                        queue_results.send(std::move(res));
                        break;
                    }
                    const int32_t n_tok = (int32_t) tokens.size();

                    // [obrain FIX-inject-dos] delegue au decodage fractionne
                    // (cf. jspace_decode_chunked) : la version mono-lot
                    // abortait le serveur au-dela de n_batch tokens.
                    auto decode_batch = [&](const std::vector<llama_token> & toks, int32_t pos_start, bool want_logits) -> int {
                        return jspace_decode_chunked(ctx, seq_id, toks, pos_start, want_logits);
                    };

                    // Natural decode: prefix, then last token WITH capture,
                    // to get the real h at the last layer.
                    llama_memory_seq_rm(llama_get_memory(ctx), seq_id, -1, -1);
                    const std::vector<llama_token> prefix(tokens.begin(), tokens.end() - 1);
                    if (!prefix.empty()) {
                        decode_batch(prefix, 0, false);
                    }
                    llama_set_layer_output_override(ctx, -1, nullptr, 0); // ensure clean state
                    const int32_t capture_layer = last_layer;
                    llama_set_layer_output_capture(ctx, &capture_layer, 1);
                    decode_batch({ tokens.back() }, n_tok - 1, true);
                    const float * h_ptr = llama_get_layer_output(ctx, last_layer, 0);
                    std::vector<float> h;
                    if (h_ptr) {
                        h.assign(h_ptr, h_ptr + n_embd);
                    }
                    llama_set_layer_output_capture(ctx, nullptr, 0);

                    if (h.empty()) {
                        auto res = std::make_unique<server_task_result_error>();
                        res->id = task.id;
                        res->err_type = ERROR_TYPE_SERVER;
                        res->err_msg = "failed to capture layer output for h";
                        queue_results.send(std::move(res));
                        break;
                    }

                    std::vector<float> g = jspace_g_out(h, gamma_gram, 1e-6f);
                    auto [top_eigenvalue, top_eigenvector] = jspace_top_eigenpair_psd(g, n_embd, 100, 42);
                    std::vector<float> random_dir = jspace_random_direction(n_embd, 777);

                    const float h_rms = jspace_rms_scale(h, 0.0f);
                    const float scale = task.jspace_scale * h_rms;

                    auto make_delta = [&](const std::vector<float> & dir) {
                        std::vector<float> delta(n_embd);
                        for (int32_t i = 0; i < n_embd; i++) delta[i] = dir[i] * scale;
                        return delta;
                    };
                    std::vector<float> delta_eigen  = make_delta(top_eigenvector);
                    std::vector<float> delta_random = make_delta(random_dir);

                    // Run one generation branch: decode prefix, decode last
                    // token (optionally overridden), then continue normally
                    // (override cleared) for n_generate tokens.
                    auto run_branch = [&](const float * delta) -> std::string {
                        llama_memory_seq_rm(llama_get_memory(ctx), seq_id, -1, -1);
                        if (!prefix.empty()) {
                            decode_batch(prefix, 0, false);
                        }
                        if (delta) {
                            std::vector<float> h_pert(n_embd);
                            for (int32_t i = 0; i < n_embd; i++) h_pert[i] = h[i] + delta[i];
                            llama_set_layer_output_override(ctx, last_layer, h_pert.data(), n_embd);
                        }
                        decode_batch({ tokens.back() }, n_tok - 1, true);
                        if (delta) {
                            llama_set_layer_output_override(ctx, -1, nullptr, 0);
                        }

                        // Greedy decoding only changes output when the
                        // ARGMAX token itself flips — too high a bar to
                        // reveal small-but-real distributional shifts
                        // (discovered live: identical text across all 3
                        // branches with greedy at scale=0.15). Matches the
                        // stochastic temp+dist sampler already validated in
                        // llm-engine's Rust experiment (PO note, real
                        // generation comparison) — fixed seed per branch
                        // for reproducibility.
                        llama_sampler * smpl = llama_sampler_chain_init(llama_sampler_chain_default_params());
                        llama_sampler_chain_add(smpl, llama_sampler_init_temp(0.7f));
                        llama_sampler_chain_add(smpl, llama_sampler_init_dist(42));
                        std::string text;
                        int32_t cur_pos = n_tok;
                        // fin de generation : tout token EOG (eos, eot, <|im_end|>...), pas seulement eos
                        for (int32_t t = 0; t < task.jspace_n_generate; t++) {
                            const llama_token tok = llama_sampler_sample(smpl, ctx, -1);
                            if (llama_vocab_is_eog(vocab, tok)) break;
                            text += common_token_to_piece(vocab, tok);
                            llama_sampler_accept(smpl, tok);
                            if (decode_batch({ tok }, cur_pos, true) != 0) break;
                            cur_pos++;
                        }
                        llama_sampler_free(smpl);
                        return text;
                    };

                    auto res = std::make_unique<server_task_result_jspace_compare>();
                    res->id             = task.id;
                    res->h_rms          = h_rms;
                    res->top_eigenvalue = top_eigenvalue;
                    res->text_baseline  = run_branch(nullptr);
                    res->text_eigen     = run_branch(delta_eigen.data());
                    res->text_random    = run_branch(delta_random.data());

                    llama_memory_seq_rm(llama_get_memory(ctx), seq_id, -1, -1);
                    queue_results.send(std::move(res));
                } break;
            case SERVER_TASK_TYPE_JSPACE_INJECT_KNOWLEDGE:
                {
                    // [obrain] PO plan 4749fb72, tâche P5-T5 — production
                    // entry point for the KV-splice mechanism validated
                    // end-to-end in llm-engine this session (notes
                    // 4392e92f/6abdcc46/2b554afe). Runs on the same
                    // reserved seq_id as JSPACE_COMPARE_GENERATION — never
                    // touched by normal completion traffic.
                    //
                    // Deliberate scope limits, established empirically and
                    // enforced by design here (see note 4392e92f):
                    //  - ONE fact per call. Concatenating multiple facts on
                    //    the same scratch sequence before extraction was
                    //    tested and causes real interference (drift to an
                    //    unrelated third theme, then incoherence) — no
                    //    multi-fact support here, by design.
                    //  - No attempt to override a well-established/contested
                    //    fact already known to the model — four distinct
                    //    continuous/reactive re-injection designs were
                    //    tried and all fail differently (repetition loops,
                    //    reasoning-restart loops). This endpoint is for
                    //    NOVEL/complementary knowledge only.
                    // (ctx/model/vocab aliased from upstream's ctx_tgt after
                    // the 2026-07 server refactor.)
                    llama_context * ctx = ctx_tgt;
                    const llama_model * model = llama_get_model(ctx);
                    const llama_vocab * vocab = llama_model_get_vocab(model);
                    const int32_t seq_id = jspace_reserved_slot_id();
                    if (seq_id < 0) {
                        auto res = std::make_unique<server_task_result_error>();
                        res->id = task.id;
                        res->err_type = ERROR_TYPE_NOT_SUPPORTED;
                        res->err_msg = "J-space live-testing requires the server to be started with "
                                       "--parallel 3 (or more) to reserve a dedicated slot; current "
                                       "--parallel is too low.";
                        queue_results.send(std::move(res));
                        break;
                    }
                    // [obrain P1] GARDE-FOU AVANT TOUT `seq_rm` (cf.
                    // reserver_sequence_pour_jspace) : cette sequence est
                    // reservee, elle n'appartient a AUCUN client -- et on le
                    // VERIFIE au lieu de le supposer, parce que c'est
                    // exactement la supposition qui etait fausse.
                    {
                        std::string pourquoi_kv;
                        if (!reserver_sequence_pour_jspace(pourquoi_kv)) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_SERVER;
                            res->err_msg = pourquoi_kv;
                            queue_results.send(std::move(res));
                            break;
                        }
                    }

                    // [obrain K1-IDS] IDs NATIFS fournis par l'appelant : utilises
                    // TELS QUELS, sans seconde tokenisation. `fact_text` est alors
                    // ignore -- une entree textuelle concurrente est REFUSEE.
                    std::vector<llama_token> fact_tokens;
                    if (!task.jspace_fact_token_ids.empty()) {
                        if (!task.jspace_fact_text.empty()) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_INVALID_REQUEST;
                            res->err_msg = "fact_token_ids and fact_text are mutually exclusive";
                            queue_results.send(std::move(res));
                            break;
                        }
                        fact_tokens = task.jspace_fact_token_ids;
                    } else {
                        fact_tokens = common_tokenize(ctx, task.jspace_fact_text, true, true);
                    }
                    std::vector<llama_token> query_tokens = common_tokenize(ctx, task.jspace_prompt, false, true);
                    const bool kv_by_ref  = !task.jspace_state_ref.empty();
                    const bool kv_extract = task.jspace_extract_only;
                    // [obrain APPEND-K3] ajout d'une page : ni question, ni generation.
                    const bool kv_append  = task.jspace_state_append;
                    // [obrain APPEND-K4] AVEC DES PREREQUIS, IL N'Y A AUCUN FAIT A
                    // DECODER : la source du prefixe est l'ensemble des pages
                    // ajoutees. Exiger `fact_text` ici rendait impossible toute
                    // question posee sur un contexte assemble -- c'est-a-dire
                    // l'usage meme de l'assemblage.
                    if (!kv_by_ref && fact_tokens.empty() && task.jspace_prerequis_refs.empty()) {
                        auto res = std::make_unique<server_task_result_error>();
                        res->id = task.id;
                        res->err_type = ERROR_TYPE_INVALID_REQUEST;
                        res->err_msg = "'fact_text' must tokenize to at least 1 token (or pass 'state_ref', or 'prerequis_refs')";
                        queue_results.send(std::move(res));
                        break;
                    }
                    if (!kv_extract && !kv_append && query_tokens.empty()) {
                        auto res = std::make_unique<server_task_result_error>();
                        res->id = task.id;
                        res->err_type = ERROR_TYPE_INVALID_REQUEST;
                        res->err_msg = "'prompt' must tokenize to at least 1 token";
                        queue_results.send(std::move(res));
                        break;
                    }
                    int32_t n_fact = (int32_t) fact_tokens.size();
                    // [obrain K1-IDS] compteurs de la requete courante
                    int32_t cur_prefix_decode_calls = 0;
                    int32_t cur_prefix_token_count  = 0;
                    // [obrain O1] collecte des IDs et positions reels
                    std::vector<llama_token> cur_decoded_ids;
                    std::vector<llama_pos>   cur_decoded_pos;
                    int32_t cur_cont_decode_calls = 0;
                    int32_t cur_cont_token_count  = 0;
                    std::vector<float>       cur_first_logits;
                    int32_t cur_emitted_count = 0;
                    std::string cur_stop = "MAX_NEW_TOKENS";
                    float ms_model_sha = -2.0f, ms_blob_sha = -2.0f;
                    float ms_lookup = -2.0f, ms_import = -2.0f, ms_compat = -2.0f;
                    bool    cur_state_imported      = false;
                    float   cur_ms_prefix_compute = -1.0f;   // -1 = NON MESURE
                    float   cur_ms_state_lookup   = -1.0f;
                    float   cur_ms_integrity      = -1.0f;
                    float   cur_ms_state_import   = -1.0f;
                    float   cur_ms_preparation    = -1.0f; // K2 : surcharge via etat injecte
                    const int32_t n_query = (int32_t) query_tokens.size();

                    // [obrain FIX-inject-dos] Garde-fou complementaire au
                    // decodage fractionne : le fractionnement supprime la mort
                    // du serveur sur GGML_ASSERT(n_tokens_all <= n_batch), mais
                    // une requete peut toujours demander plus de contexte que la
                    // sequence n'en a. On refuse ALORS proprement en HTTP 400
                    // plutot que de laisser la demande echouer profond dans le
                    // decode (ou, pire, saturer le KV de la seq reservee).
                    // Note : en K2 (injection par ref) n_fact est encore inconnu
                    // ici, il vient du blob ; ce cas est couvert plus bas.
                    {
                        const int32_t n_ctx_seq  = (int32_t) llama_n_ctx_seq(ctx);
                        const int32_t n_gen      = task.jspace_n_generate > 0 ? task.jspace_n_generate : 24;
                        const int32_t n_required = n_fact + n_query + n_gen;
                        if (!kv_by_ref && n_required > n_ctx_seq) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_INVALID_REQUEST;
                            res->err_msg = "requete trop longue pour la sequence J-space : fact_text = " +
                                           std::to_string(n_fact) + " tokens + prompt = " +
                                           std::to_string(n_query) + " tokens + n_generate = " +
                                           std::to_string(n_gen) + " => " + std::to_string(n_required) +
                                           " tokens requis, mais le contexte par sequence n'en offre que " +
                                           std::to_string(n_ctx_seq) +
                                           " (augmenter --ctx-size, ou raccourcir fact_text)";
                            queue_results.send(std::move(res));
                            break;
                        }
                    }

                    // [obrain SPEC-inject] --spec-inject-consume : consommateur
                    // speculatif de la boucle inject. Constat M1 (feat/j2-j3-couplage,
                    // 6e72f5d) : spec_seed_text alimente le map n-gram partage mais
                    // cette boucle de generation n avait AUCUN consommateur spec —
                    // la restitution d un fait injecte n etait jamais draftee.
                    // Ici : seed-first du fait dans le map partage (extract ET inject),
                    // puis draft/verify via le proposer actif sur la seq reservee.
                    // spec est initialise meme avec types={NONE} (impls vides) :
                    // n_max > 0 est le vrai temoin d un proposer actif.
                    const bool spec_consume = params_base.speculative.inject_consume &&
                                              spec != nullptr &&
                                              common_speculative_n_max(&params_base.speculative) > 0;

                    // [obrain FIX-inject-dos] delegue au decodage fractionne
                    // (cf. jspace_decode_chunked) : la version mono-lot
                    // abortait le serveur au-dela de n_batch tokens.
                    auto decode_batch = [&](const std::vector<llama_token> & toks, int32_t pos_start, bool want_logits) -> int {
                        return jspace_decode_chunked(ctx, seq_id, toks, pos_start, want_logits);
                    };

                    // ═══════════════════════════════════════════════════════════
                    // [obrain APPEND-K3] ASSEMBLAGE D'UN CONTEXTE PAR PAGES.
                    //
                    // POURQUOI CETTE BRANCHE EXISTE. Jusqu'ici, importer un etat
                    // dans la sequence J-space commencait par la PURGER (le
                    // `llama_memory_seq_rm` juste en dessous) : chaque import
                    // REMPLACAIT la sequence. Un contexte de 1M jetons ne pouvait
                    // donc se recharger qu'en recopiant tout le prefixe a chaque
                    // page ajoutee, alors que seule la derniere page a change.
                    //
                    // CE QUE FAIT K3 : elle AJOUTE la page (drapeau
                    // LLAMA_STATE_SEQ_FLAGS_APPEND), SANS purge, et s'arrete la :
                    // aucun decodage de question, aucune generation. La sequence
                    // reservee devient le contexte assemble, page apres page.
                    //
                    // OU EST EXIGEE LA CONTIGUITE : dans le MOTEUR, pas ici. Cette
                    // branche ne fait que verifier la meme condition AVANT l'appel,
                    // pour rendre un HTTP 400 lisible au lieu d'un refus profond --
                    // le moteur reste seul juge (llama_kv_cache::state_read_meta),
                    // il n'existe pas une seconde implementation de la regle.
                    // ═══════════════════════════════════════════════════════════

                    // Charge un etat (cache memoire puis fichier), verifie son
                    // manifeste, puis l'AJOUTE a la sequence reservee.
                    // Rend true si l'ajout a eu lieu ; sinon `pourquoi` porte une
                    // raison deja formulee pour un client HTTP.
                    auto ajouter_page = [&](const std::string & ref,
                                            int64_t pos_min_annonce,
                                            int64_t pos_max_annonce,
                                            std::string & pourquoi,
                                            float & ms_import_out,
                                            size_t & octets_out,
                                            int64_t & recr_from_out) -> bool {
                        kv_state_blob blob;
                        kv_states_get(ref, blob);
                        if (blob.data.empty()) {
                            // Fallback disque, meme format et meme controle
                            // d'integrite que la branche K2.
                            const std::string chemin = task.jspace_kv_dir + "/" + ref + ".bin";
                            std::ifstream f(chemin, std::ios::binary);
                            if (!f) {
                                pourquoi = "unknown state_ref '" + ref + "' (extract d'abord)";
                                return false;
                            }
                            f.close();
                            const obrain_kv::DiskLoad DL = obrain_kv::load_state_file(chemin);
                            if (!DL.ok) {
                                pourquoi = "disk state unusable: " + DL.why;
                                return false;
                            }
                            if (kv_ref_of(DL.blob.data) != ref) {
                                pourquoi = "state integrity mismatch on disk load (refusal)";
                                return false;
                            }
                            blob = DL.blob;
                            kv_states_insert(ref, kv_state_blob{blob.n_tokens, blob.data, blob.manifest});
                        }
                        if (!blob.manifest) {
                            pourquoi = "state_ref '" + ref + "' : state has NO compatibility manifest (refusal before import)";
                            return false;
                        }
                        if (blob.data.size() != blob.manifest->blob_bytes) {
                            pourquoi = "state_ref '" + ref + "' : state size mismatch vs manifest (refusal before import)";
                            return false;
                        }
                        if (kv_sha256_hex(blob.data.data(), blob.data.size()) != blob.manifest->blob_sha256) {
                            pourquoi = "state_ref '" + ref + "' : state SHA256 MISMATCH (refusal before import)";
                            return false;
                        }
                        {
                            const kv_state_manifest cur = kv_manifest_of(ctx, params_base.model.path, seq_id);
                            kv_state_manifest cur_seq = cur;
                            cur_seq.seq_id = blob.manifest->seq_id;
                            std::string why;
                            const obrain_kv::ImportDecision dec = obrain_kv::decide_import(blob, cur_seq, why);
                            if (dec != obrain_kv::ImportDecision::ACCEPT) {
                                pourquoi = "state_ref '" + ref + "' REFUSE avant import (" +
                                           obrain_kv::decision_name(dec) + ") : " + why;
                                return false;
                            }
                        }
                        // LA PLAGE ANNONCEE DOIT ETRE CELLE DU MANIFESTE. La verifier
                        // ici evite de croire qu'on ajoute [N, M) alors que le blob
                        // porte autre chose : la position est la seule identite de la
                        // page, et un ecart produirait un contexte decale.
                        if (pos_min_annonce >= 0 && (blob.manifest->pos_min != pos_min_annonce ||
                                                     blob.manifest->pos_max != pos_max_annonce)) {
                            pourquoi = "state_ref '" + ref + "' : plage annoncee [" +
                                       std::to_string(pos_min_annonce) + ", " + std::to_string(pos_max_annonce) +
                                       "] != plage du manifeste [" + std::to_string(blob.manifest->pos_min) + ", " +
                                       std::to_string(blob.manifest->pos_max) + "] (refus : un contexte decale est pire qu'un refus)";
                            return false;
                        }
                        // CONTIGUITE avec la sequence courante : la MEME condition que
                        // le moteur, verifiee AVANT pour rendre un 400 lisible.
                        llama_memory_t mem_cur = llama_get_memory(ctx);
                        const llama_pos pmax_cur = llama_memory_seq_pos_max_attn(mem_cur, seq_id);
                        const llama_pos attendu  = pmax_cur < 0 ? 0 : pmax_cur + 1;
                        if (blob.manifest->pos_min != (int64_t) attendu) {
                            pourquoi = "state_ref '" + ref + "' : la page commence a " +
                                       std::to_string(blob.manifest->pos_min) + " alors que la sequence attend " +
                                       std::to_string(attendu) + " (pos_max = " + std::to_string(pmax_cur) +
                                       "). Un AJOUT n'accepte que le delta qui prolonge exactement la sequence.";
                            return false;
                        }
                        // ── [obrain APPEND] GARDE-FOU RECURRENT ────────────────────
                        // POURQUOI CE REFUS EXISTE. Sur un modele hybride (couches a
                        // attention lineaire), l'etat recurrent est un PLI de tout ce
                        // que la sequence source a vu. Une page dont le pli ne couvre
                        // pas depuis 0, ajoutee a une sequence qui porte deja [0, k),
                        // installerait un resume QUI IGNORE ces pages : le modele
                        // repondrait faux, sans erreur, sans log, et de facon
                        // reproductible. Un refus explicite vaut mieux qu'une reponse
                        // fausse silencieuse.
                        const int64_t cible_min = (pmax_cur < 0)
                                                    ? blob.manifest->pos_min
                                                    : (int64_t) llama_memory_seq_pos_min_attn(mem_cur, seq_id);
                        if (blob.manifest->recr_from > cible_min) {
                            pourquoi = "state_ref '" + ref + "' : l'etat recurrent de cette page ne couvre qu'a partir de la position " +
                                       std::to_string(blob.manifest->recr_from) + ", alors que la sequence assemblee commence a " +
                                       std::to_string(cible_min) + ". L'ajouter produirait un etat recurrent QUI IGNORE les pages deja en place "
                                       "(reponses fausses, sans message). Re-extraire la page depuis une session qui portait deja les pages "
                                       "precedentes (champ `prerequis_refs` de l'extraction).";
                            return false;
                        }
                        const int64_t _timp = ggml_time_us();
                        const size_t rc = llama_state_seq_set_data_ext(ctx, blob.data.data(), blob.data.size(), seq_id,
                                                                       LLAMA_STATE_SEQ_FLAGS_APPEND);
                        ms_import_out = (float) (ggml_time_us() - _timp) / 1000.0f;
                        if (rc == 0) {
                            pourquoi = "state_ref '" + ref + "' : l'AJOUT a ete REFUSE par le moteur "
                                       "(positions non contigues, page trouee, ou cellules insuffisantes) -- la sequence est restee intacte";
                            return false;
                        }
                        octets_out    = blob.data.size();
                        recr_from_out = blob.manifest->recr_from;
                        return true;
                    };

                    if (kv_append) {
                        if (task.jspace_state_reset) {
                            // Remise a zero EXPLICITE (voir jspace_state_reset) :
                            // l'accumulation de pages est le but de K3, donc purger
                            // sans qu'on le demande casserait la fonction meme de
                            // cette branche. C'est le seul moyen de repartir d'un
                            // contexte neuf sans passer par une extraction.
                            llama_memory_seq_rm(llama_get_memory(ctx), seq_id, -1, -1);
                            // (SRV_INF exige au moins un argument apres le format)
                            SRV_INF("%s", "[kv-append] sequence reservee remise a zero (state_reset)\n");
                        }
                        std::string pourquoi;
                        float   ms_imp = -1.0f;
                        size_t  octets = 0;
                        int64_t recr   = -1;
                        if (!ajouter_page(task.jspace_state_ref, task.jspace_pos_min, task.jspace_pos_max,
                                          pourquoi, ms_imp, octets, recr)) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_INVALID_REQUEST;
                            res->err_msg = pourquoi;
                            queue_results.send(std::move(res));
                            break;
                        }
                        llama_memory_t mem_apres = llama_get_memory(ctx);
                        auto res = std::make_unique<server_task_result_jspace_inject>();
                        res->id              = task.id;
                        res->text            = "kv_append:" + task.jspace_state_ref;
                        res->state_append    = true;
                        res->state_imported  = true;
                        res->page_pos_min    = task.jspace_pos_min;
                        res->page_pos_max    = task.jspace_pos_max;
                        res->seq_pos_min     = llama_memory_seq_pos_min(mem_apres, seq_id);
                        res->seq_pos_max     = llama_memory_seq_pos_max(mem_apres, seq_id);
                        res->attn_pos_min    = llama_memory_seq_pos_min_attn(mem_apres, seq_id);
                        res->attn_pos_max    = llama_memory_seq_pos_max_attn(mem_apres, seq_id);
                        res->seq_cells       = res->seq_pos_max >= 0
                                                 ? (uint64_t) (res->seq_pos_max - res->seq_pos_min + 1)
                                                 : 0;
                        res->import_bytes    = octets;
                        res->blob_bytes      = octets;
                        res->fact_tokens     = (int32_t) (task.jspace_pos_max - task.jspace_pos_min + 1);
                        res->recr_from       = (int32_t) recr;
                        res->ms_state_import = ms_imp;
                        res->prefix_decode_calls = 0;   // RIEN n'a ete decode : c'est un AJOUT
                        res->prefix_token_count  = 0;
                        SRV_INF("[kv-append] ref=%s page=[%lld, %lld] octets=%zu import_ms=%.2f seq=[%lld, %lld] attn=[%lld, %lld] recr_from=%lld\n",
                                task.jspace_state_ref.c_str(),
                                (long long) task.jspace_pos_min, (long long) task.jspace_pos_max,
                                octets, (double) ms_imp,
                                (long long) res->seq_pos_min, (long long) res->seq_pos_max,
                                (long long) res->attn_pos_min, (long long) res->attn_pos_max,
                                (long long) recr);
                        queue_results.send(std::move(res));
                        break;
                    }

                    // (A) Source de l'état du fait : décodage du texte (K1 extract /
                    // chemin legacy) OU chargement d'un état persisté (K2 inject par
                    // ref — BYPASS TOTAL du texte : zéro prefill du fait).
                    //
                    // [obrain APPEND] SI DES PREREQUIS SONT FOURNIS, LA PURGE EST
                    // SAUTEE : les pages annoncees sont AJOUTEES a la sequence
                    // reservee, puis la nouvelle page est decodee a la suite. C'est ce
                    // qui rend l'etat recurrent du nouveau pli COMPLET (il couvre
                    // [0, pos_max] et non seulement la page), donc ce qui rend la page
                    // reassemblable ailleurs sans mentir.
                    // [obrain APPEND] PREREQUIS : la sequence est PURGEE PUIS les
                    // pages annoncees sont AJOUTEES, dans l'ordre, avant que la
                    // nouvelle page ne soit decodee a la suite. C'est ce qui rend
                    // l'etat recurrent du nouveau pli COMPLET (il couvre
                    // [0, pos_max] et non seulement la page), donc ce qui rend la
                    // page reassemblable ailleurs sans mentir.
                    //
                    // POURQUOI LA PURGE A LIEU MEME AVEC DES PREREQUIS -- BUG
                    // CORRIGE ICI, MESURE LE 2026-10-05 : la premiere version
                    // sautait la purge des qu'il y avait des prerequis, en
                    // confondant deux choses differentes -- « ne pas purger entre
                    // les ajouts » (vrai) et « ne pas purger du tout » (faux). Or
                    // la sequence reservee porte ENCORE les jetons du prefixe de la
                    // requete PRECEDENTE (l'extraction les y a decodes). Sans purge,
                    // le premier prerequis etait donc refuse, a juste titre, comme
                    // non contigu : « la page commence a 0 alors que la sequence
                    // attend 232 (pos_max = 231) ». L'assemblage doit partir d'une
                    // sequence PROPRE ; seule la sequence de l'ajout incremental
                    // (branche K3, sans prerequis) a le droit de s'accumuler.
                    llama_memory_seq_rm(llama_get_memory(ctx), seq_id, -1, -1);
                    if (!task.jspace_prerequis_refs.empty()) {
                        // POURQUOI UN DRAPEAU ET PAS UN `break` DANS LA BOUCLE : un
                        // `break` sortirait de la boucle `for` (et non du `switch`),
                        // donc la suite de la branche s'executerait sur une sequence
                        // PARTIELLEMENT assemblee -- exactement le genre d'etat
                        // intermediaire silencieux qu'on refuse ailleurs.
                        bool prerequis_ok = true;
                        for (const auto & ref_pre : task.jspace_prerequis_refs) {
                            std::string pourquoi_pre;
                            float   ms_pre     = -1.0f;
                            size_t  octets_pre = 0;
                            int64_t recr_pre   = -1;
                            if (!ajouter_page(ref_pre, -1, -1, pourquoi_pre, ms_pre, octets_pre, recr_pre)) {
                                auto res = std::make_unique<server_task_result_error>();
                                res->id = task.id;
                                res->err_type = ERROR_TYPE_INVALID_REQUEST;
                                res->err_msg = "prerequis_refs : " + pourquoi_pre;
                                queue_results.send(std::move(res));
                                prerequis_ok = false;
                                break;
                            }
                            SRV_INF("[kv-extract] prerequis %s ajoute : %zu octets (%.2f ms), recr_from=%lld\n",
                                    ref_pre.c_str(), octets_pre, (double) ms_pre, (long long) recr_pre);
                        }
                        if (!prerequis_ok) {
                            break;
                        }
                    }
                    const int64_t t_fact_decode_start = ggml_time_us();
                    // [obrain APPEND] CONTEXTE ASSEMBLE (K4) : prerequis SEULS.
                    //
                    // POURQUOI CE MODE EXISTE. Assembler un contexte par ajouts
                    // (K3) ne sert a rien si l'on ne peut pas ensuite POSER UNE
                    // QUESTION sur ce contexte. Or le chemin d'extraction exige un
                    // « fait » a decoder (fact_text / fact_token_ids) : avec des
                    // prerequis seuls, il n'y a rien a decoder -- le fait EST
                    // l'ensemble des pages assemblees. On saute donc le decodage du
                    // fait et l'extraction, on fixe l'offset du prefixe a la longueur
                    // assemblee, et on decode la question a la suite.
                    const bool kv_prereq_only = !task.jspace_prerequis_refs.empty() &&
                                                task.jspace_fact_token_ids.empty() &&
                                                task.jspace_fact_text.empty() &&
                                                !kv_by_ref;

                    if (kv_prereq_only) {
                        const llama_pos pmax_attn = llama_memory_seq_pos_max_attn(llama_get_memory(ctx), seq_id);
                        n_fact = pmax_attn < 0 ? 0 : (int32_t) pmax_attn + 1;
                        cur_prefix_decode_calls = 0;   // le prefixe n'est PAS rejoue
                        cur_prefix_token_count  = 0;
                        cur_state_imported      = true;
                        SRV_INF("[kv-append] contexte assemble : %d positions (prerequis=%zu), question decodee a partir de %d\n",
                                n_fact, task.jspace_prerequis_refs.size(), n_fact);
                    } else if (!kv_by_ref) {
                        cur_prefix_decode_calls = 1;
                        cur_prefix_token_count  = (int32_t) fact_tokens.size();
                        // [obrain APPEND] position de depart EXPLICITE : avec
                        // pos_start = N, le fait est decode en [N, N + n_fact), donc
                        // l'etat extrait est la PAGE [N, N + n_fact) et non le
                        // prefixe [0, n_fact). Defaut 0 : comportement inchange.
                        if (decode_batch(fact_tokens, (int32_t) task.jspace_pos_start, false) != 0) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_SERVER;
                            res->err_msg = "failed to decode fact_text";
                            queue_results.send(std::move(res));
                            break;
                        }
                        if (kv_extract) {
                            // K1 : extraire l'état complet (KV+recurrent) du fait.
                            // [obrain APPEND] BORNE PAR POSITIONS quand une page est
                            // demandee (pos_start > 0) ou quand des prerequis ont ete
                            // ajoutes : sans cette borne, l'extraction rendrait AUSSI
                            // les pages des prerequis, et la « page » stockee porterait
                            // tout le prefixe -- soit exactement le cout qu'on elimine.
                            // Sans prerequis et pos_start = 0, le chemin historique est
                            // conserve tel quel (octets identiques).
                            const llama_pos page_p0 = (llama_pos) task.jspace_pos_start;
                            const llama_pos page_p1 = (llama_pos) (task.jspace_pos_start + (int64_t) n_fact - 1);
                            const bool borne = task.jspace_pos_start != 0 || !task.jspace_prerequis_refs.empty();
                            const size_t blob_size = borne
                                ? llama_state_seq_get_size_ext_range(ctx, seq_id, page_p0, page_p1, 0)
                                : llama_state_seq_get_size(ctx, seq_id);
                            std::vector<uint8_t> data(blob_size);
                            const size_t ecrits = borne
                                ? llama_state_seq_get_data_ext_range(ctx, data.data(), blob_size, seq_id, page_p0, page_p1, 0)
                                : llama_state_seq_get_data(ctx, data.data(), blob_size, seq_id);
                            if (blob_size == 0 || ecrits != blob_size) {
                                auto res = std::make_unique<server_task_result_error>();
                                res->id = task.id;
                                res->err_type = ERROR_TYPE_SERVER;
                                res->err_msg = "failed to extract fact state";
                                queue_results.send(std::move(res));
                                break;
                            }
                            // ── [obrain SPLICE-BOUND] L'EXTRACTION EST-ELLE CELLE DU FAIT ? ──
                            //
                            // CE QUI ETAIT CROYE : `llama_state_seq_get_size(ctx, seq_id)`
                            // rendrait « l'etat du SLOT entier » (158 Mo pour un fait de
                            // 24 jetons), et il faudrait donc ne garder que les n_fact
                            // premiers jetons. CE QUI EST MESURE (2026-10-05, blob reel de
                            // 157 812 552 octets pour n_tokens=14) : l'ecriture ne retient
                            // QUE les cellules qui portent la sequence — `cell_count = 14`
                            // pour 14 jetons decodes, soit 917 888 octets de KV (16 couches,
                            // 65 536 octets par jeton). Les 156 894 336 octets restants sont
                            // l'etat RECURRENT des 48 couches a attention lineaire de ce
                            // modele hybride : une taille CONSTANTE, que le nombre de jetons
                            // ne change pas, et dont la restauration est ce qui rend le fait
                            // « su » par ces couches (elles n'ont pas de cache KV).
                            //
                            // Il n'y a donc RIEN a tronquer : couper le blob aux n_fact
                            // premiers jetons produirait un etat corrompu, et le splice
                            // restaurerait un etat partiel en silence. Ce qui manquait
                            // n'etait pas un bornage — c'etait la VERIFICATION du bornage,
                            // et le fait de pouvoir NOMMER les deux parts. Les deux sont
                            // faites ici, sur les octets qui viennent d'etre extraits :
                            // un blob illisible, ou qui porterait plus de cellules que de
                            // jetons decodes, est REFUSE au lieu d'entrer dans le cache
                            // et d'etre servi comme un etat valide.
                            const obrain_kv::BlobLayout lay = obrain_kv::blob_layout(data.data(), data.size());
                            {
                                std::string pourquoi;
                                if (!obrain_kv::extraction_bornee(lay, n_fact, pourquoi)) {
                                    auto res = std::make_unique<server_task_result_error>();
                                    res->id = task.id;
                                    res->err_type = ERROR_TYPE_SERVER;
                                    res->err_msg = "extraction d'etat REFUSEE : " + pourquoi;
                                    queue_results.send(std::move(res));
                                    break;
                                }
                            }
                            SRV_INF("[kv-extract] n_fact=%d cellules_kv=%u kv_bytes=%zu recurrent_bytes=%zu total=%zu modele=%s\n",
                                    n_fact, lay.attn_cells, lay.attn_bytes, lay.rec_bytes, (size_t) lay.attn_bytes + lay.rec_bytes,
                                    lay.rec_bytes > lay.attn_bytes ? "hybride (etat recurrent CONSTANT : la taille du blob ne suit PAS les jetons)"
                                                                   : "attention seule (la taille du blob suit les jetons)");
                            if (lay.rec_bytes > 0 && lay.rec_bytes > 16u * 1024u * 1024u) {
                                SRV_WRN("[kv-extract] %zu octets recurrents (%.1f Mo) pour %d jetons : c'est la taille INTRINSEQUE de l'etat recurrent, "
                                        "pas un exces de jetons. Le reduire demande OBRAIN_STATE_TYPE=f16 (%s) ou i8 AVANT l'extraction — "
                                        "le mode est ecrit dans le blob et dans le manifeste, donc un etat extrait dans un mode n'est "
                                        "restaure que dans le meme mode.\n",
                                        lay.rec_bytes, (double) lay.rec_bytes / 1048576.0, n_fact,
                                        "moitie");
                            }
                            const std::string ref = kv_ref_of(data);
                            // [obrain K1-MANIFEST] manifeste construit ICI, la ou les
                            // identites et la configuration sont effectives.
                            std::shared_ptr<kv_state_manifest> man = std::make_shared<kv_state_manifest>(
                                kv_manifest_of(ctx, params_base.model.path, seq_id));
                            man->format_magic = 0xaf143cd9u;   // io_magic du format de blob
                            man->blob_bytes   = data.size();
                            { const int64_t _t0 = ggml_time_us();
                              man->blob_sha256 = kv_sha256_hex(data.data(), data.size());
                              g_kv_ms_blob_sha.store((float)(ggml_time_us() - _t0) / 1000.0f); }
                            man->n_tokens     = (int32_t) n_fact;
                            man->prefix_ids   = std::vector<llama_token>(fact_tokens.begin(), fact_tokens.end());
                            // [obrain SPLICE-POS] positions ECRITES explicitement.
                            // Le fait vient d etre decode en [0, n_fact) (decode_batch(fact_tokens, 0, …)
                            // apres llama_memory_seq_rm) : relire llama_memory_seq_pos_min/max se trompe
                            // avec la tete MTP du 27B (pos_min=pos_max=13 pour 14 tokens), alors que la
                            // verite est connue par construction. On ne relit donc pas une API qui ment.
                            //
                            // [obrain APPEND] la meme regle vaut pour une page decalee :
                            // les positions sont celles DEMANDEES (pos_start + i), pas
                            // celles qu'une API relirait apres coup.
                            man->pos_min = (int64_t) task.jspace_pos_start;
                            man->pos_max = (int64_t) task.jspace_pos_start + (int64_t) n_fact - 1;
                            // [obrain APPEND] COUVERTURE DU PLI RECURRENT, ecrite ICI
                            // PAR CONSTRUCTION (elle ne se relit pas dans le blob) :
                            //  - sans prerequis, la sequence source a ete purgee puis
                            //    decodee a pos_start : le pli recurrent ne couvre donc
                            //    QUE [pos_start, pos_max). Le declarer tel quel est ce
                            //    qui empeche de l'ajouter a une sequence commencee a 0
                            //    (la route refusera, au lieu de produire des reponses
                            //    fausses en silence) ;
                            //  - avec prerequis, les pages precedentes ont ete AJOUTEES
                            //    avant le decodage : le pli couvre depuis la premiere
                            //    position assemblee, qu'on lit sur la memoire (vue
                            //    ATTENTION : sur un modele hybride, la vue combinee
                            //    peut etre deformee par la tete MTP).
                            man->recr_from = task.jspace_prerequis_refs.empty()
                                ? (int64_t) task.jspace_pos_start
                                : (int64_t) llama_memory_seq_pos_min_attn(llama_get_memory(ctx), seq_id);
                            // ETAT DU CACHE AVANT CETTE INSERTION : prefixe `avant`
                            // pour que personne ne lise ces chiffres comme le
                            // resultat de l'extraction en cours (la ligne est
                            // emise avant que le blob soit mis en cache).
                            const kv_cache_etat_t ce = kv_cache_etat();
                            SRV_INF("[kv-extract] page=[%lld, %lld] pos_start=%lld prerequis=%zu recr_from=%lld borne=%d "
                                    "cache_avant_bytes=%zu cache_avant_entrees=%zu cache_budget=%zu cache_avant_evictions=%llu cache_remplacements=%llu cache_refus_trop_gros=%llu\n",
                                    (long long) man->pos_min, (long long) man->pos_max,
                                    (long long) task.jspace_pos_start, task.jspace_prerequis_refs.size(),
                                    (long long) man->recr_from, (int) borne,
                                    ce.octets, ce.entrees, ce.budget,
                                    (unsigned long long) ce.evictions,
                                    (unsigned long long) ce.remplace,
                                    (unsigned long long) ce.refuse_trop);
                            {
                                // Aucune entree de cache si l insertion est REFUSEE
                                // (blob vide, ou plus gros que la borne memoire).
                                kv_state_blob kb{(uint32_t) n_fact, data, man};
                                if (!kv_states_insert(ref, std::move(kb))) {
                                    auto res = std::make_unique<server_task_result_error>();
                                    res->id = task.id;
                                    res->err_type = ERROR_TYPE_SERVER;
                                    res->err_msg = "state too large for the bounded cache (not stored)";
                                    queue_results.send(std::move(res));
                                    break;
                                }
                                // [obrain P1] ETAT DU CACHE APRES INSERTION.
                                // POURQUOI CETTE LIGNE EXISTE : une borne qu'on
                                // ne peut pas lire ne se verifie pas. C'est ici
                                // qu'on lit l'EFFET de cette extraction --
                                // `cache_evictions` qui avance alors que
                                // `cache_bytes` stagne EST la preuve que le cache
                                // evince au lieu de croitre (la RAM ne croit pas
                                // de facon monotone).
                                const kv_cache_etat_t ce_apres = kv_cache_etat();
                                SRV_INF("[kv-cache] apres ref=%s : cache_bytes=%zu cache_entrees=%zu "
                                        "cache_budget=%zu cache_evictions=%llu\n",
                                        ref.c_str(), ce_apres.octets, ce_apres.entrees,
                                        ce_apres.budget, (unsigned long long) ce_apres.evictions);
                            }
                            // K1b : persistance disque (les états survivent au restart).
                            try {
                                std::filesystem::create_directories(task.jspace_kv_dir);
                                // [obrain FIX-DISK] Le format precedent (4 octets + blob brut)
                                // NE PORTATI PAS le manifeste. Comme l'import REFUSE un blob
                                // sans manifeste, un etat extrait etait IRRECUPERABLE apres un
                                // redemarrage : le chemin disque etait mort par construction.
                                // On ecrit desormais blob + manifeste COMPLET (magic "OBKV"),
                                // ce qui rend la reinjection possible SANS repasser par le texte.
                                obrain_kv::save_state_file(
                                    task.jspace_kv_dir + "/" + ref + ".bin",
                                    kv_state_blob{(uint32_t) n_fact, data, man});
                            } catch (const std::exception & e) {
                                SRV_WRN("%s", ("[kv] persist failed: " + std::string(e.what())).c_str());
                            }
                            // [obrain SPEC-inject] seed a l extraction : les injections
                            // par ref ulterieures (K2, sans texte) et les completions
                            // normales pourront drafter la restitution de ce fait.
                            if (spec_consume) {
                                common_speculative_seed(spec.get(), seq_id, fact_tokens);
                                SRV_INF("[spec-inject] map n-gram seede avec %d tokens du fait (extract)\n", n_fact);
                            }
                            auto res = std::make_unique<server_task_result_jspace_inject>();
                            res->id = task.id;
                            res->text = "kv_state:" + ref;
                            res->fact_tokens = n_fact;
                            // [obrain APPEND] la PAGE reellement extraite, publiee pour
                            // que l'appelant puisse l'AJOUTER ensuite sans deviner sa
                            // plage (l'ajout exige pos_min/pos_max exacts).
                            res->page_pos_min = man->pos_min;
                            res->page_pos_max = man->pos_max;
                            res->recr_from    = (int32_t) man->recr_from;
                    res->prefix_decode_calls = cur_prefix_decode_calls;
                    res->prefix_token_count  = cur_prefix_token_count;
                    res->state_imported      = cur_state_imported;
                            // [obrain K1-METRICS] METRIQUES FINALISEES AVANT LE RETOUR.
                            // Avant ce correctif, ce `break` sortait AVANT le calcul
                            // et `fact_decode_ms` restait a sa valeur par defaut
                            // 0.0 : un zero IMPLICITE, indistinguable d'une mesure
                            // reelle nulle.
                            {
                                const float total = (float)(ggml_time_us() - t_fact_decode_start) / 1000.0f;
                                res->fact_decode_ms        = total;
                                res->query_decode_ms       = -1.0f;   // NON APPLICABLE : pas de query
                                res->ms_prefix_compute     = total;
                                res->ms_state_lookup       = -1.0f;   // NON APPLICABLE : pas d'import
                                res->ms_model_sha256       = -2.0f;   // NON MESURE sur ce chemin
                                res->ms_blob_sha256        = g_kv_ms_blob_sha.load();
                                res->ms_state_import       = -1.0f;   // NON APPLICABLE
                                res->ms_preparation_total  = total;
                            }
                            res->blob_bytes  = blob_size;
                            // [obrain SPLICE-BOUND] la decomposition, telle que MESUREE
                            // sur les octets extraits (voir le garde-fou ci-dessus).
                            res->kv_bytes        = lay.attn_bytes;
                            res->recurrent_bytes = lay.rec_bytes;
                            res->n_attn_cells    = lay.attn_cells;
                            res->prefix_decode_calls = cur_prefix_decode_calls;
                            res->prefix_token_count  = cur_prefix_token_count;
                            res->state_imported      = cur_state_imported;
                            queue_results.send(std::move(res));
                            break;
                        }
                    } else {
                        // K2 : injecter l'état persisté (copie mémoire, pas de prefill).
                        if (!kv_ref_is_valid(task.jspace_state_ref)) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_INVALID_REQUEST;
                            res->err_msg = "invalid state_ref (16 hex minuscules attendus)";
                            queue_results.send(std::move(res));
                            break;
                        }
                        kv_state_blob blob;
                        // [obrain FIX-DEADLOCK] PAS de verrou externe : `kv_states_get`
                        // prend DEJA `g_kv_cache.mtx`. Or `g_kv_states_mtx` EST ce
                        // meme mutex (reference, ligne 72). Le prendre ici puis
                        // appeler le wrapper provoquait une REACQUISITION du meme
                        // `std::mutex` NON RECURSIF par le MEME thread :
                        // auto-interblocage, 44 threads en S / futex_do_wait.
                        { const int64_t _tl = ggml_time_us();
                          kv_states_get(task.jspace_state_ref, blob);
                          ms_lookup = (float)(ggml_time_us() - _tl) / 1000.0f; }
                        if (blob.data.empty()) {
                            // K1b : fallback disque (état extrait lors d'un run précédent).
                            std::ifstream f(task.jspace_kv_dir + "/" + task.jspace_state_ref + ".bin", std::ios::binary);
                            if (!f) {
                                auto res = std::make_unique<server_task_result_error>();
                                res->id = task.id;
                                res->err_type = ERROR_TYPE_INVALID_REQUEST;
                                res->err_msg = "unknown state_ref '" + task.jspace_state_ref + "' (extract d'abord)";
                                queue_results.send(std::move(res));
                                break;
                            }
                            f.close();
                            // [obrain FIX-DISK] lecture du format AVEC manifeste. Un ancien
                            // fichier au format nu (sans manifeste) est REFUSE explicitement
                            // plutot que charge sans controle -- aucun manifeste retroactif
                            // n'est fabrique.
                            const obrain_kv::DiskLoad DL =
                                obrain_kv::load_state_file(task.jspace_kv_dir + "/" + task.jspace_state_ref + ".bin");
                            if (!DL.ok) {
                                auto res = std::make_unique<server_task_result_error>();
                                res->id = task.id; res->err_type = ERROR_TYPE_SERVER;
                                res->err_msg = "disk state unusable: " + DL.why;
                                queue_results.send(std::move(res));
                                break;
                            }
                            // INTEGRITE : le nom de fichier est la cle ; on VERIFIE le contenu.
                            {
                                const std::string recomputed = kv_ref_of(DL.blob.data);
                                if (recomputed != task.jspace_state_ref) {
                                    auto res = std::make_unique<server_task_result_error>();
                                    res->id = task.id; res->err_type = ERROR_TYPE_SERVER;
                                    res->err_msg = "state integrity mismatch on disk load (refusal)";
                                    queue_results.send(std::move(res));
                                    break;
                                }
                            }
                            blob = DL.blob;
                            kv_states_insert(task.jspace_state_ref,
                                             kv_state_blob{blob.n_tokens, blob.data, blob.manifest});
                        }
                        // [obrain K1-MANIFEST] COMPATIBILITE VERIFIEE AVANT set_data.
                        // Toute incompatibilite, ou toute donnee necessaire inconnue,
                        // REFUSE l'import -- aucun set_data n'est tente.
                        {
                            const int64_t _tcmp = ggml_time_us();
                            const kv_state_manifest cur = kv_manifest_of(ctx, params_base.model.path, seq_id);
                            ms_compat = (float)(ggml_time_us() - _tcmp) / 1000.0f;
                            if (!blob.manifest) {
                                auto res = std::make_unique<server_task_result_error>();
                                res->id = task.id; res->err_type = ERROR_TYPE_SERVER;
                                res->err_msg = "state has NO compatibility manifest (refusal before import)";
                                queue_results.send(std::move(res)); break;
                            }
                            const std::string why = kv_manifest_mismatch(*blob.manifest, cur);
                            if (!why.empty()) {
                                auto res = std::make_unique<server_task_result_error>();
                                res->id = task.id; res->err_type = ERROR_TYPE_SERVER;
                                res->err_msg = "state INCOMPATIBLE : " + why + " (refusal before import)";
                                queue_results.send(std::move(res)); break;
                            }
                            // INTEGRITE cryptographique des octets stockes.
                            // [obrain R3] mesure SEPAREE du hachage du blob.
                            const int64_t _tblob = ggml_time_us();
                            const std::string sh = kv_sha256_hex(blob.data.data(), blob.data.size());
                            ms_blob_sha = (float)(ggml_time_us() - _tblob) / 1000.0f;
                            if (sh != blob.manifest->blob_sha256) {
                                auto res = std::make_unique<server_task_result_error>();
                                res->id = task.id; res->err_type = ERROR_TYPE_SERVER;
                                res->err_msg = "state SHA256 MISMATCH (refusal before import)";
                                queue_results.send(std::move(res)); break;
                            }
                            if (blob.data.size() != blob.manifest->blob_bytes) {
                                auto res = std::make_unique<server_task_result_error>();
                                res->id = task.id; res->err_type = ERROR_TYPE_SERVER;
                                res->err_msg = "state size mismatch vs manifest (refusal before import)";
                                queue_results.send(std::move(res)); break;
                            }
                        }
                        ms_model_sha = g_kv_ms_model_sha.load();
                        // [obrain R3] ms_state_import : mesure AUTOUR de l'appel reel.
                        const int64_t _timp = ggml_time_us();
                        const size_t rc = llama_state_seq_set_data(ctx, blob.data.data(), blob.data.size(), seq_id);
                        ms_import = (float)(ggml_time_us() - _timp) / 1000.0f;
                        if (rc == 0) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_SERVER;
                            res->err_msg = "failed to load state (set_data)";
                            queue_results.send(std::move(res));
                            break;
                        }
                        n_fact = (int32_t) blob.n_tokens;
                        cur_state_imported = true;
                    }
                    const size_t blob_size = llama_state_seq_get_size(ctx, seq_id);
                    // [obrain K1-METRICS] METRIQUES SEPAREES. `fact_decode_ms` est
                    // conserve pour compatibilite, mais il ne dit PAS ce qu'il
                    // mesure : sur le chemin `state_ref` il couvre l'import, sur le
                    // chemin texte il couvre le decodage du fait. Les champs
                    // ci-dessous separent les deux, et `-1` signifie NON MESURE --
                    // jamais un zero implicite.
                    const float preparation_total_ms = (float) (ggml_time_us() - t_fact_decode_start) / 1000.0f;
                    const float fact_decode_ms = preparation_total_ms;
                    cur_ms_prefix_compute = (cur_prefix_decode_calls > 0) ? fact_decode_ms : -1.0f;
                    cur_ms_state_lookup   = cur_state_imported ? -1.0f : -1.0f;  // renseigne plus bas si import
                    cur_ms_integrity      = -1.0f;
                    cur_ms_state_import   = -1.0f;
                    cur_ms_preparation    = preparation_total_ms;

                    // (D) Decode the query prompt AFTER the spliced fact.
                    const int64_t t_query_decode_start = ggml_time_us();
                    if (decode_batch(query_tokens, n_fact, true) != 0) {
                        auto res = std::make_unique<server_task_result_error>();
                        res->id = task.id;
                        res->err_type = ERROR_TYPE_SERVER;
                        res->err_msg = "failed to decode query prompt";
                        queue_results.send(std::move(res));
                        break;
                    }
                    const float query_decode_ms = (float) (ggml_time_us() - t_query_decode_start) / 1000.0f;
                    // [obrain O1] PREMIERS LOGITS NATIFS, copies ICI -- au bon point,
                    // avant que la boucle de generation ne les invalide. Copie
                    // uniquement en mode verification (cout memoire explicite).
                    if (task.jspace_verify) {
                        llama_synchronize(ctx);
                        const float * _lg = llama_get_logits_ith(ctx, -1);
                        const int32_t _nv = (int32_t) llama_vocab_n_tokens(vocab);
                        if (_lg) cur_first_logits.assign(_lg, _lg + _nv);
                    }

                    // (E) Generate. 2026-07-13 fix : greedy by default for
                    // reliable factual recall (see jspace_greedy's doc
                    // comment in server-task.h) -- the stochastic sampler
                    // (fixed seed) inherited from JSPACE_COMPARE_GENERATION
                    // is preserved as an opt-in (greedy=false) for
                    // comparison-style experimentation, where sampling
                    // noise is intentional (reveals more than argmax flips).
                    llama_sampler * smpl = llama_sampler_chain_init(llama_sampler_chain_default_params());
                    if (task.jspace_greedy) {
                        llama_sampler_chain_add(smpl, llama_sampler_init_greedy());
                    } else {
                        llama_sampler_chain_add(smpl, llama_sampler_init_temp(0.7f));
                        llama_sampler_chain_add(smpl, llama_sampler_init_dist(42));
                    }
                    std::string text;
                    int32_t cur_pos = n_fact + n_query;
                    // fin de generation : tout token EOG (eos, eot, <|im_end|>...), pas seulement eos
                    const int32_t n_generate = task.jspace_n_generate > 0 ? task.jspace_n_generate : 24;

                    // [obrain SPEC-inject] branchement du proposer : begin() reinitialise
                    // l etat par-seq du proposer et indexe les fenetres de l historique
                    // connu (fait+query, ou query seule en K2-par-ref ou le texte du fait
                    // n existe pas), puis seed() re-injecte les fenetres du fait —
                    // seed APRES begin car begin peut reset le map sur occupancy > 0.25
                    // apres insertion, la ou seed reset AVANT (semantique 6e72f5d,
                    // preserve les seeds). hist sert aussi de dp.prompt au draft :
                    // ngram-mod ne lit que sa queue (fenetre n_match), l alignement
                    // absolu des positions passe par dp.n_past.
                    llama_tokens hist;
                    if (spec_consume) {
                        if (!kv_by_ref) {
                            hist = fact_tokens;
                        }
                        hist.insert(hist.end(), query_tokens.begin(), query_tokens.end());
                        common_speculative_begin(spec.get(), seq_id, hist);
                        if (!fact_tokens.empty()) {
                            common_speculative_seed(spec.get(), seq_id, fact_tokens);
                            SRV_INF("[spec-inject] map n-gram seede avec %d tokens du fait\n", n_fact);
                        }
                    }
                    // 2026-07-13 optimization (operator: "on devrait pouvoir
                    // le faire instantanement") — decode_batch (shared lambda
                    // above, also used for the variable-size fact/query
                    // decodes) does llama_batch_init + llama_decode +
                    // llama_batch_free on EVERY call. In this loop that meant
                    // a fresh alloc/free cycle for EVERY generated token (up
                    // to n_generate times) — real, measured overhead, and a
                    // stable, reused batch shape is a PRECONDITION for
                    // llama.cpp's CUDA-graph capture/replay (ggml_cuda_graph_
                    // get_key keys off the compute graph's first-node
                    // pointer ; MUL_MAT_ID / MoE routing has its own batch-
                    // size compatibility ceiling — ggml-cuda.cu, tag
                    // TAG_MUL_MAT_ID_CUDA_GRAPHS). CORRECTION (note 458e8317)
                    // : an earlier version of this comment claimed jspace's
                    // slot-bypass path shows "graphs reused" in the single
                    // digits vs 500+ for normal chat -- that was WRONG, based
                    // on log lines misattributed from concurrent unrelated
                    // requests. jspace emits NO slot print_timing output at
                    // all (verified live), so graph-reuse status could not
                    // actually be read off logs either way. Single-token
                    // batch allocated ONCE outside the loop and reused every
                    // iteration fixes the alloc/free churn regardless, and
                    // gives graph capture/replay a stable shape to key off of
                    // if the underlying ggml/CUDA backend chooses to use it
                    // -- whether it actually does is what per_token_ms below
                    // is for : a real per-token capture/replay pattern would
                    // show up as one/a few slow entries then a flat, much
                    // lower plateau ; an undifferentiated series means no
                    // observable reuse benefit from this vantage point.
                    // [obrain SPEC-inject] avec --spec-inject-consume le batch passe de
                    // 1 a 1+n_spec_max tokens (tok courant + draft du proposer) ; sans
                    // le flag, n_spec_max = 0 et la forme reste le mono-token historique.
                    const int32_t n_spec_max = spec_consume ? common_speculative_n_max(&params_base.speculative) : 0;
                    llama_batch gen_batch = llama_batch_init(1 + n_spec_max, 0, 1);
                    std::vector<float> per_token_ms; // par ROUND de decode (== par token sans spec)
                    per_token_ms.reserve((size_t) n_generate);

                    int32_t spec_draft_n        = 0;
                    int32_t spec_draft_accepted = 0;
                    // rollback hybride : sur les modeles SSM/GDN le seq_rm partiel est
                    // impossible (COMMON_CONTEXT_SEQ_RM_TYPE_FULL) — meme discipline
                    // checkpoint save/restore que update_slots (verify a 4598+).
                    common_prompt_checkpoint spec_ckpt;

                    // premier token : echantillonne sur les logits du prompt query,
                    // puis boucle round par round. Sequence emise identique a l
                    // ancienne boucle token-par-token (echantillonnage sequentiel,
                    // EOG jamais emis, plafond n_generate) — seul ajout : un
                    // echantillon final surnumeraire jete en sortie de boucle
                    // (sans effet en greedy, tirage RNG perdu en stochastique).
                    llama_token tok = llama_sampler_sample(smpl, ctx, -1);
                    llama_sampler_accept(smpl, tok);

                    int32_t t = 0;
                    while (t < n_generate) {
                        if (llama_vocab_is_eog(vocab, tok)) { cur_stop = "EOS"; break; }
                        // [obrain O1] ID REEL, issu du sampler de PRODUCTION.
                        // Aucun ID n'est reconstruit depuis `text`.
                        // POSITION : `cur_pos` est EXACTEMENT la position ou ce
                        // token va etre decode par le batch ci-dessous (`gen_batch.pos[0]
                        // = cur_pos`). L'ancienne formule `cur_pos + t` ajoutait `t`
                        // a une valeur qui avance DEJA : elle produisait un pas de 2.
                        cur_decoded_ids.push_back(tok);
                        cur_decoded_pos.push_back((llama_pos) cur_pos);
                        text += common_token_to_piece(vocab, tok);
                        t++;
                        cur_emitted_count++;

                        // draft du proposer actif (map n-gram partage, seede par le fait)
                        llama_tokens draft;
                        if (spec_consume && t < n_generate) {
                            auto & dp = common_speculative_get_draft_params(spec.get(), seq_id);
                            dp = {
                                /* .drafting = */ true,
                                /* .n_max    = */ n_spec_max,
                                /* .n_past   = */ cur_pos,
                                /* .id_last  = */ tok,
                                /* .prompt   = */ &hist,
                                /* .result   = */ &draft,
                            };
                            common_speculative_draft(spec.get());
                            dp.prompt = nullptr;
                            dp.result = nullptr;
                            // borne au budget de generation restant
                            if ((int32_t) draft.size() > n_generate - t) {
                                draft.resize((size_t) (n_generate - t));
                            }
                        }

                        const bool need_ckpt = !draft.empty() &&
                            (ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_FULL ||
                             (ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_RS &&
                              (int32_t) draft.size() > (int32_t) llama_n_rs_seq(ctx)));
                        if (need_ckpt) {
                            spec_ckpt.update_pos(
                                    (int64_t) hist.size(),
                                    llama_memory_seq_pos_min(llama_get_memory(ctx), seq_id),
                                    llama_memory_seq_pos_max(llama_get_memory(ctx), seq_id));
                            spec_ckpt.update_tgt(ctx, seq_id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
                        }

                        // decode tok + draft en un seul batch, logits sur chaque position
                        gen_batch.n_tokens = 1 + (int32_t) draft.size();
                        for (int32_t i = 0; i < gen_batch.n_tokens; i++) {
                            gen_batch.token[i]     = i == 0 ? tok : draft[i - 1];
                            gen_batch.pos[i]       = cur_pos + i;
                            gen_batch.n_seq_id[i]  = 1;
                            gen_batch.seq_id[i][0] = seq_id;
                            gen_batch.logits[i]    = true;
                        }
                        const int64_t t_decode_start = ggml_time_us();
                        const int rc_decode = llama_decode(ctx, gen_batch);
                        per_token_ms.push_back((float) (ggml_time_us() - t_decode_start) / 1000.0f);
                        if (rc_decode != 0) break;
                        if (spec_consume) {
                            hist.push_back(tok);
                        }

                        // verify sequentiel : on echantillonne position par position et
                        // on accepte tant que l echantillon coincide avec le draft —
                        // memes regles d arret que la boucle non-spec (EOG, n_generate)
                        int32_t k = 0;
                        llama_token next = llama_sampler_sample(smpl, ctx, 0);
                        llama_sampler_accept(smpl, next);
                        while (k < (int32_t) draft.size() && t < n_generate &&
                               !llama_vocab_is_eog(vocab, next) && next == draft[k]) {
                            // second site d'emission (acceptation speculatives)
                            cur_decoded_ids.push_back(next);
                            cur_decoded_pos.push_back((llama_pos) cur_pos);
                            text += common_token_to_piece(vocab, next);
                            cur_emitted_count++;
                            t++;
                            hist.push_back(next);
                            k++;
                            next = llama_sampler_sample(smpl, ctx, k);
                            llama_sampler_accept(smpl, next);
                        }

                        if (!draft.empty()) {
                            spec_draft_n        += (int32_t) draft.size();
                            spec_draft_accepted += k;
                            common_speculative_accept(spec.get(), seq_id, (uint16_t) k);
                        }

                        // retire du contexte les tokens draftes non acceptes
                        if (k < (int32_t) draft.size()) {
                            if (need_ckpt) {
                                // restaure l etat pre-round puis rejoue tok + prefixe accepte
                                spec_ckpt.load_tgt(ctx, seq_id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
                                common_context_seq_rm(ctx, seq_id, spec_ckpt.pos_max + 1, -1);
                                std::vector<llama_token> replay;
                                replay.push_back(tok);
                                replay.insert(replay.end(), draft.begin(), draft.begin() + k);
                                if (decode_batch(replay, cur_pos, false) != 0) break;
                            } else {
                                common_context_seq_rm(ctx, seq_id, cur_pos + 1 + k, -1);
                            }
                        }

                        cur_pos += 1 + k;
                        tok = next;
                    }
                    llama_batch_free(gen_batch);
                    llama_sampler_free(smpl);

                    llama_memory_seq_rm(llama_get_memory(ctx), seq_id, -1, -1);

                    auto res = std::make_unique<server_task_result_jspace_inject>();
                    res->id          = task.id;
                    // [obrain K1-METRICS] mesures SEPAREES, toutes renseignees.
                    res->ms_prefix_compute    = cur_ms_prefix_compute;
                    res->ms_state_lookup      = ms_lookup;
                    res->ms_model_sha256      = ms_model_sha;   // mesure SEPAREE
                    res->ms_blob_sha256       = ms_blob_sha;
                    res->ms_state_import      = ms_import;
                    res->ms_compatibility     = ms_compat;
                    res->ms_preparation_total = cur_ms_preparation;
                    // [obrain O1] verification : IDs, positions, logits, methodes.
                    if (task.jspace_verify) {
                        res->verify_enabled = true;
                        res->first_logits_n = (int32_t) cur_first_logits.size();
                        if (!cur_first_logits.empty()) {
                            res->first_logits_base64 = base64::encode(
                                reinterpret_cast<const char *>(cur_first_logits.data()),
                                cur_first_logits.size() * sizeof(float));
                        }
                        { std::string j = "[";
                          for (size_t i = 0; i < cur_decoded_ids.size(); ++i) {
                              if (i) j += ",";
                              j += std::to_string((int) cur_decoded_ids[i]); }
                          j += "]"; res->decoded_ids_json = j; }
                        { std::string j = "[";
                          for (size_t i = 0; i < cur_decoded_pos.size(); ++i) {
                              if (i) j += ",";
                              j += std::to_string((long long) cur_decoded_pos[i]); }
                          j += "]"; res->decoded_pos_json = j; }
                        res->continuation_decode_calls = cur_cont_decode_calls;
                        res->continuation_token_count  = cur_cont_token_count;
                        res->selected_ids_count  = (int32_t) cur_decoded_ids.size();
                        res->emitted_ids_count   = cur_emitted_count;
                        res->n_generate_requested= n_generate;
                        res->stop_reason         = cur_stop;
                        res->selection_method = "llama_sampler_sample (argmax en greedy, egalites au plus petit ID)";
                    }
                    res->text        = text;
                    res->fact_tokens = n_fact;
                    res->prefix_decode_calls = cur_prefix_decode_calls;
                    res->prefix_token_count  = cur_prefix_token_count;
                    res->state_imported      = cur_state_imported;
                    res->blob_bytes  = blob_size;
                    res->per_token_ms = per_token_ms;
                    res->fact_decode_ms = fact_decode_ms;
                    res->query_decode_ms = query_decode_ms;
                    res->spec_draft_n        = spec_draft_n;
                    res->spec_draft_accepted = spec_draft_accepted;
                    queue_results.send(std::move(res));
                } break;
        }
    }

    void iterate(std::vector<server_slot> & slots, std::function<void(server_slot &)> callback) {
        for (auto & slot : slots) {
            try {
                callback(slot);
            } catch (const std::exception & e) {
                SLT_ERR(slot, "got exception: %s\n", e.what());
                send_error(slot, std::string("got exception: ") + e.what(), ERROR_TYPE_SERVER);
                slot.release();
            }
        }
    }

    void iterate(std::vector<server_slot *> & slots, std::function<void(server_slot &)> callback) {
        for (auto & slot : slots) {
            try {
                callback(*slot);
            } catch (const std::exception & e) {
                SLT_ERR(*slot, "got exception: %s\n", e.what());
                send_error(*slot, std::string("got exception: ") + e.what(), ERROR_TYPE_SERVER);
                slot->release();
            }
        }
    }

    void abort_all_slots(const std::string & reason) {
        for (auto & slot : slots) {
            if (slot.is_processing()) {
                send_error(slot, reason, ERROR_TYPE_SERVER);
                slot.release();
            }
        }
    }

    // @ngxson : for debugging only
    int64_t t_pre_decode  = 0;
    int64_t t_decode      = 0;
    int64_t t_post_decode = 0;
    int64_t t_sampl       = 0;
    int64_t n_pre_decode  = 0;
    int64_t n_decode      = 0;
    int64_t n_post_decode = 0;
    int64_t n_sampl       = 0;
// #define DEBUG_TIMINGS
#ifdef DEBUG_TIMINGS
    struct scoped_timer {
        int64_t & t;
        int64_t & n;
        int64_t t_start;
        scoped_timer(int64_t & t_, int64_t & n_) : t(t_), n(n_) {
            t_start = ggml_time_us();
        }
        ~scoped_timer() {
            t += ggml_time_us() - t_start;
            n++;
        }
    };
#else
    struct scoped_timer {
        scoped_timer(int64_t &, int64_t &) {}
        ~scoped_timer() {}
    };
#endif

    void update_slots() {
#ifdef DEBUG_TIMINGS
        static int64_t t_prev = 0;
        int64_t t_start = ggml_time_us();
        if (t_start - t_prev > 5 * 1000 * 1000) { // every 5 seconds
            t_prev = t_start;
            SRV_INF("n_pre_decode      = %" PRId64 "\n", n_pre_decode);
            SRV_INF("avg t_pre_decode  = %f ms\n", (double) t_pre_decode / n_pre_decode / 1000.0);
            SRV_INF("avg t_decode      = %f ms\n", (double) t_decode / n_decode / 1000.0);
            SRV_INF("avg t_post_decode = %f ms\n", (double) t_post_decode / n_post_decode / 1000.0);
            SRV_INF("avg t_sampl       = %f ms\n", (double) t_sampl / n_sampl / 1000.0);
        }
#endif

        // check if all slots are idle
        {
            bool all_idle = true;

            for (auto & slot : slots) {
                if (slot.is_processing()) {
                    all_idle = false;
                    break;
                }
            }

            if (all_idle) {
                SRV_TRC("%s", "all slots are idle\n");
                return; // skip further processing

            } else {
                SRV_DBG("%s", "posting NEXT_RESPONSE\n");

                server_task task(SERVER_TASK_TYPE_NEXT_RESPONSE);
                task.id = queue_tasks.get_new_id();
                queue_tasks.post(std::move(task));
            }
        }

        try {
            scoped_timer t(t_pre_decode, n_pre_decode);
            pre_decode();
            batch.render();
        } catch (const std::exception & e) {
            SRV_ERR("pre_decode() failed: %s\n", e.what());
            abort_all_slots("pre_decode() failed: " + std::string(e.what()));
        }

        llama_batch batch_view;
        int32_t off_next = 0;
        int32_t n_batch = llama_n_batch(ctx_tgt);
        for (int32_t off = 0; off < batch.size(); off = off_next) {
            const int32_t n_tokens = std::min(n_batch, batch.size() - off);
            try {
                scoped_timer t(t_decode, n_decode);
                // TODO @ngxson : maybe handle n_batch == 1 here instead of inside decode()

                batch_view = batch.get_view(off, n_tokens);
                bool ok = decode(n_batch, off, batch_view);
#ifdef DEBUG_TIMINGS
                llama_synchronize(ctx_tgt);
#endif

                if (ok) {
                    // move the head of the batch forward with the number of tokens we just processed
                    off_next = off + n_tokens;

                    // on successful decode, restore the original batch size
                    n_batch = llama_n_batch(ctx_tgt);
                } else {
                    // try again with the updated n_batch
                    continue;
                }
            } catch (const std::exception & e) {
                SRV_ERR("decode() failed: %s\n", e.what());
                abort_all_slots("decode() failed: " + std::string(e.what()));
                break; // stop any further processing
            }

            try {
                scoped_timer t(t_post_decode, n_post_decode);
                post_decode(n_tokens, off, batch_view);
            } catch (const std::exception & e) {
                SRV_ERR("post_decode() failed: %s\n", e.what());
                abort_all_slots("post_decode() failed: " + std::string(e.what()));
                break; // stop any further processing
            }

        }
    }

    void pre_decode() {
        // apply context-shift if needed
        // TODO: simplify and improve
        iterate(slots, [&](server_slot & slot) {
            if (slot.state == SLOT_STATE_GENERATING && slot.prompt.n_tokens() + 1 >= slot.n_ctx) {
                if (!params_base.ctx_shift) {
                    // this check is redundant (for good)
                    // we should never get here, because generation should already stopped in process_token()
                    send_error(slot, "context shift is disabled", ERROR_TYPE_SERVER);
                    slot.release();
                    return;
                }

                if (mctx) {
                    // we should never reach this because params_base.ctx_shift is automatically disabled if mmproj is loaded
                    // we don't support ctx_shift because an image chunk may contains multiple tokens
                    GGML_ABORT("not supported by multimodal");
                }

                if (slot.task->is_parent() || slot.task->is_child()) {
                    send_error(slot, "context shift cannot be used for shared prompt", ERROR_TYPE_SERVER);
                    slot.release();
                    return;
                }

                // Shift context
                int n_keep = slot.task->params.n_keep < 0 ? slot.task->n_tokens() : slot.task->params.n_keep;

                if (add_bos_token) {
                    n_keep += 1;
                }

                n_keep = std::min(slot.n_ctx - 4, n_keep);

                const int n_left    = slot.prompt.n_tokens() - n_keep;
                int       n_discard = slot.task->params.n_discard ? slot.task->params.n_discard : (n_left / 2);

                // ref: https://github.com/ggml-org/llama.cpp/pull/24786
                n_discard = std::clamp(n_discard, 0, std::max(0, n_left - 1));

                SLT_WRN(slot, "slot context shift, n_keep = %d, n_left = %d, n_discard = %d\n", n_keep, n_left, n_discard);

                common_context_seq_rm (ctx_tgt, slot.id, n_keep            , n_keep + n_discard);
                common_context_seq_add(ctx_tgt, slot.id, n_keep + n_discard, slot.prompt.n_tokens(), -n_discard);

                if (ctx_dft) {
                    common_context_seq_rm (ctx_dft, slot.id, n_keep            , n_keep + n_discard);
                    common_context_seq_add(ctx_dft, slot.id, n_keep + n_discard, slot.prompt.tokens.pos_next(), -n_discard);
                }

                // add generated tokens to cache
                // ref: https://github.com/ggml-org/llama.cpp/pull/16818#discussion_r2473269481
                {
                    GGML_ASSERT(!slot.prompt.tokens.has_mtmd);

                    llama_tokens new_tokens = slot.prompt.tokens.get_tokens(); // copy
                    for (size_t i = n_keep + n_discard; i < new_tokens.size(); i++) {
                        new_tokens[i - n_discard] = new_tokens[i];
                    }

                    new_tokens.resize(slot.prompt.tokens.size() - n_discard);

                    slot.prompt.tokens.clear();
                    slot.prompt.tokens.insert(new_tokens);
                }

                slot.truncated = true;
            }
        });

        // start populating the batch for this iteration
        batch.clear();

        // track if given slot can be batched with slots already in the batch
        auto & slot_batched = batch.slot_batched;

        std::vector<server_slot *> generating;
        std::vector<server_slot *> drafting;

        // determine which slots are generating and drafting
        iterate(slots, [&](server_slot & slot) {
            if (slot.state != SLOT_STATE_GENERATING) {
                return;
            }

            // check if we can batch this slot with the previous one
            if (!slot_batched) {
                slot_batched = &slot;
            } else if (!slot_batched->can_batch_with(slot)) {
                return;
            }

            generating.push_back(&slot);

            if (spec) {
                common_speculative_get_draft_params(spec.get(), slot.id).drafting = false;

                const bool use_ckpt_tgt = ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_FULL;
                const bool use_ckpt_dft = ctx_dft_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_FULL;

                const int n_draft_max = slot.get_n_draft_max();

                // [obrain SPLICE-CHAT] `spec_allowed()` (et non can_speculate) :
                // sur une tache a prefixe importe, aucun brouillon n'est demande
                // -- ctx_dft n'a pas le prefixe (cf. server_slot::spec_allowed).
                if (n_draft_max > 0 && slot.spec_allowed()) {
                    GGML_ASSERT(slot.can_speculate());

                    if (!slot.spec_draft.empty()) {
                        // we have a previous (partial) draft to reuse
                        if (use_ckpt_tgt) {
                            GGML_ASSERT(!slot.spec_ckpt.empty());
                        }
                    } else {
                        GGML_ASSERT(slot.spec_i_batch.empty());

                        slot.spec_ckpt.update_pos(
                                slot.prompt.n_tokens(),
                                llama_memory_seq_pos_min(llama_get_memory(ctx_tgt), slot.id),
                                llama_memory_seq_pos_max(llama_get_memory(ctx_tgt), slot.id));

                        if (use_ckpt_dft) {
                            slot.spec_ckpt.update_dft(ctx_dft, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
                        }

                        slot.spec_prompt = slot.prompt.tokens.get_text_tokens();

                        common_speculative_get_draft_params(spec.get(), slot.id) = {
                            /* .drafting = */ true,
                            /* .n_max    = */ n_draft_max,
                            /* .n_past   = */ slot.prompt.n_tokens(),
                            /* .id_last  = */ slot.sampled,
                            /* .prompt   = */ &slot.spec_prompt,
                            /* .result   = */ &slot.spec_draft,
                        };

                        drafting.push_back(&slot);
                    }
                }
            }
        });

        // generate the actual drafts (if any)
        //
        // [obrain SPEC-METRICS] c'est ICI que se paie le brouillon : `common_speculative_draft` fait
        // tourner le modèle brouillon (qui lit ses propres poids) pour proposer des jetons. Le temps
        // mesuré ici couvre le brouillon de TOUS les emplacements brouillonneurs ; avec un seul
        // emplacement actif (le cas d'un banc mono-requête) il lui est attribué en entier.
        const int64_t t_draft_start = ggml_time_us();
        {
            common_speculative_draft(spec.get());
        }
        const float spec_draft_ms = (float) (ggml_time_us() - t_draft_start) / 1000.0f;

        // [obrain R3-A] SITE 1 -- DECIDE. Le brouillon du cadre est REMPLACE par celui du proposeur
        // certifie. Init(prompt) a lieu ICI, UNE SEULE FOIS par requete logique ; ensuite l'etat
        // n'evolue plus que par update() sur les jetons COMMITTED (site 2).
        iterate(drafting, [&](server_slot & slot) {
            if (!slot.obrain_enabled) { return; }
            if (slot.obrain_need_init) {
                std::vector<int32_t> pr;
                pr.reserve(slot.prompt.tokens.size());
                for (size_t k = 0; k < slot.prompt.tokens.size(); ++k) {
                    pr.push_back((int32_t) slot.prompt.tokens[k]);
                }
                // [R3-A7] FRONTIERE CAUSALE : les jetons deja GENERES dans cette requete avant le
                // premier DECIDE font partie de l'histoire initialisee. Sans cela, le tout premier
                // jeton genere d'une requete n'entre jamais dans l'etat (leak causal d'un jeton).
                {
                    const int64_t cle = g_pre_init_ctx_key_public((int64_t) slot.id,
                                                                                        slot.obrain_generation);
                    auto it2 = obrain_proposer_state::g_pre_init_by_ctx_public().find(cle);
                    if (it2 != obrain_proposer_state::g_pre_init_by_ctx_public().end()) {
                        for (int32_t tk : it2->second) { pr.push_back(tk); }
                    }
                }
                slot.obrain_st.init(pr, 0, (int64_t) slot.id, slot.obrain_generation);
                slot.obrain_need_init = false;
                fprintf(stderr, "OBRAIN_R3A init slot=%d init_count=%lld prompt_length=%lld "
                                "state_token_count=%lld digest=%016llx\n",
                        slot.id, (long long) slot.obrain_st.init_count,
                        (long long) slot.obrain_st.prompt_tokens_inserted,
                        (long long) slot.obrain_st.state_token_count(),
                        (unsigned long long) slot.obrain_st.state_digest());
                fflush(stderr);
            }
            const int64_t st_before = slot.obrain_st.state_token_count();
            const int     dsize     = (int) slot.spec_draft.size();
            // [R3-B decouplage] LA CAPACITE N'EST PLUS HERITEE DU BROUILLON MTP.
            const int     n_max     = obrain_verify_capacity_b();
            const obrain_decision d = slot.obrain_st.propose(n_max);
            std::vector<llama_token> mine;
            if (d.propose) {
                const int n_rem_ = (slot.task->params.n_predict != -1)
                                     ? (slot.task->params.n_predict - slot.n_decoded) : -1;
                const int p_cap  = std::min(d.P, n_max - 1);                       // P_after_capacity
                const int peff   = (n_rem_ >= 1) ? std::min(p_cap, n_rem_ - 1) : p_cap;   // clip terminal
                for (int k = 0; k < peff; ++k) mine.push_back((llama_token) d.chemin[k]);
            }
            slot.obrain_draft = mine;
            slot.spec_draft   = mine;                  // remplace le brouillon du cadre
            if (!slot.obrain_st.check_invariants("site1")) { GGML_ABORT("OBRAIN_INVARIANT_KO site1"); }
            if (slot.obrain_st.state_token_count() != st_before) {
                GGML_ABORT("OBRAIN_INVARIANT_KO site1 : Propose a modifie l'etat");
            }
            // [R3-B instrumentation] LES CINQ VALEURS DU MEME EVENEMENT. `remaining` est la valeur
            // RUNTIME du serveur (sa propre formule), JAMAIS une reconstruction depuis state_before,
            // prompt_length, ordinal ou n_predict approche.
            const int n_rem = (slot.task->params.n_predict != -1)
                                ? (slot.task->params.n_predict - slot.n_decoded) : -1;
            const int verify_capacity_b = (g_obrain_capacity_b >= 2) ? g_obrain_capacity_b : n_max;  // [OBRAIN_CAPACITY]
            const int p_after_capacity  = std::min(d.P, n_max - 1);     // P_capacity = min(P_star, C-1)
            const int p_eff_computed    = std::min(p_after_capacity, n_rem - 1);   // clip terminal
            const char * clip_reason    = (n_rem >= 1 && p_after_capacity > n_rem - 1) ? "TERMINAL"
                                        : ((d.P > p_after_capacity) ? "CAPACITY" : "NONE");
            fprintf(stderr, "OBRAIN_R3A site1 slot=%d decision_index=%lld state_before=%lld "
                            "spec_draft_size_avant=%d P_star=%d verify_capacity_b=%d P_after_capacity=%d "
                            "remaining=%d P_eff=%d clip_reason=%s N=%d draft_size=%d digest=%016llx\n",
                    slot.id, (long long) slot.obrain_st.decision_count, (long long) st_before,
                    dsize, d.P, verify_capacity_b, p_after_capacity, n_rem, p_eff_computed, clip_reason,
                    d.N, (int) mine.size(),
                    (unsigned long long) slot.obrain_st.state_digest());
            fflush(stderr);
        });

        // make checkpoints if needed
        iterate(drafting, [&](server_slot & slot) {
            auto & draft = slot.spec_draft;
            auto & ckpt  = slot.spec_ckpt;

            slot.n_draft_total += draft.size();

            // [obrain SPEC-METRICS] ouvre la passe : la proposition est connue, la vérification suit
            slot.spec_n_prop_pending    = (int32_t) draft.size();
            slot.spec_t_round_pending   = t_draft_start;
            slot.spec_draft_ms_pending  = spec_draft_ms;
            slot.spec_verify_ms_pending = 0.0f;

            // TODO: avoid restoring the draft context and re-evaluating the drafted tokens when not needed [TAG_SPEC_AVOID_DRAFT_REEVAL]
            const bool use_ckpt_dft = ctx_dft_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_FULL;

            if (ctx_dft) {
                if (use_ckpt_dft) {
                    ckpt.load_dft(ctx_dft, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
                }

                common_context_seq_rm(ctx_dft, slot.id, ckpt.pos_max + 1, -1);
            }

            if (!draft.empty()) {
                const bool use_ckpt_tgt =
                    ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_FULL ||
                   (ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_RS && draft.size() > llama_n_rs_seq(ctx_tgt));

                const bool use_ckpt_dft =
                   (ctx_dft_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_RS && draft.size() > llama_n_rs_seq(ctx_dft));

                if (use_ckpt_tgt) {
                    //const int64_t t_start = ggml_time_us();

                    ckpt.update_tgt(ctx_tgt, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);

                    //const int64_t t_total = ggml_time_us() - t_start;
                    //printf("checkpoint total: %f ms\n", t_total / 1000.0);

                    SLT_DBG(slot, "created speculative checkpoint (pos_min = %d, pos_max = %d, n_tokens = %d, size = %.3f MiB, draft = %.3f MiB)\n",
                            ckpt.pos_min, ckpt.pos_max, slot.prompt.n_tokens(),
                            (float) ckpt.size() / 1024 / 1024,
                            (float) ckpt.data_dft.size() / 1024 / 1024);
                }

                if (use_ckpt_dft) {
                    ckpt.update_dft(ctx_dft, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
                }
            }
        });

        // update the batch with the sampled/drafted tokens
        iterate(generating, [&](server_slot & slot) {
            slot.handle_last_sampled_token(batch);
        });

        // process in chunks of params.n_batch
        int32_t n_batch  = llama_n_batch(ctx_tgt);
        int32_t n_ubatch = llama_n_ubatch(ctx_tgt);

        auto & alora_scale       = batch.alora_scale;
        auto & alora_disabled_id = batch.alora_disabled_id;

        // next, batch any pending prompts without exceeding n_batch
        if (params_base.cont_batching || batch.size() == 0) {
            bool add_ok = true; // false means the batch is full, skip remaining slots

            iterate(slots, [&](server_slot & slot) {
                if (!add_ok || batch.size() >= n_batch) {
                    return; // batch is full, skip remaining slots
                }

                if (!slot.is_processing()) {
                    return;
                }

                // check if we can batch this slot with the previous one
                if (slot_batched && !slot_batched->can_batch_with(slot)) {
                    return;
                }

                // check if this is a child slot
                if (slot.state == SLOT_STATE_WAIT_OTHER) {
                    SLT_DBG(slot, "%s", "waiting for parent slot to complete\n");
                    return;
                }

                // this slot still has a prompt to be processed
                if (slot.state == SLOT_STATE_PROCESSING_PROMPT || slot.state == SLOT_STATE_STARTED) {
                    const auto & input_tokens = slot.task->tokens;

                    // used to determine the number of tokens added to the batch for the current slot
                    const auto n_tokens_prev = batch.size();

                    // TODO: maybe move branch to outside of this loop in the future
                    if (slot.state == SLOT_STATE_STARTED) {
                        slot.t_start_process_prompt = ggml_time_us();
                        slot.t_start_generation = 0;

                        slot.state = SLOT_STATE_PROCESSING_PROMPT;

                        SLT_TRC(slot, "new prompt, n_ctx_slot = %d, n_keep = %d, task.n_tokens = %d\n",
                                slot.n_ctx, slot.task->params.n_keep, slot.task->n_tokens());

                        // print prompt tokens (for debugging)
                        /*if (1) {
                            // first 16 tokens (avoid flooding logs)
                            for (int i = 0; i < std::min<int>(16, input_tokens.size()); i++) {
                                SLT_DBG(slot, "prompt token %3d: %6d '%s'\n", i, input_tokens[i], common_token_to_piece(ctx_tgt, input_tokens[i]).c_str());
                            }
                        } else {
                            // all
                            for (int i = 0; i < (int) input_tokens.size(); i++) {
                                SLT_DBG(slot, "prompt token %3d: %6d '%s'\n", i, input_tokens[i], common_token_to_piece(ctx_tgt, input_tokens[i]).c_str());
                            }
                        }*/

                        // keep track how many tokens we can reuse from the previous state
                        int n_past = 0;

                        // empty prompt passed -> release the slot and send empty response
                        if (input_tokens.empty()) {
                            SLT_WRN(slot, "%s", "empty prompt - releasing slot\n");

                            slot.print_timings();
                            send_final_response(slot);
                            slot.release();

                            return;
                        }

                        // TODO: support memory-less logits computation
                        if (slot.task->need_logits() && !llama_get_memory(ctx_tgt)) {
                            send_error(slot, "the current context does not logits computation. skipping", ERROR_TYPE_SERVER);
                            slot.release();
                            return;
                        }

                        // ═══════════════════════════════════════════════════════════
                        // [obrain SPLICE-CHAT] IMPORT DE L'ETAT KV PREFIXE (marche 2).
                        //
                        // CE QUI EST REUTILISE, ET CE QUI NE L'EST PAS :
                        //  - REUTILISE : le cache d'etats (g_kv_cache), le format
                        //    disque OBKV, le controle d'integrite et la DECISION
                        //    D'IMPORT de production (obrain_kv::decide_import), ainsi
                        //    que l'appel bas niveau llama_state_seq_set_data — le meme
                        //    que la branche K2 de /jspace/inject_knowledge. Il n'y a
                        //    donc PAS deux logiques KV dans le serveur.
                        //  - NON REUTILISE : la boucle de generation de jspace. Elle
                        //    ecrit sa reponse d'un bloc (server_task_result_jspace_inject)
                        //    et ne peut PAS streamer ; la remplacer ici casserait le
                        //    streaming SSE et le formatage OAI/tool-calls. Le chat
                        //    garde donc SA boucle (update_slots + samplers) et se
                        //    contente de demarrer son prefill APRES l'etat importe :
                        //    c'est precisement ce que veut dire « splice invisible ».
                        //
                        // L'INVARIANT QUI REND CELA SÛR : les IDs du prefixe ont ete
                        // prependus a task.tokens par handle_completions_impl. Le
                        // prefixe importe occupe les positions [0, n_state_prefix) et
                        // les tokens de la tache s'alignent dessus ; la suite du
                        // prefill reprend a l'index n_state_prefix et a la position
                        // n_state_prefix, sans qu'aucune autre ligne de cette
                        // fonction n'ait besoin de connaitre le splice.
                        // ═══════════════════════════════════════════════════════════
                        if (slot.task->spliced()) {
                            const int32_t n_prefix = slot.task->n_state_prefix;
                            const kv_state_blob & blob = *slot.task->state_blob;

                            if (n_prefix <= 0 || (size_t) n_prefix > slot.task->tokens.size()) {
                                send_error(slot, string_format("state_ref '%s' : prefixe incoherent (%d tokens pour %d tokens de prompt)",
                                                               slot.task->state_ref.c_str(), n_prefix, slot.task->n_tokens()),
                                           ERROR_TYPE_SERVER);
                                slot.release();
                                return;
                            }

                            // (1) COMPATIBILITE — AUCUN set_data n'est tente si la
                            //     decision n'est pas ACCEPT : c'est la meme fonction
                            //     que les tests exercent (obrain-kv-cache-tests.cpp),
                            //     pas une seconde copie des regles.
                            const kv_state_manifest cur = kv_manifest_of(ctx_tgt, params_base.model.path, slot.id);
                            // La convention de sequence du manifeste decrit la sequence
                            // SOURCE : l'extraction J-space vit sur la sequence
                            // RESERVEE (jspace_reserved_slot_id), alors que l'import
                            // ci-dessous vise la sequence de CE slot de chat. Ce SEUL
                            // champ est donc compare a convention egale, parce que
                            // l'implementation le rejette explicitement a la lecture
                            // (llama-kv-cache.cpp, state_read_meta avec dest_seq_id != -1 :
                            // "read the sequence id, but directly discard it - we will
                            // use dest_seq_id instead"). Toutes les autres clauses
                            // (identite modele/Q1, types KV, configuration de contexte,
                            // SHA256, taille) restent REFUSANTES.
                            kv_state_manifest cur_seq = cur;
                            if (blob.manifest) {
                                cur_seq.seq_id = blob.manifest->seq_id;
                            }
                            std::string why;
                            const obrain_kv::ImportDecision dec = obrain_kv::decide_import(blob, cur_seq, why);
                            if (dec != obrain_kv::ImportDecision::ACCEPT) {
                                send_error(slot,
                                           string_format("state_ref '%s' REFUSE avant import (%s) : %s",
                                                         slot.task->state_ref.c_str(),
                                                         obrain_kv::decision_name(dec), why.c_str()),
                                           ERROR_TYPE_INVALID_REQUEST);
                                slot.release();
                                return;
                            }

                            // (2) IMPORT REEL. La sequence du slot est purgee d'abord
                            //     (state_read_meta le fait aussi ; on le rend
                            //     explicite). Le contexte brouillon est purge pour ne
                            //     pas conserver un KV etranger sous le meme id de
                            //     sequence — il ne recoit PAS le prefixe (cf.
                            //     server_slot::spec_allowed).
                            llama_memory_seq_rm(llama_get_memory(ctx_tgt), slot.id, -1, -1);
                            if (ctx_dft) {
                                common_context_seq_rm(ctx_dft, slot.id, -1, -1);
                            }
                            const size_t n_set = llama_state_seq_set_data(ctx_tgt, blob.data.data(), blob.data.size(), slot.id);
                            if (n_set == 0) {
                                send_error(slot, string_format("state_ref '%s' : llama_state_seq_set_data a echoue (etat non importe)",
                                                               slot.task->state_ref.c_str()),
                                           ERROR_TYPE_SERVER);
                                slot.release();
                                return;
                            }

                            // (3) LE PREFIXE DEVIENT LE PREFIXE DE LA TACHE. Sans ces
                            //     IDs, slot.prompt.tokens[i] ne correspondrait plus a
                            //     task.tokens[i] et la boucle de prefill relirait le
                            //     mauvais token. Les checkpoints herites du cache de
                            //     prompts sont INVALIDES (leurs positions decrivent un
                            //     autre contenu).
                            slot.prompt.tokens.clear();
                            slot.prompt.checkpoints.clear();
                            {
                                const auto & tks = slot.task->tokens.get_tokens();
                                llama_tokens pre(tks.begin(), tks.begin() + n_prefix);
                                slot.prompt.tokens.insert(pre);
                            }
                            slot.n_state_prefix = n_prefix;

                            SLT_INF(slot, "[splice] etat %s importe : prefixe de %d tokens (positions [0, %d)), prompt de %d tokens decode a partir de la position %d\n",
                                    slot.task->state_ref.c_str(), n_prefix, n_prefix,
                                    slot.task->n_tokens() - n_prefix, n_prefix);
                        }

                        if (!slot.can_split()) {
                            if (slot.task->n_tokens() > n_ubatch) {
                                send_error(slot,
                                           string_format(
                                               "input (%d tokens) is too large to process. increase the physical batch "
                                               "size (current batch size: %d)",
                                               slot.task->n_tokens(), n_ubatch),
                                           ERROR_TYPE_SERVER);
                                slot.release();
                                return;
                            }

                            if (slot.task->n_tokens() > slot.n_ctx) {
                                send_error(
                                    slot,
                                    string_format(
                                        "input (%d tokens) is larger than the max context size (%d tokens). skipping",
                                        slot.task->n_tokens(), slot.n_ctx),
                                    ERROR_TYPE_EXCEED_CONTEXT_SIZE);
                                slot.release();
                                return;
                            }
                        } else if (slot.n_state_prefix > 0) {
                            // [obrain SPLICE-CHAT] Le prefixe importe est DEJA dans le
                            // KV (positions [0, n_state_prefix)) : rien a reutiliser
                            // depuis le cache de prompts, rien a recalculer, aucun
                            // checkpoint a chercher. n_past vaut donc EXACTEMENT la
                            // taille du prefixe — et c'est ce seul nombre qui fait
                            // reprendre le prefill a la position n_state_prefix, sans
                            // qu'aucune autre ligne de la boucle de prefill n'ait a
                            // connaitre le splice.
                            n_past = (int) slot.n_state_prefix;

                            // Meme garde de taille que le chemin cache : le prefixe
                            // importe OCCUPE des positions, il compte dans n_ctx.
                            if (slot.task->n_tokens() >= slot.n_ctx) {
                                send_error(slot,
                                           string_format("request (%d tokens, prefixe importe compris) exceeds the available "
                                                         "context size (%d tokens), try increasing it",
                                                         slot.task->n_tokens(), slot.n_ctx),
                                           ERROR_TYPE_EXCEED_CONTEXT_SIZE);
                                slot.release();
                                return;
                            }
                        } else {
                            if (slot.task->n_tokens() >= slot.n_ctx) {
                                send_error(slot,
                                           string_format("request (%d tokens) exceeds the available context size (%d "
                                                         "tokens), try increasing it",
                                                         slot.task->n_tokens(), slot.n_ctx),
                                           ERROR_TYPE_EXCEED_CONTEXT_SIZE);
                                slot.release();
                                return;
                            }

                            if (slot.task->params.cache_prompt) {
                                // reuse any previously computed tokens that are common with the new prompt
                                n_past = slot.prompt.tokens.get_common_prefix(input_tokens);

                                // if there is an alora invoked, don't cache after the invocation start
                                if (slot.alora_invocation_start > 0) {
                                    SLT_DBG(slot, "only caching to alora invocation start (n_past = %d, alora_invocation_start = %d)\n", n_past, slot.alora_invocation_start);
                                    n_past = std::min(n_past, slot.alora_invocation_start - 1);
                                }

                                const auto n_cache_reuse = slot.task->params.n_cache_reuse;

                                const bool can_cache_reuse =
                                    llama_memory_can_shift(llama_get_memory(ctx_tgt)) &&
                                    !slot.prompt.tokens.has_mtmd;

                                if (!can_cache_reuse && n_cache_reuse > 0) {
                                    SLT_WRN(slot, "cache reuse is not supported - ignoring n_cache_reuse = %d\n", n_cache_reuse);
                                }

                                // reuse chunks from the cached prompt by shifting their KV cache in the new position
                                if (can_cache_reuse && n_cache_reuse > 0) {
                                    GGML_ASSERT(!slot.prompt.tokens.has_mtmd);

                                    size_t head_c = n_past; // cache
                                    size_t head_p = n_past; // current prompt

                                    if (mctx) {
                                        // we should never reach this
                                        GGML_ABORT("not supported by multimodal");
                                    }

                                    SLT_DBG(slot, "trying to reuse chunks with size > %d, n_past = %d\n", n_cache_reuse, n_past);

                                    while (head_c < slot.prompt.tokens.size() &&
                                           head_p < input_tokens.size()) {

                                        size_t n_match = 0;
                                        while (head_c + n_match < slot.prompt.tokens.size() &&
                                               head_p + n_match < input_tokens.size()       &&
                                               slot.prompt.tokens[head_c + n_match] == input_tokens[head_p + n_match]) {
                                            n_match++;
                                        }

                                        if (n_match >= (size_t) n_cache_reuse) {
                                            SLT_TRC(slot, "reusing chunk with size %zu, shifting KV cache [%zu, %zu) -> [%zu, %zu)\n", n_match, head_c, head_c + n_match, head_p, head_p + n_match);
                                            //for (size_t i = head_p; i < head_p + n_match; i++) {
                                            //    SLT_DBG(slot, "cache token %3zu: %6d '%s'\n", i, prompt_tokens[i], common_token_to_piece(ctx_tgt, prompt_tokens[i]).c_str());
                                            //}

                                            const int64_t kv_shift = (int64_t) head_p - (int64_t) head_c;

                                            common_context_seq_rm (ctx_tgt, slot.id, head_p, head_c);
                                            common_context_seq_add(ctx_tgt, slot.id, head_c, head_c + n_match, kv_shift);

                                            if (ctx_dft) {
                                                common_context_seq_rm (ctx_dft, slot.id, head_p, head_c);
                                                common_context_seq_add(ctx_dft, slot.id, head_c, head_c + n_match, kv_shift);
                                            }

                                            for (size_t i = 0; i < n_match; i++) {
                                                slot.prompt.tokens.set_token(head_p + i, slot.prompt.tokens[head_c + i]);
                                                n_past++;
                                            }

                                            head_c += n_match;
                                            head_p += n_match;
                                        } else {
                                            head_c += 1;
                                        }
                                    }

                                    SLT_DBG(slot, "after context reuse, new n_past = %d\n", n_past);
                                }
                            } else {
                                // if we don't cache the prompt, we have to remove all previous tokens
                                n_past = 0;
                            }

                            llama_pos pos_next = slot.prompt.tokens.pos_next(n_past);

                            // ref: https://github.com/ggml-org/llama.cpp/pull/24110
                            const bool has_new_tokens = (n_past < slot.task->n_tokens());

                            // the largest pos_min required for a checkpoint to be useful
                            const auto pos_min_thold = std::max(0, pos_next - n_swa - (has_new_tokens ? 0 : 1));

                            if (n_past > 0 && n_past <= slot.prompt.n_tokens()) {
                                const auto pos_min = llama_memory_seq_pos_min(llama_get_memory(ctx_tgt), slot.id);
                                if (pos_min == -1) {
                                    SLT_ERR(slot, "n_past = %d, slot.prompt.tokens.size() = %d, seq_id = %d, pos_min = %d\n", n_past, (int) slot.prompt.tokens.size(), slot.id, pos_min);
                                    GGML_ABORT("pos_min == -1, but n_past > 0 - should not happen: https://github.com/ggml-org/llama.cpp/pull/13833#discussion_r2116181237");
                                }

                                // when the prompt prefix does not match, print the tokens around the mismatch
                                // this is useful for debugging prompt caching
                                if (slots_debug) {
                                    const int np0 = std::max<int>(n_past - 4, 0);
                                    const int np1 = std::min<int>(n_past + 6, std::min(slot.prompt.tokens.size(), slot.task->tokens.size()));

                                    std::stringstream ss0;
                                    std::stringstream ss1;

                                    std::stringstream st0;
                                    std::stringstream st1;

                                    ss0 << "old: ... ";
                                    ss1 << "new: ... ";

                                    for (int i = np0; i < np1; i++) {
                                        if (i == n_past) {
                                            ss0 << " | ";
                                            ss1 << " | ";
                                        }

                                        {
                                            const auto token = slot.prompt.tokens[i];
                                            const auto piece = token != LLAMA_TOKEN_NULL ? common_token_to_piece(ctx_tgt, token) : "[mtmd]";
                                            ss0 << piece;
                                            st0 << std::setw(8) << token;
                                        }

                                        {
                                            const auto token = slot.task->tokens[i];
                                            const auto piece = token != LLAMA_TOKEN_NULL ? common_token_to_piece(ctx_tgt, token) : "[mtmd]";
                                            ss1 << piece;
                                            st1 << std::setw(8) << token;
                                        }
                                    }

                                    SLT_WRN(slot, "%s\n", ss0.str().c_str());
                                    SLT_WRN(slot, "%s\n", ss1.str().c_str());

                                    SLT_WRN(slot, "%s\n", st0.str().c_str());
                                    SLT_WRN(slot, "%s\n", st1.str().c_str());
                                }

                                if (pos_min >= pos_min_thold) {
                                    // search for a context checkpoint
                                    const auto it = std::find_if(
                                        slot.prompt.checkpoints.rbegin(),
                                        slot.prompt.checkpoints.rend(),
                                        [&](const auto & cur) {
                                            // guarantee that a checkpoint will result in at least one token being processed [TAG_PROMPT_LOGITS]
                                            SLT_TRC(slot, "checking checkpoint with [%d, %d] against %d...\n", cur.pos_min, cur.pos_max, pos_min_thold);
                                            // workaround for [TAG_CHECKPOINTS_FIX_POS_MIN]
                                            if (cur.pos_max > pos_next) {
                                                return false;
                                            }
                                            return cur.pos_min < pos_min_thold || cur.pos_min == 0;
                                        }
                                    );

                                    bool do_reset = it == slot.prompt.checkpoints.rend();

                                    if (!do_reset) {
                                        // restore the context checkpoint
                                        it->load_tgt(ctx_tgt, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
                                        it->load_dft(ctx_dft, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
                                        // restore the draft's speculative state
                                        common_speculative_set_state(spec.get(), slot.id, it->data_spec);

                                        pos_next = std::min(pos_next, std::max(it->pos_min + 1, it->pos_max));
                                        n_past   = std::min(slot.prompt.tokens.size_up_to_pos(pos_next), (size_t) it->n_tokens);
                                        SLT_TRC(slot, "restored context checkpoint (pos_min = %d, pos_max = %d, n_tokens = %" PRId64 ", n_past = %d, size = %.3f MiB)\n", it->pos_min, it->pos_max, it->n_tokens, n_past, (float) it->size() / 1024 / 1024);
                                    }

                                    if (do_reset) {
                                        SLT_TRC(slot, "forcing full prompt re-processing due to lack of cache data (likely due to SWA or hybrid/recurrent memory, see %s)\n",
                                                "https://github.com/ggml-org/llama.cpp/pull/13194#issuecomment-2868343055");
                                        pos_next = 0;
                                        n_past = 0;
                                    }
                                }
                            }

                            {
                                // erase any checkpoints with pos_max > pos_next
                                for (auto it = slot.prompt.checkpoints.begin(); it != slot.prompt.checkpoints.end();) {
                                    const auto & cur = *it;
                                    if (cur.pos_max > pos_next) {
                                        SLT_TRC(slot, "erased invalidated context checkpoint (pos_min = %d, pos_max = %d, n_tokens = %" PRId64 ", n_swa = %d, pos_next = %d, size = %.3f MiB)\n", cur.pos_min, cur.pos_max, cur.n_tokens, n_swa, pos_next, (float) cur.size() / 1024 / 1024);
                                        it = slot.prompt.checkpoints.erase(it);
                                    } else {
                                        ++it;
                                    }
                                }
                            }
                        }

                        // [TAG_PROMPT_LOGITS]
                        if (n_past == slot.task->n_tokens() && n_past > 0) {
                            SLT_WRN(slot, "need to evaluate at least 1 token for each active slot (n_past = %d, task.n_tokens() = %d)\n", n_past, slot.task->n_tokens());
                            n_past--;
                            SLT_WRN(slot, "n_past was set to %d\n", n_past);
                        }

                        slot.n_prompt_tokens_cache = n_past;
                        slot.n_prompt_tokens_processed = 0;

                        slot.prompt.tokens.keep_first(n_past);

                        // this is to signal the client that the request has started processing
                        if (slot.task->params.stream) {
                            if (slot.task->params.return_progress) {
                                // send initial 0% progress update if needed
                                send_partial_response(slot, {}, true);
                            } else {
                                // otherwise, for streaming without progress, signal HTTP to send the headers (i.e. 200 status)
                                send_partial_response(slot, {}, false, true);
                            }
                        }
                    } // end of SLOT_STATE_STARTED

                    if (!slot.can_split()) {
                        // cannot fit the prompt in the current batch - will try next iter
                        if (batch.size() + slot.task->n_tokens() > n_batch) {
                            return;
                        }
                    }

                    const int64_t t_now = ggml_time_us();
                    slot.t_prompt_processing = (t_now - slot.t_start_process_prompt) / 1e3;
                    slot.print_timings_pp();

                    // truncate any tokens that are beyond n_past for this slot
                    const llama_pos p0 = slot.prompt.tokens.pos_next();

                    SLT_TRC(slot, "cached n_tokens = %d, memory_seq_rm [%d, end)\n", slot.prompt.n_tokens(), p0);

                    common_context_seq_rm(ctx_tgt, slot.id, p0, -1);
                    if (ctx_dft) {
                        common_context_seq_rm(ctx_dft, slot.id, p0, -1);
                    }

                    // If using an alora, there may be uncached tokens that come
                    // before the invocation sequence. When this happens, the
                    // tokens before the invocation sequence need to be
                    // processed without the adapter in a separate batch, then
                    // the adapter needs to be enabled for the remaining tokens.
                    if (lora_all_alora(slot.lora) && slot.alora_invocation_start - 1 > slot.prompt.n_tokens()) {
                        SLT_DBG(slot, "processing pre-alora tokens without the adapter (n_tokens = %d, alora_invocation_start = %d)\n", slot.prompt.n_tokens(), slot.alora_invocation_start);
                        const auto & enabled_loras = lora_get_enabled_ids(slot.lora);
                        GGML_ASSERT(enabled_loras.size() == 1);
                        alora_scale = slot.lora[enabled_loras[0]].scale;
                        slot.lora[enabled_loras[0]].scale = 0.0f;
                        alora_disabled_id = enabled_loras[0];
                    }

                    bool do_checkpoint = params_base.n_ctx_checkpoints > 0;

                    // make checkpoints only for completion tasks
                    do_checkpoint = do_checkpoint && slot.task->type == SERVER_TASK_TYPE_COMPLETION;

                    // make a checkpoint of the parts of the memory that cannot be rolled back.
                    // checkpoints are created only if:
                    // - the model does not support partial sequence removal
                    // - the model uses SWA (and we are not using `swa_full`)
                    // - the model supports partial sequence removal but only up to a fixed bound
                    do_checkpoint = do_checkpoint && (
                            ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_FULL ||
                            ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_RS ||
                            n_swa > 0);

                    bool has_mtmd = false;

                    // check if we should process the image
                    while (true) {
                        auto cur_token_idx = slot.prompt.n_tokens();
                        if (
                            cur_token_idx >= slot.task->n_tokens() ||
                            input_tokens[cur_token_idx] != LLAMA_TOKEN_NULL // encountered a text token
                        ) {
                            break;
                        }

                        // process the image
                        size_t n_tokens_out = 0;
                        int32_t res = slot.process_mtmd_chunk(cur_token_idx, n_tokens_out);
                        if (res != 0) {
                            SLT_ERR(slot, "failed to process image, res = %d\n", res);
                            send_error(slot, "failed to process image", ERROR_TYPE_SERVER);
                            slot.release();
                            continue;
                        }

                        slot.n_prompt_tokens_processed += n_tokens_out;

                        // add the image chunk to cache
                        {
                            const auto & chunk = input_tokens.find_chunk(cur_token_idx);
                            slot.prompt.tokens.push_back(chunk.get()); // copy
                        }

                        has_mtmd = true;
                    }

                    const auto & spans = slot.task->params.message_spans;
                    const auto last_user_pos = spans.last_user_message_pos();

                    // add prompt tokens for processing in the current batch
                    while (slot.prompt.n_tokens() < slot.task->n_tokens() && batch.size() < n_batch) {
                        // get next token to process
                        llama_token cur_tok = input_tokens[slot.prompt.n_tokens()];
                        if (cur_tok == LLAMA_TOKEN_NULL) {
                            break; // end of text chunk
                        }

                        // if this is an alora request with pre-invocation
                        // tokens that are not cached, we need to stop filling
                        // this batch at those pre-invocation tokens.
                        if (alora_scale > 0 && slot.prompt.n_tokens() == slot.alora_invocation_start - 1) {
                            SLT_DBG(slot, "stop prompt batch filling at (n_tokens = %d, alora_invocation_start = %d)\n", slot.prompt.n_tokens(), slot.alora_invocation_start);
                            break;
                        }

                        // embedding requires all tokens in the batch to be output;
                        // MTP also wants logits at every prompt position so the
                        // streaming hook can mirror t_h_nextn into ctx_dft.
                        add_ok &= batch.add(slot.id,
                            cur_tok,
                            slot.prompt.tokens.pos_next(),
                            slot.need_embd());
                        slot.prompt.tokens.push_back(cur_tok);

                        slot.n_prompt_tokens_processed++;

                        // break at the last user message, or at user messages at least min step past the last checkpoint
                        if (do_checkpoint && spans.is_user_start(slot.prompt.n_tokens())) {
                            const auto pos = slot.prompt.n_tokens();
                            const auto & checkpoints = slot.prompt.checkpoints;

                            if (pos == last_user_pos || checkpoints.empty() || pos > checkpoints.back().n_tokens + params_base.checkpoint_min_step) {
                                break;
                            }
                        }

                        // process the last few tokens of the prompt separately in order to allow for a checkpoint to be created.
                        // create checkpoints that many tokens before the end of the prompt:
                        //  - 4 + n_ubatch
                        //  - 4
                        // ref: https://github.com/ggml-org/llama.cpp/pull/20288
                        if (do_checkpoint) {
                            static const int checkpoint_offsets[] = {4 + n_ubatch, 4};

                            bool should_break = false;
                            for (int offset : checkpoint_offsets) {
                                const int n_last = std::min(n_batch, offset);
                                if (slot.task->n_tokens() == slot.prompt.n_tokens() + n_last) {
                                    should_break = true;
                                    break;
                                }
                            }
                            if (should_break) {
                                break;
                            }
                        }
                    }

                    // the number of tokens added to the batch for the current slot
                    const auto n_tokens_cur = batch.size() - n_tokens_prev;

                    const auto n_tokens_start = slot.prompt.n_tokens() - n_tokens_cur;

                    const bool near_prompt_end = slot.task->n_tokens() < slot.prompt.n_tokens() + n_ubatch;

                    const bool is_user_start = spans.is_user_start(n_tokens_start);
                    const bool is_last_user_message = n_tokens_start == last_user_pos;

                    // entire prompt has been processed
                    if (slot.prompt.n_tokens() == slot.task->n_tokens()) {
                        slot.state = SLOT_STATE_DONE_PROMPT;

                        GGML_ASSERT(batch.size() > 0);

                        // extract the logits only for the last token
                        batch.set_output(batch.size() - 1, true);

                        slot.n_decoded = 0;
                        slot.i_batch   = batch.size() - 1;

                        slot.init_sampler();
                    } else {
                        // skip ordinary mid-prompt checkpoints, unless the batch starts a user
                        // message or we are near the end of the prompt
                        if (!is_user_start && !near_prompt_end) {
                            do_checkpoint = false;
                        }
                    }

                    const auto pos_min = llama_memory_seq_pos_min(llama_get_memory(ctx_tgt), slot.id);
                    const auto pos_max = llama_memory_seq_pos_max(llama_get_memory(ctx_tgt), slot.id);

                    // nothing to checkpoint yet
                    // TODO: is this check needed?
                    if (do_checkpoint && pos_min < 0) {
                        do_checkpoint = false;
                    }

                    // do not checkpoint after mtmd chunks
                    do_checkpoint = do_checkpoint && !has_mtmd;

                    // no need to create checkpoints that are too close together, unless it's the last user message
                    do_checkpoint = do_checkpoint && (slot.prompt.checkpoints.empty() || is_last_user_message || n_tokens_start > slot.prompt.checkpoints.back().n_tokens + params_base.checkpoint_min_step);
                    SLT_DBG(slot, "main/do_checkpoint = %s, pos_min = %d, pos_max = %d\n", do_checkpoint ? "yes" : "no", pos_min, pos_max);

                    // note: we create the checkpoint before calling llama_decode(), so the current batch is not
                    //       yet processed and therefore it is not part of the checkpoint.
                    if (do_checkpoint) {
                        create_checkpoint(slot, n_tokens_cur, pos_min, pos_max);
                    }
                }

                if (!slot_batched) {
                    slot_batched = &slot;
                }
            });
        }
    }

    // returns true = success ; false = retry with smaller batch size
    // throw std::runtime_error on fatal error
    bool decode(int32_t & n_batch, int32_t off, llama_batch & batch_view) {
        SRV_DBG("n_batch (effective) = %d, off = %d\n", n_batch, off);

        auto & slot_batched      = batch.slot_batched;
        auto & alora_scale       = batch.alora_scale;
        auto & alora_disabled_id = batch.alora_disabled_id;

        // TODO @ngxson : alora handling is too messy, need to refactor it to be more clear and maintainable
        if (slot_batched) {
            // apply lora, only need to do it once per batch
            common_set_adapter_lora(ctx_tgt, slot_batched->lora);

            // if the lora is temporarily disabled for an alora, re-enable it
            // for next time
            if (alora_scale > 0.0f) {
                SRV_DBG("re-enabling alora with scale %f\n", alora_scale);
                slot_batched->lora[alora_disabled_id].scale = alora_scale;
            }

            llama_set_embeddings(ctx_tgt, slot_batched->need_embd());
        }

        if (batch.size() == 0) {
            SRV_WRN("%s", "no tokens to decode\n");

            if (++n_empty_consecutive > 3) {
                GGML_ABORT("fatal error - please provide logs and repro in %s\n", "https://github.com/ggml-org/llama.cpp/pull/20277");
            }

            return true; // nothing to decode
        } else {
            n_empty_consecutive = 0;
        }

        // [obrain SPEC-METRICS] la VÉRIFICATION commence ici. Attention : `llama_decode` rend la main
        // après l'ENFILEMENT, pas après l'exécution — mesurer sa seule durée donne 1,73 ms par passe
        // et laisse 75 ms dans le « reste ». L'attente du GPU se paie au premier échantillonnage des
        // logits, donc c'est là que la vérification se clôt (voir le point de clôture plus bas).
        const int64_t t_verify_start = ggml_time_us();
        const int ret = llama_decode(ctx_tgt, batch_view);
        g_obrain_t_verify0 = t_verify_start;
        g_obrain_t_dec1    = ggml_time_us();   // [obrain] fin du decode cible

        {
            for (auto & s : slots) {
                if (s.spec_n_prop_pending > 0) {
                    s.spec_t_verify_pending  = t_verify_start;
                    s.spec_verify_ms_pending = 0.0f;
                }
            }
        }

        metrics.on_decoded(slots);

        if (ret != 0) {
            {
                std::string err;

                if (n_batch == 1 && ret == 1) {
                    // TODO: try to terminate only the largest active slot/sequence and continue with the rest
                    //       need to remove the tokens from the current batch too
                    err = "Context size has been exceeded.";
                }

                if (ret == -1) {
                    err = "Invalid input batch.";
                }

                if (ret < -1) {
                    // TODO: update slot state based on llama_memory_seq_pos_min() and llama_memory_seq_pos_max()
                    err = "Compute error.";
                }

                // TODO: handle ret == 2 (abort) when we start aborting

                if (!err.empty()) {
                    SRV_ERR("%s off = %d, n_batch = %d, ret = %d\n", err.c_str(), off, n_batch, ret);

                    for (auto & slot : slots) {
                        if (slot.is_processing()) {
                            send_error(slot, err);
                            slot.release();

                            // note: it's complicated to keep track of how much of the current batch has been
                            //       processed before the error occurred, so we simply clear the entire context
                            slot.prompt_clear(false);
                        }
                    }

                    // stop, do not retry with smaller batch size
                    throw std::runtime_error(err);
                }
            }

            // retry with half the batch size to try to find a free slot in the KV cache
            if (!try_clear_idle_slots()) {
                n_batch /= 2;
            }

            SRV_WRN("failed to find free space in the KV cache, retrying with smaller batch size, off = %d, n_batch = %d, ret = %d\n", off, n_batch, ret);

            return false; // retry with the updated n_batch
        }

        // TODO: avoid restoring the draft context and re-evaluating the drafted tokens when not needed [TAG_SPEC_AVOID_DRAFT_REEVAL]
        //       for now, always re-evaluate for simplicity
        //       ref: https://github.com/ggml-org/llama.cpp/pull/22728#issuecomment-4400925384
        if (!common_speculative_process(spec.get(), batch_view)) {
            SRV_ERR("%s", "failed to process speculative batch\n");

            // TODO: handle error
            throw std::runtime_error("failed to process speculative batch");
        }

        // handle `n_cmpl > 1` tasks - when the main prompt is processed, activate all child tasks too
        for (auto & slot : slots) {
            if (slot.state == SLOT_STATE_DONE_PROMPT && slot.task->is_parent()) {
                std::vector<server_slot *> children;
                for (auto & other : slots) {
                    if (other.state == SLOT_STATE_WAIT_OTHER && slot.task->id == other.task->id_parent) {
                        children.push_back(&other);
                    }
                }

                // all children slots should already launched by launch_slots_with_parent_task()
                // copy state to the child slots
                for (auto & child : children) {
                    SLT_TRC(slot, " - copying state to child %d\n", child->id);

                    GGML_ASSERT(child->state == SLOT_STATE_WAIT_OTHER);

                    slot.copy_state_to(*child);
                    child->state = SLOT_STATE_DONE_PROMPT;
                }
            }
        }

        return true;
    }

    void post_decode(int32_t n_batch_tokens, int32_t off, llama_batch & batch_view) {
        // for checking if a given batch index is inside batch_view
        auto is_inside_view = [&](int32_t idx) {
            return idx >= off && idx < off + n_batch_tokens;
        };

        // TODO @ngxson : it's tricky to make sub-batch compatible with common_sampler_sample_and_accept_n,
        // so for now we will throw an error in this case: https://github.com/ggml-org/llama.cpp/issues/24840
        iterate(slots, [&](server_slot & slot) {
            for (auto & i : slot.spec_i_batch) {
                if (!is_inside_view(i)) {
                    throw std::runtime_error(string_format("speculative batch index %d is not inside the current sub-batch [%d, %d)", i, off, off + n_batch_tokens));
                }
            }
        });

        auto accept_special_token = [&](server_slot & slot, llama_token token) {
            return params_base.special ||
                slot.task->params.sampling.preserved_tokens.find(token) != slot.task->params.sampling.preserved_tokens.end();
        };

        iterate(slots, [&](server_slot & slot) {
            // optionally send prompt processing progress
            if (slot.state == SLOT_STATE_PROCESSING_PROMPT || slot.state == SLOT_STATE_DONE_PROMPT) {
                if (slot.task->params.stream && slot.task->params.return_progress) {
                    send_partial_response(slot, {}, true);
                }
            }

            if (!is_inside_view(slot.i_batch)) {
                // the required token not in this sub-batch, skip
                return;
            }

            if (slot.state == SLOT_STATE_DONE_PROMPT) {
                if (slot.task->type == SERVER_TASK_TYPE_EMBEDDING) {
                    // prompt evaluated for embedding
                    send_embedding(slot, batch_view);
                    slot.release();
                    slot.i_batch = -1;
                    return;
                }

                if (slot.task->type == SERVER_TASK_TYPE_RERANK) {
                    send_rerank(slot, batch_view);
                    slot.release();
                    slot.i_batch = -1;
                    return;
                }

                GGML_ASSERT(slot.task->need_sampling());

                // prompt evaluated for next-token prediction
                slot.state = SLOT_STATE_GENERATING;

                if (slot.spec_allowed()) {
                    common_speculative_begin(spec.get(), slot.id, slot.prompt.tokens.get_text_tokens());

                    // [obrain J3] seed the n-gram structures with request-provided raw texts.
                    // agnostic by construction: opaque strings, tokenized as plain text.
                    // note: text already present in the prompt is redundant here — begin()
                    // above has just indexed the full prompt. this is for OUT-OF-PROMPT text.
                    for (const auto & text : slot.task->params.spec_seed_texts) {
                        const auto seed_tokens = common_tokenize(slot.ctx_tgt, text, false, false);
                        common_speculative_seed(spec.get(), slot.id, seed_tokens);
                        SLT_INF(slot, "[J3] seeded speculative map with %zu tokens of raw text\n", seed_tokens.size());
                    }
                }
            } else if (slot.state != SLOT_STATE_GENERATING) {
                return;
            }

            if (slot.spec_allowed() && !slot.spec_draft.empty()) {
                return; // sample using speculative decoding
            }

            // shifted according to the current sub-batch
            const int tok_idx = slot.i_batch - off;

            llama_token id;
            {
                scoped_timer timer(t_sampl, n_sampl);
                id = common_sampler_sample(slot.smpl.get(), slot.ctx_tgt, tok_idx);
            }

            // [obrain J2] read-only confidence probe on the freshly sampled candidates
            if (slot.task->params.confidence_probe) {
                probe_confidence(slot, id);
            }

            slot.i_batch = -1;

            common_sampler_accept(slot.smpl.get(), id, true);

            // here we have synchronized the llama_context (due to the sampling above), so we can do time measurement
            const int64_t t_now = ggml_time_us();

            slot.n_decoded += 1;

            if (slot.n_decoded == 1) {
                slot.t_start_generation = t_now;
                slot.t_print_last = t_now;
                slot.n_decoded_last = 0;
                slot.t_prompt_processing = (slot.t_start_generation - slot.t_start_process_prompt) / 1e3;
                metrics.on_prompt_eval(slot);
            }

            slot.t_token_generation = std::max<int64_t>(1, t_now - slot.t_start_generation) / 1e3;

            completion_token_output result;
            result.tok          = id;
            result.text_to_send = common_token_to_piece(slot.ctx_tgt, result.tok, accept_special_token(slot, result.tok));
            result.prob         = 1.0f; // TODO: set it here instead of doing inside populate_token_probs

            if (slot.task->params.sampling.n_probs > 0) {
                populate_token_probs(slot, result, slot.task->params.post_sampling_probs, params_base.special, tok_idx);
            }

            if (!process_token(result, slot)) {
                // release slot because of stop condition
                slot.print_timings();
                send_final_response(slot);
                metrics.on_prediction(slot);
                slot.release();

                return;
            }

            slot.print_timings_tg();
        });

        // speculative decoding - main model sample and accept
        iterate(slots, [&](server_slot & slot) {
            if (slot.state != SLOT_STATE_GENERATING || !slot.spec_allowed() || slot.spec_draft.empty()) {
                return;
            }

            // save the original draft size
            const size_t n_draft = slot.spec_draft.size();

            GGML_ASSERT(n_draft > 0);

            // verify and try to accept the draft
            {
                // save the sampler sampler state in case we need to restore it
                common_sampler_ptr smpl_save(common_sampler_clone(slot.smpl.get()));

                GGML_ASSERT(slot.spec_i_batch.size() == n_draft + 1);
                g_obrain_t_post0 = ggml_time_us();   // [obrain] debut du post (acceptation)
                g_obrain_b        = (int) slot.spec_draft.size() + 1;   // b = P+1
                g_obrain_sampled  = (int) slot.spec_i_batch.size();     // positions echantillonnees
                const int actual_target_batch_b = (int) slot.spec_i_batch.size();   // AVANT le clear
                auto accepted = common_sampler_sample_and_accept_n(slot.smpl.get(), slot.ctx_tgt, slot.spec_i_batch, slot.spec_draft);
                slot.spec_i_batch.clear();

                // [obrain R3-A] SITE 2 -- COMMIT. accepted.size() = A_d + 1 = C : c'est une
                // HYPOTHESE DE RACCORDEMENT, journalisee et verifiee ici, pas une verite gelee.
                // SEULS ces jetons mettent a jour l'etat ; aucun brouillon rejete n'y entre.
                if (slot.obrain_enabled) {
                    const int      nb = (int) slot.spec_draft.size();
                    const int      na = (int) accepted.size();
                    const uint32_t n_rb = (uint32_t) (nb + 1 - na);
                    std::string di, ai;
                    for (auto tk : slot.obrain_draft) di += std::to_string((int) tk) + ",";
                    for (auto tk : accepted)          ai += std::to_string((int) tk) + ",";
                    fprintf(stderr, "OBRAIN_R3A site2 slot=%d request=%lld state_before=%lld "
                                    "spec_draft_size=%d actual_target_batch_b=%d accepted_size=%d n_rollback=%u "
                                    "draft_ids=[%s] accepted_ids=[%s]\n",
                            slot.id, (long long) slot.obrain_st.request_id,
                            (long long) slot.obrain_st.state_token_count(), nb, actual_target_batch_b, na, n_rb,
                            di.c_str(), ai.c_str());
                    fflush(stderr);
                    if ((int) n_rb != nb + 1 - na) { GGML_ABORT("OBRAIN_R3A n_rollback incoherent"); }
                    if (na < 1) { GGML_ABORT("OBRAIN_R3A accepted vide"); }
                    if (!slot.obrain_st.check_invariants("site2 avant update")) {
                        GGML_ABORT("OBRAIN_INVARIANT_KO site2 avant update");
                    }
                    std::vector<int32_t> acc;
                    acc.reserve(na);
                    for (auto tk : accepted) acc.push_back((int32_t) tk);
                    slot.obrain_st.sum_A_d   += (na - 1);
                    slot.obrain_st.sum_P_eff += (int) slot.obrain_draft.size();
                    // EXCLUSIVITE : `accepted` inclut le JETON BONUS (accepted.back()), qui entre par la
                    // voie NORMALE (process_token). On ne committe donc ici que les A_d jetons acceptes.
                    acc.pop_back();
                    for (int k = 0; k < (int) acc.size(); ++k) {
                        slot.obrain_st.obrain_count_output((int) acc[k], "OUTPUT_SPEC", 0, 0,
                                                         (int64_t) slot.id, slot.obrain_generation);
                    }
                    slot.obrain_st.obrain_commit(acc, obrain_proposer_state::OBRAIN_SRC_SPEC);
                    // [R3-A5] idem : le ledger est la seule source des evenements OUTPUT
                    if (!slot.obrain_st.check_invariants("site2 apres update")) {
                        GGML_ABORT("OBRAIN_INVARIANT_KO site2 apres update");
                    }
                    fprintf(stderr, "OBRAIN_R3A site2ok slot=%d decision_index=%lld "
                                    "committed_updates=%lld state_after=%lld output_tokens=%lld "
                                    "rejected_draft_updates=%lld\n",
                            slot.id, (long long) slot.obrain_st.decision_count,
                            (long long) slot.obrain_st.committed_updates,
                            (long long) slot.obrain_st.state_token_count(),
                            (long long) slot.obrain_output_tokens,
                            (long long) slot.obrain_st.rejected_draft_updates);
                    fflush(stderr);
                }

                // [obrain SPEC-METRICS] CLÔTURE DE LA VÉRIFICATION. C'est ici que le GPU est attendu
                // (lecture des logits) : la durée couvre donc le decode de vérification ET son
                // attente, ce qui est le vrai coût de la vérification pour le décideur.
                if (slot.spec_t_verify_pending > 0) {
                    slot.spec_verify_ms_pending = (float) (ggml_time_us() - slot.spec_t_verify_pending) / 1000.0f;
                    slot.spec_t_verify_pending  = 0;
                }
                // [obrain] DECOMPOSITION DU VERIFY LIVE : live_total = decode + entre + post.
                if (g_obrain_t_verify0 > 0) {
                    const int64_t obrain_t_post1 = ggml_time_us();
                    const double obrain_dec  = (double) (g_obrain_t_dec1  - g_obrain_t_verify0) / 1000.0;
                    const double obrain_btw  = (double) (g_obrain_t_post0 - g_obrain_t_dec1)    / 1000.0;
                    const double obrain_post = (double) (obrain_t_post1  - g_obrain_t_post0)   / 1000.0;
                    SLT_INF(slot, "OBRAIN_SPEC b=%d proposed=%d accepted_draft=%d committed=%d "
                                  "sampled_positions=%d live_total_ms=%.3f decode_ms=%.3f between_ms=%.3f "
                                  "post_ms=%.3f residual_ms=%.3f\n",
                            g_obrain_b,                              // b = P+1 (capture avant consommation)
                            (int) slot.spec_draft.size(),            // P = brouillons proposes
                            (int) accepted.size() - 1,               // A_d = brouillons acceptes
                            (int) accepted.size(),                   // C = jetons commis = A_d+1
                            g_obrain_sampled,
                            obrain_dec + obrain_btw + obrain_post, obrain_dec, obrain_btw, obrain_post,
                            obrain_btw);
                    g_obrain_t_verify0 = 0;
                }

                GGML_ASSERT(accepted.size() >= 1);

                const uint32_t n_rollback = slot.spec_draft.size() + 1 - accepted.size();

                const bool use_ckpt_tgt =
                    ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_FULL ||
                    (ctx_tgt_seq_rm_type == COMMON_CONTEXT_SEQ_RM_TYPE_RS && n_rollback > llama_n_rs_seq(ctx_tgt));

                // check for partial draft acceptance
                if (n_rollback > 0) {
                    if (use_ckpt_tgt) {
                        if (trace > 0) {
                            SLT_INF(slot, "accepted %2zu/%2zu draft tokens (restore checkpoint)\n", accepted.size() - 1, slot.spec_draft.size());
                        }

                        // partial acceptance is not supported by the context -> truncate the draft and restore the state
                        slot.spec_draft = std::move(accepted);

                        const auto & ckpt = slot.spec_ckpt;

                        SLT_DBG(slot, "restoring speculative checkpoint (pos_min = %d, pos_max = %d, size = %zu)\n", ckpt.pos_min, ckpt.pos_max, ckpt.size());

                        {
                            ckpt.load_tgt(slot.ctx_tgt, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);

                            common_context_seq_rm(slot.ctx_tgt, slot.id, ckpt.pos_max + 1, -1);
                        }

                        if (slot.ctx_dft) {
                            ckpt.load_dft(slot.ctx_dft, slot.id, LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);

                            common_context_seq_rm(slot.ctx_dft, slot.id, ckpt.pos_max + 1, -1);
                        }

                        slot.prompt.tokens.keep_first(ckpt.n_tokens);
                        slot.smpl = std::move(smpl_save);

                        return;
                    }
                }

                if (trace > 0) {
                    SLT_INF(slot, "accepted %2zu/%2zu draft tokens\n", accepted.size() - 1, n_draft);
                }

                common_speculative_accept(spec.get(), slot.id, accepted.size() - 1);

                slot.spec_draft = std::move(accepted);
            }

            const int64_t t_now = ggml_time_us();

            const auto ids = std::move(slot.spec_draft);

            slot.t_token_generation = std::max<int64_t>(1, t_now - slot.t_start_generation) / 1e3;

            // update how many tokens out of those tested were accepted
            slot.n_draft_accepted += ids.size() - 1;
            slot.n_draft_verif_steps += 1;

            // [obrain SPEC-METRICS] clôture de la passe : une longueur acceptée de plus dans la
            // distribution, et la décomposition brouillon / vérification / reste de CETTE passe.
            if (slot.spec_n_prop_pending > 0) {
                server_slot::spec_pass_stat st;
                st.n_prop    = slot.spec_n_prop_pending;
                st.n_acc     = (int32_t) ids.size() - 1;
                st.draft_ms  = slot.spec_draft_ms_pending;
                st.verify_ms = slot.spec_verify_ms_pending;
                st.round_ms  = slot.spec_t_round_pending > 0
                             ? (float) (t_now - slot.spec_t_round_pending) / 1000.0f
                             : 0.0f;
                slot.spec_passes.push_back(st);

                slot.spec_n_prop_pending    = 0;
                slot.spec_t_round_pending   = 0;
                slot.spec_draft_ms_pending  = 0.0f;
                slot.spec_verify_ms_pending = 0.0f;
            }

            if (slot.n_accepted_per_pos.empty()) {
                slot.n_accepted_per_pos.resize(common_speculative_n_max(&params_base.speculative), 0);
            }
            for (size_t i = 0; i < ids.size() - 1 && i < slot.n_accepted_per_pos.size(); ++i) {
                slot.n_accepted_per_pos[i]++;
            }

            // add accepted tokens to the prompt
            slot.prompt.tokens.keep_first(slot.prompt.n_tokens() - n_draft);
            slot.prompt.tokens.insert({ids.begin(), ids.end() - 1});

            slot.sampled = ids.back(); // last accepted token
            SLT_DBG(slot, "add accepted tokens: sampled=%d, ids.size=%zu, n_draft=%zu\n", slot.sampled, ids.size(), n_draft);

            common_context_seq_rm(slot.ctx_tgt, slot.id, slot.prompt.tokens.pos_next(), -1);
            if (slot.ctx_dft) {
                common_context_seq_rm(slot.ctx_dft, slot.id, slot.prompt.tokens.pos_next(), -1);
            }

            for (size_t i = 0; i < ids.size(); ++i) {
                completion_token_output result;

                result.tok          = ids[i];
                result.text_to_send = common_token_to_piece(slot.ctx_tgt, result.tok, accept_special_token(slot, result.tok));
                result.prob         = 1.0f; // set later

                // TODO: set result.probs

                slot.n_decoded += 1;

                if (!process_token(result, slot)) {
                    slot.print_timings();
                    send_final_response(slot);
                    metrics.on_prediction(slot);
                    slot.release();

                    return;
                }
            }

            slot.print_timings_tg();

            SLT_DBG(slot, "accepted %d/%d draft tokens, new n_tokens = %d\n", (int) ids.size() - 1, (int) n_draft, slot.prompt.n_tokens());
        });
    }

    int get_slot_n_ctx() {
        return slots.back().n_ctx;
    }

    server_response_reader get_response_reader() {
        return server_response_reader(queue_tasks, queue_results, HTTP_POLLING_SECONDS);
    }
};

//
// server_context (public API)
//

server_context::server_context() : impl(new server_context_impl()) {}
server_context::~server_context() = default;

bool server_context::load_model(common_params & params) {
    return impl->load_model(params);
}

void server_context::start_loop() {
    auto & params = impl->params_base;
    impl->queue_tasks.start_loop(params.sleep_idle_seconds * 1000);
}

void server_context::terminate() {
    impl->queue_tasks.terminate();
}

llama_context * server_context::get_llama_context() const {
    return impl->ctx_tgt;
}

server_response_reader server_context::get_response_reader() {
    return impl->get_response_reader();
}

server_context_meta server_context::get_meta() const {
    auto bos_id = llama_vocab_bos(impl->vocab);
    auto eos_id = llama_vocab_eos(impl->vocab);
    auto bos_token_str = bos_id != LLAMA_TOKEN_NULL ? common_token_to_piece(impl->ctx_tgt, bos_id, true) : "";
    auto eos_token_str = eos_id != LLAMA_TOKEN_NULL ? common_token_to_piece(impl->ctx_tgt, eos_id, true) : "";

    const char * ftype_name = llama_ftype_name(llama_model_ftype(impl->model_tgt));

    return server_context_meta {
        /* build_info             */ std::string(llama_build_info()),
        /* model_name             */ impl->model_name,
        /* model_aliases          */ impl->model_aliases,
        /* model_tags             */ impl->model_tags,
        /* model_path             */ impl->params_base.model.path,
        /* has_mtmd               */ impl->mctx != nullptr,
        /* has_inp_image          */ impl->chat_params.allow_image,
        /* has_inp_audio          */ impl->chat_params.allow_audio,
        /* has_inp_video          */ impl->chat_params.allow_video,
        /* json_ui_settings       */ impl->json_ui_settings,
        /* slot_n_ctx             */ impl->get_slot_n_ctx(),
        /* pooling_type           */ llama_pooling_type(impl->ctx_tgt),

        /* chat_params            */ impl->chat_params,
        /* chat_template_caps     */ common_chat_templates_get_caps(impl->chat_params.tmpls.get()),

        /* bos_token_str          */ bos_token_str,
        /* eos_token_str          */ eos_token_str,
        /* fim_pre_token          */ llama_vocab_fim_pre(impl->vocab),
        /* fim_sub_token          */ llama_vocab_fim_suf(impl->vocab),
        /* fim_mid_token          */ llama_vocab_fim_mid(impl->vocab),
        /* fim_pad_token          */ llama_vocab_fim_pad(impl->vocab),
        /* fim_rep_token          */ llama_vocab_fim_rep(impl->vocab),
        /* fim_sep_token          */ llama_vocab_fim_sep(impl->vocab),

        /* logit_bias_eog         */ impl->params_base.sampling.logit_bias_eog,

        /* model_vocab_type       */ llama_vocab_type(impl->vocab),
        /* model_vocab_n_tokens   */ llama_vocab_n_tokens(impl->vocab),
        /* model_n_ctx_train      */ llama_model_n_ctx_train(impl->model_tgt),
        /* model_n_embd_inp       */ llama_model_n_embd(impl->model_tgt),
        /* model_n_params         */ llama_model_n_params(impl->model_tgt),
        /* model_size             */ llama_model_size(impl->model_tgt),
        /* model_ftype            */ ftype_name,
    };
}

// generator-like API for HTTP response generation
// may have bypass_sleep = true if the task does not use ctx_server
struct server_res_generator : server_res_spipe {
    server_response_reader rd;
    server_res_generator(server_queue & queue_tasks, server_response & queue_results, int sleep_idle_seconds, bool bypass_sleep = false)
            : rd(queue_tasks, queue_results, HTTP_POLLING_SECONDS) {
        // fast path in case sleeping is disabled
        bypass_sleep |= sleep_idle_seconds < 0;
        if (!bypass_sleep) {
            queue_tasks.wait_until_no_sleep();
        }
    }
    void ok(const json & response_data) {
        status = 200;
        data = safe_json_to_str(response_data);
    }
    void error(const json & error_data) {
        status = json_value(error_data, "code", 500);
        data = safe_json_to_str({{ "error", error_data }});
    }
};

void server_context::set_state_callback(server_state_callback_t callback) {
    impl->callback_state = std::move(callback);
    impl->queue_tasks.on_sleeping_state([this](bool sleeping) {
        if (sleeping) {
            impl->callback_state(SERVER_STATE_SLEEPING, {});
        }
        // for sleeping == false, event is emitted by load_model()
    });
}

//
// server_routes
//

std::unique_ptr<server_res_generator> server_routes::handle_completions_impl(
            const server_http_req & req,
            server_task_type type,
            const json & data,
            const std::vector<raw_buffer> & files,
            task_response_type res_type) {
    GGML_ASSERT(type == SERVER_TASK_TYPE_COMPLETION || type == SERVER_TASK_TYPE_INFILL);

    auto res = create_response();
    auto completion_id = gen_chatcmplid();
    auto & rd = res->rd;
    auto & params = this->params;

    res->set_req(&req); // will also set spipe if needed

    int32_t sse_ping_interval = params.sse_ping_interval;

    try {
        std::vector<server_task> tasks;

        const auto & prompt = data.at("prompt");
        // TODO: this log can become very long, put it behind a flag or think about a more compact format
        //SRV_DBG("Prompt: %s\n", prompt.is_string() ? prompt.get<std::string>().c_str() : prompt.dump(2).c_str());

        if (!params.path_prompts_log_dir.empty()) {
            const auto file_path = std::filesystem::path(params.path_prompts_log_dir) / string_format("%012" PRId64 ".txt", ggml_time_ms());
            std::ofstream f(file_path);
            if (f) {
                f << (prompt.is_string() ? prompt.get<std::string>().c_str() : prompt.dump(2).c_str());
            } else {
                SRV_ERR("failed to create %s\n", file_path.string().c_str());
            }
        }

        // process prompt
        std::vector<server_tokens> inputs;

        if (res_type != TASK_RESPONSE_TYPE_NONE && ctx_server.mctx != nullptr) {
            // This is the case used by OAI compatible chat path with MTMD. TODO It can be moved to the path below.
            inputs.push_back(process_mtmd_prompt(ctx_server.mctx, prompt.get<std::string>(), files));
        } else {
            // Everything else, including multimodal completions.
            inputs = tokenize_input_prompts(ctx_server.vocab, ctx_server.mctx, prompt, true, true);
        }

        // ═══════════════════════════════════════════════════════════════════════
        // [obrain SPLICE-CHAT] `state_ref` OPTIONNEL — marche 2 : le flux de chat
        // peut demarrer sa generation APRES un etat KV spliCe.
        //
        // REGLE N.1 : si `state_ref` est absent ou vide, RIEN de ce bloc ne
        // s'execute — pas de lecture de cache, pas d'I/O, pas de branche
        // supplementaire dans la boucle de taches. Le chemin historique est
        // inchange.
        //
        // La ref est resolue ICI (couche HTTP) et pas dans le slot, pour trois
        // raisons : (a) un `state_ref` inconnu doit produire un HTTP 400 SEC,
        // AVANT toute mise en file ; (b) le prefixe doit etre CONNU a la
        // construction de la tache puisque ses IDs sont prependus aux tokens ;
        // (c) le travail disque + SHA256 ne doit pas bloquer le thread
        // ordonnanceur qui sert tous les slots.
        // ═══════════════════════════════════════════════════════════════════════
        std::string state_ref = json_value(data, "state_ref", std::string());
        std::shared_ptr<const kv_state_blob> state_blob;
        if (!state_ref.empty()) {
            std::string why;
            if (!kv_state_resolve(state_ref, DEFAULT_KV_STATES_DIR, state_blob, why)) {
                res->error(format_error_response(why, ERROR_TYPE_INVALID_REQUEST));
                return res;
            }

            // Le prefixe doit etre connu EN ENTIER : ses IDs deviennent les
            // tokens [0, n) de la tache, et les positions du KV importe sont
            // [0, n). Un manifeste tronque (extraction J-space plafonnee a 4096
            // IDs) est donc REFUSE ici, explicitement, plutot que de produire un
            // prompt decale en silence.
            const obrain_kv::Manifest & man = *state_blob->manifest;
            if (man.prefix_ids.empty() || (int32_t) man.prefix_ids.size() != man.n_tokens) {
                res->error(format_error_response(
                        "state_ref '" + state_ref + "' : prefixe INCOMPLET dans le manifeste (" +
                        std::to_string(man.prefix_ids.size()) + " IDs pour " + std::to_string(man.n_tokens) +
                        " tokens) — re-extraire un fait de <= 4096 tokens",
                        ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
            // Le prefixe doit occuper EXACTEMENT les positions [0, n_tokens) :
            // c'est la seule hypothese que fait le splice (le prompt reprend a la
            // position n_tokens). Un etat dont les positions seraient decalees ou
            // trouees produirait un prompt place au mauvais endroit.
            if (man.pos_min != 0 || man.pos_max + 1 != (int64_t) man.n_tokens) {
                res->error(format_error_response(
                        "state_ref '" + state_ref + "' : positions non contigues depuis 0 (pos_min=" +
                        std::to_string(man.pos_min) + ", pos_max=" + std::to_string(man.pos_max) +
                        ", n_tokens=" + std::to_string(man.n_tokens) + ") — un splice exige des positions [0, n_tokens) : "
                        "re-extraire l'etat avec cette version du serveur",
                        ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
        }

        // tasks.reserve(inputs.size()); // TODO: this is inaccurate due to child tasks

        // message delimiters for checkpointing
        auto delimiters = common_chat_msg_delimiters_parse(json_value(data, "message_delimiters", json::array()));
        delimiters.tokenize(ctx_server.vocab);

        for (size_t i = 0; i < inputs.size(); i++) {
            server_task task = server_task(type);

            task.id = rd.get_new_id();

            task.tokens = std::move(inputs[i]);

            // [obrain SPLICE-CHAT] LE PREFIXE IMPORTE DEVIENT LE PREFIXE DE LA TACHE.
            //
            // POURQUOI PREPENDRE PLUTOT QUE TENIR UN DECALAGE : tout le serveur
            // raisonne sur `slot.prompt.tokens[i] <-> task.tokens[i] <-> position i`
            // (boucle de prefill, spans de messages, checkpoints, troncature,
            // echantillonneur). Ajouter un « prefixe fantome » avec un decalage
            // aurait demande de corriger CHACUN de ces sites ; prepend les IDs
            // reels du fait laisse la regle VRAIE et le prefixe se comporte
            // exactement comme un texte de fait present dans le prompt.
            //
            // Le prefixe n'est PAS re-decode : le slot importe l'etat KV (voir
            // update_slots) et reprend le prefill a la position n_state_prefix.
            if (state_blob) {
                if (task.tokens.has_mtmd) {
                    throw std::runtime_error("state_ref n'est pas compatible avec un prompt multimodal "
                                             "(le prefixe KV ne peut pas precede des chunks media)");
                }
                const obrain_kv::Manifest & man = *state_blob->manifest;

                llama_tokens spliced;
                spliced.reserve(man.prefix_ids.size() + task.tokens.size());
                for (int32_t tid : man.prefix_ids) {
                    spliced.push_back((llama_token) tid);
                }
                {
                    const auto & cur = task.tokens.get_tokens();
                    spliced.insert(spliced.end(), cur.begin(), cur.end());
                }
                task.tokens = server_tokens(spliced, false);

                task.state_blob     = state_blob;
                task.state_ref      = state_ref;
                task.n_state_prefix = (int32_t) man.n_tokens;
            }

            task.params = server_schema::eval_llama_cmpl_schema(
                    ctx_server.vocab,
                    params,
                    meta->slot_n_ctx,
                    meta->logit_bias_eog,
                    data);

            task.params.message_spans = task.tokens.find_message_spans(delimiters);

            task.id_slot = json_value(data, "id_slot", -1);
            // [obrain P1] IDENTITE EXPLICITE DU CLIENT (facultative).
            //
            // POURQUOI CE CHAMP EXISTE ALORS QUE `id_slot` SUFFIT DEJA :
            // `id_slot = hash(cid) % total_slots` est une identite HACHEE, donc
            // deux conversations peuvent y entrer en collision -- le serveur ne
            // peut alors pas les distinguer, et elles se partagent une sequence.
            // Un client qui envoie son cid donne au serveur de quoi attribuer une
            // sequence DISTINCTE par conversation. Le champ est FACULTATIF : sans
            // lui, le comportement reste celui d'aujourd'hui (identite derivee du
            // `id_slot`), donc aucun client existant n'a besoin d'etre modifie.
            task.client_id = json_value(data, "client_id", std::string());
            sse_ping_interval = task.params.sse_ping_interval;

            // OAI-compat
            task.params.res_type          = res_type;
            task.params.oaicompat_cmpl_id = completion_id;
            task.params.oaicompat_model   = meta->model_name;

            // prepare child tasks
            if (task.params.n_cmpl > 1) {
                int n_children = task.params.n_cmpl - 1;
                for (int j = 0; j < n_children; j++) {
                    task.add_child(task.id, rd.get_new_id());
                }
            }

            tasks.push_back(std::move(task));
        }

        rd.post_tasks(std::move(tasks));
    } catch (const std::exception & e) {
        res->error(format_error_response(e.what(), ERROR_TYPE_INVALID_REQUEST));
        return res;
    }

    bool stream = json_value(data, "stream", false);

    if (!stream) {
        // non-stream, wait for the results
        auto all_results = rd.wait_for_all(req.should_stop);
        if (all_results.is_terminated) {
            return res; // connection is closed
        } else if (all_results.error) {
            res->error(all_results.error->to_json());
            return res;
        } else {
            json arr = json::array();
            for (auto & res : all_results.results) {
                GGML_ASSERT(dynamic_cast<server_task_result_cmpl_final*>(res.get()) != nullptr);
                arr.push_back(res->to_json());
            }
            GGML_ASSERT(!arr.empty() && "empty results");
            if (arr.size() == 1) {
                // if single request, return single object instead of array
                res->ok(arr[0]);
            } else if (res_type == TASK_RESPONSE_TYPE_OAI_CHAT || res_type == TASK_RESPONSE_TYPE_OAI_CMPL) {
                // if multiple results in OAI format, we need to re-format them
                json & choices = arr[0]["choices"];
                for (size_t i = 1; i < arr.size(); i++) {
                    choices.push_back(std::move(arr[i]["choices"][0]));
                }
                res->ok(arr[0]);
            } else {
                // multi-results, non-OAI compat
                res->ok(arr);
            }
        }
    } else {
        // in streaming mode, the first error must be treated as non-stream response
        // this is to match the OAI API behavior
        // ref: https://github.com/ggml-org/llama.cpp/pull/16486#discussion_r2419657309
        auto first_result = rd.next(req.should_stop);
        if (first_result == nullptr) {
            GGML_ASSERT(req.should_stop());
            return res; // connection is closed
        }

        if (first_result->is_error()) {
            res->error(first_result->to_json());
            return res;
        }

        GGML_ASSERT(
            dynamic_cast<server_task_result_cmpl_partial*>(first_result.get()) != nullptr ||
            dynamic_cast<server_task_result_cmpl_final*>  (first_result.get()) != nullptr
        );

        // next responses are streamed
        // to be sent immediately
        json first_result_json = first_result->to_json();
        if (first_result_json == nullptr) {
            res->data = ""; // simply send HTTP headers and status code
        } else if (res_type == TASK_RESPONSE_TYPE_ANTHROPIC) {
            res->data = format_anthropic_sse(first_result_json);
        } else if (res_type == TASK_RESPONSE_TYPE_OAI_RESP) {
            res->data = format_oai_resp_sse(first_result_json);
        } else {
            res->data = format_oai_sse(first_result_json);
        }
        res->status = 200;
        res->content_type = "text/event-stream";
        res->set_next([res_this = res.get(), res_type, sse_ping_interval](std::string & output) -> bool {
            static auto format_error = [](task_response_type res_type, const json & res_json) {
                if (res_type == TASK_RESPONSE_TYPE_ANTHROPIC) {
                    return format_anthropic_sse({
                        {"event", "error"},
                        {"data", res_json},
                    });
                } else {
                    return format_oai_sse(json {{ "error", res_json }});
                }
            };

            auto effective_should_stop = [&res_this]() {
                return res_this->should_stop();
            };

            try {
                if (effective_should_stop()) {
                    SRV_DBG("%s", "stopping streaming due to should_stop condition\n");
                    return false; // should_stop condition met
                }

                if (!res_this->data.empty()) {
                    // flush the first chunk
                    output = std::move(res_this->data);
                    res_this->data.clear();
                    return true;
                }

                server_response_reader & rd = res_this->rd;

                // check if there is more data
                if (!rd.has_next()) {
                    switch (res_type) {
                        case TASK_RESPONSE_TYPE_NONE:
                        case TASK_RESPONSE_TYPE_OAI_RESP:
                        case TASK_RESPONSE_TYPE_ANTHROPIC:
                            output = "";
                            break;

                        default:
                            output = "data: [DONE]\n\n";
                            break;
                    }
                    SRV_DBG("%s", "all results received, terminating stream\n");
                    return false; // no more data, terminate
                }

                // receive subsequent results
                bool timeout = false;
                int64_t start_time = ggml_time_ms();
                auto result = rd.next([&timeout, &start_time, sse_ping_interval, &effective_should_stop]() {
                    if (effective_should_stop()) {
                        return true; // should_stop condition met
                    } else if (sse_ping_interval > 0 && ggml_time_ms() - start_time > (int64_t)sse_ping_interval * 1000) {
                        timeout = true;
                        return true; // timeout
                    }
                    return false;
                });

                if (timeout) {
                    // some clients may time out (e.g. undici) will time out if no data is received for a while, so we need to send a ping to keep the connection alive
                    SRV_DBG("%s", "sending SSE ping\n");
                    output = ":\n\n";
                    return true;
                }

                if (result == nullptr) {
                    SRV_DBG("%s", "stopping streaming due to should_stop condition\n");
                    GGML_ASSERT(effective_should_stop());
                    return false; // should_stop condition met
                }

                // send the results
                if (result->is_error()) {
                    json res_json = result->to_json();
                    output = format_error(res_type, res_json);
                    SRV_DBG("%s", "error received during streaming, terminating stream\n");
                    return false; // terminate on error
                } else {
                    GGML_ASSERT(
                        dynamic_cast<server_task_result_cmpl_partial*>(result.get()) != nullptr
                        || dynamic_cast<server_task_result_cmpl_final*>(result.get()) != nullptr
                    );
                    json res_json = result->to_json();
                    if (res_type == TASK_RESPONSE_TYPE_ANTHROPIC) {
                        output = format_anthropic_sse(res_json);
                    } else if (res_type == TASK_RESPONSE_TYPE_OAI_RESP) {
                        output = format_oai_resp_sse(res_json);
                    } else {
                        output = format_oai_sse(res_json);
                    }
                }

                // has next data, continue
                return true;

            } catch (const std::exception & e) {
                json error_json = format_error_response(e.what(), ERROR_TYPE_SERVER);
                output = format_error(res_type, error_json);

                // terminate on exception
                return false;
            }
        });
    }

    return res;
}

std::unique_ptr<server_res_generator> server_routes::create_response(bool bypass_sleep) {
    return std::make_unique<server_res_generator>(queue_tasks, queue_results, params.sleep_idle_seconds, bypass_sleep);
}

server_routes::server_routes(const common_params & params, server_context & ctx_server)
        : params(params),
          ctx_server(*ctx_server.impl),
          queue_tasks(ctx_server.impl->queue_tasks),
          queue_results(ctx_server.impl->queue_results) {
    init_routes();
}

void server_routes::init_routes() {
    // IMPORTANT: all lambda functions must start with create_response()
    // this is to ensure that the server_res_generator can handle sleeping case correctly

    this->get_health = [this](const server_http_req &) {
        // error and loading states are handled by middleware
        auto res = create_response(true);

        // this endpoint can be accessed during sleeping
        // the next LOC is to avoid someone accidentally use ctx_server
        bool ctx_server; // do NOT delete this line
        GGML_UNUSED(ctx_server);

        res->ok({{"status", "ok"}});
        return res;
    };

    this->get_metrics = [this](const server_http_req & req) {
        auto res = create_response();
        if (!params.endpoint_metrics) {
            res->error(format_error_response("This server does not support metrics endpoint. Start it with `--metrics`", ERROR_TYPE_NOT_SUPPORTED));
            return res;
        }

        // request slots data using task queue
        {
            server_task task(SERVER_TASK_TYPE_METRICS);
            task.id = res->rd.get_new_id();
            res->rd.post_task(std::move(task), true); // high-priority task
        }

        // get the result
        auto result = res->rd.next(req.should_stop);
        if (!result) {
            // connection was closed
            GGML_ASSERT(req.should_stop());
            return res;
        }

        if (result->is_error()) {
            res->error(result->to_json());
            return res;
        }

        // TODO: get rid of this dynamic_cast
        auto res_task = dynamic_cast<server_task_result_metrics*>(result.get());
        GGML_ASSERT(res_task != nullptr);

        // metrics definition: https://prometheus.io/docs/practices/naming/#metric-names
        json all_metrics_def = json {
            {"counter", {{
                    {"name",  "prompt_tokens_total"},
                    {"help",  "Number of prompt tokens processed."},
                    {"value",  (uint64_t) res_task->n_prompt_tokens_processed_total}
            }, {
                    {"name",  "prompt_seconds_total"},
                    {"help",  "Prompt process time"},
                    {"value",  (uint64_t) res_task->t_prompt_processing_total / 1.e3}
            }, {
                    {"name",  "tokens_predicted_total"},
                    {"help",  "Number of generation tokens processed."},
                    {"value",  (uint64_t) res_task->n_tokens_predicted_total}
            }, {
                    {"name",  "tokens_predicted_seconds_total"},
                    {"help",  "Predict process time"},
                    {"value",  (uint64_t) res_task->t_tokens_generation_total / 1.e3}
            }, {
                    {"name",  "n_decode_total"},
                    {"help",  "Total number of llama_decode() calls"},
                    {"value",  res_task->n_decode_total}
            }, {
                    {"name",  "n_tokens_max"},
                    {"help",  "Largest observed n_tokens."},
                    {"value",  res_task->n_tokens_max}
            }}},
            {"gauge", {{
                    {"name",  "prompt_tokens_seconds"},
                    {"help",  "Average prompt throughput in tokens/s."},
                    {"value",  res_task->n_prompt_tokens_processed ? 1.e3 / res_task->t_prompt_processing * res_task->n_prompt_tokens_processed : 0.}
            },{
                    {"name",  "predicted_tokens_seconds"},
                    {"help",  "Average generation throughput in tokens/s."},
                    {"value",  res_task->n_tokens_predicted ? 1.e3 / res_task->t_tokens_generation * res_task->n_tokens_predicted : 0.}
            },{
                    {"name",  "requests_processing"},
                    {"help",  "Number of requests processing."},
                    {"value",  (uint64_t) res_task->n_processing_slots}
            },{
                    {"name",  "requests_deferred"},
                    {"help",  "Number of requests deferred."},
                    {"value",  (uint64_t) res_task->n_tasks_deferred}
            },{
                    {"name",  "n_busy_slots_per_decode"},
                    {"help",  "Average number of busy slots per llama_decode() call"},
                    {"value",  (float) res_task->n_busy_slots_total / std::max((float) res_task->n_decode_total, 1.f)}
            }}}
        };

        std::stringstream prometheus;

        for (const auto & el : all_metrics_def.items()) {
            const auto & type        = el.key();
            const auto & metrics_def = el.value();

            for (const auto & metric_def : metrics_def) {
                const std::string name = metric_def.at("name");
                const std::string help = metric_def.at("help");

                auto value = json_value(metric_def, "value", 0.);
                prometheus << "# HELP llamacpp:" << name << " " << help  << "\n"
                            << "# TYPE llamacpp:" << name << " " << type  << "\n"
                            << "llamacpp:"        << name << " " << value << "\n";
            }
        }

        res->headers["Process-Start-Time-Unix"] = std::to_string(res_task->t_start);
        res->content_type = "text/plain; version=0.0.4";
        res->status = 200;
        res->data = prometheus.str();
        return res;
    };

    this->get_slots = [this](const server_http_req & req) {
        auto res = create_response();
        if (!params.endpoint_slots) {
            res->error(format_error_response("This server does not support slots endpoint. Start it with `--slots`", ERROR_TYPE_NOT_SUPPORTED));
            return res;
        }

        // request slots data using task queue
        {
            server_task task(SERVER_TASK_TYPE_METRICS);
            task.id = res->rd.get_new_id();
            res->rd.post_task(std::move(task), true); // high-priority task
        }

        // get the result
        auto result = res->rd.next(req.should_stop);
        if (!result) {
            // connection was closed
            GGML_ASSERT(req.should_stop());
            return res;
        }

        if (result->is_error()) {
            res->error(result->to_json());
            return res;
        }

        // TODO: get rid of this dynamic_cast
        auto * res_task = dynamic_cast<server_task_result_metrics*>(result.get());
        GGML_ASSERT(res_task != nullptr);

        // optionally return "fail_on_no_slot" error
        if (!req.get_param("fail_on_no_slot").empty()) {
            if (res_task->n_idle_slots == 0) {
                res->error(format_error_response("no slot available", ERROR_TYPE_UNAVAILABLE));
                return res;
            }
        }

        res->ok(res_task->slots_data);
        return res;
    };

    this->post_slots = [this](const server_http_req & req) {
        auto res = create_response();

        std::string id_slot_str = req.get_param("id_slot");

        int id_slot;
        try {
            id_slot = std::stoi(id_slot_str);
        } catch (const std::exception &) {
            res->error(format_error_response("Invalid slot ID", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        std::string action = req.get_param("action");

        // [obrain LONG-RESUME] compaction/info — no --slot-save-path required
        if (action == "compact" || action == "info") {
            return handle_slots_compact(req, id_slot, action == "compact");
        }

        if (params.slot_save_path.empty()) {
            res->error(format_error_response("This server does not support slots action. Start it with `--slot-save-path`", ERROR_TYPE_NOT_SUPPORTED));
            return res;
        }

        if (action == "save") {
            return handle_slots_save(req, id_slot);
        }
        if (action == "restore") {
            return handle_slots_restore(req, id_slot);
        }
        if (action == "erase") {
            return handle_slots_erase(req, id_slot);
        }

        res->error(format_error_response("Invalid action", ERROR_TYPE_INVALID_REQUEST));
        return res;
    };

    this->get_props = [this](const server_http_req &) {
        auto res = create_response(true);

        // this endpoint can be accessed during sleeping
        // the next LOC is to avoid someone accidentally use ctx_server
        bool ctx_server; // do NOT delete this line
        GGML_UNUSED(ctx_server);

        task_params tparams;
        tparams.sampling = params.sampling;
        json default_generation_settings_for_props = json {
            { "params", tparams.to_json(true) },
            { "n_ctx",  meta->slot_n_ctx },
        };

        std::string tmpl_default = common_chat_templates_source(meta->chat_params.tmpls.get(), "");
        std::string tmpl_tools   = common_chat_templates_source(meta->chat_params.tmpls.get(), "tool_use");

        json props = {
            { "default_generation_settings", default_generation_settings_for_props },
            // [obrain P1] LE POOL REELLEMENT OFFERT, PAS n_parallel.
            //
            // POURQUOI CE CHIFFRE CHANGE QUELQUE CHOSE : le client derive son
            // `id_slot` de ce nombre (`hash(cid) % total_slots`). Publier
            // `n_parallel` alors que la derniere sequence est reservee faisait
            // hacher le client sur un espace qui contient une sequence
            // INUTILISABLE : un fil sur n tombait dessus a chaque demarrage.
            // Le registre le re-mappe (le cas reste donc correct), mais autant
            // que le client ne vise pas une sequence interdite : publier le
            // pool reel supprime la collision au lieu de la rattraper.
            //
            // Le calcul vient de jspace_slots_offerts_of, LA MEME fonction que
            // la classe : /props ne peut pas se tromper sur la taille du pool
            // sans que la classe se trompe aussi (et elle refuse d'offrir la
            // reservee, cf. les tests).
            { "total_slots",                 jspace_slots_offerts_of((int32_t) params.n_parallel) },
            { "model_alias",                 meta->model_name },
            { "model_ftype",                 meta->model_ftype },
            { "model_path",                  meta->model_path },
            { "modalities",                  json {
                {"vision", meta->has_inp_image},
                {"video",  meta->has_inp_video},
                {"audio",  meta->has_inp_audio},
            } },
            { "media_marker",                get_media_marker() },
            { "endpoint_slots",              params.endpoint_slots },
            { "endpoint_props",              params.endpoint_props },
            { "endpoint_metrics",            params.endpoint_metrics },
            { "ui",                          params.ui },
            { "ui_settings",                 meta->json_ui_settings },
            { "chat_template",               tmpl_default },
            { "chat_template_caps",          meta->chat_template_caps },
            { "bos_token",                   meta->bos_token_str },
            { "eos_token",                   meta->eos_token_str },
            { "build_info",                  meta->build_info },
            { "is_sleeping",                 queue_tasks.is_sleeping() },
            { "cors_proxy_enabled",          params.ui_mcp_proxy },
        };
        if (params.use_jinja) {
            if (!tmpl_tools.empty()) {
                props["chat_template_tool_use"] = tmpl_tools;
            }
        }
        res->ok(props);
        return res;
    };

    this->post_props = [this](const server_http_req &) {
        auto res = create_response();
        if (!params.endpoint_props) {
            res->error(format_error_response("This server does not support changing global properties. Start it with `--props`", ERROR_TYPE_NOT_SUPPORTED));
            return res;
        }
        // update any props here

        res->ok({{ "success", true }});
        return res;
    };

    this->post_infill = [this](const server_http_req & req) {
        auto res = create_response();
        // check model compatibility
        std::string err;
        if (llama_vocab_fim_pre(ctx_server.vocab) == LLAMA_TOKEN_NULL) {
            err += "prefix token is missing. ";
        }
        if (llama_vocab_fim_suf(ctx_server.vocab) == LLAMA_TOKEN_NULL) {
            err += "suffix token is missing. ";
        }
        if (llama_vocab_fim_mid(ctx_server.vocab) == LLAMA_TOKEN_NULL) {
            err += "middle token is missing. ";
        }
        if (!err.empty()) {
            res->error(format_error_response(string_format("Infill is not supported by this model: %s", err.c_str()), ERROR_TYPE_NOT_SUPPORTED));
            return res;
        }

        // validate input
        json data = json::parse(req.body);
        if (data.contains("prompt") && !data.at("prompt").is_string()) {
            // prompt is optional
            res->error(format_error_response("\"prompt\" must be a string", ERROR_TYPE_INVALID_REQUEST));
        }

        if (!data.contains("input_prefix")) {
            res->error(format_error_response("\"input_prefix\" is required", ERROR_TYPE_INVALID_REQUEST));
        }

        if (!data.contains("input_suffix")) {
            res->error(format_error_response("\"input_suffix\" is required", ERROR_TYPE_INVALID_REQUEST));
        }

        if (data.contains("input_extra") && !data.at("input_extra").is_array()) {
            // input_extra is optional
            res->error(format_error_response("\"input_extra\" must be an array of {\"filename\": string, \"text\": string}", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        json input_extra = json_value(data, "input_extra", json::array());
        for (const auto & chunk : input_extra) {
            // { "text": string, "filename": string }
            if (!chunk.contains("text") || !chunk.at("text").is_string()) {
                res->error(format_error_response("extra_context chunk must contain a \"text\" field with a string value", ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
            // filename is optional
            if (chunk.contains("filename") && !chunk.at("filename").is_string()) {
                res->error(format_error_response("extra_context chunk's \"filename\" field must be a string", ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
        }
        data["input_extra"] = input_extra; // default to empty array if it's not exist

        std::string prompt = json_value(data, "prompt", std::string());
        std::vector<server_tokens> tokenized_prompts = tokenize_input_prompts(ctx_server.vocab, ctx_server.mctx, prompt, false, true);
        SRV_DBG("creating infill tasks, n_prompts = %d\n", (int) tokenized_prompts.size());
        data["prompt"] = format_prompt_infill(
            ctx_server.vocab,
            data.at("input_prefix"),
            data.at("input_suffix"),
            data.at("input_extra"),
            params.n_batch,
            params.n_predict,
            meta->slot_n_ctx,
            params.spm_infill,
            tokenized_prompts[0].get_tokens() // TODO: this could maybe be multimodal.
        );

        std::vector<raw_buffer> files; // dummy
        return handle_completions_impl(
            req,
            SERVER_TASK_TYPE_INFILL,
            data,
            files,
            TASK_RESPONSE_TYPE_NONE); // infill is not OAI compatible
    };

    this->post_completions = [this](const server_http_req & req) {
        auto res = create_response();
        std::vector<raw_buffer> files; // dummy
        const json body = json::parse(req.body);
        return handle_completions_impl(
            req,
            SERVER_TASK_TYPE_COMPLETION,
            body,
            files,
            TASK_RESPONSE_TYPE_NONE);
    };

    this->post_completions_oai = [this](const server_http_req & req) {
        auto res = create_response();
        std::vector<raw_buffer> files; // dummy
        const json body = json::parse(req.body);
        return handle_completions_impl(
            req,
            SERVER_TASK_TYPE_COMPLETION,
            body,
            files,
            TASK_RESPONSE_TYPE_OAI_CMPL);
    };

    this->post_chat_completions = [this](const server_http_req & req) {
        auto res = create_response();
        std::vector<raw_buffer> files;
        json body = json::parse(req.body);
        json body_parsed = oaicompat_chat_params_parse(
            body,
            meta->chat_params,
            files);
        // [obrain SPLICE-CHAT] `oaicompat_chat_params_parse` ne recopie que les
        // champs qu'il connait (il RECONSTRUIT le corps interne) : `state_ref`,
        // qui n'existe que cote obrain, serait donc perdu en route. On le
        // transporte explicitement depuis le corps ORIGINAL du client.
        if (body.contains("state_ref")) {
            body_parsed["state_ref"] = body.at("state_ref");
        }
        return handle_completions_impl(
            req,
            SERVER_TASK_TYPE_COMPLETION,
            body_parsed,
            files,
            TASK_RESPONSE_TYPE_OAI_CHAT);
    };

    this->post_chat_completions_tok = [this](const server_http_req & req) {
        return handle_count_tokens(ctx_server.vocab, ctx_server.mctx, req, TASK_RESPONSE_TYPE_OAI_CHAT);
    };

    this->post_control = [this](const server_http_req & req) {
        auto res = create_response();
        const json body = json::parse(req.body);

        const std::string cmpl_id = json_value(body, "id", std::string());
        const std::string action  = json_value(body, "action", std::string());
        if (cmpl_id.empty()) {
            res->error(format_error_response("missing completion id", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }
        if (action != "reasoning_end") {
            res->error(format_error_response("unknown control action", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        auto & rd = res->rd;
        {
            server_task task(SERVER_TASK_TYPE_CONTROL);
            task.id              = rd.get_new_id();
            task.params.control_cmpl_id = cmpl_id;
            task.params.control_action  = action;
            rd.post_task(std::move(task));
        }

        auto result = rd.next(req.should_stop);
        if (!result) {
            GGML_ASSERT(req.should_stop());
            return res;
        }
        if (result->is_error()) {
            res->error(result->to_json());
            return res;
        }
        res->ok(result->to_json());
        return res;
    };

    this->post_responses_oai = [this](const server_http_req & req) {
        auto res = create_response();
        std::vector<raw_buffer> files;
        json body = server_chat_convert_responses_to_chatcmpl(json::parse(req.body));
        SRV_DBG("%s\n", "Request converted: OpenAI Responses -> OpenAI Chat Completions");
        SRV_DBG("converted request: %s\n", body.dump().c_str());
        json body_parsed = oaicompat_chat_params_parse(
            body,
            meta->chat_params,
            files);
        return handle_completions_impl(
            req,
            SERVER_TASK_TYPE_COMPLETION,
            body_parsed,
            files,
            TASK_RESPONSE_TYPE_OAI_RESP);
    };

    this->post_responses_tok_oai = [this](const server_http_req & req) {
        return handle_count_tokens(ctx_server.vocab, ctx_server.mctx, req, TASK_RESPONSE_TYPE_OAI_RESP);
    };

    this->post_transcriptions_oai = [this](const server_http_req & req) {
        auto res = create_response();

        if (!meta->has_mtmd || !meta->chat_params.allow_audio) {
            res->error(format_error_response("The current model does not support audio input.", ERROR_TYPE_NOT_SUPPORTED));
            return res;
        }

        std::vector<raw_buffer> files;
        json body = convert_transcriptions_to_chatcmpl(
            json::parse(req.body),
            meta->chat_params.tmpls.get(),
            req.files,
            files);
        SRV_DBG("%s\n", "Request converted: OpenAI Transcriptions -> OpenAI Chat Completions");
        SRV_DBG("converted request: %s\n", body.dump().c_str());
        json body_parsed = oaicompat_chat_params_parse(
            body,
            meta->chat_params,
            files);
        return handle_completions_impl(
            req,
            SERVER_TASK_TYPE_COMPLETION,
            body_parsed,
            files,
            TASK_RESPONSE_TYPE_OAI_ASR);
    };

    this->post_anthropic_messages = [this](const server_http_req & req) {
        auto res = create_response();
        std::vector<raw_buffer> files;
        json body = server_chat_convert_anthropic_to_oai(json::parse(req.body));
        SRV_DBG("%s\n", "Request converted: Anthropic -> OpenAI Chat Completions");
        SRV_DBG("converted request: %s\n", body.dump().c_str());
        json body_parsed = oaicompat_chat_params_parse(
            body,
            meta->chat_params,
            files);
        return handle_completions_impl(
            req,
            SERVER_TASK_TYPE_COMPLETION,
            body_parsed,
            files,
            TASK_RESPONSE_TYPE_ANTHROPIC);
    };

    this->post_anthropic_count_tokens = [this](const server_http_req & req) {
        return handle_count_tokens(ctx_server.vocab, ctx_server.mctx, req, TASK_RESPONSE_TYPE_ANTHROPIC);
    };

    // same with handle_chat_completions, but without inference part
    this->post_apply_template = [this](const server_http_req & req) {
        auto res = create_response();
        std::vector<raw_buffer> files; // dummy, unused
        json body = json::parse(req.body);
        json data = oaicompat_chat_params_parse(
            body,
            meta->chat_params,
            files);
        res->ok({{ "prompt", std::move(data.at("prompt")) }});
        return res;
    };

    this->get_models = [this](const server_http_req &) {
        auto res = create_response(true);

        // this endpoint can be accessed during sleeping
        // the next LOC is to avoid someone accidentally use ctx_server
        bool ctx_server; // do NOT delete this line
        GGML_UNUSED(ctx_server);

        json models = {
            {"models", {
                {
                    {"name",  meta->model_name},
                    {"model", meta->model_name},
                    {"modified_at", ""},
                    {"size", ""},
                    {"digest", ""}, // dummy value, llama.cpp does not support managing model file's hash
                    {"type", "model"},
                    {"description", ""},
                    {"tags", {""}},
                    {"capabilities", meta->has_mtmd ? json({"completion","multimodal"}) : json({"completion"})},
                    {"parameters", ""},
                    {"details", {
                        {"parent_model", ""},
                        {"format", "gguf"},
                        {"family", ""},
                        {"families", {""}},
                        {"parameter_size", ""},
                        {"quantization_level", ""}
                    }}
                }
            }},
            {"object", "list"},
            {"data", {
                get_model_info(),
            }}
        };

        res->ok(models);
        return res;
    };

    this->post_tokenize = [this](const server_http_req & req) {
        auto res = create_response();
        const json body = json::parse(req.body);
        json tokens_response = json::array();
        if (body.count("content") != 0) {
            const bool add_special = json_value(body, "add_special", false);
            const bool parse_special = json_value(body, "parse_special", true);
            const bool with_pieces = json_value(body, "with_pieces", false);

            llama_tokens tokens = tokenize_mixed(ctx_server.vocab, body.at("content"), add_special, parse_special);

            if (with_pieces) {
                for (const auto& token : tokens) {
                    std::string piece = common_token_to_piece(ctx_server.vocab, token);
                    json piece_json;

                    // Check if the piece is valid UTF-8
                    if (is_valid_utf8(piece)) {
                        piece_json = piece;
                    } else {
                        // If not valid UTF-8, store as array of byte values
                        piece_json = json::array();
                        for (unsigned char c : piece) {
                            piece_json.push_back(static_cast<int>(c));
                        }
                    }

                    tokens_response.push_back({
                        {"id", token},
                        {"piece", piece_json}
                    });
                }
            } else {
                tokens_response = tokens;
            }
        }

        res->ok(json{{"tokens", std::move(tokens_response)}});
        return res;
    };

    this->post_detokenize = [this](const server_http_req & req) {
        auto res = create_response();
        const json body = json::parse(req.body);

        std::string content;
        if (body.count("tokens") != 0) {
            const llama_tokens tokens = body.at("tokens");
            content = tokens_to_str(ctx_server.vocab, tokens);
        }

        res->ok(json{{"content", std::move(content)}});
        return res;
    };

    this->post_embeddings = [this](const server_http_req & req) {
        return handle_embeddings_impl(req, TASK_RESPONSE_TYPE_NONE);
    };

    this->post_embeddings_oai = [this](const server_http_req & req) {
        return handle_embeddings_impl(req, TASK_RESPONSE_TYPE_OAI_EMBD);
    };

    this->post_rerank = [this](const server_http_req & req) {
        auto res = create_response();
        if (!params.embedding || params.pooling_type != LLAMA_POOLING_TYPE_RANK) {
            res->error(format_error_response("This server does not support reranking. Start it with `--reranking`", ERROR_TYPE_NOT_SUPPORTED));
            return res;
        }

        const json body = json::parse(req.body);

        // if true, use TEI API format, otherwise use Jina API format
        // Jina: https://jina.ai/reranker/
        // TEI: https://huggingface.github.io/text-embeddings-inference/#/Text%20Embeddings%20Inference/rerank
        bool is_tei_format = body.contains("texts");

        json query;
        if (body.count("query") == 1) {
            query = body.at("query");
            if (!query.is_string()) {
                res->error(format_error_response("\"query\" must be a string", ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
        } else {
            res->error(format_error_response("\"query\" must be provided", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        std::vector<std::string> documents = json_value(body, "documents",
                                             json_value(body, "texts", std::vector<std::string>()));
        if (documents.empty()) {
            res->error(format_error_response("\"documents\" must be a non-empty string array", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        int top_n = json_value(body, "top_n", (int)documents.size());

        // create and queue the task
        json responses = json::array();
        auto & rd = res->rd;
        {
            std::vector<server_task> tasks;
            tasks.reserve(documents.size());
            for (size_t i = 0; i < documents.size(); i++) {
                auto tmp = format_prompt_rerank(ctx_server.model_tgt, ctx_server.vocab, ctx_server.mctx, query, documents[i]);
                server_task task = server_task(SERVER_TASK_TYPE_RERANK);
                task.id     = rd.get_new_id();
                task.tokens = std::move(tmp);
                tasks.push_back(std::move(task));
            }
            rd.post_tasks(std::move(tasks));
        }

        // wait for the results
        auto all_results = rd.wait_for_all(req.should_stop);

        // collect results
        if (all_results.is_terminated) {
            return res; // connection is closed
        } else if (all_results.error) {
            res->error(all_results.error->to_json());
            return res;
        } else {
            for (auto & res : all_results.results) {
                GGML_ASSERT(dynamic_cast<server_task_result_rerank*>(res.get()) != nullptr);
                responses.push_back(res->to_json());
            }
        }

        // write JSON response
        json root = format_response_rerank(
            body,
            meta->model_name,
            responses,
            is_tei_format,
            documents,
            top_n);

        res->ok(root);
        return res;
    };

    this->get_lora_adapters = [this](const server_http_req & req) {
        auto res = create_response();

        auto & rd = res->rd;
        {
            server_task task(SERVER_TASK_TYPE_GET_LORA);
            task.id = rd.get_new_id();
            rd.post_task(std::move(task));
        }

        // get the result
        auto result = rd.next(req.should_stop);
        if (!result) {
            // connection was closed
            GGML_ASSERT(req.should_stop());
            return res;
        }

        if (result->is_error()) {
            res->error(result->to_json());
            return res;
        }

        GGML_ASSERT(dynamic_cast<server_task_result_get_lora*>(result.get()) != nullptr);
        res->ok(result->to_json());
        return res;
    };

    this->post_lora_adapters = [this](const server_http_req & req) {
        auto res = create_response();
        const json body = json::parse(req.body);
        if (!body.is_array()) {
            res->error(format_error_response("Request body must be an array", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        auto & rd = res->rd;
        {
            server_task task(SERVER_TASK_TYPE_SET_LORA);
            task.id = rd.get_new_id();
            task.set_lora = parse_lora_request(body);
            rd.post_task(std::move(task));
        }

        // get the result
        auto result = rd.next(req.should_stop);
        if (!result) {
            // connection was closed
            GGML_ASSERT(req.should_stop());
            return res;
        }

        if (result->is_error()) {
            res->error(result->to_json());
            return res;
        }

        GGML_ASSERT(dynamic_cast<server_task_result_apply_lora*>(result.get()) != nullptr);
        res->ok(result->to_json());
        return res;
    };

    // POST /attn-mask — set or clear custom attention mask
    // Body: { "mask": [float...], "positions": [int...] }  or  { "mask": null } to clear
    this->post_attn_mask = [this](const server_http_req & req) {
        auto res = create_response();
        const json body = json::parse(req.body);

        auto & rd = res->rd;
        {
            server_task task(SERVER_TASK_TYPE_SET_ATTN_MASK);
            task.id = rd.get_new_id();

            if (body.contains("mask") && !body["mask"].is_null()) {
                const auto & j_mask = body["mask"];
                const auto & j_pos  = body["positions"];

                if (!j_mask.is_array() || !j_pos.is_array()) {
                    res->error(format_error_response("'mask' and 'positions' must be arrays", ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }

                const int32_t n_pos = (int32_t) j_pos.size();
                const int32_t n_head_groups = body.value("n_head_groups", 0);
                const int32_t slot_id       = body.value("slot_id", -1);

                const int32_t expected_mask_size = (n_head_groups > 1)
                    ? n_head_groups * n_pos * n_pos
                    : n_pos * n_pos;

                if ((int32_t) j_mask.size() != expected_mask_size) {
                    res->error(format_error_response(
                        "mask size must be " + std::to_string(expected_mask_size) + ", got " + std::to_string(j_mask.size()),
                        ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }

                task.attn_mask.resize(expected_mask_size);
                for (int32_t i = 0; i < expected_mask_size; ++i) {
                    task.attn_mask[i] = j_mask[i].get<float>();
                }

                task.attn_mask_pos.resize(n_pos);
                for (int32_t i = 0; i < n_pos; ++i) {
                    task.attn_mask_pos[i] = j_pos[i].get<llama_pos>();
                }

                task.attn_mask_n_head_groups = n_head_groups;
                task.attn_mask_slot_id       = slot_id;
            }
            // else: empty vectors → will clear the mask

            rd.post_task(std::move(task));
        }

        auto result = rd.next(req.should_stop);
        if (!result) {
            GGML_ASSERT(req.should_stop());
            return res;
        }

        if (result->is_error()) {
            res->error(result->to_json());
            return res;
        }

        GGML_ASSERT(dynamic_cast<server_task_result_set_attn_mask*>(result.get()) != nullptr);
        res->ok(result->to_json());
        return res;
    };

    // GET /jspace/gram — readout Gram matrix (W_out^T @ W_out), base64-encoded
    // f32 LE, row-major n_embd x n_embd. Model constant — computed once,
    // lazily, on first request, cached for the server process lifetime. See
    // PO plan 4749fb72 (project cognitive-canvas): this is the expensive
    // constant term of the g^out(h) = J^T @ gram @ J pullback metric — meant
    // to be fetched ONCE by a client and reused locally for many cheap
    // per-activation g^out(h) evaluations (never recomputed server-side per
    // request). Model-weight-only read — thread-safe, no task queue needed
    // (same class of endpoint as get_props/post_props above).
    this->get_jspace_gram = [this](const server_http_req &) {
        auto res = create_response(true);

        if (!ctx_server.model_tgt) {
            res->error(format_error_response("model not loaded (server may be sleeping)", ERROR_TYPE_UNAVAILABLE));
            return res;
        }

        const int32_t n_embd = llama_model_n_embd(ctx_server.model_tgt);

        std::lock_guard<std::mutex> lock(jspace_mutex);
        if (!jspace_gram_cached) {
            std::vector<float> gram((size_t) n_embd * n_embd);
            SRV_INF("%s", "[jspace] computing readout Gram matrix (one-time, ~15-30s)...\n");
            const int32_t rc = llama_model_get_output_gram_matrix(ctx_server.model_tgt, gram.data());
            if (rc != n_embd) {
                res->error(format_error_response("failed to compute Gram matrix", ERROR_TYPE_SERVER));
                return res;
            }
            jspace_gram_b64_cache = base64::encode(
                reinterpret_cast<const char *>(gram.data()), gram.size() * sizeof(float));
            jspace_gram_cached = true;
            SRV_INF("%s", "[jspace] Gram matrix computed and cached\n");
        }

        json out = {
            { "n_embd",      n_embd },
            { "dtype",       "f32" },
            { "layout",      "row_major" },
            { "gram_base64", jspace_gram_b64_cache },
        };
        res->ok(out);
        return res;
    };

    // POST /jspace/readout_forward — real logits for an arbitrary
    // pre-output_norm hidden state h (RMSNorm + matmul with output.weight).
    // Ground truth for validating the g^out(h) pullback prediction against
    // real KL divergence — see H4 in PO plans 5675adc9 / 0648bb5e
    // (cognitive-canvas project). Cheap (O(n_vocab*n_embd), not
    // O(n_vocab*n_embd^2) like the Gram matrix above) — safe to call once
    // per request. Body: { "h": [float, ...] } (length must equal n_embd).
    this->post_jspace_readout_forward = [this](const server_http_req & req) {
        auto res = create_response(true);

        if (!ctx_server.model_tgt) {
            res->error(format_error_response("model not loaded (server may be sleeping)", ERROR_TYPE_UNAVAILABLE));
            return res;
        }

        json body;
        try {
            body = json::parse(req.body);
        } catch (const std::exception & e) {
            res->error(format_error_response(std::string("invalid JSON body: ") + e.what(), ERROR_TYPE_INVALID_REQUEST));
            return res;
        }
        if (!body.contains("h") || !body["h"].is_array()) {
            res->error(format_error_response("'h' must be an array of floats", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        const int32_t n_embd = llama_model_n_embd(ctx_server.model_tgt);
        const auto & j_h = body["h"];
        if ((int32_t) j_h.size() != n_embd) {
            res->error(format_error_response(
                "'h' length must be " + std::to_string(n_embd) + ", got " + std::to_string(j_h.size()),
                ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        std::vector<float> h(n_embd);
        for (int32_t i = 0; i < n_embd; ++i) {
            h[i] = j_h[i].get<float>();
        }

        const int32_t n_vocab = llama_vocab_n_tokens(ctx_server.vocab);
        std::vector<float> logits((size_t) n_vocab);
        const int32_t rc = llama_model_readout_forward(ctx_server.model_tgt, h.data(), logits.data());
        if (rc != n_vocab) {
            res->error(format_error_response("readout_forward failed", ERROR_TYPE_SERVER));
            return res;
        }

        json out = {
            { "n_vocab",       n_vocab },
            { "dtype",         "f32" },
            { "logits_base64", base64::encode(reinterpret_cast<const char *>(logits.data()), logits.size() * sizeof(float)) },
        };
        res->ok(out);
        return res;
    };

    // POST /jspace/compare_generation — [obrain] real end-to-end demonstration
    // of the J-space geometry's practical effect: generates THREE
    // continuations of the same prompt — baseline (no perturbation),
    // perturbed along g^out's dominant eigenvector, and perturbed along a
    // random direction of the SAME Euclidean magnitude — so the caller can
    // directly read the actual generated TEXT differences, not just an
    // abstract KL number. Runs on the DEDICATED reserved slot (see
    // jspace_reserved_slot_id() in server-context.cpp) via the normal task
    // queue — safe to call while the server is handling concurrent
    // production traffic on the other slots. Requires the server to be
    // started with --parallel 3 or more.
    // Body: { "prompt": string, "scale"?: float (default 0.15), "n_generate"?: int (default 24) }
    this->post_jspace_compare_generation = [this](const server_http_req & req) {
        auto res = create_response();
        const json body = json::parse(req.body);

        if (!body.contains("prompt") || !body["prompt"].is_string()) {
            res->error(format_error_response("'prompt' must be a string", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }

        auto & rd = res->rd;
        {
            server_task task(SERVER_TASK_TYPE_JSPACE_COMPARE_GENERATION);
            task.id = rd.get_new_id();
            task.jspace_prompt     = body.at("prompt").get<std::string>();
            task.jspace_scale      = body.value("scale", 0.15f);
            task.jspace_n_generate = body.value("n_generate", 24);
            rd.post_task(std::move(task));
        }

        auto result = rd.next(req.should_stop);
        if (!result) {
            GGML_ASSERT(req.should_stop());
            return res;
        }

        if (result->is_error()) {
            res->error(result->to_json());
            return res;
        }

        GGML_ASSERT(dynamic_cast<server_task_result_jspace_compare*>(result.get()) != nullptr);
        res->ok(result->to_json());
        return res;
    };

    // POST /jspace/inject_knowledge — [obrain] production entry point for
    // the KV-splice knowledge-fusion mechanism (PO plan 4749fb72, tâche
    // P5-T5). Processes `fact_text` once on the reserved slot, extracts
    // its full state (attention+recurrent), round-trips it (extract+
    // reinject — the seam a future cross-request cache would split at),
    // then decodes `prompt` AFTER the spliced fact and generates a
    // continuation — the fact's literal tokens are never part of the
    // query's visible context, only its resulting KV/recurrent state.
    //
    // Scope limits enforced by design (see server-context.cpp switch case
    // for the full rationale, backed by empirical findings this session):
    // ONE fact per call (multi-fact concatenation causes interference),
    // and NOT intended to override a contested/well-established fact
    // (four distinct re-injection designs all fail on that use case) —
    // use only for novel/complementary knowledge.
    // Body: { "fact_text": string, "prompt": string, "n_generate"?: int (default 24) }
    this->post_jspace_inject_knowledge = [this](const server_http_req & req) {
        auto res = create_response();
        const json body = json::parse(req.body);

        const bool kv_extract_route = body.value("extract_only", false);
        // [obrain O1] mode de verification EXPLICITEMENT active.
        const bool kv_verify_route = body.value("verify", false);
        // [obrain APPEND-K3] AJOUT D'UNE PAGE : ni question, ni generation.
        const bool kv_append_route = body.value("state_append", false);
        const bool kv_ref_route     = body.contains("state_ref") && body["state_ref"].is_string()
                                      && !body["state_ref"].get<std::string>().empty();
        // [obrain K1-IDS] `fact_token_ids` est une source de prefixe VALIDE, au
        // meme titre que `fact_text`. Sans cette ligne, une requete n'envoyant
        // que des IDs natifs etait refusee avant meme d'atteindre le parsing.
        const bool kv_ids_route = body.contains("fact_token_ids") && body["fact_token_ids"].is_array()
                                  && !body["fact_token_ids"].empty();
        // [obrain APPEND-K4] PREREQUIS SEULS : la source du prefixe n'est pas un
        // texte a decoder mais l'ENSEMBLE DES PAGES AJOUTEES. Sans cette ligne, une
        // question posee sur un contexte assemble serait refusee faute de
        // `fact_text` -- alors que le fait EST le contexte assemble.
        const bool kv_prereq_route = body.contains("prerequis_refs") && body["prerequis_refs"].is_array()
                                     && !body["prerequis_refs"].empty();
        if (kv_ref_route && !kv_ref_is_valid(body["state_ref"].get<std::string>())) {
            res->error(format_error_response("'state_ref' invalide : 16 caracteres hexadecimaux minuscules attendus "
                                             "(la partie apres 'kv_state:')", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }
        if ((!body.contains("fact_text") || !body["fact_text"].is_string()) && !kv_ids_route) {
            if (!kv_ref_route && !kv_prereq_route) {
                res->error(format_error_response("'fact_text' must be a string, or pass 'state_ref', or pass 'fact_token_ids'", ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
        }
        // [obrain APPEND-K3] `state_append` ne decode AUCUNE question : exiger
        // `prompt` obligerait l'appelant a inventer un texte que le serveur
        // jetterait. Le champ reste accepte (il n'est simplement pas exige).
        if ((!body.contains("prompt") || !body["prompt"].is_string()) && !kv_extract_route && !kv_append_route) {
            res->error(format_error_response("'prompt' must be a string (sauf extract_only / state_append)", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }
        // [obrain APPEND-K3] `state_append` exige un `state_ref` et une plage
        // ANNONCEE : sans plage, la route ne pourrait pas dire a l'appelant
        // quelle page a ete ajoutee, et le client ne pourrait pas verifier que
        // c'est bien celle qu'il croyait.
        if (kv_append_route) {
            if (!kv_ref_route) {
                res->error(format_error_response("'state_append' requires a non-empty 'state_ref'", ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
            if (!body.contains("pos_min") || !body.contains("pos_max")) {
                res->error(format_error_response("'state_append' requires 'pos_min' and 'pos_max' (the page's position range)", ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
        }

        auto & rd = res->rd;
        {
            server_task task(SERVER_TASK_TYPE_JSPACE_INJECT_KNOWLEDGE);
            task.id = rd.get_new_id();
            task.jspace_fact_text    = body.value("fact_text", std::string());
            task.jspace_prompt       = body.value("prompt", std::string());
            task.jspace_state_ref    = body.value("state_ref", std::string());      // [obrain K2] inject par état (bypass texte)
            task.jspace_extract_only = body.value("extract_only", false);           // [obrain K1] extraire → ref
            // [obrain APPEND-K3] ajout incremental d'une PAGE : la plage annoncee
            // est comparee au manifeste du blob ET a la sequence cible avant tout
            // appel au moteur. `pos_start` / `prerequis_refs` concernent le chemin
            // EXTRACTION (fabriquer une page decalee sans mentir sur son etat
            // recurrent).
            task.jspace_state_append = kv_append_route;
            task.jspace_state_reset  = body.value("state_reset", false);
            task.jspace_pos_min      = body.value("pos_min", (int64_t) -1);
            task.jspace_pos_max      = body.value("pos_max", (int64_t) -1);
            task.jspace_pos_start    = body.value("pos_start", (int64_t) 0);
            if (body.contains("prerequis_refs")) {
                if (!body["prerequis_refs"].is_array()) {
                    res->error(format_error_response("'prerequis_refs' must be an array of state refs", ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }
                if (body["prerequis_refs"].size() > 64) {
                    res->error(format_error_response("'prerequis_refs' : 64 pages au maximum (un contexte se charge page a page)", ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }
                for (const auto & v : body["prerequis_refs"]) {
                    if (!v.is_string() || v.get<std::string>().empty()) {
                        res->error(format_error_response("'prerequis_refs' must contain non-empty strings", ERROR_TYPE_INVALID_REQUEST));
                        return res;
                    }
                    task.jspace_prerequis_refs.push_back(v.get<std::string>());
                }
            }
            // [obrain K1-IDS] IDs NATIFS du prefixe (mode de transfert exact).
            // Borne explicite : 1 a 4096 IDs. Refuse une entree textuelle
            // concurrente, pour qu'aucune tokenisation implicite n'ait lieu.
            if (body.contains("fact_token_ids")) {
                if (!body["fact_token_ids"].is_array()) {
                    res->error(format_error_response("'fact_token_ids' must be an array of ints", ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }
                if (body.contains("fact_text")) {
                    res->error(format_error_response("'fact_token_ids' and 'fact_text' are mutually exclusive", ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }
                const size_t n = body["fact_token_ids"].size();
                if (n < 1 || n > 4096) {
                    res->error(format_error_response("'fact_token_ids' length must be 1..4096, got " + std::to_string(n), ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }
                task.jspace_fact_token_ids.reserve(n);
                const int32_t n_vocab_ids = llama_vocab_n_tokens(ctx_server.vocab);
                for (const auto & v : body["fact_token_ids"]) {
                    const int id = v.get<int>();
                    if (id < 0 || id >= n_vocab_ids) {
                        res->error(format_error_response("fact_token_ids contains an out-of-range id " + std::to_string(id), ERROR_TYPE_INVALID_REQUEST));
                        return res;
                    }
                    task.jspace_fact_token_ids.push_back((llama_token) id);
                }
            }
            task.jspace_n_generate = body.value("n_generate", 160); // 2026-07-13 : 24 was too short for this model's verbose thinking style, empirically confirmed live (facts converged correctly around 100-160 tokens, not 24)
            task.jspace_greedy     = body.value("greedy", true);
            task.jspace_verify     = kv_verify_route;   // [obrain O1]
            rd.post_task(std::move(task));
        }

        auto result = rd.next(req.should_stop);
        if (!result) {
            GGML_ASSERT(req.should_stop());
            return res;
        }

        if (result->is_error()) {
            res->error(result->to_json());
            return res;
        }

        GGML_ASSERT(dynamic_cast<server_task_result_jspace_inject*>(result.get()) != nullptr);
        res->ok(result->to_json());
        return res;
    };
}

json server_routes::get_model_info() const {
    return json {
        {"id",       meta->model_name},
        {"aliases",  meta->model_aliases},
        {"tags",     meta->model_tags},
        {"object",   "model"},
        {"created",  std::time(0)},
        {"owned_by", "llamacpp"},
        {"meta",     {
            {"vocab_type",  meta->model_vocab_type},
            {"n_vocab",     meta->model_vocab_n_tokens},
            {"n_ctx",       meta->slot_n_ctx},
            {"n_ctx_train", meta->model_n_ctx_train},
            {"n_embd",      meta->model_n_embd_inp},
            {"n_params",    meta->model_n_params},
            {"size",        meta->model_size},
            {"ftype",       meta->model_ftype},
        }},
    };
}

std::unique_ptr<server_res_generator> server_routes::handle_slots_save(const server_http_req & req, int id_slot) {
    auto res = create_response();
    const json request_data = json::parse(req.body);
    std::string filename = request_data.at("filename");
    if (!fs_validate_filename(filename)) {
        res->error(format_error_response("Invalid filename", ERROR_TYPE_INVALID_REQUEST));
        return res;
    }
    std::string filepath = params.slot_save_path + filename;

    auto & rd = res->rd;
    {
        server_task task(SERVER_TASK_TYPE_SLOT_SAVE);
        task.id = rd.get_new_id();
        task.slot_action.id_slot  = id_slot;
        task.slot_action.filename = filename;
        task.slot_action.filepath = filepath;
        rd.post_task(std::move(task));
    }

    auto result = rd.next(req.should_stop);
    if (!result) {
        // connection was closed
        GGML_ASSERT(req.should_stop());
        return res;
    }

    if (result->is_error()) {
        res->error(result->to_json());
        return res;
    }

    res->ok(result->to_json());
    return res;
}

std::unique_ptr<server_res_generator> server_routes::handle_slots_restore(const server_http_req & req, int id_slot) {
    auto res = create_response();
    const json request_data = json::parse(req.body);
    std::string filename = request_data.at("filename");
    if (!fs_validate_filename(filename)) {
        res->error(format_error_response("Invalid filename", ERROR_TYPE_INVALID_REQUEST));
        return res;
    }
    std::string filepath = params.slot_save_path + filename;

    auto & rd = res->rd;
    {
        server_task task(SERVER_TASK_TYPE_SLOT_RESTORE);
        task.id = rd.get_new_id();
        task.slot_action.id_slot  = id_slot;
        task.slot_action.filename = filename;
        task.slot_action.filepath = filepath;
        rd.post_task(std::move(task));
    }

    auto result = rd.next(req.should_stop);
    if (!result) {
        // connection was closed
        GGML_ASSERT(req.should_stop());
        return res;
    }

    if (result->is_error()) {
        res->error(result->to_json());
        return res;
    }

    GGML_ASSERT(dynamic_cast<server_task_result_slot_save_load*>(result.get()) != nullptr);
    res->ok(result->to_json());
    return res;
}

// [obrain LONG-RESUME] POST /slots/:id?action=compact  body {"tail_m": N}
//                      POST /slots/:id?action=info     (no body) — stats only
std::unique_ptr<server_res_generator> server_routes::handle_slots_compact(const server_http_req & req, int id_slot, bool do_compact) {
    auto res = create_response();

    int32_t tail_m = -1;
    if (do_compact) {
        json request_data = json::object();
        if (!req.body.empty()) {
            request_data = json::parse(req.body);
        }
        tail_m = request_data.value("tail_m", 256);
        if (tail_m < 0) {
            res->error(format_error_response("tail_m must be >= 0", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }
    }

    auto & rd = res->rd;
    {
        server_task task(SERVER_TASK_TYPE_SLOT_COMPACT);
        task.id = rd.get_new_id();
        task.slot_action.id_slot = id_slot;
        task.slot_action.tail_m  = tail_m;
        rd.post_task(std::move(task));
    }

    auto result = rd.next(req.should_stop);
    if (!result) {
        // connection was closed
        GGML_ASSERT(req.should_stop());
        return res;
    }

    if (result->is_error()) {
        res->error(result->to_json());
        return res;
    }

    res->ok(result->to_json());
    return res;
}

std::unique_ptr<server_res_generator> server_routes::handle_slots_erase(const server_http_req & req, int id_slot) {
    auto res = create_response();
    auto & rd = res->rd;
    {
        server_task task(SERVER_TASK_TYPE_SLOT_ERASE);
        task.id = rd.get_new_id();
        task.slot_action.id_slot = id_slot;
        rd.post_task(std::move(task));
    }

    auto result = rd.next(req.should_stop);
    if (!result) {
        // connection was closed
        GGML_ASSERT(req.should_stop());
        return res;
    }

    if (result->is_error()) {
        res->error(result->to_json());
        return res;
    }

    GGML_ASSERT(dynamic_cast<server_task_result_slot_erase*>(result.get()) != nullptr);
    res->ok(result->to_json());
    return res;
}

std::unique_ptr<server_res_generator> server_routes::handle_embeddings_impl(const server_http_req & req, task_response_type res_type) {
    auto res = create_response();
    if (!params.embedding) {
        res->error(format_error_response("This server does not support embeddings. Start it with `--embeddings`", ERROR_TYPE_NOT_SUPPORTED));
        return res;
    }

    if (res_type != TASK_RESPONSE_TYPE_NONE && meta->pooling_type == LLAMA_POOLING_TYPE_NONE) {
        res->error(format_error_response("Pooling type 'none' is not OAI compatible. Please use a different pooling type", ERROR_TYPE_INVALID_REQUEST));
        return res;
    }

    const json body = json::parse(req.body);

    // for the shape of input/content, see tokenize_input_prompts()
    json prompt;
    if (body.count("input") != 0) {
        prompt = body.at("input");
    } else if (body.contains("content")) {
        res_type = TASK_RESPONSE_TYPE_NONE; // "content" field is not OAI compatible
        prompt = body.at("content");
    } else {
        res->error(format_error_response("\"input\" or \"content\" must be provided", ERROR_TYPE_INVALID_REQUEST));
        return res;
    }

    bool use_base64 = false;
    if (body.count("encoding_format") != 0) {
        const std::string & format = body.at("encoding_format");
        if (format == "base64") {
            use_base64 = true;
        } else if (format != "float") {
            res->error(format_error_response("The format to return the embeddings in. Can be either float or base64", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }
    }

    auto tokenized_prompts = tokenize_input_prompts(ctx_server.vocab, ctx_server.mctx, prompt, true, true);
    for (const auto & tokens : tokenized_prompts) {
        // this check is necessary for models that do not add BOS token to the input
        if (tokens.empty()) {
            res->error(format_error_response("Input content cannot be empty", ERROR_TYPE_INVALID_REQUEST));
            return res;
        }
    }

    int embd_normalize = params.embd_normalize;
    if (body.count("embd_normalize") != 0) {
        embd_normalize = body.at("embd_normalize");
        if (meta->pooling_type == LLAMA_POOLING_TYPE_NONE) {
            SRV_DBG("embd_normalize is not supported by pooling type %d, ignoring it\n", meta->pooling_type);
        }
    }

    // create and queue the task
    json responses = json::array();
    auto & rd = res->rd;
    {
        std::vector<server_task> tasks;
        for (size_t i = 0; i < tokenized_prompts.size(); i++) {
            server_task task = server_task(SERVER_TASK_TYPE_EMBEDDING);

            task.id     = rd.get_new_id();
            task.tokens = std::move(tokenized_prompts[i]);

            // OAI-compat
            task.params.res_type = res_type;
            task.params.embd_normalize = embd_normalize;

            tasks.push_back(std::move(task));
        }
        rd.post_tasks(std::move(tasks));
    }

    // wait for the results
    auto all_results = rd.wait_for_all(req.should_stop);

    // collect results
    if (all_results.is_terminated) {
        return res; // connection is closed
    } else if (all_results.error) {
        res->error(all_results.error->to_json());
        return res;
    } else {
        for (auto & res : all_results.results) {
            GGML_ASSERT(dynamic_cast<server_task_result_embd*>(res.get()) != nullptr);
            responses.push_back(res->to_json());
        }
    }

    // write JSON response
    json root = res_type == TASK_RESPONSE_TYPE_OAI_EMBD
        ? format_embeddings_response_oaicompat(body, meta->model_name, responses, use_base64)
        : json(responses);
    res->ok(root);
    return res;
}

std::unique_ptr<server_res_generator> server_routes::handle_count_tokens(const llama_vocab * vocab, mtmd_context * mctx, const server_http_req & req, task_response_type res_type) {
    auto res = create_response();
    std::vector<raw_buffer> files;
    json body = json::parse(req.body);
    bool is_oai = false;

    switch (res_type) {
        case TASK_RESPONSE_TYPE_OAI_CHAT:
            {
                is_oai = true;
            } break;
        case TASK_RESPONSE_TYPE_OAI_RESP:
            {
                is_oai = true;
                body = server_chat_convert_responses_to_chatcmpl(body);
            } break;
        case TASK_RESPONSE_TYPE_ANTHROPIC:
            {
                body = server_chat_convert_anthropic_to_oai(body);
            } break;
        default:
            res->error(format_error_response("invalid res_type", ERROR_TYPE_INVALID_REQUEST));
            return res;
    }

    json body_parsed = oaicompat_chat_params_parse(
            body,
            meta->chat_params,
            files);
    json prompt = body_parsed.at("prompt");
    // SRV_DBG("prompt = %s\n", prompt.dump().c_str());

    // TODO @ngxson : refactor this code block, move this to server-common and reuse it in other places
    size_t n_tokens;
    if (mctx != nullptr) {
        if (!prompt.is_string()) {
            throw std::runtime_error("for mtmd, input prompt must be a string.");
        }
        n_tokens = process_mtmd_prompt(mctx, prompt.get<std::string>(), files, true).size();
    } else {
        n_tokens = tokenize_mixed(vocab, prompt, true, true).size();
    }

    json response = {{"input_tokens", static_cast<int64_t>(n_tokens)}};
    if (is_oai) {
        response["object"] = "response.input_tokens";
    }
    res->ok(response);
    return res;
}
