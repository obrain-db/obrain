import re
p="/home/triviere/projects/llama-obrain-cuda-f1a/tools/server/server-context.cpp"
s=open(p,encoding="utf-8",errors="surrogateescape").read()
orig_len=len(s)

# ---- ANCRE 1 : declarations du cache
old1="""static std::mutex g_kv_states_mtx;
static std::unordered_map<std::string, kv_state_blob> g_kv_states;"""
assert old1 in s, "A1"
new1=old1+"""

// [obrain K1/K2] BORNE MEMOIRE, EVICTION LRU ET INTEGRITE.
// Avant ce correctif `g_kv_states` etait un unordered_map SANS limite ni
// eviction : chaque entree pese la taille MESUREE du blob (160 697 016 octets
// pour un prefixe de 58 tokens). Une poignee d extractions epuisait la memoire,
// sans autre issue que l OOM du processus. La borne est explicite et
// configurable par OBRAIN_KV_STATES_MAX_BYTES (defaut 2 Gio) ; l eviction
// retire les entrees les moins recemment utilisees.
static size_t g_kv_states_bytes = 0;
static std::list<std::string> g_kv_states_lru;
static std::unordered_map<std::string, std::list<std::string>::iterator> g_kv_states_pos;

static size_t kv_states_max_bytes() {
    static size_t v = []() -> size_t {
        const char * e = std::getenv("OBRAIN_KV_STATES_MAX_BYTES");
        if (e != nullptr && e[0] != 0) { const long long x = atoll(e); if (x > 0) return (size_t) x; }
        return (size_t) 2 * 1024 * 1024 * 1024;
    }();
    return v;
}
// false si l entree est REFUSEE (vide, ou plus grosse que la borne totale).
static bool kv_states_insert(const std::string & ref, kv_state_blob && b) {
    const size_t bytes = b.data.size();
    if (bytes == 0) return false;
    if (bytes > kv_states_max_bytes()) return false;
    while (g_kv_states_bytes + bytes > kv_states_max_bytes() && !g_kv_states_lru.empty()) {
        const std::string victim = g_kv_states_lru.back();
        g_kv_states_lru.pop_back();
        auto it = g_kv_states.find(victim);
        if (it != g_kv_states.end()) { g_kv_states_bytes -= it->second.data.size(); g_kv_states.erase(it); }
        g_kv_states_pos.erase(victim);
    }
    auto ex = g_kv_states.find(ref);
    if (ex != g_kv_states.end()) {
        g_kv_states_bytes -= ex->second.data.size();
        g_kv_states.erase(ex);
        auto q = g_kv_states_pos.find(ref);
        if (q != g_kv_states_pos.end()) { g_kv_states_lru.erase(q->second); g_kv_states_pos.erase(q); }
    }
    g_kv_states_bytes += bytes;
    g_kv_states[ref] = std::move(b);
    g_kv_states_lru.push_front(ref);
    g_kv_states_pos[ref] = g_kv_states_lru.begin();
    return true;
}
static bool kv_states_get(const std::string & ref, kv_state_blob & out) {
    auto it = g_kv_states.find(ref);
    if (it == g_kv_states.end()) return false;
    out = it->second;
    auto q = g_kv_states_pos.find(ref);
    if (q != g_kv_states_pos.end()) {
        g_kv_states_lru.erase(q->second);
        g_kv_states_lru.push_front(ref);
        q->second = g_kv_states_lru.begin();
    }
    return true;
}"""
s=s.replace(old1,new1,1)

# ---- ANCRE 2 : insertion (indentation 28)
m=re.search(r'( +)\{\n( +)std::lock_guard<std::mutex> lk\(g_kv_states_mtx\);\n( +)g_kv_states\[ref\] = kv_state_blob\{\(uint32_t\) n_fact, data\};\n( +)\}\n', s)
assert m, "A2"
i0=m.start(); i1=m.end(); I=m.group(1)
new2=(f"{I}{{\n{I}    std::lock_guard<std::mutex> lk(g_kv_states_mtx);\n"
      f"{I}    // Aucune entree de cache si l insertion est REFUSEE\n"
      f"{I}    // (blob vide, ou plus gros que la borne memoire).\n"
      f"{I}    if (!kv_states_insert(ref, kv_state_blob{{(uint32_t) n_fact, data}})) {{\n"
      f"{I}        auto res = std::make_unique<server_task_result_error>();\n"
      f"{I}        res->id = task.id;\n"
      f"{I}        res->err_type = ERROR_TYPE_SERVER;\n"
      f"{I}        res->err_msg = \"state too large for the bounded cache (not stored)\";\n"
      f"{I}        queue_results.send(std::move(res));\n"
      f"{I}        break;\n"
      f"{I}    }}\n{I}}}\n")
s=s[:i0]+new2+s[i1:]

# ---- ANCRE 3 : lecture memoire (indentation reelle)
m=re.search(r'( +)\{\n( +)std::lock_guard<std::mutex> lk\(g_kv_states_mtx\);\n( +)auto it = g_kv_states\.find\(task\.jspace_state_ref\);\n( +)if \(it != g_kv_states\.end\(\)\) \{\n( +)blob = it->second;\n( +)\}\n( +)\}\n', s)
assert m, "A3"
i0=m.start(); i1=m.end(); I=m.group(1)
new3=(f"{I}{{\n{I}    std::lock_guard<std::mutex> lk(g_kv_states_mtx);\n"
      f"{I}    kv_states_get(task.jspace_state_ref, blob);\n{I}}}\n")
s=s[:i0]+new3+s[i1:]

# ---- ANCRE 4 : lecture disque + integrite
m=re.search(r'( +)uint32_t hdr = 0; f\.read\(\(char \*\) &hdr, 4\);\n( +)std::vector<uint8_t> payload\(\(std::istreambuf_iterator<char>\(f\)\), \{\}\);\n( +)blob = kv_state_blob\{hdr, std::move\(payload\)\};\n( +)std::lock_guard<std::mutex> lk\(g_kv_states_mtx\);\n( +)g_kv_states\[task\.jspace_state_ref\] = blob;\n', s)
assert m, "A4"
i0=m.start(); i1=m.end(); I=m.group(1)
new4=(f"{I}uint32_t hdr = 0; f.read((char *) &hdr, 4);\n"
      f"{I}std::vector<uint8_t> payload((std::istreambuf_iterator<char>(f)), {{}});\n"
      f"{I}// INTEGRITE : le nom de fichier est la cle ; on VERIFIE que le contenu y\n"
      f"{I}// correspond. Un blob altere est REFUSE au lieu d etre injecte.\n"
      f"{I}{{\n"
      f"{I}    const std::string recomputed = kv_ref_of(payload);\n"
      f"{I}    if (recomputed != task.jspace_state_ref) {{\n"
      f"{I}        auto res = std::make_unique<server_task_result_error>();\n"
      f"{I}        res->id = task.id;\n"
      f"{I}        res->err_type = ERROR_TYPE_SERVER;\n"
      f"{I}        res->err_msg = \"state integrity mismatch on disk load (refusal)\";\n"
      f"{I}        queue_results.send(std::move(res));\n"
      f"{I}        break;\n"
      f"{I}    }}\n"
      f"{I}}}\n"
      f"{I}blob = kv_state_blob{{hdr, std::move(payload)}};\n"
      f"{I}{{ std::lock_guard<std::mutex> lk(g_kv_states_mtx);\n"
      f"{I}  kv_states_insert(task.jspace_state_ref, kv_state_blob{{hdr, blob.data}}); }}\n")
s=s[:i0]+new4+s[i1:]

open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("PATCH OK : 4 ancres regex verifiees ; %d -> %d octets" % (orig_len, len(s)))
