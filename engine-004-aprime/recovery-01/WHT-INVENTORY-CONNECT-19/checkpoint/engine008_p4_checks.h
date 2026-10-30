// ENGINE-008 -- PREFLIGHT-REMAINING-CHECKS-17
// Verificateurs des SIX conditions de P4' -- PROPRIETE STRUCTURELLE PROPOSEE.
// NON ADOPTEE : ces resultats ne remplacent PAS P4 dans les gates du protocole
// actuel. Ils sont serialises comme verification d'une proposition.
#pragma once
#include <string>
#include <utility>
#include <vector>

enum class CondVerdict { PASS, FAIL, UNKNOWN };
const char * cond_verdict_name(CondVerdict v);

struct ConditionResult {
    std::string  id;        // C1..C6
    std::string  title;
    CondVerdict  verdict = CondVerdict::UNKNOWN;
    std::string  proof;     // preuve utilisee
    std::string  scope;     // perimetre
    std::string  reason;    // motif precis
};

// Entrees du verificateur. AUCUNE n'est deduite du nom `Q4_K_M`.
struct P4Inputs {
    // C1 : registre natif
    bool        registry_acquired = false;
    bool        registry_complete = false;   // toutes les entrees, pas un echantillon
    long long   n_tensors = 0;
    bool        model_loaded = false;
    // C2 : types REELS lus sur les tenseurs
    bool        types_read_from_tensors = false;
    long long   n_types_read = 0;
    bool        type_inferred_from_name = false;   // interdit : doit rester faux
    // C3 : poids pertinents atteignant build_lora_mm
    long long   n_relevant_weights = 0;
    long long   n_relevant_tq2g128 = 0;
    std::vector<std::string> tq2_offenders;        // noms des poids TQ2_G128 pertinents
    // C4 : audit des autres sites de creation WHT
    bool        wht_sites_audited = false;
    long long   n_wht_creation_sites = -1;
    std::vector<std::string> wht_sites;
    // C5 : identite du code et conditions du chemin
    std::string libllama_sha256;
    std::string expected_libllama_sha256;
    bool        obrain_no_rotation_present = true; // doit etre ABSENT
    std::string ctx_type;
    std::string gtype_status;
};

struct P4CheckReport {
    std::vector<ConditionResult> conditions;
    CondVerdict overall = CondVerdict::UNKNOWN;
    bool        all_pass = false;
    std::string note;       // rappel : proposition non adoptee
};

P4CheckReport check_p4_structural(const P4Inputs & in);
std::string   serialize_p4_checks(const P4CheckReport & r);
