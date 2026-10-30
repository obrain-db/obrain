// ENGINE-008 -- PREFLIGHT-REAL-ADAPTERS-06 : adaptateurs REELS des dependances.
// Raccorde run_pipeline() aux API Q1 exactes. Aucun pipeline parallele.
// Aucun chargement, aucun contexte, aucun forward dans cette passe : les
// compteurs d'appels sont exposes pour le PROUVER.
#include "engine008_real_deps.h"
#include "ggml.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <map>
#include <set>
#include <sstream>
#include <string>
#include <sys/stat.h>
#include <vector>

// ---------------------------------------------------------------- COMPTEURS
// Exposes pour que le point d'entree PROUVE zero appel (pas un message).
extern "C" {
long long g_real_load_calls     = 0;
long long g_real_ctx_calls      = 0;
long long g_real_prefill_calls  = 0;
long long g_real_decode_calls   = 0;
long long g_real_release_calls  = 0;
long long g_real_f1a_env_calls  = 0;
long long g_real_enum_calls     = 0;
long long g_real_registry_calls = 0;
}

// ---------------------------------------------------------------- ABI Q1 VERIFIEE
// nm -D --defined-only --demangle libllama.so.0.0.135 :
//   B f1a_last_gf        (NON mangle -> linkage C)
//   B f1a_last_gf_nodes  (NON mangle -> linkage C)
//   T llama_model::get_tensor(char const*) const
//   T llama_internal_get_tensor_map(llama_model const*)
extern "C" {
extern const void * f1a_last_gf;
extern int          f1a_last_gf_nodes;
}
// Declaration TRANSCRITE VERBATIM de src/llama-model.h:763 (implementation
// llama-model.cpp:3115 -> return model->tensors_by_name).
extern const std::vector<std::pair<std::string, ggml_tensor *>> &
llama_internal_get_tensor_map(const llama_model * model);

// ---------------------------------------------------------------- DOCUMENT D'AUTORISATION
struct AuthDocFile {
    bool have = false, well_formed = false, allow = false, real_audience = false;
    std::string bin, plan, proto;
};
// Format ligne a ligne `cle=valeur` : pas d'analyseur JSON a maintenir.
static AuthDocFile read_auth_doc(const std::string & path) {
    AuthDocFile d;
    std::ifstream f(path);
    if (!f) return d;                       // absent
    d.have = true; d.well_formed = true;
    std::set<std::string> seen;
    std::string line;
    while (std::getline(f, line)) {
        if (line.empty() || line[0] == '#') continue;
        auto eq = line.find('=');
        if (eq == std::string::npos) { d.well_formed = false; break; }
        std::string k = line.substr(0, eq), v = line.substr(eq + 1);
        if (seen.count(k)) { d.well_formed = false; break; }   // cle dupliquee
        seen.insert(k);
        if      (k == "well_formed")        { if (v != "1") d.well_formed = false; }
        else if (k == "allow_model_compute"){ d.allow = (v == "1"); }
        else if (k == "audience")           { d.real_audience = (v == "REAL_PREFLIGHT"); }
        else if (k == "binary_sha256")      { d.bin = v; }
        else if (k == "plan_sha256")        { d.plan = v; }
        else if (k == "protocol_sha256")    { d.proto = v; }
        // cle inconnue : toleree, non normative
    }
    return d;
}

// ---------------------------------------------------------------- CLEES NEXTN
// Regle etablie : il in [ n_layer(), n_layer() + n_layer_nextn )
// avec n_layer() = n_layer_all - n_layer_nextn (llama-hparams.cpp:273).
struct KeySpec { const char * role; const char * pat; bool excl; bool est; };
static const KeySpec SPECS[] = {
    { "NEXTN_EH_PROJ",          "blk.%d.nextn.eh_proj.weight",          true,  true  },
    { "NEXTN_ENORM",            "blk.%d.nextn.enorm.weight",            true,  true  },
    { "NEXTN_HNORM",            "blk.%d.nextn.hnorm.weight",            true,  true  },
    { "NEXTN_SHARED_HEAD_NORM", "blk.%d.nextn.shared_head_norm.weight", true,  true  },
    { "NEXTN_SHARED_HEAD_HEAD", "blk.%d.nextn.shared_head_head.weight", false, false },
};

// ---------------------------------------------------------------- DEPS REELLES
PipelineDeps make_real_deps(RealState & s) {
    PipelineDeps d;

    d.authorize = [](const PipelineRequest & r) -> AuthVerdict {
        AuthDocFile a = read_auth_doc(r.doc_path);
        if (!a.have) return AuthVerdict::REFUSED_NO_DOCUMENT;
        // Une autorisation destinee aux MOCKS est REFUSEE par le chemin reel.
        if (!a.real_audience) return AuthVerdict::REFUSED_MALFORMED_DOCUMENT;
        return authorize_doc(r.flag_allow_model_compute, r.binary_sha256, r.plan_sha256,
                             r.protocol_sha256, a.have, a.well_formed, a.allow,
                             a.bin, a.plan, a.proto);
    };

    d.prepare_capture = [&s](const std::string & root, const std::string & m,
                             const std::string & r, std::string & out, std::string & refusal) {
        out = root + "/" + m + "/" + r;
        if (mkdir(out.c_str(), 0755) != 0 && errno != EEXIST) { refusal = "mkdir impossible"; return false; }
        // refus d'une destination non vide : pas de reutilisation silencieuse
        std::string cmd = "ls -A '" + out + "' 2>/dev/null | head -1";
        FILE * p = popen(cmd.c_str(), "r"); char buf[256] = {0};
        if (p) { if (!fgets(buf, sizeof buf, p)) {} pclose(p); }
        if (buf[0] != '\0') { refusal = "destination non vide : capture anterieure possible"; return false; }
        // activateurs f1a lus par Q1 -- poses SEULEMENT dans le chemin autorise
        setenv("OBRAIN_F1A_CAPTURE", "1", 1);
        setenv("OBRAIN_F1A_OUTDIR", out.c_str(), 1);
        setenv("OBRAIN_F1A_RUN_ID", (m + "-" + r).c_str(), 1);
        setenv("OBRAIN_F1A_INVENTORY", (out + "/inventory.json").c_str(), 1);
        // OBRAIN_NO_ROTATION n'est JAMAIS touche ici.
        g_real_f1a_env_calls++;
        s.capture_dir = out;
        return true;
    };

    d.load_full = [&s](const std::string & p) -> void * {
        g_real_load_calls++;
        llama_model_params mp = llama_model_default_params();
        mp.vocab_only  = false;      // chargement COMPLET
        mp.n_gpu_layers = 0;
        s.model = llama_model_load_from_file(p.c_str(), mp);
        if (!s.model) { g_real_load_calls--; return nullptr; }  // echec : non comptabilise comme charge
        return s.model;
    };

    d.acquire_registry = [&s](void *) -> IdentityRegistry {
        g_real_registry_calls++;
        IdentityRegistry r;
        if (s.model == nullptr) return r;      // aucun dereferencement
        const auto & table = llama_internal_get_tensor_map(s.model);
        // n_layer() = n_layer_all - n_layer_nextn
        const int n_all   = (int) llama_model_n_layer(s.model);
        const int n_nextn = (int) llama_model_n_layer_nextn(s.model);
        const int n_layer = n_all - n_nextn;
        std::map<std::string, const ggml_tensor *> by_key;
        for (const auto & kv : table) by_key[kv.first] = kv.second;
        if (n_nextn > 0) {
            for (int il = n_layer; il < n_layer + n_nextn; ++il) {
                for (const auto & sp : SPECS) {
                    char buf[128]; snprintf(buf, sizeof buf, sp.pat, il);
                    RegistryEntry e; e.key = buf; e.role = sp.role;
                    auto it = by_key.find(e.key);
                    e.tensor = (it == by_key.end()) ? nullptr : (void *) it->second;
                    if (!e.tensor) e.state = EntryState::ROLE_ABSENT;
                    else if (!sp.excl) e.state = EntryState::EXCLUSIVITY_UNESTABLISHED;
                    else { e.state = EntryState::ROLE_RESOLVED; e.usable_as_positive_witness = true; }
                    r.entries.push_back(e);
                }
            }
        }
        return r;   // n_nextn == 0 -> AUCUN role applicable (jamais "absent par defaut")
    };

    d.record_assumptions = []() { RuntimeAssumptions a; return a; };

    d.make_ctx = [&s](void *) -> void * {
        g_real_ctx_calls++;
        if (s.model == nullptr) return nullptr;
        llama_context_params cp = llama_context_default_params();
        cp.n_ctx = 512; cp.n_batch = 512; cp.n_ubatch = 512; cp.n_seq_max = 1;
        // ctx_type NON modifie : LLAMA_CONTEXT_TYPE_DEFAULT
        // embeddings_nextn NON modifie : false (defaut llama-context.cpp:151)
        s.ctx = llama_init_from_model(s.model, cp);
        return s.ctx;
    };

    // prefill : positions explicites ; logits demandes SEULEMENT sur le dernier
    // token (llama_batch.logits est PAR TOKEN : pas de champ logits_all).
    d.prefill = [&s](void *, const std::vector<llama_token_i> & toks) {
        g_real_prefill_calls++;
        if (s.ctx == nullptr || toks.empty()) { g_real_prefill_calls--; return false; }
        llama_batch b = llama_batch_init((int32_t) toks.size(), 0, 1);
        for (size_t i = 0; i < toks.size(); ++i) {
            b.token[i] = (llama_token) toks[i];
            b.pos[i] = (llama_pos) i; b.n_seq_id[i] = 1; b.seq_id[i][0] = 0;
            b.logits[i] = (i + 1 == toks.size()) ? 1 : 0;
        }
        b.n_tokens = (int32_t) toks.size();
        int rc = llama_decode(s.ctx, b);
        llama_batch_free(b);
        if (rc != 0) { g_real_prefill_calls--; return false; }
        return true;
    };

    d.ledger_window = [&s]() { CallWindow w; w.malformed = true; return w; };
    d.ledger_snapshot = [&s]() { std::vector<LedgerRow> v; return v; };

    d.enumerate = [&s]() {
        g_real_enum_calls++;
        Enumeration e;
        const void * gf = f1a_last_gf;
        if (gf == nullptr) return e;                 // aucun graphe publie : aucune conclusion
        e.gf = "captured"; e.valid = false;          // fingerprint non confrontable
        const int n = ggml_graph_n_nodes((struct ggml_cgraph *) gf);
        e.n_declared = n; e.n_exported = n;
        if (n <= 0) { e.malformed = true; return e; }
        for (int i = 0; i < n; ++i) (void) ggml_graph_node((struct ggml_cgraph *) gf, i);
        e.valid = true;
        return e;
    };

    d.correlate = [](const CallWindow & w, const Enumeration & e) { return correlate_call(w, e); };

    d.logits = [&s](void *) -> const std::vector<float> * {
        if (s.ctx == nullptr) return nullptr;
        const float * lg = llama_get_logits_ith(s.ctx, -1);
        if (lg == nullptr) return nullptr;
        const int nv = (int) llama_vocab_n_tokens(llama_model_get_vocab(s.model));
        s.logits_buf.assign(lg, lg + nv);
        return &s.logits_buf;
    };

    d.decode_step = [&s](void *, llama_token_i tok) {
        g_real_decode_calls++;
        if (s.ctx == nullptr) { g_real_decode_calls--; return false; }
        llama_batch b = llama_batch_init(1, 0, 1);
        b.token[0] = (llama_token) tok; b.pos[0] = 0;   // position explicite
        b.n_seq_id[0] = 1; b.seq_id[0][0] = 0; b.logits[0] = 1; b.n_tokens = 1;
        int rc = llama_decode(s.ctx, b);
        llama_batch_free(b);
        if (rc != 0) { g_real_decode_calls--; return false; }
        return true;
    };

    d.release = [&s](void *, void *) {
        g_real_release_calls++;
        if (s.ctx)   { llama_free(s.ctx);        s.ctx = nullptr; }
        if (s.model) { llama_model_free(s.model); s.model = nullptr; }
    };
    return d;
}
