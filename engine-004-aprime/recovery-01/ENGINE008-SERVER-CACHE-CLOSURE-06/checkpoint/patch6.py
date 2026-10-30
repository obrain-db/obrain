import re
F="/home/triviere/projects/llama-obrain-cuda-f1a"
p=F+"/tools/server/server-context.cpp"
s=open(p,encoding="utf-8",errors="surrogateescape").read()

# ---------- MANIFESTE DE COMPATIBILITE + SHA256 + METRIQUES SEPAREES ----------
anchor='static std::mutex g_kv_states_mtx;'
assert anchor in s, "A1"
man='''// [obrain K1-MANIFEST] MANIFESTE DE COMPATIBILITE associe au blob.
// Un hash de contenu etablit que le contenu est LE MEME ; il n'etablit PAS
// qu'il est COMPATIBLE avec le modele et la configuration courants. Le
// manifeste porte ces elements, qui sont EXTERIEURS au blob.
// Toute donnee necessaire INCONNUE refuse l'import (jamais "compatible par
// defaut").
struct kv_state_manifest {
    std::string model_path;      // identite du modele charge
    size_t      model_bytes   = 0;
    uint64_t    model_params  = 0;
    int32_t     n_vocab       = 0, n_layer = 0, n_embd = 0;
    std::string q1_sha256;       // identite du calcul Q1 (libllama reellement chargee)
    uint32_t    format_magic  = 0;   // format du blob
    size_t      blob_bytes    = 0;
    std::string blob_sha256;         // INTEGRITE cryptographique des octets stockes
    int32_t     n_tokens      = 0;
    int32_t     type_k = 0, type_v = 0;
    uint32_t    n_ctx = 0, n_batch = 0, n_ubatch = 0, n_seq_max = 0;
    int32_t     state_mode    = 0;   // OBRAIN_STATE_TYPE effectif
    int32_t     seq_id        = 0;
    std::vector<llama_token> prefix_ids;   // PREFIXE EXACT
    llama_pos   pos_min = 0, pos_max = 0;
    bool complete() const {
        return !model_path.empty() && model_bytes != 0 && model_params != 0 &&
               n_vocab > 0 && n_layer > 0 && n_embd > 0 && !q1_sha256.empty() &&
               format_magic != 0 && blob_bytes != 0 && !blob_sha256.empty() &&
               n_tokens > 0 && type_k != 0 && type_v != 0 &&
               n_ctx > 0 && n_batch > 0 && n_ubatch > 0 && n_seq_max > 0 &&
               !prefix_ids.empty();
    }
};

static std::string kv_sha256_hex(const uint8_t * d, size_t n) {
    // SHA256 via un fichier temporaire : aucun ajout de dependance.
    static std::atomic<uint64_t> ctr{0};
    char path[64]; snprintf(path, sizeof path, "/tmp/.obrain_kvsha_%llu",
                            (unsigned long long) ctr.fetch_add(1));
    { std::ofstream f(path, std::ios::binary); f.write((const char *) d, (std::streamsize) n); }
    std::string cmd = std::string("sha256sum '") + path + "' 2>/dev/null | cut -d' ' -f1";
    FILE * fp = popen(cmd.c_str(), "r"); char buf[128] = {0};
    if (fp) { if (!fgets(buf, sizeof buf, fp)) {} pclose(fp); }
    remove(path);
    std::string r(buf); while (!r.empty() && (r.back()=='\\n'||r.back()=='\\r')) r.pop_back();
    return r;
}

// SHA256 de la libllama REELLEMENT chargee (une fois par processus).
static const std::string & kv_q1_sha256() {
    static const std::string v = []() -> std::string {
        std::ifstream f("/proc/self/maps"); std::string line, path;
        while (std::getline(f, line)) {
            auto q = line.find("libllama.so");
            if (q != std::string::npos) { auto sl = line.find('/'); if (sl != std::string::npos) path = line.substr(sl); }
        }
        while (!path.empty() && (path.back()=='\\n'||path.back()==' ')) path.pop_back();
        if (path.empty()) return std::string();
        std::string cmd = "sha256sum '" + path + "' 2>/dev/null | cut -d' ' -f1";
        FILE * fp = popen(cmd.c_str(), "r"); char b[128] = {0};
        if (fp) { if (!fgets(b, sizeof b, fp)) {} pclose(fp); }
        std::string r(b); while (!r.empty() && (r.back()=='\\n'||r.back()=='\\r')) r.pop_back();
        return r;
    }();
    return v;
}

// Manifeste COURANT, derive du contexte et de la configuration effectifs.
static kv_state_manifest kv_manifest_of(llama_context * ctx, llama_model * model,
                                        const std::string & model_path,
                                        uint32_t n_ctx, uint32_t n_batch, uint32_t n_ubatch,
                                        int32_t seq_id) {
    kv_state_manifest m;
    m.model_path  = model_path;
    m.model_bytes = 0;
    { std::ifstream f(model_path, std::ios::binary | std::ios::ate);
      if (f) m.model_bytes = (size_t) f.tellg(); }
    m.model_params = (uint64_t) llama_model_n_params(model);
    m.n_vocab      = (int32_t) llama_vocab_n_tokens(llama_model_get_vocab(model));
    m.n_layer      = (int32_t) llama_model_n_layer(model);
    m.n_embd       = (int32_t) llama_model_n_embd(model);
    m.q1_sha256    = kv_q1_sha256();
    m.type_k       = (int32_t) llama_kv_cache_type_k(ctx);
    m.type_v       = (int32_t) llama_kv_cache_type_v(ctx);
    m.n_ctx = n_ctx; m.n_batch = n_batch; m.n_ubatch = n_ubatch; m.n_seq_max = 1;
    { const char * e = std::getenv("OBRAIN_STATE_TYPE");
      const char * f16 = std::getenv("OBRAIN_STATE_F16");
      m.state_mode = (e && e[0]) ? (int32_t) e[0] : ((f16 && f16[0]=='1') ? 1 : 0); }
    m.seq_id = seq_id;
    return m;
}

// Comparaison STRICTE. Rend une raison de refus, ou une chaine vide.
static std::string kv_manifest_mismatch(const kv_state_manifest & stored,
                                        const kv_state_manifest & cur) {
    if (!stored.complete())            return "manifeste d'etat INCOMPLET (donnee necessaire inconnue)";
    if (stored.model_path  != cur.model_path)  return "chemin du modele different";
    if (stored.model_bytes != cur.model_bytes) return "taille du modele differente";
    if (stored.model_params!= cur.model_params)return "nombre de parametres different";
    if (stored.n_vocab != cur.n_vocab || stored.n_layer != cur.n_layer ||
        stored.n_embd  != cur.n_embd)          return "topologie du modele differente";
    if (stored.q1_sha256 != cur.q1_sha256)     return "identite du calcul Q1 differente";
    if (stored.format_magic == 0)              return "format d'etat inconnu";
    if (stored.type_k != cur.type_k || stored.type_v != cur.type_v)
                                               return "types KV differents";
    if (stored.state_mode != cur.state_mode)   return "mode de serialisation different";
    if (stored.n_ctx != cur.n_ctx || stored.n_batch != cur.n_batch ||
        stored.n_ubatch != cur.n_ubatch || stored.n_seq_max != cur.n_seq_max)
                                               return "configuration de contexte differente";
    if (stored.seq_id != cur.seq_id)           return "convention de sequence differente";
    return std::string();
}

'''
s=s.replace(anchor, man+anchor, 1)

# ---------- le blob porte son manifeste ----------
old_struct='struct kv_state_blob { uint32_t n_tokens = 0; std::vector<uint8_t> data; };'
assert old_struct in s, "A2 blob"
s=s.replace(old_struct,'struct kv_state_blob { uint32_t n_tokens = 0; std::vector<uint8_t> data;\n                       std::shared_ptr<kv_state_manifest> manifest; };',1)

# ---------- REFUS AVANT set_data ----------
old_set='                        const size_t rc = llama_state_seq_set_data(ctx, blob.data.data(), blob.data.size(), seq_id);'
assert old_set in s, "A3 set_data"
new_pre = '''                        // [obrain K1-MANIFEST] COMPATIBILITE VERIFIEE AVANT set_data.
                        // Toute incompatibilite, ou toute donnee necessaire inconnue,
                        // REFUSE l'import -- aucun set_data n'est tente.
                        {
                            const kv_state_manifest cur = kv_manifest_of(
                                ctx, ctx_server.model_tgt, ctx_server.params_base.model,
                                (uint32_t) llama_n_ctx(ctx), (uint32_t) llama_n_batch(ctx),
                                (uint32_t) llama_n_ubatch(ctx), seq_id);
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
                            const std::string sh = kv_sha256_hex(blob.data.data(), blob.data.size());
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
'''
s=s.replace(old_set, new_pre+old_set,1)
open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("PATCH 6a : manifeste + SHA256 + refus avant set_data")
