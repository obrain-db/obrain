// obrain-kv-cache.h -- [obrain K1/K2] CACHE D'ETATS HYBRIDES.
// EXTRAIT de server-context.cpp pour que le serveur ET les tests incluent le
// MEME code : les tests n'exercent pas une copie simplifiee des regles.
// Aucune dependance a llama.h : les types d'etat sont des entiers.
#pragma once
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <list>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>
#include <unistd.h>

namespace obrain_kv {

// Manifeste de COMPATIBILITE. Un hash de contenu etablit que le contenu est LE
// MEME ; il n'etablit PAS qu'il est COMPATIBLE. Le manifeste porte ces elements,
// qui sont EXTERIEURS au blob.
struct Manifest {
    std::string model_desc;        // description du modele charge
    std::string model_sha256;      // IDENTITE EXACTE du fichier de modele
    size_t      model_bytes   = 0;
    uint64_t    model_params  = 0;
    int32_t     n_vocab = 0, n_layer = 0, n_embd = 0;
    std::string q1_sha256;         // identite du calcul Q1
    uint32_t    format_magic  = 0;
    size_t      blob_bytes    = 0;
    std::string blob_sha256;       // integrite cryptographique des octets
    int32_t     n_tokens      = 0;
    int32_t     type_k = 0, type_v = 0;
    uint32_t    n_ctx = 0, n_batch = 0, n_ubatch = 0, n_seq_max = 0;
    int32_t     state_mode    = 0;
    int32_t     seq_id        = 0;
    std::vector<int32_t> prefix_ids;
    int64_t     pos_min = 0, pos_max = 0;

    // Toute donnee necessaire INCONNUE => manifeste INCOMPLET => refus.
    bool complete() const {
        return !model_desc.empty() && !model_sha256.empty() && model_bytes != 0 &&
               model_params != 0 && n_vocab > 0 && n_layer > 0 && n_embd > 0 &&
               !q1_sha256.empty() && format_magic != 0 && blob_bytes != 0 &&
               !blob_sha256.empty() && n_tokens > 0 && type_k != 0 && type_v != 0 &&
               n_ctx > 0 && n_batch > 0 && n_ubatch > 0 && n_seq_max > 0 &&
               !prefix_ids.empty();
    }
};

struct Blob {
    uint32_t n_tokens = 0;
    std::vector<uint8_t> data;
    std::shared_ptr<Manifest> manifest;
};

inline std::string sha256_hex(const uint8_t * d, size_t n) {
    static std::atomic<uint64_t> ctr{0};
    char path[80];
    snprintf(path, sizeof path, "/tmp/.obrain_kvsha_%d_%llu", (int) getpid(),
             (unsigned long long) ctr.fetch_add(1));
    { std::ofstream f(path, std::ios::binary); f.write((const char *) d, (std::streamsize) n); }
    std::string cmd = std::string("sha256sum '") + path + "' 2>/dev/null | cut -d' ' -f1";
    FILE * fp = popen(cmd.c_str(), "r"); char buf[128] = {0};
    if (fp) { if (!fgets(buf, sizeof buf, fp)) {} pclose(fp); }
    remove(path);
    std::string r(buf);
    while (!r.empty() && (r.back() == '\n' || r.back() == '\r')) r.pop_back();
    return r;
}

// SHA256 d'un FICHIER (utilise pour l'identite du modele et de Q1).
inline std::string sha256_hex_file(const std::string & path) {
    std::string cmd = "sha256sum '" + path + "' 2>/dev/null | cut -d' ' -f1";
    FILE * fp = popen(cmd.c_str(), "r"); char buf[128] = {0};
    if (fp) { if (!fgets(buf, sizeof buf, fp)) {} pclose(fp); }
    std::string r(buf);
    while (!r.empty() && (r.back() == '\n' || r.back() == '\r')) r.pop_back();
    return r;
}

inline uint64_t fnv1a64(const std::vector<uint8_t> & d) {
    uint64_t h = 0xcbf29ce484222325ULL;
    for (uint8_t b : d) { h ^= b; h *= 0x100000001b3ULL; }
    return h;
}

inline std::string ref_of(const std::vector<uint8_t> & data) {
    char buf[17]; snprintf(buf, sizeof buf, "%016llx", (unsigned long long) fnv1a64(data));
    return std::string(buf);
}

// Comparaison STRICTE du manifeste. Rend la raison du refus, ou "" si compatible.
inline std::string manifest_mismatch(const Manifest & stored, const Manifest & cur) {
    if (!stored.complete())          return "manifeste d'etat INCOMPLET (donnee necessaire inconnue)";
    if (stored.model_sha256.empty()) return "identite SHA256 du modele inconnue";
    if (stored.model_sha256 != cur.model_sha256)
                                     return "SHA256 du modele DIFFERENT (identite exacte)";
    if (stored.model_desc   != cur.model_desc)   return "description du modele differente";
    if (stored.model_bytes  != cur.model_bytes)  return "taille du modele differente";
    if (stored.model_params != cur.model_params) return "nombre de parametres different";
    if (stored.n_vocab != cur.n_vocab || stored.n_layer != cur.n_layer ||
        stored.n_embd  != cur.n_embd)            return "topologie du modele differente";
    if (stored.q1_sha256 != cur.q1_sha256)       return "identite du calcul Q1 differente";
    if (stored.format_magic == 0)                return "format d'etat inconnu";
    if (stored.type_k != cur.type_k || stored.type_v != cur.type_v)
                                                 return "types KV differents";
    if (stored.state_mode != cur.state_mode)     return "mode de serialisation different";
    if (stored.n_ctx != cur.n_ctx || stored.n_batch != cur.n_batch ||
        stored.n_ubatch != cur.n_ubatch || stored.n_seq_max != cur.n_seq_max)
                                                 return "configuration de contexte differente";
    if (stored.seq_id != cur.seq_id)             return "convention de sequence differente";
    return std::string();
}

// --------------------------------------------------- DECISION D'IMPORT
// FONCTION DE PRODUCTION : c'est ELLE que la route appelle avant set_data.
// Le test l'exerce directement, donc il n'existe qu'une seule implementation
// des regles de refus.
enum class ImportDecision { ACCEPT, NO_MANIFEST, INCOMPATIBLE, BAD_SHA256, BAD_SIZE };

inline ImportDecision decide_import(const Blob & b, const Manifest & cur, std::string & why) {
    if (!b.manifest)          { why = "state has NO compatibility manifest"; return ImportDecision::NO_MANIFEST; }
    const std::string m = manifest_mismatch(*b.manifest, cur);
    if (!m.empty())           { why = "state INCOMPATIBLE : " + m; return ImportDecision::INCOMPATIBLE; }
    if (b.data.size() != b.manifest->blob_bytes)
                              { why = "state size mismatch vs manifest"; return ImportDecision::BAD_SIZE; }
    if (sha256_hex(b.data.data(), b.data.size()) != b.manifest->blob_sha256)
                              { why = "state SHA256 MISMATCH"; return ImportDecision::BAD_SHA256; }
    why.clear();
    return ImportDecision::ACCEPT;
}
inline const char * decision_name(ImportDecision d) {
    switch (d) {
        case ImportDecision::ACCEPT:       return "ACCEPT";
        case ImportDecision::NO_MANIFEST:  return "REFUSE_NO_MANIFEST";
        case ImportDecision::INCOMPATIBLE: return "REFUSE_INCOMPATIBLE";
        case ImportDecision::BAD_SHA256:   return "REFUSE_BAD_SHA256";
        case ImportDecision::BAD_SIZE:     return "REFUSE_BAD_SIZE";
    }
    return "?";
}

// ------------------------------------------- PERSISTANCE DISQUE (blob+manifeste)
// Format : magic "OBKV" | uint32 version | uint32 n_fields | champs cle=valeur
// separes par \n | uint64 blob_bytes | octets du blob.
// Un lecteur REFUSE un fichier tronque, sans manifeste ou dont l'association
// blob/manifeste est discordante.
struct DiskLoad { bool ok = false; std::string why; Blob blob; };

inline bool save_state_file(const std::string & path, const Blob & b) {
    if (!b.manifest || b.data.empty()) return false;
    std::ofstream f(path, std::ios::binary);
    if (!f) return false;
    const char * magic = "OBKV"; const uint32_t ver = 1;
    f.write(magic, 4); f.write((const char *) &ver, sizeof ver);
    f << "blob_sha256=" << b.manifest->blob_sha256 << "\n"
      << "model_sha256=" << b.manifest->model_sha256 << "\n"
      << "q1_sha256=" << b.manifest->q1_sha256 << "\n"
      << "format_magic=" << b.manifest->format_magic << "\n"
      << "n_tokens=" << b.manifest->n_tokens << "\n"
      << "type_k=" << b.manifest->type_k << "\n"
      << "type_v=" << b.manifest->type_v << "\n"
      << "n_ctx=" << b.manifest->n_ctx << "\n"
      << "n_batch=" << b.manifest->n_batch << "\n"
      << "n_ubatch=" << b.manifest->n_ubatch << "\n"
      << "n_seq_max=" << b.manifest->n_seq_max << "\n"
      << "state_mode=" << b.manifest->state_mode << "\n"
      << "seq_id=" << b.manifest->seq_id << "\n"
      << "blob_bytes=" << b.manifest->blob_bytes << "\n"
      << "END\n";
    const uint64_t n = b.data.size();
    f.write((const char *) &n, sizeof n);
    f.write((const char *) b.data.data(), (std::streamsize) b.data.size());
    return (bool) f;
}

inline DiskLoad load_state_file(const std::string & path) {
    DiskLoad r;
    std::ifstream f(path, std::ios::binary);
    if (!f) { r.why = "fichier illisible"; return r; }
    char magic[4] = {0}; f.read(magic, 4);
    if (memcmp(magic, "OBKV", 4) != 0) { r.why = "magic invalide"; return r; }
    uint32_t ver = 0; f.read((char *) &ver, sizeof ver);
    if (ver != 1) { r.why = "version inconnue"; return r; }
    std::string line; std::unordered_map<std::string, std::string> kv;
    while (std::getline(f, line)) {
        if (line == "END") break;
        auto e = line.find('=');
        if (e != std::string::npos) kv[line.substr(0, e)] = line.substr(e + 1);
    }
    if (kv.empty()) { r.why = "manifeste ABSENT dans le fichier"; return r; }
    uint64_t n = 0; f.read((char *) &n, sizeof n);
    if (!f) { r.why = "en-tete de taille illisible"; return r; }
    std::vector<uint8_t> data(n);
    f.read((char *) data.data(), (std::streamsize) n);
    if ((uint64_t) f.gcount() != n) { r.why = "blob TRONQUE"; return r; }
    auto M = std::make_shared<Manifest>();
    M->blob_sha256  = kv["blob_sha256"];
    M->model_sha256 = kv["model_sha256"];
    M->q1_sha256    = kv["q1_sha256"];
    M->format_magic = (uint32_t) strtoul(kv["format_magic"].c_str(), nullptr, 10);
    M->n_tokens     = atoi(kv["n_tokens"].c_str());
    M->type_k       = atoi(kv["type_k"].c_str());
    M->type_v       = atoi(kv["type_v"].c_str());
    M->n_ctx        = (uint32_t) strtoul(kv["n_ctx"].c_str(), nullptr, 10);
    M->n_batch      = (uint32_t) strtoul(kv["n_batch"].c_str(), nullptr, 10);
    M->n_ubatch     = (uint32_t) strtoul(kv["n_ubatch"].c_str(), nullptr, 10);
    M->n_seq_max    = (uint32_t) strtoul(kv["n_seq_max"].c_str(), nullptr, 10);
    M->state_mode   = atoi(kv["state_mode"].c_str());
    M->seq_id       = atoi(kv["seq_id"].c_str());
    M->blob_bytes   = (size_t) strtoull(kv["blob_bytes"].c_str(), nullptr, 10);
    M->prefix_ids.assign(1, 0);      // marqueur : le prefixe exact n'est pas
                                     // persiste dans ce format -> manifeste
                                     // declare INCOMPLET par `complete()`.
    r.blob.n_tokens = (uint32_t) M->n_tokens;
    r.blob.data = std::move(data);
    r.blob.manifest = M;
    r.ok = true;
    return r;
}

// ------------------------------------------------------------------ CACHE LRU
struct Cache {
    mutable std::mutex mtx;
    std::unordered_map<std::string, Blob> map;
    std::list<std::string> lru;   // front = plus recemment utilise
    std::unordered_map<std::string, std::list<std::string>::iterator> pos;
    size_t bytes = 0;
    // Compteurs d'observation (publiables dans les rapports).
    uint64_t n_insert = 0, n_replace = 0, n_evict = 0, n_refuse_too_big = 0;
    size_t   max_bytes = 0;
    // set_data_calls : incremente UNIQUEMENT par le chemin d'import REEL.
    // Permet de prouver SET_DATA_CALLS=0 sur un refus.
    mutable uint64_t set_data_calls = 0;

    explicit Cache(size_t mb) : max_bytes(mb) {}
    static size_t default_max_bytes() {
        const char * e = std::getenv("OBRAIN_KV_STATES_MAX_BYTES");
        if (e && e[0]) { const long long x = atoll(e); if (x > 0) return (size_t) x; }
        return (size_t) 2 * 1024 * 1024 * 1024;   // 2 GiB, unites OCTETS
    }
    // false si l'entree est REFUSEE (vide, ou plus grosse que la borne totale).
    bool insert(const std::string & ref, Blob && b) {
        const size_t n = b.data.size();
        if (n == 0) return false;
        if (n > max_bytes) { n_refuse_too_big++; return false; }
        // [DEFAAUT CORRIGE] La cle existante doit etre retiree AVANT de calculer
        // le besoin : sinon un simple REMPLACEMENT evinçait une entree NON LIEE,
        // parce que sa propre taille comptait encore dans le total.
        auto ex = map.find(ref);
        if (ex != map.end()) {
            bytes -= ex->second.data.size();
            map.erase(ex);
            auto q = pos.find(ref);
            if (q != pos.end()) { lru.erase(q->second); pos.erase(q); }
            n_replace++;
        }
        while (bytes + n > max_bytes && !lru.empty()) {
            const std::string victim = lru.back();
            lru.pop_back();
            auto it = map.find(victim);
            if (it != map.end()) { bytes -= it->second.data.size(); map.erase(it); n_evict++; }
            pos.erase(victim);
        }
        bytes += n;
        map[ref] = std::move(b);
        lru.push_front(ref);
        pos[ref] = lru.begin();
        n_insert++;
        return true;
    }
    bool get(const std::string & ref, Blob & out) {
        auto it = map.find(ref);
        if (it == map.end()) return false;
        out = it->second;
        auto q = pos.find(ref);
        if (q != pos.end()) {
            lru.erase(q->second);
            lru.push_front(ref);
            q->second = lru.begin();
        }
        return true;
    }
    void clear() {
        map.clear(); lru.clear(); pos.clear(); bytes = 0;
    }
    size_t accounted_bytes() const { return bytes; }
    size_t size() const { return map.size(); }
};

} // namespace obrain_kv
