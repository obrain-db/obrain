#include "engine008_p4_checks.h"
#include <cstdio>

const char * cond_verdict_name(CondVerdict v) {
    switch (v) {
        case CondVerdict::PASS: return "PASS";
        case CondVerdict::FAIL: return "FAIL";
        default:                return "UNKNOWN";
    }
}

static std::string jesc(const std::string & s) {
    std::string o; for (char c : s) { if (c=='"'||c=='\\') { o+='\\'; o+=c; } else if (c=='\n') o+="\\n"; else o+=c; } return o;
}

P4CheckReport check_p4_structural(const P4Inputs & in) {
    P4CheckReport r;
    r.note = "P4' est une PROPOSITION NON ADOPTEE : ces resultats ne remplacent pas P4 "
             "dans les gates du protocole actuel.";

    // ---- C1 : acquisition complete du registre natif
    { ConditionResult c; c.id = "C1";
      c.title = "acquisition complete du registre natif (llama_internal_get_tensor_map)";
      c.scope = "tous les tenseurs du modele charge";
      if (!in.model_loaded) { c.verdict = CondVerdict::UNKNOWN;
        c.reason = "aucun modele charge : l acquisition native est NOT_EXERCISED";
        c.proof = "model_loaded=false"; }
      else if (!in.registry_acquired) { c.verdict = CondVerdict::UNKNOWN;
        c.reason = "registre non acquis"; c.proof = "registry_acquired=false"; }
      else if (!in.registry_complete) { c.verdict = CondVerdict::FAIL;
        c.reason = "registre PARTIEL : un echantillon ne suffit pas";
        c.proof = "registry_complete=false"; }
      else { c.verdict = CondVerdict::PASS; c.proof = "n_tensors=" + std::to_string(in.n_tensors); }
      r.conditions.push_back(c); }

    // ---- C2 : types REELS lus sur les tenseurs
    { ConditionResult c; c.id = "C2";
      c.title = "inventaire des TYPES REELS, lus sur les tenseurs";
      c.scope = "chaque tenseur du registre";
      if (in.type_inferred_from_name) { c.verdict = CondVerdict::FAIL;
        c.reason = "un type a ete DEDUIT du nom (Q4_K_M) : INTERDIT";
        c.proof = "type_inferred_from_name=true"; }
      else if (!in.types_read_from_tensors || in.n_types_read == 0) { c.verdict = CondVerdict::UNKNOWN;
        c.reason = "les types n ont pas ete lus sur les tenseurs";
        c.proof = "n_types_read=0"; }
      else { c.verdict = CondVerdict::PASS; c.proof = "n_types_read=" + std::to_string(in.n_types_read); }
      r.conditions.push_back(c); }

    // ---- C3 : aucun poids pertinent n est TQ2_G128
    { ConditionResult c; c.id = "C3";
      c.title = "aucun poids pertinent (atteignant build_lora_mm) n est TQ2_G128";
      c.scope = "poids atteignant build_lora_mm -- goulot unique de toutes les "
                "projections lineaires (attention q/k/v/o, MLP, sortie)";
      if (in.n_relevant_weights <= 0) { c.verdict = CondVerdict::UNKNOWN;
        c.reason = "aucun poids pertinent denombre : rien a conclure";
        c.proof = "n_relevant_weights=0"; }
      else if (in.n_relevant_tq2g128 > 0) { c.verdict = CondVerdict::FAIL;
        c.reason = "au moins un poids pertinent est TQ2_G128 : la branche WHT est ATTEIGNABLE";
        char b[256]; snprintf(b,sizeof b,"n_relevant_tq2g128=%lld, ex. %s",
          in.n_relevant_tq2g128, in.tq2_offenders.empty() ? "?" : in.tq2_offenders[0].c_str());
        c.proof = b; }
      else { c.verdict = CondVerdict::PASS;
        c.proof = "n_relevant_weights=" + std::to_string(in.n_relevant_weights) +
                  ", n_relevant_tq2g128=0"; }
      r.conditions.push_back(c); }

    // ---- C4 : audit des autres sites de creation WHT
    { ConditionResult c; c.id = "C4";
      c.title = "audit des autres sites de creation WHT";
      c.scope = "tout le fork qualifie (src/, ggml/)";
      if (!in.wht_sites_audited) { c.verdict = CondVerdict::UNKNOWN;
        c.reason = "audit non mene sur le fork qualifie";
        c.proof = "wht_sites_audited=false"; }
      else if (in.n_wht_creation_sites < 0) { c.verdict = CondVerdict::UNKNOWN;
        c.reason = "nombre de sites non etabli"; c.proof = "n_sites<0"; }
      else { c.verdict = CondVerdict::PASS;
        c.proof = "n_sites=" + std::to_string(in.n_wht_creation_sites);
        for (size_t i = 0; i < in.wht_sites.size(); ++i) c.proof += (i?", ":" [") + in.wht_sites[i];
        if (!in.wht_sites.empty()) c.proof += "]"; }
      r.conditions.push_back(c); }

    // ---- C5 : identite du code et conditions du chemin
    { ConditionResult c; c.id = "C5";
      c.title = "identite du code applicable et conditions du chemin";
      c.scope = "libllama, OBRAIN_NO_ROTATION, ctx_type, gtype";
      std::string why;
      if (!in.expected_libllama_sha256.empty() && in.libllama_sha256 != in.expected_libllama_sha256)
        why = "SHA de libllama non conforme";
      else if (in.obrain_no_rotation_present)
        why = "OBRAIN_NO_ROTATION PRESENT : la branche WHT est desactivee, la propriete est vide";
      else if (in.libllama_sha256.empty())
        why = "SHA de libllama non releve";
      if (!why.empty()) { c.verdict = CondVerdict::UNKNOWN;
        c.reason = why;
        c.proof = "libllama=" + (in.libllama_sha256.empty()? std::string("<vide>") : in.libllama_sha256) +
                  ", obrain_no_rotation_present=" + (in.obrain_no_rotation_present? "true":"false") +
                  ", ctx_type=" + in.ctx_type; }
      else { c.verdict = CondVerdict::PASS;
        c.proof = "libllama=" + in.libllama_sha256 + ", OBRAIN_NO_ROTATION absent, ctx_type=" + in.ctx_type;
        if (!in.gtype_status.empty()) c.proof += ", gtype=" + in.gtype_status; }
      r.conditions.push_back(c); }

    // ---- C6 : limites de la conclusion (declaratif, toujours explicite)
    { ConditionResult c; c.id = "C6";
      c.title = "limites de la conclusion";
      c.scope = "declaratif";
      // Cette condition est satisfaite si et seulement si les limites sont
      // ENONCEES -- elle ne peut jamais transformer un FAIL en PASS.
      c.verdict = CondVerdict::PASS;
      c.proof = "limites enoncees : inatteignabilite STRUCTURELLE et non absence observee ; "
                "porte sur la CONSTRUCTION du graphe ; ne dit RIEN de l execution ; "
                "subordonnee a C2 ; ne remplace pas P3";
      c.reason = "enonce";
      r.conditions.push_back(c); }

    // ---- verdict global : le PIRE
    r.all_pass = true; r.overall = CondVerdict::PASS;
    for (const auto & c : r.conditions) {
        if (c.verdict == CondVerdict::FAIL) { r.overall = CondVerdict::FAIL; r.all_pass = false; }
        else if (c.verdict == CondVerdict::UNKNOWN && r.overall != CondVerdict::FAIL) {
            r.overall = CondVerdict::UNKNOWN; r.all_pass = false; }
    }
    return r;
}

std::string serialize_p4_checks(const P4CheckReport & r) {
    std::string o = "{\"schema\":\"ENGINE008_P4PRIME_CHECKS_v1\",\"adopted\":false";
    o += ",\"overall\":\"" + std::string(cond_verdict_name(r.overall)) + "\"";
    o += ",\"all_pass\":" + std::string(r.all_pass ? "true" : "false");
    o += ",\"note\":\"" + jesc(r.note) + "\"";
    o += ",\"conditions\":[";
    for (size_t i = 0; i < r.conditions.size(); ++i) {
        const ConditionResult & c = r.conditions[i];
        if (i) o += ",";
        o += "{\"id\":\"" + jesc(c.id) + "\",\"title\":\"" + jesc(c.title) + "\""
             ",\"verdict\":\"" + std::string(cond_verdict_name(c.verdict)) + "\""
             ",\"proof\":\"" + jesc(c.proof) + "\""
             ",\"scope\":\"" + jesc(c.scope) + "\""
             ",\"reason\":\"" + jesc(c.reason) + "\"}";
    }
    o += "]}";
    return o;
}
