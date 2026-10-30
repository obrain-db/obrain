// ENGINE-007B-F1A-S2bis-I2 — implementations du registre de capture.
//
// INERTE sans OBRAIN_F1A_CAPTURE=1.
//
// INVENTAIRE: f1a_eligible() exige le TYPE TQ2_G128 ET le NOM exact present dans le CSV
// autoritatif (paragraphe 7). Aucune heuristique de prefixe ni de role.

#include "f1a-capture.h"
#include "ggml-backend.h"

#include <sys/stat.h>

static const char * F1A_INVENTORY_ENV = "OBRAIN_F1A_INVENTORY";
static const char * F1A_OUTDIR_ENV    = "OBRAIN_F1A_OUTDIR";
static const char * F1A_ENABLE_ENV    = "OBRAIN_F1A_CAPTURE";

// ---------------------------------------------------------------- inventaire

namespace {
struct f1a_state {
    bool inited   = false;
    bool enabled  = false;
    std::string inventory_path;
    std::string outdir;
    std::unordered_set<std::string> eligible_names;
    size_t loaded = 0;
    size_t dup    = 0;
};
f1a_state g_f1a;
}

// charge le CSV fige : colonne 0 = tensor_name. En-tete detecte par "tensor_name".
static void f1a_load_inventory() {
    if (g_f1a.inited) return;
    g_f1a.inited = true;
    const char * en = getenv(F1A_ENABLE_ENV);
    g_f1a.enabled = (en != nullptr && en[0] == '1');
    const char * inv = getenv(F1A_INVENTORY_ENV);
    const char * out = getenv(F1A_OUTDIR_ENV);
    g_f1a.inventory_path = inv ? inv : "";
    g_f1a.outdir         = out ? out : "/tmp/f1a-out";
    if (!g_f1a.enabled) return;                       // rien n'est lu quand la capture est off
    if (g_f1a.inventory_path.empty()) {
        fprintf(stderr, "[f1a] %s=1 mais %s non defini : capture DESACTIVEE\n",
                F1A_ENABLE_ENV, F1A_INVENTORY_ENV);
        g_f1a.enabled = false;
        return;
    }
    FILE * f = fopen(g_f1a.inventory_path.c_str(), "r");
    if (f == nullptr) {
        fprintf(stderr, "[f1a] inventaire illisible : %s : capture DESACTIVEE\n", g_f1a.inventory_path.c_str());
        g_f1a.enabled = false;
        return;
    }
    char line[1024];
    while (fgets(line, sizeof(line), f)) {
        std::string s(line);
        while (!s.empty() && (s.back() == '\n' || s.back() == '\r')) s.pop_back();
        if (s.empty()) continue;
        if (s.compare(0, 11, "tensor_name") == 0) continue;   // en-tete
        if (s[0] == '#') continue;                            // commentaire
        const size_t c = s.find(',');
        const std::string name = (c == std::string::npos) ? s : s.substr(0, c);
        if (name.empty()) continue;
        if (!g_f1a.eligible_names.insert(name).second) g_f1a.dup++;
    }
    fclose(f);
    g_f1a.loaded = g_f1a.eligible_names.size();
    fprintf(stderr, "[f1a] inventaire charge : %zu noms eligibles, %zu doublons, fichier=%s\n",
            g_f1a.loaded, g_f1a.dup, g_f1a.inventory_path.c_str());
    if (g_f1a.dup != 0) {
        fprintf(stderr, "[f1a] DOUBLONS DANS L INVENTAIRE : capture DESACTIVEE\n");
        g_f1a.enabled = false;
    }
}

f1a_capture_ctx * f1a_capture_get_ctx() {
    f1a_load_inventory();
    if (!g_f1a.enabled) return nullptr;
    // Une epoque par execution de graphe : creee ici, finalisee par f1a_capture_end_epoch().
    static thread_local f1a_capture_ctx ctx;
    return &ctx;
}

bool f1a_eligible(const ggml_tensor * w) {
    if (!g_f1a.enabled || w == nullptr) return false;
    if (w->type != GGML_TYPE_TQ2_G128) return false;
    if (w->name[0] == '\0') return false;
    return g_f1a.eligible_names.find(std::string(w->name)) != g_f1a.eligible_names.end();
}

// ---------------------------------------------------------------- registre

static int f1a_family_of(int64_t K) {
    if (K == 5120) return 5;
    if (K == 6144) return 6;
    return 0;
}

void f1a_capture_note(f1a_capture_ctx * cc,
                      const ggml_tensor * w,
                      const ggml_tensor * cur_raw,
                      const ggml_tensor * cur_used,
                      const ggml_tensor * cur_capture,
                      const ggml_tensor * mm_raw,
                      const ggml_tensor * mm_capture) {
    if (cc == nullptr) return;
    f1a_target_rec r{};
    r.target_id  = cc->next_target_id++;
    r.epoch      = cc->epoch;
    r.weight_name= w->name;
    r.weight_type= (int) w->type;
    for (int i = 0; i < 4; ++i) r.weight_ne[i] = w->ne[i];
    r.cur_raw_id = (const void *) cur_raw;
    r.cur_eff_id = (const void *) cur_used;
    r.rotation_applied = (cur_raw != cur_used);
    for (int i = 0; i < 4; ++i) { r.eff_ne[i] = cur_used->ne[i]; r.eff_nb[i] = cur_used->nb[i]; }
    r.eff_bytes  = ggml_nbytes(cur_used);
    r.expected_K = cur_used->ne[0];
    r.family     = f1a_family_of(r.expected_K);
    r.t_cur_capture = const_cast<ggml_tensor *>(cur_capture);
    r.t_mm_raw      = const_cast<ggml_tensor *>(mm_raw);
    r.t_mm_capture  = const_cast<ggml_tensor *>(mm_capture);

    // Deduplication : cle = (identite effective, dtype, ne0, nb0). Reutilisation SEULEMENT
    // si la correspondance est exacte (paragraphe 12).
    char key[256];
    snprintf(key, sizeof(key), "%p|%d|%lld|%lld",
             (const void *) cur_used, (int) cur_used->type,
             (long long) cur_used->ne[0], (long long) cur_used->nb[0]);
    auto it = cc->act_key_to_id.find(key);
    if (it == cc->act_key_to_id.end()) {
        r.activation_capture_id = cc->next_act_id++;
        cc->act_key_to_id.emplace(key, r.activation_capture_id);
    } else {
        r.activation_capture_id = it->second;
    }
    cc->targets.push_back(r);
}

// ---------------------------------------------------------------- fin d'epoque

bool f1a_capture_active(const f1a_capture_ctx * cc) {
    return g_f1a.enabled && cc != nullptr && !cc->targets.empty();
}

// Ecrit un payload binaire en respectant la disposition LOGIQUE du tenseur.
// Contigu -> copie directe. Non contigu -> reconstruction par ne[]/nb[] reels
// (paragraphe 8 : ne jamais supposer nb[i] compact).
static bool f1a_write_payload(const std::string & path, const ggml_tensor * t) {
    FILE * f = fopen(path.c_str(), "wb");
    if (f == nullptr) return false;
    bool ok = true;
    if (ggml_is_contiguous(t)) {
        std::vector<char> buf(ggml_nbytes(t));
        ggml_backend_tensor_get(t, buf.data(), 0, buf.size());
        ok = (fwrite(buf.data(), 1, buf.size(), f) == buf.size());
    } else {
        const size_t ts = ggml_type_size(t->type);
        const int64_t n0 = t->ne[0];
        std::vector<char> row(ggml_row_size(t->type, n0));
        int64_t idx[4];
        for (idx[3] = 0; idx[3] < t->ne[3]; ++idx[3])
        for (idx[2] = 0; idx[2] < t->ne[2]; ++idx[2])
        for (idx[1] = 0; idx[1] < t->ne[1]; ++idx[1]) {
            const size_t off = (size_t) idx[1]*t->nb[1] + (size_t) idx[2]*t->nb[2] + (size_t) idx[3]*t->nb[3];
            ggml_backend_tensor_get(t, row.data(), off, row.size());
            if (fwrite(row.data(), 1, row.size(), f) != row.size()) { ok = false; break; }
        }
        (void) ts;
    }
    fclose(f);
    return ok;
}

void f1a_capture_end_epoch(f1a_capture_ctx * cc, ggml_backend_t backend) {
    if (cc == nullptr) return;
    if (cc->targets.empty()) { cc->epoch++; return; }

    // 1. SYNCHRONISATION avant toute lecture : le graphe a ete soumis en asynchrone.
    if (backend != nullptr) ggml_backend_synchronize(backend);

    if (mkdir(g_f1a.outdir.c_str(), 0755) != 0) { /* existe deja */ }

    // 2. EXTRACTION des payloads, sur les pointeurs de tenseurs vivants.
    const std::string mpath = g_f1a.outdir + "/GRAPH_CAPTURE_MANIFEST.csv";
    FILE * m = fopen(mpath.c_str(), "w");
    if (m == nullptr) {
        fprintf(stderr, "[f1a] manifeste non ouvrable : %s\n", mpath.c_str());
        cc->targets.clear(); cc->act_key_to_id.clear(); cc->epoch++;
        return;
    }
    fprintf(m, "epoch,target_id,capture_id,weight_name,family,expected_K,rotation_applied,"
               "kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous\n");

    size_t n_act = 0, n_out = 0, n_fail = 0;
    for (const auto & r : cc->targets) {
        const ggml_tensor * ts[2] = { r.t_cur_capture, r.t_mm_capture };
        const char * kinds[2] = { "activation", "base_output" };
        for (int k = 0; k < 2; ++k) {
            const ggml_tensor * t = ts[k];
            if (t == nullptr) { n_fail++; continue; }
            char p[512];
            snprintf(p, sizeof(p), "%s/ep%llu_t%d_%s.bin",
                     g_f1a.outdir.c_str(), r.epoch, r.target_id, kinds[k]);
            if (!f1a_write_payload(p, t)) { n_fail++; continue; }
            if (k == 0) n_act++; else n_out++;
            fprintf(m, "%llu,%d,%d,%s,%d,%lld,%d,%s,%s,%d,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%d\n",
                    r.epoch, r.target_id, r.activation_capture_id, r.weight_name.c_str(), r.family,
                    (long long) r.expected_K, r.rotation_applied ? 1 : 0, kinds[k], p,
                    (int) t->type,
                    (long long)t->ne[0],(long long)t->ne[1],(long long)t->ne[2],(long long)t->ne[3],
                    (long long)t->nb[0],(long long)t->nb[1],(long long)t->nb[2],(long long)t->nb[3],
                    (long long) ggml_nelements(t), (long long) ggml_nbytes(t),
                    ggml_is_contiguous(t) ? 1 : 0);
        }
    }
    fclose(m);
    fprintf(stderr, "[f1a] epoque %llu : %zu cibles, %zu payloads activation, %zu sorties, %zu echecs -> %s\n",
            cc->epoch, cc->targets.size(), n_act, n_out, n_fail, mpath.c_str());

    // 3. INVALIDATION du registre : aucune identite ne survit a l epoque.
    cc->targets.clear();
    cc->act_key_to_id.clear();
    cc->next_target_id = 1;
    cc->next_act_id    = 1;
    cc->epoch++;
}
