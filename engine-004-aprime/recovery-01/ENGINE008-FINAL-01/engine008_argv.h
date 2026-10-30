// ENGINE-008 -- parsing des arguments, PARTAGE entre le point d'entree reel et
// le mode de verification sans calcul. Meme parsing, memes refus.
#pragma once
#include <string>
#include <vector>

struct ArgSpec { const char * name; bool requires_value; };
// Liste EXACTE des arguments acceptes. Tout autre argument est REFUSE.
inline const std::vector<ArgSpec> & accepted_args() {
    static const std::vector<ArgSpec> A = {
        {"--mode", true}, {"--auth-doc", true}, {"--plan", true}, {"--protocol", true},
        {"--binary", true}, {"--model", true}, {"--template", true},
        {"--capture-root", true}, {"--model-tag", true}, {"--run-tag", true},
        {"--max-new-tokens", true}, {"--allow-model-compute", false},
    };
    return A;
}
struct ParsedArgs {
    bool ok = false;
    std::string why;
    std::string mode, auth_doc, plan, protocol, binary, model, templ, capture_root,
                model_tag, run_tag;
    int  max_new_tokens = -1;
    bool flag_allow_model_compute = false;
};
ParsedArgs parse_args(int argc, char ** argv);

// Verification AVANT calcul. Rend une raison de refus, ou une chaine vide.
std::string validate_precompute(const ParsedArgs & a, const std::string & expected_plan,
                                const std::string & expected_protocol,
                                const std::string & expected_binary);
