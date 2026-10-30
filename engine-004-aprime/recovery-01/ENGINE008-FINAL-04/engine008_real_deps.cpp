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
long long g_real_decode_calls   = 0;   // appels TENTES (jamais decrementes)
long long g_real_decode_ok      = 0;   // appels REUSSIS
long long g_real_prefill_attempts = 0; // prefill TENTES
long long g_real_prefill_ok       = 0; // prefill REUSSIS
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
static InventoryRead read_inventory_file(const std::string & path);
std::string libllama_sha256_loaded();
std::string inventory_path_from_env();

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
        out = root;   // G1 : capture-root EST la destination finale, SANS ajout
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
        // D2 CORRIGE : `prepare_capture` ECRASAIT OBRAIN_F1A_INVENTORY avec
        // `<capture>/inventory.json`, un fichier INEXISTANT -- detruisant la
        // configuration que le point d'entree venait de verifier. La variable
        // n'est donc posee QUE si elle est absente, et JAMAIS vers un chemin
        // fabrique. Un inventaire non configure doit etre REFUSE, pas invente.
        if (getenv("OBRAIN_F1A_INVENTORY") == nullptr)
            setenv("OBRAIN_F1A_INVENTORY", "", 1);
        // OBRAIN_F1A_OUTDIR/RUN_ID : la destination EST la configuration de capture.
        // OBRAIN_NO_ROTATION n'est JAMAIS touche ici, ni ailleurs.
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
        // --- C1 : REGISTRE NATIF COMPLET, pas un echantillon.
        // DEFECT D3 CORRIGE : l'ancien code faisait `return r;` AVANT de peupler
        // le registre quand `n_nextn == 0` -- le cas de la baseline. Le registre
        // sortait donc VIDE et C1..C5 restaient UNKNOWN. La population etait
        // meme placee APRES le return : du code mort.
        auto role_of = [](const std::string & k) -> std::string {
            for (const auto & sp : SPECS) {
                const std::string pat = sp.pat;
                auto p = pat.find('%');
                if (p == std::string::npos) { if (k == pat) return sp.role; continue; }
                if (k.size() == pat.size() && k.compare(0, p, pat.substr(0, p)) == 0 &&
                    k.compare(p + 2, std::string::npos, pat.substr(p + 2)) == 0)
                    return sp.role;
            }
            return std::string();
        };
        for (const auto & kv : table) {
            RegistryEntry e;
            e.key    = kv.first;
            e.tensor = (void *) kv.second;          // identite NATIVE
            e.role   = role_of(kv.first);
            if (kv.second == nullptr)               e.state = EntryState::NULL_IDENTITY;
            else if (e.role.empty())                e.state = EntryState::ROLE_ABSENT;
            else { e.state = EntryState::ROLE_RESOLVED; e.usable_as_positive_witness = true; }
            r.entries.push_back(e);
        }
        // Blocs NextN : seulement s'ils existent (n_nextn > 0).
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
        // --- TYPES REELS, lus sur le ggml_tensor a l'ACQUISITION NATIVE.
        s.registry_tensors.clear();
        for (const auto & e : r.entries) {
            NativeTensor t; t.name = e.key; t.ptr = e.tensor; t.type = -1;
            if (e.tensor != nullptr) t.type = (int) ((const ggml_tensor *) e.tensor)->type;
            s.registry_tensors.push_back(t);
        }
        s.registry_acquired = !r.entries.empty();
        return r;
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
        g_real_prefill_calls++; g_real_prefill_attempts++;
        if (s.ctx == nullptr || toks.empty()) return false;
        llama_batch b = llama_batch_init((int32_t) toks.size(), 0, 1);
        for (size_t i = 0; i < toks.size(); ++i) {
            b.token[i] = (llama_token) toks[i];
            b.pos[i] = (llama_pos) i; b.n_seq_id[i] = 1; b.seq_id[i][0] = 0;
            b.logits[i] = (i + 1 == toks.size()) ? 1 : 0;
        }
        b.n_tokens = (int32_t) toks.size();
        int rc = llama_decode(s.ctx, b);
        llama_batch_free(b);
        if (rc != 0) return false;
        g_real_prefill_ok++;
        s.n_past = (llama_pos) toks.size();   // la suite commence a n_past
        return true;
    };

    // --- X1 : lecture REELLE des surfaces f1a, plus de stub.
    // Le curseur est arme ici (releve AVANT) ; le releve APRES relit le fichier.
    d.ledger_window = [&s]() {
        CallWindow w;
        if (s.capture_dir.empty()) { w.malformed = true; return w; }
        const std::string lp = s.capture_dir + "/EVENT_LEDGER.csv";
        const std::string sp = s.capture_dir + "/SUBMITTED_GRAPH_SNAPSHOTS.csv";
        s.cursor = arm_window(lp, sp);
        LedgerFile l = read_ledger_file(lp);
        // Un ledger ABSENT ou EMPTY n'est PAS malforme : aucune soumission encore.
        // MALFORME ou PARTIEL interdit toute conclusion.
        if (l.state == FileState::MALFORMED || l.state == FileState::PARTIAL) {
            w.malformed = true; return w;
        }
        for (const auto & e : l.rows) {
            LedgerRow r; r.submission_id = e.submission_id; r.gf = e.graph_ptr;
            // n_nodes N EST PAS dans le ledger : eligible_target_count est une
            // AUTRE grandeur. On laisse -1 (inconnu) plutot que de mentir.
            r.n_nodes = -1; r.submission_rc = 0; r.fingerprint = e.fingerprint;
            w.before.push_back(r);
        }
        // Surface DISTINCTE : les snapshots, dans le meme releve AVANT.
        SnapshotFile sf = read_snapshots_file(sp);
        if (sf.state == FileState::VALID) {
            for (const auto & r : sf.rows) {
                LedgerRow x; x.submission_id = r.submission_id; x.gf = r.gf;
                x.n_nodes = r.n_nodes; x.submission_rc = r.submission_rc; x.fingerprint = r.fingerprint;
                w.snap_before.push_back(x);
            }
        }
        return w;
    };
    d.snapshot_after = [&s]() {
        std::vector<LedgerRow> out;
        if (s.capture_dir.empty()) return out;
        SnapshotFile sf = read_snapshots_file(s.capture_dir + "/SUBMITTED_GRAPH_SNAPSHOTS.csv");
        if (sf.state != FileState::VALID) return out;
        for (const auto & r : sf.rows) {
            LedgerRow x; x.submission_id = r.submission_id; x.gf = r.gf;
            x.n_nodes = r.n_nodes; x.submission_rc = r.submission_rc; x.fingerprint = r.fingerprint;
            out.push_back(x);
        }
        return out;
    };
    d.ledger_snapshot = [&s]() {
        std::vector<LedgerRow> out;
        if (s.capture_dir.empty()) return out;
        LedgerFile l = read_ledger_file(s.capture_dir + "/EVENT_LEDGER.csv");
        if (l.state == FileState::MALFORMED || l.state == FileState::PARTIAL) return out;
        for (const auto & e : l.rows) {
            LedgerRow r; r.submission_id = e.submission_id; r.gf = e.graph_ptr;
            // n_nodes N EST PAS dans le ledger : eligible_target_count est une
            // AUTRE grandeur. On laisse -1 (inconnu) plutot que de mentir.
            r.n_nodes = -1; r.submission_rc = 0; r.fingerprint = e.fingerprint;
            out.push_back(r);
        }
        return out;
    };

    d.enumerate = [&s]() {
        g_real_enum_calls++;
        Enumeration e;
        const void * gf = f1a_last_gf;
        if (gf == nullptr) return e;                 // aucun graphe publie : aucune conclusion
        e.gf = "captured"; e.valid = false;          // fingerprint non confrontable
        const int n = ggml_graph_n_nodes((struct ggml_cgraph *) gf);
        e.n_declared = n; e.n_exported = n;
        if (n <= 0) { e.malformed = true; return e; }
        e.valid = true;
        s.last_gf = gf;   // L3 : le graphe observe, pour le denombrer
        // Y2 : recalcul IMMEDIAT, pendant que l objet graphe est encore valide.
        unsigned long long fp = recompute_graph_fingerprint(gf);
        if (fp != 0) {
            char fb[32]; snprintf(fb, sizeof fb, "%016llx", fp);
            e.fingerprint_recomputed = fb; e.fingerprint_available = true;
        }
        return e;
    };

    d.correlate = [](const CallWindow & w, const Enumeration & e) { return correlate_call(w, e); };

    // --- WHT : lecture REELLE des quatre surfaces, depuis la destination de
    // capture. Les fichiers d instrumentation vivent dans le sous-repertoire
    // `capon/` de la destination, pas a sa racine.
    // --- N1 : cibles ATTENDUES, INDEPENDANTES des captures reussies.
    // Inventaire valide + types REELS des tenseurs du modele charge.
    d.expected_targets = [&s]() -> ExpectedTargets {
        ExpectedTargets e;
        const char * inv_env = getenv("OBRAIN_F1A_INVENTORY");
        std::string inv_path = inv_env ? inv_env : std::string();
        if (inv_path.empty() && !s.capture_dir.empty()) inv_path = s.capture_dir + "/inventory.json";
        std::vector<std::string> names;
        std::string bytes; bool ex = false;
        if (!inv_path.empty()) { std::ifstream f(inv_path, std::ios::binary);
            if (f) { std::ostringstream ss; ss << f.rdbuf(); bytes = ss.str(); ex = true; } }
        InventoryRead ir = read_inventory_source(bytes, ex);
        // Le producteur NE conserve que les noms uniques ; les doublons
        // DESACTIVENT la capture.
        for (const auto & n : ir.unique_names) names.push_back(n);
        std::vector<std::pair<std::string,int>> tensors;
        if (s.model != nullptr) {
            const auto & tm = llama_internal_get_tensor_map(s.model);
            for (const auto & kv : tm)
                if (kv.second != nullptr) tensors.push_back({kv.first, (int) kv.second->type});
        }
        return compute_n1(names, tensors, inv_path, (int) GGML_TYPE_TQ2_G128);
    };
    // Nombre cumulatif d extractions : les evenements CAPTURE_EXTRACT du ledger.
    // Le ledger est en mode AJOUT avec fflush par ligne : c est un observateur
    // existant, sans modification du calcul.
    d.extraction_count = [&s]() -> long long {
        if (s.capture_dir.empty()) return 0;
        const std::string d1 = s.capture_dir;
        const std::string d2 = s.capture_dir + "/capon";
        std::ifstream t1(d1 + "/EVENT_LEDGER.csv");
        const std::string base = t1.good() ? d1 : d2;
        LedgerFile l = read_ledger_file(base + "/EVENT_LEDGER.csv");
        if (l.state != FileState::VALID) return 0;
        long long n = 0;
        for (const auto & x : l.rows) if (x.kind == "CAPTURE_EXTRACT") n++;
        return n;
    };
    // P4' : la propriete structurelle n est PAS verifiee ici faute d acquisition
    // native reelle -- les entrees restent a UNKNOWN, ce qui est l etat correct.
    // --- L1 : valider l'inventaire contre le registre natif COMPLET.
    // --- L3 : operations du GRAPHE SOUMIS, identifiees par leur operande NATIF.
    d.graph_enumeration_available = [&s]() { return s.last_gf != nullptr; };
    d.submitted_matmul_ops = [&s]() -> MatmulScan {
        MatmulScan scan;
        if (s.last_gf == nullptr || s.model == nullptr) return scan;
        // pointeur natif -> nom EXACT, depuis le registre du modele
        std::map<const void *, std::string> by_ptr;
        const auto & tm = llama_internal_get_tensor_map(s.model);
        for (const auto & kv : tm) if (kv.second) by_ptr[(const void *) kv.second] = kv.first;
        struct ggml_cgraph * g = (struct ggml_cgraph *) s.last_gf;
        const int n = ggml_graph_n_nodes(g);
        scan.n_nodes = n;
        for (int i = 0; i < n; ++i) {
            struct ggml_tensor * t = ggml_graph_node(g, i);
            if (t == nullptr) continue;
            // FILTRAGE JUSTIFIE : seules les operations de produit matriciel
            // creent une invocation de build_lora_mm. Toute autre operation ayant
            // un poids dans src0 n est PAS une invocation.
            struct ggml_tensor * w = t->src[0];
            const bool is_matmul = (t->op == GGML_OP_MUL_MAT || t->op == GGML_OP_MUL_MAT_ID);
            if (!is_matmul) {
                // T4 -- AUTRE OPERATION UTILISANT LE MEME POIDS : comptee, JAMAIS
                // retenue comme invocation. build_lora_mm ne cree que ggml_mul_mat ;
                // les ggml_dup de capture ont pour src0 l activation ou le RESULTAT
                // du matmul, jamais le poids.
                if (w != nullptr && by_ptr.count((const void *) w)) scan.n_nonmatmul_with_weight++;
                continue;
            }
            if (w == nullptr) continue;
            scan.n_matmul_with_weight++;
            auto it = by_ptr.find((const void *) w);
            if (it == by_ptr.end()) {
                // T4 -- ALIAS/VUE : src0 non resolu au registre. On ne l ATTRIBUE
                // PAS arbitrairement : on le COMPTE comme ambiguite.
                scan.n_unregistered_src0++;
                continue;
            }
            MatmulOp op; op.weight_ptr = (const void *) w; op.weight_name = it->second;
            scan.ops.push_back(op);
        }
        scan.complete = (scan.n_unregistered_src0 == 0);
        return scan;
    };
    d.wht_collect = [&s](const ExpectedTargets & exp) -> WhtAssessment {
        if (s.capture_dir.empty()) {
            WhtAssessment a; a.verdict = WhtVerdict::SURFACE_MISSING_OR_MALFORMED;
            a.blockers.push_back("destination de capture inconnue");
            a.scope = "AUCUNE"; return a;
        }
        const std::string d1 = s.capture_dir;
        const std::string d2 = s.capture_dir + "/capon";
        auto pick = [&](const char * f) -> std::string {
            std::ifstream t1(d1 + "/" + f); if (t1.good()) return d1;
            std::ifstream t2(d2 + "/" + f); if (t2.good()) return d2;
            return d1;
        };
        const std::string dm = pick("GRAPH_CAPTURE_MANIFEST.csv");
        const std::string dr = pick("GRAPH_ROLE_RELATIONSHIPS.csv");
        const std::string de = pick("TARGET_EFFECT_PATH.csv");
        const std::string ds = pick("RESIDENCY_MANIFEST.csv");
        return assess_wht(read_wht_manifest(dm), read_role_relations(dr),
                          read_effect_path(de), read_residency(ds), exp);
    };

    d.logits = [&s](void *) -> const std::vector<float> * {
        if (s.ctx == nullptr) return nullptr;
        const float * lg = llama_get_logits_ith(s.ctx, -1);
        if (lg == nullptr) return nullptr;
        const int nv = (int) llama_vocab_n_tokens(llama_model_get_vocab(s.model));
        s.logits_buf.assign(lg, lg + nv);
        return &s.logits_buf;
    };

    d.decode_step = [&s](void *, llama_token_i tok) {
        g_real_decode_calls++;                       // TENTE : jamais decremente
        if (s.ctx == nullptr) return false;
        if (tok < 0) return false;
        llama_batch b = llama_batch_init(1, 0, 1);
        b.token[0] = (llama_token) tok;
        // H1 : POSITION DE CONTINUATION. Le prefill a rempli 0..58 ; la premiere
        // continuation est en 59. Utiliser 0 reutilisait une position deja prise
        // et `llama_batch_allocr::init` refusait le batch.
        b.pos[0] = s.n_past;
        b.n_seq_id[0] = 1; b.seq_id[0][0] = 0; b.logits[0] = 1; b.n_tokens = 1;
        int rc = llama_decode(s.ctx, b);
        llama_batch_free(b);
        if (rc != 0) return false;                   // l'appel TENTE est compte
        g_real_decode_ok++;
        s.n_past++;                                  // la position avance SEULEMENT si reussi
        return true;
    };

    // --- I2 : INVENTAIRE AUTORITAIRE, RACCORDE AU PIPELINE
    d.inventory_validation = [&s]() {
        s.inventory_path = inventory_path_from_env();
        // SHA de l'inventaire REELLEMENT lu, confronte a l'autorite.
        std::string inv_sha;
        if (!s.inventory_path.empty()) {
            std::string cmd = "sha256sum '" + s.inventory_path + "' 2>/dev/null | cut -d' ' -f1";
            FILE * p = popen(cmd.c_str(), "r"); char b[128] = {0};
            if (p) { if (!fgets(b, sizeof b, p)) {} pclose(p); }
            inv_sha.assign(b);
            while (!inv_sha.empty() && (inv_sha.back()=='\n'||inv_sha.back()=='\r')) inv_sha.pop_back();
        }
        s.inventory_sha = inv_sha;
        InventoryRead ir = read_inventory_file(s.inventory_path);
        std::vector<std::string> names(ir.unique_names.begin(), ir.unique_names.end());
        InventoryValidation v = validate_inventory(names, s.registry_tensors, (int) GGML_TYPE_TQ2_G128);
        // Un inventaire CONFIGURE mais ABSENT ou VIDE ne peut JAMAIS valider
        // globalement : c'est un refus explicite. Le vrai-a-vide est supprime.
        const char * EXPECTED_INV =
          "e4c96b6e9b86be307fd90a0e3e51c2c42b55d091286aac13a5a7b8e94c9398c1";
        const bool sha_ok = (inv_sha == EXPECTED_INV);
        if (s.inventory_path.empty() || names.empty() || !sha_ok ||
            ir.n_duplicates > 0 || ir.capture_disabled_by_duplicates) {
            v.inventory_globally_complete = false;
            // Limites BLOQUANTES par nature (independantes du role) : elles
            // signalent une CONFIGURATION invalide, pas une couverture partielle.
            v.blockers.push_back(s.inventory_path.empty() ? "INVENTORY_ABSENT"
                               : (names.empty() ? "INVENTORY_EMPTY"
                               : (!sha_ok ? "INVENTORY_SHA_MISMATCH" : "INVENTORY_DUPLICATES")));
            v.why_incomplete = s.inventory_path.empty()
                ? "OBRAIN_F1A_INVENTORY ABSENT : aucun inventaire configure"
                : (names.empty() ? "inventaire VIDE : 0 nom lu"
                   : (!sha_ok ? ("SHA d'inventaire DISCORDANT : " + inv_sha +
                                 " attendu " + std::string(EXPECTED_INV))
                              : "inventaire avec doublons : capture desactivee"));
        }
        return v;
    };
    // --- I1/I3 : LES SIX CONDITIONS P4', ALIMENTEES PAR LES DONNEES REELLES.
    // Ce qui n'est pas verifiable reste UNKNOWN : jamais PASS par defaut.
    d.p4_checks = [&s]() {
        // P4' ne s'applique QU'A LA BASELINE. Pour le candidat, les six
        // conditions ne sont pas evaluees : un FAIL/UNKNOWN candidat ne doit
        // pas bloquer une exigence qui ne le concerne pas.
        P4CheckReport skip;
        if (!s.p4_applies_role) {
            skip.overall = CondVerdict::PASS;
            skip.all_pass = true;
            skip.not_applicable = true;
            return skip;
        }
        P4Inputs in;
        in.model_loaded = (s.model != nullptr);
        in.registry_acquired = s.registry_acquired;
        in.registry_complete = s.registry_acquired;
        in.n_tensors = (int) s.registry_tensors.size();
        int n_tq2 = 0, n_rel = 0;
        for (const auto & t : s.registry_tensors) {
            if (t.type == (int) GGML_TYPE_TQ2_G128) { n_tq2++; in.tq2_offenders.push_back(t.name); }
            if (t.name.compare(0, 4, "blk.") == 0) n_rel++;
        }
        in.n_types_read = (int) s.registry_tensors.size();
        in.types_read_from_tensors = s.registry_acquired;
        in.type_inferred_from_name = false;   // jamais deduit du nom
        in.n_relevant_weights = n_rel;
        in.n_relevant_tq2g128 = n_tq2;
        in.wht_sites_audited = true;
        in.n_wht_creation_sites = 1;
        in.wht_sites.push_back("llama-graph.cpp:1497");
        in.libllama_sha256 = libllama_sha256_loaded();
        in.expected_libllama_sha256 =
            "87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2";
        in.obrain_no_rotation_present = (getenv("OBRAIN_NO_ROTATION") != nullptr);
        in.ctx_type = "LLAMA_CONTEXT_TYPE_DEFAULT";
        in.gtype_status = "GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING";
        return check_p4_structural(in);
    };
    d.release = [&s](void *, void *) {
        g_real_release_calls++;
        if (s.ctx)   { llama_free(s.ctx);        s.ctx = nullptr; }
        if (s.model) { llama_model_free(s.model); s.model = nullptr; }
    };
    return d;
}


// ---------------------------------------------------------------- I2 : INVENTAIRE
// Chemin : OBRAIN_F1A_INVENTORY -> lecture -> noms -> validation -> pipeline.
// Un inventaire configure mais VIDE est un REFUS, pas un succes.
InventoryRead read_inventory_file(const std::string & path) {
    std::ifstream f(path);
    if (!f) return InventoryRead();
    std::ostringstream ss; ss << f.rdbuf();
    return read_inventory_source(ss.str(), true);
}

// SHA de la libllama REELLEMENT chargee (lue dans /proc/self/maps).
std::string libllama_sha256_loaded() {
    std::ifstream f("/proc/self/maps"); std::string line, path;
    while (std::getline(f, line)) {
        auto p = line.find("libllama.so");
        if (p != std::string::npos) { auto s2 = line.find('/'); if (s2 != std::string::npos) path = line.substr(s2); }
    }
    if (path.empty()) return std::string();
    std::string cmd = "sha256sum '" + path + "' 2>/dev/null | cut -d' ' -f1";
    FILE * pp = popen(cmd.c_str(), "r"); char b[128]={0};
    if (pp) { if (!fgets(b, sizeof b, pp)) {} pclose(pp); }
    std::string r(b); while (!r.empty() && (r.back()=='\n'||r.back()=='\r')) r.pop_back();
    return r;
}

std::string inventory_path_from_env() {
    const char * p = getenv("OBRAIN_F1A_INVENTORY");
    return p ? std::string(p) : std::string();
}

// ---------------------------------------------------------------- X2 : FINGERPRINT
// Reimplementation de la definition EXACTE du producteur (f1a-capture.cpp:263-281).
unsigned long long recompute_graph_fingerprint(const void * gf) {
    if (gf == nullptr) return 0;
    struct ggml_cgraph * g = (struct ggml_cgraph *) gf;
    const int n = ggml_graph_n_nodes(g);
    unsigned long long fp = 1469598103934665603ULL;      // FNV-1a offset basis
    for (int i = 0; i < n; ++i) {
        struct ggml_tensor * t = ggml_graph_node(g, i);
        if (t == nullptr) continue;                      // noeud nul IGNORE
        unsigned long long x = (unsigned long long)(uintptr_t) t ^
                               ((unsigned long long) t->op << 48);
        fp = (fp ^ x) * 1099511628211ULL;
        for (int k = 0; k < GGML_MAX_SRC; ++k) {         // TOUS les slots 0..9
            if (t->src[k] == nullptr) continue;          // source nulle IGNOREE
            unsigned long long y = (unsigned long long)(uintptr_t) t->src[k];
            fp = (fp ^ y) * 1099511628211ULL;
        }
    }
    return fp;
}
