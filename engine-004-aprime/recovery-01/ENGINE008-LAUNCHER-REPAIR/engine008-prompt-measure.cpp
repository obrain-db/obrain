// ENGINE-008 -- mesure de l'entree EXACTE du preflight.
// vocab_only : AUCUN forward, AUCUN contexte de calcul.
#include "llama.h"
#include "common.h"
#include "chat.h"
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>
#include <fstream>
#include <sstream>

static std::string sha256_of(const std::string & s) {
    std::string tmp = "/tmp/.pm_" + std::to_string((size_t) s.data());
    { std::ofstream f(tmp, std::ios::binary); f << s; }
    std::string cmd = "sha256sum '" + tmp + "' | cut -d' ' -f1";
    FILE * p = popen(cmd.c_str(), "r"); char b[128] = {0};
    if (p) { if (!fgets(b, sizeof b, p)) {} pclose(p); }
    std::string r(b); while (!r.empty() && (r.back()=='\n'||r.back()=='\r')) r.pop_back();
    remove(tmp.c_str());
    return r;
}

int main(int argc, char ** argv) {
    std::string model_path, tpl_path, user_msg = "Réponds uniquement : OK.";
    std::string label = "?";
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        if      (a == "--model" && i+1 < argc) model_path = argv[++i];
        else if (a == "--template" && i+1 < argc) tpl_path = argv[++i];
        else if (a == "--message" && i+1 < argc) user_msg = argv[++i];
        else if (a == "--label" && i+1 < argc) label = argv[++i];
    }
    if (model_path.empty() || tpl_path.empty()) { printf("usage: --model M --template T [--message S] [--label L]\n"); return 2; }

    llama_backend_init();
    llama_model_params mp = llama_model_default_params();
    mp.vocab_only = true;                 // AUCUN chargement de poids
    llama_model * model = llama_model_load_from_file(model_path.c_str(), mp);
    if (model == nullptr) { printf("MODELE_ILLISIBLE=%s\n", model_path.c_str()); return 3; }

    std::string tmpl;
    { std::ifstream f(tpl_path); std::ostringstream ss; ss << f.rdbuf(); tmpl = ss.str(); }

    auto tmpls = common_chat_templates_init(model, tmpl);
    common_chat_templates_inputs inputs;
    common_chat_msg m; m.role = "user"; m.content = user_msg;
    inputs.messages.push_back(m);
    inputs.add_generation_prompt = true;
    inputs.use_jinja = true;
    inputs.enable_thinking = true;

    common_chat_params cp = common_chat_templates_apply(tmpls.get(), inputs);
    const std::string rendered = cp.prompt;
    printf("LABEL=%s\n", label.c_str());
    printf("MODEL=%s\n", model_path.c_str());
    printf("VOCAB_ONLY=YES\nFULL_MODEL_LOAD_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    printf("RENDERED_SHA256=%s\n", sha256_of(rendered).c_str());
    printf("RENDERED_BYTES=%zu\n", rendered.size());

    const llama_vocab * vocab = llama_model_get_vocab(model);
    // politique : add_special=false, parse_special=true (le template emet deja ses tokens speciaux)
    std::vector<llama_token> toks(rendered.size() + 8);
    int n = llama_tokenize(vocab, rendered.c_str(), (int) rendered.size(),
                           toks.data(), (int) toks.size(),
                           /*add_special*/ false, /*parse_special*/ true);
    if (n < 0) { toks.resize(-n); n = llama_tokenize(vocab, rendered.c_str(), (int) rendered.size(),
                                                     toks.data(), (int) toks.size(), false, true); }
    if (n < 0) { printf("TOKENIZE_FAILED=YES\n"); llama_model_free(model); llama_backend_free(); return 4; }
    toks.resize(n);
    std::string ids;
    for (int i = 0; i < n; ++i) { if (i) ids += ","; ids += std::to_string(toks[i]); }
    printf("ADD_SPECIAL=0\nPARSE_SPECIAL=1\n");
    printf("PREFLIGHT_RENDERED_TOKEN_COUNT=%d\n", n);
    printf("TOKEN_IDS_SHA256=%s\n", sha256_of(ids).c_str());
    printf("TOKEN_IDS=%s\n", ids.c_str());
    printf("N_CTX_REQUIRED_MIN=%d\n", n + 8);
    llama_model_free(model);
    llama_backend_free();
    return 0;
}
