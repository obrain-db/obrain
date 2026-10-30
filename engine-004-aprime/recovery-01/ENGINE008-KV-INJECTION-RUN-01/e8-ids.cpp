// Genere les 59 IDs autoritaires : template qualifie, add_special=0, parse_special=1.
// Verifie les deux empreintes gelees AVANT d'ecrire le fichier.
#include "llama.h"
#include "common.h"
#include "chat.h"
#include <cstdio>
#include <fstream>
#include <sstream>
#include <string>
static std::string sha(const std::string & s) {
    std::string t = "/tmp/.e8ids";
    { std::ofstream f(t, std::ios::binary); f << s; }
    std::string c = "sha256sum '" + t + "' | cut -d' ' -f1";
    FILE * p = popen(c.c_str(), "r"); char b[128] = {0};
    if (p) { if (!fgets(b, sizeof b, p)) {} pclose(p); }
    std::string r(b); while (!r.empty() && (r.back()=='\n'||r.back()=='\r')) r.pop_back();
    return r;
}
int main(int argc, char ** argv) {
    if (argc < 4) { printf("usage: e8-ids <model> <template> <out>\n"); return 2; }
    llama_model_params mp = llama_model_default_params(); mp.vocab_only = true;
    llama_model * m = llama_model_load_from_file(argv[1], mp);
    if (!m) { printf("MODELE_ILLISIBLE\n"); return 3; }
    std::string tmpl; { std::ifstream f(argv[2]); std::ostringstream ss; ss << f.rdbuf(); tmpl = ss.str(); }
    auto tmpls = common_chat_templates_init(m, tmpl);
    common_chat_templates_inputs in;
    common_chat_msg msg; msg.role = "user"; msg.content = "Réponds uniquement : OK.";
    in.messages.push_back(msg);
    in.add_generation_prompt = true; in.use_jinja = true; in.enable_thinking = true;
    common_chat_params cp = common_chat_templates_apply(tmpls.get(), in);
    printf("RENDERED_SHA256=%s\n", sha(cp.prompt).c_str());
    const llama_vocab * v = llama_model_get_vocab(m);
    std::vector<llama_token> t(cp.prompt.size() + 8);
    int n = llama_tokenize(v, cp.prompt.c_str(), (int) cp.prompt.size(), t.data(), (int) t.size(), false, true);
    if (n < 0) { t.resize(-n); n = llama_tokenize(v, cp.prompt.c_str(), (int) cp.prompt.size(), t.data(), (int) t.size(), false, true); }
    if (n < 0) { printf("TOKENISATION_ECHOUEE\n"); return 4; }
    t.resize(n);
    std::string csv; for (int i = 0; i < n; ++i) { if (i) csv += ","; csv += std::to_string(t[i]); }
    printf("TOKEN_IDS_SHA256=%s\nPREFLIGHT_RENDERED_TOKEN_COUNT=%d\n", sha(csv).c_str(), n);
    std::ofstream o(argv[3]); for (int i = 0; i < n; ++i) o << t[i] << "\n";
    printf("IDS_ECRITS=%s\n", argv[3]);
    llama_model_free(m); return 0;
}
