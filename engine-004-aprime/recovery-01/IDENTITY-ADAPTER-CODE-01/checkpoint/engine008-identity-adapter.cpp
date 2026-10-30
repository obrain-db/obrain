// ENGINE-008 -- IDENTITY-ADAPTER-CODE-01
// Adaptateur d'identite natif : registre fonde sur les OBJETS natifs du modele.
//
// Aucun chargement complet de modele. Aucun forward. Aucune evaluation de graphe.
// L'acquisition reelle reste derriere la garde runtime et n'est PAS exercee ici.

#include "ggml.h"
#include "llama.h"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <map>
#include <set>
#include <string>
#include <vector>

// ------------------------------------------------------------------ ACCES NATIF
// Declaration TRANSCRITE VERBATIM depuis src/llama-model.h:763 (verifiee, non
// devinee). L'implementation est src/llama-model.cpp:3115 :
//     return model->tensors_by_name;
// Le symbole est exporte par libllama.so.0.0.135 (verifie par nm --demangle).
extern const std::vector<std::pair<std::string, ggml_tensor *>> &
llama_internal_get_tensor_map(const llama_model * model);

// ------------------------------------------------------------------ CONTROLE DE LINKAGE
// §3 : le symbole decouvert par nm NE SUFFIT PAS. Ce controle REFERENCE
// reellement la fonction (prise d'adresse) sans l'appeler et sans modele nul.
// Le simple fait que ce fichier se lie prouve que le symbole est resolu.
typedef const std::vector<std::pair<std::string, ggml_tensor *>> & (*get_map_fn)(const llama_model *);
static get_map_fn g_get_tensor_map = &llama_internal_get_tensor_map;
static bool linkage_control() { return g_get_tensor_map != nullptr; }

// ------------------------------------------------------------------ CONTRAT DE ROLES
// Identifiants issus de la table normative LLM_TENSOR_NAMES (llama-arch.cpp:492-499)
// et des metadonnees reellement observees sur le candidat.
struct RoleSpec {
    const char * role;
    const char * exact_key;          // cle EXACTE, jamais un motif
    bool         exclusive;          // exclusif au chemin NextN
    bool         exclusivity_established;
};

static const std::vector<RoleSpec> ROLES = {
    { "NEXTN_EH_PROJ",          "blk.64.nextn.eh_proj.weight",          true,  true  },
    { "NEXTN_ENORM",            "blk.64.nextn.enorm.weight",            true,  true  },
    { "NEXTN_HNORM",            "blk.64.nextn.hnorm.weight",            true,  true  },
    { "NEXTN_SHARED_HEAD_NORM", "blk.64.nextn.shared_head_norm.weight", true,  true  },
    // v1_1 : exclusivite NON ETABLIE (ni prouvee, ni refutee). Reste HORS des
    // temoins positifs tant qu'elle ne l'est pas.
    { "NEXTN_SHARED_HEAD_HEAD", "blk.64.nextn.shared_head_head.weight", false, false },
};

static const RoleSpec * role_by_key(const std::string & key) {
    for (const auto & r : ROLES) if (key == r.exact_key) return &r;   // EGALITE EXACTE
    return nullptr;
}

// ------------------------------------------------------------------ ETATS
enum class EntryState {
    ROLE_RESOLVED,
    ROLE_ABSENT,
    KEY_AMBIGUOUS,
    NULL_IDENTITY,
    MULTI_KEY_OBJECT,
    EXCLUSIVITY_UNESTABLISHED
};
static const char * st_name(EntryState s) {
    switch (s) {
        case EntryState::ROLE_RESOLVED:            return "ROLE_RESOLVED";
        case EntryState::ROLE_ABSENT:              return "ROLE_ABSENT";
        case EntryState::KEY_AMBIGUOUS:            return "KEY_AMBIGUOUS";
        case EntryState::NULL_IDENTITY:            return "NULL_IDENTITY";
        case EntryState::MULTI_KEY_OBJECT:         return "MULTI_KEY_OBJECT";
        default:                                   return "EXCLUSIVITY_UNESTABLISHED";
    }
}

struct RegistryEntry {
    std::string  key;            // cle native exacte
    ggml_tensor * tensor = nullptr;
    std::string  role;           // vide si la cle n'est pas un role du contrat
    bool         exclusive = false;
    bool         exclusivity_established = false;
    EntryState   state = EntryState::ROLE_ABSENT;
    bool         usable_as_positive_witness = false;
};

// ------------------------------------------------------------------ REGISTRE
struct IdentityRegistry {
    std::vector<RegistryEntry> entries;
    std::map<const ggml_tensor *, std::vector<std::string>> by_object;

    // Recherche par IDENTITE NATIVE. Un tensor portant le bon NOM mais une
    // autre identite n'est PAS celui du registre (§6).
    std::vector<std::string> roles_of(const ggml_tensor * t) const {
        auto it = by_object.find(t);
        return it == by_object.end() ? std::vector<std::string>{} : it->second;
    }
    bool is_registered_object(const ggml_tensor * t) const {
        return by_object.find(t) != by_object.end();
    }
    bool usable_witness(const ggml_tensor * t) const {
        for (const auto & e : entries)
            if (e.tensor == t && e.usable_as_positive_witness) return true;
        return false;
    }
};

// ------------------------------------------------------------------ CONSTRUCTION PURE
// Fonction PURE : ne touche a aucun modele. Prend les entrees de table.
static IdentityRegistry build_registry(
        const std::vector<std::pair<std::string, ggml_tensor *>> & table) {
    IdentityRegistry reg;

    // 1. ambiguite de cle : la meme cle apparait deux fois
    std::map<std::string, int> key_count;
    for (const auto & kv : table) key_count[kv.first]++;

    // 2. occurrences par objet (pour MULTI_KEY_OBJECT)
    std::map<const ggml_tensor *, int> obj_count;
    for (const auto & kv : table) if (kv.second) obj_count[kv.second]++;

    for (const auto & kv : table) {
        RegistryEntry e;
        e.key    = kv.first;
        e.tensor = kv.second;

        if (kv.second == nullptr) {
            e.state = EntryState::NULL_IDENTITY;      // un tensor nul n'est
            reg.entries.push_back(e);                 // jamais "normal"
            continue;
        }
        if (key_count[kv.first] > 1) {
            e.state = EntryState::KEY_AMBIGUOUS;
            reg.entries.push_back(e);
            continue;
        }
        const RoleSpec * r = role_by_key(kv.first);
        if (r == nullptr) {
            // Cle hors contrat : enregistree comme OBJET connu, sans role.
            // L'anomalie MULTI_KEY_OBJECT s'applique a TOUTES les entrees d'un
            // objet associe a plusieurs cles, y compris celles hors contrat --
            // sinon l'anomalie serait signalee sur une seule des deux lignes.
            e.state = (obj_count[kv.second] > 1) ? EntryState::MULTI_KEY_OBJECT
                                                 : EntryState::ROLE_ABSENT;
            reg.entries.push_back(e);
            reg.by_object[kv.second];
            continue;
        }
        e.role = r->role;
        e.exclusive = r->exclusive;
        e.exclusivity_established = r->exclusivity_established;
        e.usable_as_positive_witness = r->exclusive && r->exclusivity_established;
        e.state = e.usable_as_positive_witness ? EntryState::ROLE_RESOLVED
                                               : EntryState::EXCLUSIVITY_UNESTABLISHED;
        if (obj_count[kv.second] > 1) e.state = EntryState::MULTI_KEY_OBJECT;
        reg.entries.push_back(e);
        reg.by_object[kv.second].push_back(r->role);   // TOUTES les associations
    }
    return reg;
}

// ------------------------------------------------------------------ ACQUISITION NATIVE
// Destinee au FUTUR modele charge. Derriere la garde runtime, jamais exercee ici.
// Refuse explicitement un modele nul : on ne dereference pas (§3).
struct NativeAcquireResult {
    bool ok = false;
    std::string refusal;
    IdentityRegistry registry;
};

static NativeAcquireResult acquire_native_identity(const llama_model * model) {
    NativeAcquireResult r;
    if (model == nullptr) {
        r.refusal = "modele nul : aucun dereferencement";
        return r;
    }
    // NOTE : cet appel exige un modele REELLEMENT charge. Il est present pour la
    // future passe autorisee ; il n'est pas exerce ici.
    const auto & table = llama_internal_get_tensor_map(model);
    r.registry = build_registry(table);
    r.ok = true;
    return r;
}

// ------------------------------------------------------------------ CONSOMMATEUR
enum class OperandVerdict {
    NEXTN_WITNESS_CONSUMED,
    OPERAND_NOT_A_NEXTN_WITNESS,
    NOT_CONFIRMED_ALIAS_PROVENANCE,
    NOT_CONFIRMED_IDENTITY_MAPPING
};
static const char * ov_name(OperandVerdict v) {
    switch (v) {
        case OperandVerdict::NEXTN_WITNESS_CONSUMED:        return "NEXTN_WITNESS_CONSUMED";
        case OperandVerdict::OPERAND_NOT_A_NEXTN_WITNESS:   return "OPERAND_NOT_A_NEXTN_WITNESS";
        case OperandVerdict::NOT_CONFIRMED_ALIAS_PROVENANCE:return "NOT_CONFIRMED_ALIAS_PROVENANCE";
        default:                                            return "NOT_CONFIRMED_IDENTITY_MAPPING";
    }
}

// Le classificateur demande les roles d'un operande PAR SON IDENTITE NATIVE.
// `derived` signale un operande dont la provenance n'est pas encore resolue
// (resultat d'une operation, alias non etabli) : les regles d'alias ne sont
// PAS implementees dans cette passe -> NOT_CONFIRMED_ALIAS_PROVENANCE.
static OperandVerdict classify_operand(const IdentityRegistry & reg,
                                       const ggml_tensor * operand,
                                       bool derived) {
    if (derived) return OperandVerdict::NOT_CONFIRMED_ALIAS_PROVENANCE;
    if (operand == nullptr) return OperandVerdict::NOT_CONFIRMED_IDENTITY_MAPPING;
    if (!reg.is_registered_object(operand)) {
        // Non enregistre : ni temoin, ni preuve d'absence. On ne conclut RIEN
        // sur NextN a partir d'un objet inconnu.
        return OperandVerdict::OPERAND_NOT_A_NEXTN_WITNESS;
    }
    return reg.usable_witness(operand) ? OperandVerdict::NEXTN_WITNESS_CONSUMED
                                       : OperandVerdict::OPERAND_NOT_A_NEXTN_WITNESS;
}

// ------------------------------------------------------------------ TESTS
static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-60s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}

static ggml_tensor * synth_tensor(struct ggml_context * ctx, const char * name) {
    ggml_tensor * t = ggml_new_tensor_1d(ctx, GGML_TYPE_F32, 8);
    ggml_set_name(t, name);
    return t;
}

int main() {
    printf("ENGINE-008 -- ADAPTATEUR D'IDENTITE NATIF (registre MOCKE, aucun modele)\n\n");

    // --- CONTROLE DE LINKAGE (§3) : reference reelle au symbole Q1
    ck("linkage_control_references_q1_symbol", linkage_control());

    struct ggml_init_params p;
    p.mem_size = 4 * 1024 * 1024; p.mem_buffer = nullptr; p.no_alloc = true;
    struct ggml_context * ctx = ggml_init(p);

    // Table MOCKEE. Les objets sont des tensors ggml SYNTHETIQUES, non evalues.
    ggml_tensor * t_eh    = synth_tensor(ctx, "blk.64.nextn.eh_proj.weight");
    ggml_tensor * t_enorm = synth_tensor(ctx, "blk.64.nextn.enorm.weight");
    ggml_tensor * t_head  = synth_tensor(ctx, "blk.64.nextn.shared_head_head.weight");
    ggml_tensor * t_other = synth_tensor(ctx, "blk.0.ffn_gate.weight");
    ggml_tensor * t_trap  = synth_tensor(ctx, "blk.64.nextn.eh_proj.weight");  // MEME NOM

    std::vector<std::pair<std::string, ggml_tensor *>> table = {
        { "blk.64.nextn.eh_proj.weight",          t_eh    },   // role exclusif
        { "blk.64.nextn.enorm.weight",            t_enorm },   // role exclusif
        { "blk.64.nextn.shared_head_head.weight", t_head  },   // exclusivite NON etablie
        { "blk.0.ffn_gate.weight",                t_other },   // cle hors contrat
        { "token_embd.weight",                    t_enorm },   // MEME OBJET, 2e cle
        { "blk.9.attn_q.weight",                  nullptr },   // identite nulle
        // t_trap porte le MEME NOM D AFFICHAGE que t_eh, mais il est enregistre
        // sous une AUTRE cle : c est donc un objet connu SANS role NextN.
        { "blk.99.other.weight",                  t_trap  },
    };
    IdentityRegistry reg = build_registry(table);

    // Cas d anomalie SEPARE : une table contenant une CLE EN DOUBLE. Dans une
    // table native reelle les cles sont uniques ; ce cas est une anomalie
    // volontaire, et les deux entrees doivent etre marquees ambigues.
    ggml_tensor * d1 = synth_tensor(ctx, "dup.a");
    ggml_tensor * d2 = synth_tensor(ctx, "dup.b");
    std::vector<std::pair<std::string, ggml_tensor *>> dup_table = {
        { "blk.64.nextn.eh_proj.weight", d1 },
        { "blk.64.nextn.eh_proj.weight", d2 },
    };
    IdentityRegistry reg_dup = build_registry(dup_table);

    // --- 1. cle exacte resolue
    { auto roles = reg.roles_of(t_eh);
      ck("exact_key_resolves_role", roles.size() >= 1 &&
         std::find(roles.begin(), roles.end(), "NEXTN_EH_PROJ") != roles.end()); }
    // --- 2. temoin positif utilisable (exclusif ET exclusivite etablie)
    ck("exclusive_role_is_usable_witness", reg.usable_witness(t_eh));
    // --- 3. role attendu absent (cle hors contrat)
    { auto roles = reg.roles_of(t_other);
      ck("key_outside_contract_has_no_role", roles.empty()); }
    // --- 4. cle ressemblante refusee (egalite exacte, pas de prefixe)
    ck("similar_key_is_refused", role_by_key("blk.64.nextn.eh_proj") == nullptr &&
                                 role_by_key("blk.64.nextn.eh_proj.weights") == nullptr);
    // --- 5. nom trompeur SANS identite enregistree
    { ggml_tensor * ghost = synth_tensor(ctx, "blk.64.nextn.eh_proj.weight");
      ck("misleading_name_without_registered_identity", !reg.is_registered_object(ghost));
      ck("misleading_name_is_not_a_witness", !reg.usable_witness(ghost)); }
    // --- 6. MEME NOM, AUTRE OBJET : t_trap porte le bon nom mais n'est pas
    //        celui du registre sous cette identite -> aucune conclusion
    ck("same_name_other_object_is_not_a_witness",
       reg.is_registered_object(t_trap) && !reg.usable_witness(t_trap));
    // --- 7. identite enregistree retrouvee par identite (pas par nom)
    ck("identity_lookup_by_native_object", reg.is_registered_object(t_eh) &&
                                           reg.is_registered_object(t_enorm));
    // --- 8. plusieurs cles pour un meme objet -> MULTI_KEY_OBJECT, et TOUTES
    //        les associations sont preservees
    { int n = 0; EntryState s = EntryState::ROLE_ABSENT;
      for (const auto & e : reg.entries) if (e.tensor == t_enorm) { n++; s = e.state; }
      ck("multi_key_object_detected", n == 2 && s == EntryState::MULTI_KEY_OBJECT); }
    // --- 9. exclusivite indeterminee : hors temoins positifs
    ck("unestablished_exclusivity_is_not_a_witness", !reg.usable_witness(t_head));
    { EntryState s = EntryState::ROLE_ABSENT;
      for (const auto & e : reg.entries) if (e.tensor == t_head) s = e.state;
      ck("unestablished_exclusivity_state", s == EntryState::EXCLUSIVITY_UNESTABLISHED); }
    // --- 10. tensor nul -> NULL_IDENTITY, jamais "normal par defaut"
    { bool found = false;
      for (const auto & e : reg.entries)
          if (e.tensor == nullptr && e.state == EntryState::NULL_IDENTITY) found = true;
      ck("null_identity_state_explicit", found); }
    // --- 11. cle ambigue (double)
    { int n = 0;
      for (const auto & e : reg_dup.entries) if (e.state == EntryState::KEY_AMBIGUOUS) n++;
      ck("duplicate_key_marks_both_entries_ambiguous", n == 2);
      ck("ambiguous_key_never_yields_witness",
         !reg_dup.usable_witness(d1) && !reg_dup.usable_witness(d2)); }
    // --- 12. consommateur : temoin consomme
    ck("consumer_witness_consumed",
       classify_operand(reg, t_eh, false) == OperandVerdict::NEXTN_WITNESS_CONSUMED);
    // --- 13. consommateur : objet non temoin
    ck("consumer_non_witness",
       classify_operand(reg, t_other, false) == OperandVerdict::OPERAND_NOT_A_NEXTN_WITNESS);
    // --- 14. consommateur : operande derive non resolu -> alias non implemente
    ck("consumer_derived_operand_not_confirmed_alias",
       classify_operand(reg, t_eh, true) == OperandVerdict::NOT_CONFIRMED_ALIAS_PROVENANCE);
    // --- 15. consommateur : identite nulle
    ck("consumer_null_identity_not_confirmed",
       classify_operand(reg, nullptr, false) == OperandVerdict::NOT_CONFIRMED_IDENTITY_MAPPING);
    // --- 16. acquisition native : modele nul -> refus, AUCUN dereferencement
    { auto a = acquire_native_identity(nullptr);
      ck("native_acquire_refuses_null_model", !a.ok &&
         a.refusal.find("nul") != std::string::npos); }

    ggml_free(ctx);

    printf("\nIDENTITY_REGISTRY_MOCK_SELFTEST=%s\n", failed==0?"PASS":"FAIL");
    printf("ADAPTER_FAILED_COUNT=%d\n", failed);
    printf("NATIVE_TENSOR_ACCESS_LINK_VERIFIED=YES\n");
    printf("NATIVE_MODEL_IDENTITY_ACQUISITION=NOT_EXERCISED\n");
    printf("FULL_MODEL_LOAD_COUNT=0\nMODEL_FORWARD_CALL_COUNT=0\n");
    printf("SYNTHETIC_GRAPH_NUMERIC_EVALUATION_COUNT=0\n");
    return failed == 0 ? 0 : 1;
}
