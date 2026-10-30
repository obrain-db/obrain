#include "engine008_inventory.h"
#include <algorithm>
#include <map>

InventoryRead read_inventory_source(const std::string & bytes, bool file_existed) {
    InventoryRead r;
    if (!file_existed) { r.why = "inventaire illisible : la capture est DESACTIVEE"; return r; }
    std::string line;
    size_t pos = 0;
    while (pos <= bytes.size()) {
        size_t nl = bytes.find('\n', pos);
        line = bytes.substr(pos, (nl == std::string::npos) ? std::string::npos : nl - pos);
        pos = (nl == std::string::npos) ? bytes.size() + 1 : nl + 1;
        while (!line.empty() && (line.back() == '\r' || line.back() == '\n')) line.pop_back();
        if (line.empty()) continue;
        r.n_lines++;
        // en-tete : les 11 premiers caracteres valent "tensor_name"
        if (line.size() >= 11 && line.compare(0, 11, "tensor_name") == 0) {
            r.n_skipped_header++; r.header_seen = true; continue;
        }
        if (line[0] == '#') { r.n_skipped_comment++; continue; }
        const size_t c = line.find(',');
        const std::string name = (c == std::string::npos) ? line : line.substr(0, c);
        if (name.empty()) { r.n_skipped_empty_name++; continue; }
        if (!r.unique_names.insert(name).second) r.n_duplicates++;
        r.names.push_back(name);
    }
    if (r.n_duplicates != 0) {
        r.capture_disabled_by_duplicates = true;
        r.why = "DOUBLONS DANS L INVENTAIRE : capture DESACTIVEE (regle du producteur)";
    }
    return r;
}

const char * key_state_name(KeyState s) {
    switch (s) {
        case KeyState::VALID:        return "VALID";
        case KeyState::ABSENT:       return "ABSENT";
        case KeyState::AMBIGUOUS:    return "AMBIGUOUS";
        case KeyState::DUPLICATED:   return "DUPLICATED";
        case KeyState::IDENTITY_NULL:return "IDENTITY_NULL";
        default:                     return "TYPE_NOT_TQ2";
    }
}

InventoryValidation validate_inventory(const std::vector<std::string> & inv,
                                       const std::vector<NativeTensor> & reg,
                                       int tq2) {
    InventoryValidation v;
    v.n_inventory_entries = (int) inv.size();
    v.n_registry_entries  = (int) reg.size();
    // index du registre par nom EXACT
    std::map<std::string, std::vector<const NativeTensor *>> by_name;
    for (const auto & t : reg) by_name[t.name].push_back(&t);
    // doublons DANS l inventaire
    std::set<std::string> seen, dup;
    for (const auto & k : inv) if (!seen.insert(k).second) dup.insert(k);
    v.duplicated_keys = dup;

    for (const auto & k : inv) {
        InventoryKeyResult r; r.key = k;
        // BUG CORRIGE : `state` vaut ABSENT par defaut, donc « pas encore
        // determine » etait indiscernable de « determine ABSENT ». Un drapeau
        // explicite separe les deux.
        bool determined = false;
        if (dup.count(k)) {
            r.state = KeyState::DUPLICATED;
            r.reason = "cle presente plusieurs fois dans l inventaire";
            determined = true;
        } else {
            auto it = by_name.find(k);
            if (it == by_name.end()) {
                r.state = KeyState::ABSENT;
                r.reason = "aucune entree de ce nom dans le registre natif";
                determined = true;
            } else {
                const NativeTensor * t0 = it->second[0];
                if (it->second.size() > 1) {
                    bool distinct = false;
                    for (size_t i = 1; i < it->second.size(); ++i)
                        if (it->second[i]->ptr != t0->ptr) distinct = true;
                    if (distinct) {
                        r.state = KeyState::AMBIGUOUS;
                        r.reason = "plusieurs entrees distinctes portent ce nom";
                        determined = true;
                    }
                }
                if (!determined) {
                    r.type = t0->type;
                    if (t0->ptr == nullptr) {
                        r.state = KeyState::IDENTITY_NULL;
                        r.reason = "identite native nulle";
                    } else if (t0->type != tq2) {
                        r.state = KeyState::TYPE_NOT_TQ2;
                        r.reason = "type REEL different de TQ2_G128";
                    } else {
                        r.state = KeyState::VALID;
                        r.reason = "resolue exactement, unique, identite non nulle, type TQ2_G128";
                    }
                    determined = true;
                }
            }
        }
        switch (r.state) {
            case KeyState::VALID:         v.valid_keys.insert(k); break;
            case KeyState::ABSENT:        v.absent_keys.insert(k); break;
            case KeyState::AMBIGUOUS:     v.ambiguous_keys.insert(k); break;
            case KeyState::DUPLICATED:    v.duplicated_keys.insert(k); break;
            case KeyState::IDENTITY_NULL: v.identity_null_keys.insert(k); break;
            default:                      v.type_not_tq2_keys.insert(k); break;
        }
        v.keys.push_back(r);
    }
    // poids TQ2_G128 du registre HORS inventaire
    for (const auto & t : reg)
        if (t.type == tq2 && !seen.count(t.name)) v.tq2_outside_inventory.insert(t.name);

    v.scope = "perimetre DECLARE par l inventaire, resolu contre le registre natif complet";
    v.inventory_globally_complete = v.absent_keys.empty() && v.ambiguous_keys.empty() &&
                                    v.duplicated_keys.empty() && v.identity_null_keys.empty() &&
                                    v.type_not_tq2_keys.empty() && v.tq2_outside_inventory.empty();
    if (!v.tq2_outside_inventory.empty())
        v.why_incomplete = "des poids TQ2_G128 du registre sont HORS inventaire : "
                           "le perimetre declare n est PAS globalement complet";
    else if (!v.absent_keys.empty())
        v.why_incomplete = "des cles de l inventaire sont ABSENTES du registre";
    else if (!v.ambiguous_keys.empty())
        v.why_incomplete = "des cles de l inventaire sont AMBIGUES";
    return v;
}

// ---------------------------------------------------------------- J1
ExpectedInvocations compute_expected_invocations(const std::set<std::string> & elig,
                                                 const std::vector<MatmulOp> & ops,
                                                 bool graph_available) {
    ExpectedInvocations e;
    e.method = "denombrement des operations du GRAPHE SOUMIS dont l operande src0 est un "
               "poids ELIGIBLE, identifie par son NOM EXACT et son pointeur NATIF";
    e.scope  = "operations atteignant build_lora_mm dans le graphe soumis";
    if (!graph_available) {
        e.known = false;
        e.why = "enumeration du graphe soumis indisponible : sans les operations et leurs "
                "operandes natifs, les invocations attendues ne peuvent pas etre denombrees. "
                "Aucune formule de couches n est utilisee a la place.";
        return e;
    }
    if (elig.empty()) { e.known = false; e.why = "aucun poids eligible : rien a denombrer"; return e; }
    std::map<std::string,int> count;
    for (const auto & op : ops) {
        if (!elig.count(op.weight_name)) continue;   // nom EXACT, jamais approximatif
        e.n_invocations++;
        e.invoked_weights.insert(op.weight_name);
        count[op.weight_name]++;
    }
    for (const auto & kv : count) if (kv.second > 1) e.weights_invoked_twice.insert(kv.first);
    e.known = true;
    return e;
}

InvocationReconciliation reconcile_invocations(const ExpectedInvocations & exp,
                                               const std::set<std::string> & elig,
                                               const std::set<std::string> & captured,
                                               int n_captured_targets,
                                               int n_relations) {
    InvocationReconciliation r;
    r.expected_known = exp.known;
    r.n_eligible_weights = (int) elig.size();
    r.n_expected_invocations = exp.known ? exp.n_invocations : -1;
    r.n_captured_targets = n_captured_targets;
    r.n_relations_available = n_relations;
    if (exp.known) {
        for (const auto & w : exp.invoked_weights) if (!captured.count(w)) r.invoked_never_captured.insert(w);
        for (const auto & w : captured) if (!exp.invoked_weights.count(w)) r.captured_never_invoked.insert(w);
        for (const auto & w : exp.weights_invoked_twice) r.ambiguous_references.insert(w);
    }
    // Une reference UNKNOWN INTERDIT COMPLETE.
    r.complete = exp.known && r.invoked_never_captured.empty() &&
                 r.captured_never_invoked.empty() && r.ambiguous_references.empty() &&
                 r.n_captured_targets > 0;
    if (!exp.known) r.verdict = "EXPECTED_INVOCATIONS_UNKNOWN_BLOCKS_COMPLETENESS";
    else if (!r.invoked_never_captured.empty()) r.verdict = "INVOCATIONS_WITHOUT_CAPTURE";
    else if (!r.captured_never_invoked.empty()) r.verdict = "CAPTURES_UNEXPECTED";
    else if (!r.ambiguous_references.empty()) r.verdict = "AMBIGUOUS_REFERENCES";
    else if (r.complete) r.verdict = "COMPLETE";
    else r.verdict = "EMPTY_CAPTURE";
    return r;
}
