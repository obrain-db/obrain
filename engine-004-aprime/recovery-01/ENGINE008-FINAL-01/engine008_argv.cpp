#include "engine008_argv.h"
#include <cstdlib>
#include <cstring>
#include <sys/stat.h>

ParsedArgs parse_args(int argc, char ** argv) {
    ParsedArgs p;
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        const ArgSpec * spec = nullptr;
        for (const auto & s : accepted_args()) if (a == s.name) { spec = &s; break; }
        if (spec == nullptr) { p.why = "argument INCONNU : " + a; return p; }
        std::string v;
        if (spec->requires_value) {
            if (i + 1 >= argc) { p.why = "valeur manquante pour " + a; return p; }
            v = argv[++i];
            if (v.empty()) { p.why = "valeur VIDE pour " + a; return p; }
        }
        if      (a == "--mode") p.mode = v;
        else if (a == "--auth-doc") p.auth_doc = v;
        else if (a == "--plan") p.plan = v;
        else if (a == "--protocol") p.protocol = v;
        else if (a == "--binary") p.binary = v;
        else if (a == "--model") p.model = v;
        else if (a == "--template") p.templ = v;
        else if (a == "--capture-root") p.capture_root = v;
        else if (a == "--model-tag") p.model_tag = v;
        else if (a == "--run-tag") p.run_tag = v;
        else if (a == "--max-new-tokens") {
            if (v.find_first_not_of("0123456789") != std::string::npos) {
                p.why = "--max-new-tokens non numerique : " + v; return p; }
            p.max_new_tokens = atoi(v.c_str());
        }
        else if (a == "--allow-model-compute") p.flag_allow_model_compute = true;
    }
    p.ok = true;
    return p;
}

static bool dir_empty_or_absent(const std::string & d) {
    struct stat st;
    if (stat(d.c_str(), &st) != 0) return true;
    std::string cmd = "ls -A '" + d + "' 2>/dev/null | head -1";
    FILE * f = popen(cmd.c_str(), "r"); if (!f) return false;
    char b[512] = {0}; if (!fgets(b, sizeof b, f)) b[0] = '\0';
    pclose(f);
    return b[0] == '\0';
}

std::string validate_precompute(const ParsedArgs & a, const std::string & ep,
                                const std::string & epr, const std::string & eb) {
    if (!a.ok) return a.why;
    if (a.mode != "real-preflight" && a.mode != "verify-nominal")
        return "mode inconnu : " + a.mode;
    // parametres OBLIGATOIRES
    if (a.model.empty())        return "--model OBLIGATOIRE et absent";
    if (a.templ.empty())        return "--template OBLIGATOIRE et absent";
    if (a.capture_root.empty()) return "--capture-root OBLIGATOIRE et absent";
    if (a.model_tag.empty())    return "--model-tag OBLIGATOIRE et absent";
    if (a.run_tag.empty())      return "--run-tag OBLIGATOIRE et absent";
    if (a.auth_doc.empty())     return "--auth-doc OBLIGATOIRE et absent";
    if (a.max_new_tokens < 0)   return "--max-new-tokens OBLIGATOIRE et absent";
    // la configuration du plan est IMPOSEE : un argument ne peut pas la changer
    if (a.max_new_tokens != 8)  return "--max-new-tokens doit valoir 8 (plan gele), recu " +
                                       std::to_string(a.max_new_tokens);
    if (a.model_tag != "baseline" && a.model_tag != "candidate")
        return "--model-tag doit etre baseline ou candidate, recu " + a.model_tag;
    if (a.run_tag != "run1" && a.run_tag != "run2")
        return "--run-tag doit etre run1 ou run2, recu " + a.run_tag;
    // destination DANGEREUSE
    if (a.capture_root == "/" || a.capture_root == "//" )
        return "racine de capture DANGEREUSE : " + a.capture_root;
    // REUTILISATION d une destination
    if (!dir_empty_or_absent(a.capture_root))
        return "destination de capture NON VIDE (reutilisation interdite) : " + a.capture_root;
    // identites : plan, protocole, binaire
    if (!ep.empty()  && a.plan != ep)      return "plan DISCORDANT";
    if (!epr.empty() && a.protocol != epr) return "protocole DISCORDANT";
    if (!eb.empty()  && a.binary != eb)    return "binaire DISCORDANT";
    return "";
}
