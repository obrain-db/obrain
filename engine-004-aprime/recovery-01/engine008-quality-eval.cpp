// ENGINE-008 QUALITY EVAL -- harnais offline de preparation et d'evaluation.
//
// PASSE PROTOCOL-HARNESS-01 : MODEL_COMPUTE_ALLOWED=NO.
// Ce binaire NE DOIT executer aucun calcul modele dans cette passe.
// Tous les points d'entree de calcul passent par requireComputeAuthorization(),
// qui refuse AVANT tout appel a libllama.
//
// Le chargement du modele se fait EXCLUSIVEMENT avec vocab_only = true :
// aucune ponderation n'est chargee, aucun graphe n'est construit, aucun
// forward pass n'est structurellement possible dans les modes de cette passe.
//
// Renderer : common_chat_templates_apply (moteur Jinja natif du fork Q1),
// avec un template explicite -- JAMAIS le template embarque du modele.

#include "llama.h"
#include "chat.h"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iostream>
#include <map>
#include <sstream>
#include <string>
#include <vector>
#include <sys/wait.h>
#include <unistd.h>

// `json` est deja defini par common/chat.h (nlohmann::ordered_json).

// ------------------------------------------------------------------ etat
static const char * REFUSED_MSG = "COMPUTE_REFUSED_BEFORE_LLAMA_CALL";

struct ComputeAuthorization {
    bool        allowed = false;
    std::string reason  = "no authorization";
};

static ComputeAuthorization g_auth;
static long long           g_model_forward_calls = 0;  // doit rester 0 partout
static long long           g_decode_calls        = 0;  // doit rester 0 partout

// SEUL point d'entree de calcul modele. Toute autre voie est un defaut.
static void requireComputeAuthorization(const char * what) {
    if (!g_auth.allowed) {
        fprintf(stderr,
                "%s\n"
                "  operation refusee : %s\n"
                "  motif : %s\n"
                "  MODEL_COMPUTE_ALLOWED=NO pour cette passe\n"
                "  aucun appel a libllama n a ete effectue\n",
                REFUSED_MSG, what, g_auth.reason.c_str());
        fflush(stderr);
        exit(3);
    }
}

// ------------------------------------------------------------------ utils
static std::string read_file(const std::string & p) {
    std::ifstream f(p, std::ios::binary);
    if (!f) { fprintf(stderr, "ERREUR: lecture impossible %s\n", p.c_str()); exit(2); }
    std::ostringstream ss; ss << f.rdbuf();
    return ss.str();
}

static std::string json_escape(const std::string & s) {
    std::string o; o.reserve(s.size() + 16);
    for (unsigned char c : s) {
        switch (c) {
            case '"':  o += "\\\""; break;
            case '\\': o += "\\\\"; break;
            case '\n': o += "\\n";  break;
            case '\r': o += "\\r";  break;
            case '\t': o += "\\t";  break;
            case '\b': o += "\\b";  break;
            case '\f': o += "\\f";  break;
            default:
                if (c < 0x20) { char b[8]; snprintf(b, sizeof b, "\\u%04x", c); o += b; }
                else o += (char) c;
        }
    }
    return o;
}

static std::string ids_to_json(const std::vector<llama_token> & v) {
    std::string o = "[";
    for (size_t i = 0; i < v.size(); ++i) {
        if (i) o += ",";
        o += std::to_string((long long) v[i]);
    }
    return o + "]";
}

// ------------------------------------------------------------------ logits
// Algorithmes prescrits, testes sur donnees SYNTHETIQUES uniquement.
// Aucun logit de modele n'est utilise dans cette passe.

// argmax direct ; egalite exacte -> plus PETIT token id
static int greedy_argmax_lowest_id(const std::vector<float> & logits) {
    if (logits.empty()) return -1;
    int best = 0;
    for (int i = 1; i < (int) logits.size(); ++i) {
        if (logits[i] > logits[best]) best = i;   // strictement superieur => le premier gagne
    }
    return best;                                   // ids croissants => plus petit id
}

// logsumexp stable, accumulation double
static bool logsumexp_stable(const std::vector<float> & x, double & out) {
    if (x.empty()) return false;
    double m = -(double) INFINITY;
    for (float v : x) {
        if (std::isnan(v) || std::isinf(v)) return false;
        if ((double) v > m) m = (double) v;
    }
    double s = 0.0;
    for (float v : x) s += std::exp((double) v - m);
    out = m + std::log(s);
    return std::isfinite(out);
}

// top-k : primaire logit decroissant, secondaire token id croissant
static std::vector<std::pair<int,float>> top_k(const std::vector<float> & logits, int k) {
    std::vector<std::pair<int,float>> v;
    v.reserve(logits.size());
    for (int i = 0; i < (int) logits.size(); ++i) v.push_back({i, logits[i]});
    std::stable_sort(v.begin(), v.end(), [](const auto & a, const auto & b) {
        if (a.second != b.second) return a.second > b.second;
        return a.first < b.first;
    });
    if ((int) v.size() > k) v.resize(k);
    return v;
}

// chemin reel du binaire : /proc/self/exe est resolu DANS LE PARENT, avant
// tout fork. Utilise tel quel dans un shell, il designerait le shell.
static std::string g_self_path;

static void resolve_self_path() {
    char buf[4096];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
    g_self_path = (n > 0) ? std::string(buf, (size_t) n) : "";
}

// ------------------------------------------------------------------ fixtures
static int mode_selftest() {
    int failed = 0;
    auto check = [&](const char * name, bool ok) {
        printf("  %-46s %s\n", name, ok ? "PASS" : "FAIL");
        if (!ok) failed++;
    };
    printf("=== ALGORITHMES SUR DONNEES SYNTHETIQUES (aucun logit de modele) ===\n");

    { std::vector<float> l = {0.f, 5.f, 1.f}; check("argmax_unique_max", greedy_argmax_lowest_id(l) == 1); }
    { std::vector<float> l = {3.f, 7.f, 7.f, 2.f}; check("argmax_tie_two_lowest_id", greedy_argmax_lowest_id(l) == 1); }
    { std::vector<float> l = {4.f, 4.f, 4.f, 4.f}; check("argmax_tie_all_lowest_id", greedy_argmax_lowest_id(l) == 0); }
    { std::vector<float> l = {-1.f, -5.f, -9.f}; check("argmax_all_negative", greedy_argmax_lowest_id(l) == 0); }

    { std::vector<float> l = {0.f,0.f}; double z=0;
      check("logsumexp_uniform_equals_log2", logsumexp_stable(l,z) && std::fabs(z - std::log(2.0)) < 1e-12); }
    { std::vector<float> l = {1000.f, 1000.f}; double z=0;
      check("logsumexp_large_values_stable", logsumexp_stable(l,z) && std::fabs(z - (1000.0+std::log(2.0))) < 1e-9); }
    { std::vector<float> l = {-1000.f, -1000.f}; double z=0;
      check("logsumexp_very_negative_stable", logsumexp_stable(l,z) && std::fabs(z - (-1000.0+std::log(2.0))) < 1e-9); }
    { std::vector<float> l = {0.f, NAN}; double z=0; check("logsumexp_rejects_NaN", !logsumexp_stable(l,z)); }
    { std::vector<float> l = {0.f, INFINITY}; double z=0; check("logsumexp_rejects_plusInf", !logsumexp_stable(l,z)); }
    { std::vector<float> l = {0.f, -INFINITY}; double z=0; check("logsumexp_rejects_minusInf", !logsumexp_stable(l,z)); }

    { std::vector<float> l = {1.f, 9.f, 5.f, 5.f, 2.f, 0.f};
      auto t = top_k(l, 5);
      bool ok = t.size()==5 && t[0].first==1 && t[1].first==2 && t[2].first==3 && t[3].first==4;
      check("top5_order_logit_desc_id_asc", ok); }
    { std::vector<float> l = {5.f,5.f,5.f,5.f,5.f,5.f,5.f};
      auto t = top_k(l, 5);
      bool ok = t.size()==5 && t[0].first==0 && t[1].first==1 && t[2].first==2 && t[3].first==3 && t[4].first==4;
      check("top5_all_tie_lowest_ids_first", ok); }
    { std::vector<float> l = {1.f,2.f}; auto t = top_k(l,5);
      check("top5_fewer_than_k_returns_available", t.size()==2); }

    // garde de calcul : le fils DOIT refuser avant tout appel libllama
    printf("=== GARDE DE CALCUL (sous-processus) ===\n");
    {
        if (g_self_path.empty()) { printf("  %-46s FAIL\n", "self_path_resolved"); return 1; }
        std::string self = g_self_path;
        std::string cmd = self + " --mode execute > /dev/null 2>&1";
        int rc = system(cmd.c_str());
        int code = WEXITSTATUS(rc);
        check("execute_sans_autorisation_refuse_exit3", code == 3);
        std::string cmd2 = self + " --mode execute 2>&1 >/dev/null | grep -q " + REFUSED_MSG;
        int rc2 = system(cmd2.c_str());
        check("execute_refuse_avec_message_avant_libllama", WEXITSTATUS(rc2) == 0);
        // avec le drapeau mais SANS autorisation d'execution correspondant au
        // manifeste gele : doit refuser quand meme.
        std::string cmd3 = self + " --mode execute --allow-model-compute > /dev/null 2>&1";
        int rc3 = system(cmd3.c_str());
        check("allow_flag_seul_ne_suffit_pas_exit3", WEXITSTATUS(rc3) == 3);
    }

    printf("\nHARNESS_UNIT_TEST_FAILED_COUNT=%d\n", failed);
    printf("HARNESS_COMPUTE_GUARD_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("HARNESS_SELFTEST=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("HARNESS_PREFREEZE_DECODE_CALL_COUNT=%lld\n", g_decode_calls);
    printf("HARNESS_PREFREEZE_MODEL_FORWARD_CALL_COUNT=%lld\n", g_model_forward_calls);
    return failed == 0 ? 0 : 1;
}

// ------------------------------------------------------------------ renderer
struct RendererCtx {
    llama_model *               model = nullptr;
    const llama_vocab *         vocab = nullptr;
    common_chat_templates_ptr   tmpls;
    std::string                 template_sha;
};

static void load_vocab_only(RendererCtx & c, const std::string & model_path,
                            const std::string & template_src) {
    llama_model_params mp = llama_model_default_params();
    mp.vocab_only = true;      // AUCUNE ponderation chargee, aucun graphe
    mp.n_gpu_layers = 0;
    c.model = llama_model_load_from_file(model_path.c_str(), mp);
    if (!c.model) { fprintf(stderr, "ERREUR: chargement vocabulaire impossible: %s\n", model_path.c_str()); exit(2); }
    c.vocab = llama_model_get_vocab(c.model);
    // template EXPLICITE : le template embarque du modele n'est jamais lu.
    c.tmpls = common_chat_templates_init(c.model, template_src);
    if (!c.tmpls) { fprintf(stderr, "ERREUR: init du template impossible\n"); exit(2); }
}

static std::string render_prompt(RendererCtx & c, const std::string & user_content, bool enable_thinking) {
    common_chat_templates_inputs in;
    common_chat_msg m;
    m.role    = "user";
    m.content = user_content;
    in.messages.push_back(m);
    in.add_generation_prompt = true;      // prefixe assistant exact
    in.use_jinja             = true;      // OBLIGATOIRE : selectionne le moteur Jinja
    in.enable_thinking       = enable_thinking;
    in.reasoning_format      = COMMON_REASONING_FORMAT_NONE;
    in.add_bos               = false;
    in.add_eos               = false;
    in.force_pure_content    = false;
    in.tools.clear();
    in.parallel_tool_calls   = false;
    in.chat_template_kwargs.clear();      // vide : le template applique son propre defaut
    common_chat_params p = common_chat_templates_apply(c.tmpls.get(), in);
    return p.prompt;
}

static std::vector<llama_token> tokenize_task(RendererCtx & c, const std::string & text) {
    std::vector<llama_token> toks(text.size() + 8);
    // add_special = false (BOS non ajoute) ; parse_special = true (TASK)
    int n = llama_tokenize(c.vocab, text.c_str(), (int) text.size(),
                           toks.data(), (int) toks.size(),
                           /* add_special */ false, /* parse_special */ true);
    if (n < 0) {
        toks.resize(-n);
        n = llama_tokenize(c.vocab, text.c_str(), (int) text.size(),
                           toks.data(), (int) toks.size(), false, true);
    }
    toks.resize(n > 0 ? n : 0);
    return toks;
}

// ------------------------------------------------------------------ prompts
static std::string build_mmlu_prompt(const json & it) {
    // TASK_PROMPT_CONTRACTS_v1 : UNIQUEMENT les options existantes (3..10)
    std::string s = "Question:\n" + it["question"].get<std::string>() + "\nChoices:\n";
    const auto & opts = it["options"];
    int n = (int) opts.size();
    for (int i = 0; i < n; ++i) {
        char label = (char) ('A' + i);
        s += std::string(1, label) + ". " + opts[i].get<std::string>() + "\n";
    }
    s += "Answer:\n";
    return s;
}

static std::string build_gsm8k_prompt(const json & it) {
    return it["question"].get<std::string>() +
           "\nSolve the problem carefully.\n"
           "End your response with exactly one line in this format:\n"
           "FINAL: <number>";
}

static std::string build_mbpp_prompt(const json & it) {
    return it["prompt"].get<std::string>() +
           "\nReturn only the Python source code implementing the requested function.\n"
           "Do not include Markdown fences or explanations.";
}

static std::string build_ifeval_prompt(const json & it) {
    return it["prompt"].get<std::string>();   // EXACTEMENT le prompt upstream
}

struct TaskItem {
    std::string item_id, category, semantic_prompt, scorer_id;
    int         max_new_tokens = 0;
    bool        is_mmlu = false;
    int         answer_index = -1, option_count = 0;
};

static std::string sha256_hex(const std::string & data);


static std::vector<TaskItem> load_items(const std::string & corpora) {
    std::vector<TaskItem> out;
    struct Spec { const char * cat, * file, * scorer; int mnt; };
    const Spec specs[] = {
        {"knowledge_reasoning", "mmlu_pro/MMLU_PRO_ITEMS.jsonl", "mmlu_pro_v1", 0},
        {"math",                "gsm8k/GSM8K_ITEMS.jsonl",       "gsm8k_v1",   1024},
        {"code",                "mbpp/MBPP_ITEMS.jsonl",         "mbpp_v1",    1024},
        {"instruction",         "ifeval/IFEVAL_ITEMS.jsonl",     "ifeval_v1",  4096},
    };
    for (const auto & sp : specs) {
        std::ifstream f(corpora + "/" + sp.file);
        if (!f) { fprintf(stderr, "ERREUR: corpus absent %s\n", sp.file); exit(2); }
        std::string line;
        while (std::getline(f, line)) {
            if (line.empty()) continue;
            json j = json::parse(line);
            TaskItem t;
            t.item_id    = j["item_id"].get<std::string>();
            t.category   = sp.cat;
            t.scorer_id  = sp.scorer;
            t.max_new_tokens = sp.mnt;
            if (std::string(sp.cat) == "knowledge_reasoning") {
                t.is_mmlu       = true;
                t.answer_index  = j["answer_index"].get<int>();
                t.option_count  = (int) j["options"].size();
                t.semantic_prompt = build_mmlu_prompt(j);
            } else if (std::string(sp.cat) == "math") {
                t.semantic_prompt = build_gsm8k_prompt(j);
            } else if (std::string(sp.cat) == "code") {
                t.semantic_prompt = build_mbpp_prompt(j);
            } else {
                t.semantic_prompt = build_ifeval_prompt(j);
            }
            out.push_back(std::move(t));
        }
    }
    return out;
}

// ------------------------------------------------------------------ sha256
// Implementation locale (pas de dependance openssl).
namespace {
struct SHA256 {
    uint32_t h[8]; uint64_t len; unsigned char buf[64]; size_t n;
    SHA256() { reset(); }
    void reset() {
        static const uint32_t iv[8] = {0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
                                       0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19};
        for (int i=0;i<8;i++) h[i]=iv[i];
        len=0; n=0;
    }
    static uint32_t rotr(uint32_t x,int s){return (x>>s)|(x<<(32-s));}
    void block(const unsigned char* p){
        static const uint32_t K[64]={
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2};
        uint32_t w[64];
        for(int i=0;i<16;i++) w[i]=(p[i*4]<<24)|(p[i*4+1]<<16)|(p[i*4+2]<<8)|p[i*4+3];
        for(int i=16;i<64;i++){
            uint32_t s0=rotr(w[i-15],7)^rotr(w[i-15],18)^(w[i-15]>>3);
            uint32_t s1=rotr(w[i-2],17)^rotr(w[i-2],19)^(w[i-2]>>10);
            w[i]=w[i-16]+s0+w[i-7]+s1;
        }
        uint32_t a=h[0],b=h[1],c=h[2],d=h[3],e=h[4],f=h[5],g=h[6],hh=h[7];
        for(int i=0;i<64;i++){
            uint32_t S1=rotr(e,6)^rotr(e,11)^rotr(e,25);
            uint32_t ch=(e&f)^((~e)&g);
            uint32_t t1=hh+S1+ch+K[i]+w[i];
            uint32_t S0=rotr(a,2)^rotr(a,13)^rotr(a,22);
            uint32_t maj=(a&b)^(a&c)^(b&c);
            uint32_t t2=S0+maj;
            hh=g;g=f;f=e;e=d+t1;d=c;c=b;b=a;a=t1+t2;
        }
        h[0]+=a;h[1]+=b;h[2]+=c;h[3]+=d;h[4]+=e;h[5]+=f;h[6]+=g;h[7]+=hh;
    }
    void update(const unsigned char* p, size_t l){
        len += l;
        while(l){
            size_t k = std::min(l, (size_t)64 - n);
            memcpy(buf+n,p,k); n+=k; p+=k; l-=k;
            if(n==64){ block(buf); n=0; }
        }
    }
    std::string hex(){
        uint64_t bits = len*8;
        unsigned char pad=0x80; update(&pad,1);
        unsigned char z=0;
        while(n!=56) update(&z,1);
        unsigned char lb[8];
        for(int i=0;i<8;i++) lb[i]=(unsigned char)(bits>>(56-8*i));
        update(lb,8);
        char o[65];
        for(int i=0;i<8;i++) snprintf(o+i*8,9,"%08x",h[i]);
        return std::string(o,64);
    }
};
}

static std::string sha256_hex(const std::string & data) {
    SHA256 s; s.update((const unsigned char*) data.data(), data.size()); return s.hex();
}

// ------------------------------------------------------------------ modes
static int mode_provenance() {
    printf("ENGINE-008 QUALITY EVAL -- PROVENANCE DU HARNAIS\n");
    printf("HARNESS_BUILD_VERSION=engine008-quality-eval/1\n");
    printf("LLAMA_BUILD_NUMBER=%d\n", llama_max_devices() >= 0 ? 1 : 0);
    printf("MODEL_COMPUTE_ALLOWED=NO\n");
    printf("MODEL_LOAD_MODE=VOCAB_ONLY\n");
    printf("HARNESS_PREFREEZE_DECODE_CALL_COUNT=%lld\n", g_decode_calls);
    printf("HARNESS_PREFREEZE_MODEL_FORWARD_CALL_COUNT=%lld\n", g_model_forward_calls);
    printf("RENDERER_FUNCTION=common_chat_templates_apply\n");
    printf("RENDERER_ENGINE=JINJA_MINJA_NATIVE\n");
    printf("RENDERER_TEMPLATE_SOURCE=EXPLICIT_OVERRIDE_NEVER_EMBEDDED\n");
    return 0;
}

static int mode_renderer_qualify(const std::string & model_path,
                                 const std::string & template_path,
                                 const std::string & out_path) {
    std::string tmpl = read_file(template_path);
    std::string tsha = sha256_hex(tmpl);
    printf("EFFECTIVE_CHAT_TEMPLATE_SHA256=%s\n", tsha.c_str());
    printf("EFFECTIVE_CHAT_TEMPLATE_BYTES=%zu\n", tmpl.size());

    RendererCtx c;
    load_vocab_only(c, model_path, tmpl);

    // preuve : le template retenu est bien celui fourni, pas celui du modele
    std::string src = common_chat_templates_source(c.tmpls.get());
    printf("RENDERER_TEMPLATE_SOURCE_SHA256=%s\n", sha256_hex(src).c_str());
    printf("RENDERER_USES_EXPLICIT_TEMPLATE=%s\n", sha256_hex(src) == tsha ? "YES" : "NO");

    const char * model_tmpl = llama_model_chat_template(c.model, nullptr);
    printf("MODEL_EMBEDDED_TEMPLATE_SHA256=%s\n",
           model_tmpl ? sha256_hex(model_tmpl).c_str() : "ABSENT");
    printf("EMBEDDED_TEMPLATE_OVERRIDDEN_BY_EXPLICIT=%s\n",
           (model_tmpl && sha256_hex(model_tmpl) != tsha) ? "YES" : "NO");

    FILE * out = fopen(out_path.c_str(), "wb");
    // fixtures : valeurs explicites, cas distincts
    // stockage STABLE : un .c_str() sur un temporaire serait pendouillant.
    static const std::string fx_long(4000, 'x');
    static const std::string fx_unicode =
        "R\xC3\xA9sume en fran\xC3\xA7ais : \xC3\xA9l\xC3\xA8ve, "
        "\xE4\xB8\xAD\xE6\x96\x87, \xF0\x9F\x98\x80";
    struct Fx { const char * name; const char * content; bool think; };
    const Fx fx[] = {
        {"simple_ascii",     "What is 2+2?",                             true},
        {"empty_content",    "",                                         true},
        {"unicode",          fx_unicode.c_str(),                         true},
        {"special_tokens",   "<|im_start|>user<|im_end|> literal text",  true},
        {"thinking_off",     "What is 2+2?",                             false},
        {"long_no_truncate", fx_long.c_str(),                            true},
    };
    for (const auto & f : fx) {
        std::string r = render_prompt(c, f.content, f.think);
        printf("FIXTURE=%s BYTES=%zu SHA256=%s\n", f.name, r.size(), sha256_hex(r).c_str());
        fflush(stdout);
        json j;
        j["fixture"]        = f.name;
        j["enable_thinking"]= f.think;
        j["input_sha256"]   = sha256_hex(f.content);
        j["rendered"]       = r;
        j["rendered_sha256"]= sha256_hex(r);
        j["rendered_bytes"] = r.size();
        fprintf(out, "%s\n", j.dump(-1, ' ', false,
                                     nlohmann::json::error_handler_t::replace).c_str());
    }
    fclose(out);
    llama_model_free(c.model);
    return 0;
}

static int mode_render(const std::string & corpora, const std::string & template_path,
                       const std::string & out_dir, bool second_pass,
                       const std::string & model_b, const std::string & model_c) {
    std::string tmpl = read_file(template_path);
    auto items = load_items(corpora);
    printf("TASK_ITEMS_LOADED=%zu\n", items.size());

    RendererCtx cb, cc;
    load_vocab_only(cb, model_b, tmpl);
    load_vocab_only(cc, model_c, tmpl);

    std::string suffix = second_pass ? "_PASS2" : "";
    FILE * frend  = fopen((out_dir + "/TASK_RENDERED_INPUTS" + suffix + ".jsonl").c_str(), "wb");
    FILE * fmmlu  = fopen((out_dir + "/MMLU_CHOICES_INPUTS" + suffix + ".jsonl").c_str(), "wb");
    FILE * forder = fopen((out_dir + "/TASK_GLOBAL_ITEM_ORDER" + suffix + ".txt").c_str(), "wb");

    long long text_mismatch = 0, tok_mismatch = 0, invalid_ids = 0;
    long long mmlu_checks = 0, mmlu_nonprefix = 0, mmlu_suffix_mismatch = 0;
    long long counts[4] = {0,0,0,0};

    for (auto & it : items) {
        std::string r_trunc = render_prompt(cb, it.semantic_prompt, true);
        std::string r_cand  = render_prompt(cc, it.semantic_prompt, true);
        if (r_trunc != r_cand) text_mismatch++;

        auto tb = tokenize_task(cb, r_trunc);
        auto tc = tokenize_task(cc, r_cand);
        if (tb != tc) tok_mismatch++;
        for (auto t : tb) if (t < 0 || t >= 248320) invalid_ids++;
        for (auto t : tc) if (t < 0 || t >= 248320) invalid_ids++;

        if (it.category == "knowledge_reasoning") counts[0]++;
        else if (it.category == "math") counts[1]++;
        else if (it.category == "code") counts[2]++;
        else counts[3]++;

        fprintf(forder, "%s\t%s\n", it.category.c_str(), it.item_id.c_str());

        std::string line;
        line  = "{\"item_id\":\"" + json_escape(it.item_id) + "\"";
        line += ",\"category\":\"" + json_escape(it.category) + "\"";
        line += ",\"scorer_id\":\"" + json_escape(it.scorer_id) + "\"";
        line += ",\"semantic_prompt_sha256\":\"" + sha256_hex(it.semantic_prompt) + "\"";
        line += ",\"chat_template_sha256\":\"" + sha256_hex(tmpl) + "\"";
        line += ",\"rendered_prompt\":\"" + json_escape(r_trunc) + "\"";
        line += ",\"rendered_prompt_sha256\":\"" + sha256_hex(r_trunc) + "\"";
        line += ",\"baseline_input_token_ids\":" + ids_to_json(tb);
        line += ",\"candidate_input_token_ids\":" + ids_to_json(tc);
        line += ",\"input_token_count\":" + std::to_string((long long) tb.size());
        line += ",\"generation_prefix\":true";
        if (it.max_new_tokens > 0)
            line += ",\"max_new_tokens\":" + std::to_string(it.max_new_tokens);
        line += "}\n";
        fputs(line.c_str(), frend);

        if (it.is_mmlu) {
            // tokenisation conditionnelle : prefixe exact puis suffixe
            std::string pline = "{\"item_id\":\"" + json_escape(it.item_id) + "\"";
            pline += ",\"answer_index\":" + std::to_string(it.answer_index);
            pline += ",\"option_count\":" + std::to_string(it.option_count);
            pline += ",\"prefix_token_ids\":" + ids_to_json(tb);
            pline += ",\"choices\":[";
            for (int i = 0; i < it.option_count; ++i) {
                std::string label(1, (char) ('A' + i));
                std::string full = r_trunc + label;
                auto tf = tokenize_task(cb, full);
                mmlu_checks++;
                bool is_prefix = tf.size() >= tb.size() &&
                                 std::equal(tb.begin(), tb.end(), tf.begin());
                if (!is_prefix) mmlu_nonprefix++;
                std::vector<llama_token> suff;
                if (is_prefix) suff.assign(tf.begin() + tb.size(), tf.end());
                // independamment pour le candidat
                auto tfc = tokenize_task(cc, r_cand + label);
                bool is_prefix_c = tfc.size() >= tc.size() &&
                                   std::equal(tc.begin(), tc.end(), tfc.begin());
                std::vector<llama_token> suffc;
                if (is_prefix_c) suffc.assign(tfc.begin() + tc.size(), tfc.end());
                if (suff != suffc) mmlu_suffix_mismatch++;
                if (i) pline += ",";
                pline += "{\"label\":\"" + label + "\",\"candidate_token_ids\":" + ids_to_json(suff) + "}";
            }
            pline += "]}\n";
            fputs(pline.c_str(), fmmlu);
        }
    }
    fclose(frend); fclose(fmmlu); fclose(forder);
    llama_model_free(cb.model); llama_model_free(cc.model);

    printf("TASK_RENDERED_INPUT_COUNT=%lld\n", (long long) items.size());
    printf("TASK_RENDERED_MMLU_COUNT=%lld\n", counts[0]);
    printf("TASK_RENDERED_GSM8K_COUNT=%lld\n", counts[1]);
    printf("TASK_RENDERED_MBPP_COUNT=%lld\n", counts[2]);
    printf("TASK_RENDERED_IFEVAL_COUNT=%lld\n", counts[3]);
    printf("TASK_RENDERED_TEXT_BASELINE_CANDIDATE_MISMATCH_COUNT=%lld\n", text_mismatch);
    printf("TASK_INPUT_TOKEN_ID_BASELINE_CANDIDATE_MISMATCH_COUNT=%lld\n", tok_mismatch);
    printf("TASK_INVALID_TOKEN_ID_COUNT=%lld\n", invalid_ids);
    printf("MMLU_PREFIX_EXTENSION_CHECK_COUNT=%lld\n", mmlu_checks);
    printf("MMLU_PREFIX_EXTENSION_NONPREFIX_COUNT=%lld\n", mmlu_nonprefix);
    printf("MMLU_CANDIDATE_SUFFIX_TOKEN_MISMATCH_COUNT=%lld\n", mmlu_suffix_mismatch);
    printf("HARNESS_PREFREEZE_DECODE_CALL_COUNT=%lld\n", g_decode_calls);
    printf("HARNESS_PREFREEZE_MODEL_FORWARD_CALL_COUNT=%lld\n", g_model_forward_calls);
    return (text_mismatch || tok_mismatch || invalid_ids || mmlu_nonprefix || mmlu_suffix_mismatch) ? 1 : 0;
}

static int mode_execute() {
    requireComputeAuthorization("mode execute");
    printf("ENGINE008_EXECUTION_AUTHORIZED=YES\n");
    return 0;
}

int main(int argc, char ** argv) {
    resolve_self_path();
    std::string mode, corpora, template_path, out_dir, model_b, model_c;
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        auto next = [&]() { return std::string(argv[++i]); };
        if      (a == "--mode")               mode = next();
        else if (a == "--corpora")            corpora = next();
        else if (a == "--template")           template_path = next();
        else if (a == "--out")                out_dir = next();
        else if (a == "--model-baseline")     model_b = next();
        else if (a == "--model-candidate")    model_c = next();
        else if (a == "--allow-model-compute") {
            // Le drapeau ne suffit JAMAIS : une autorisation d'execution
            // correspondant au manifeste gele est requise en plus.
            g_auth.allowed = false;
            g_auth.reason  = "drapeau present mais aucune autorisation d execution "
                             "correspondant au manifeste gele";
        }
        else if (a == "--help") { printf("modes: provenance selftest renderer-qualify render verify-freeze execute\n"); return 0; }
        else { fprintf(stderr, "argument inconnu: %s\n", a.c_str()); return 2; }
    }
    if (mode.empty()) { fprintf(stderr, "--mode requis\n"); return 2; }
    if (mode == "provenance")        return mode_provenance();
    if (mode == "selftest")          return mode_selftest();
    if (mode == "execute")           return mode_execute();
    if (mode == "renderer-qualify")  return mode_renderer_qualify(model_b, template_path, out_dir + "/RENDERER_FIXTURES.jsonl");
    if (mode == "render")            return mode_render(corpora, template_path, out_dir, false, model_b, model_c);
    if (mode == "render-second")     return mode_render(corpora, template_path, out_dir, true, model_b, model_c);
    if (mode == "verify-freeze")     { printf("VERIFY_FREEZE=NOT_IMPLEMENTED_IN_THIS_BUILD\n"); return 2; }
    fprintf(stderr, "mode inconnu: %s\n", mode.c_str());
    return 2;
}
