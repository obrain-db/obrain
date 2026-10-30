// ENGINE-008 -- WHT-EXPECTED-INVOCATIONS-18 : J2 (validation de l'inventaire)
// et J1 (invocations attendues). Fonctions PURES, registres mockables.
#pragma once
#include <set>
#include <string>
#include <utility>
#include <vector>

// ============================================================ J2
// Une entree du registre natif : nom, type REEL, pointeur natif.
struct NativeTensor {
    std::string name;      // cle EXACTE du registre
    int         type = -1; // type REEL lu sur le tenseur, jamais deduit
    const void * ptr = nullptr;
};

enum class KeyState {
    VALID,            // resolue exactement, unique, identite non nulle, type TQ2
    ABSENT,           // aucune entree de ce nom dans le registre
    AMBIGUOUS,        // plusieurs entrees distinctes portent ce nom
    DUPLICATED,       // la cle apparait plusieurs fois DANS l inventaire
    IDENTITY_NULL,    // entree presente mais identite nulle
    TYPE_NOT_TQ2      // presente, mais le type REEL n est pas TQ2_G128
};
const char * key_state_name(KeyState s);

struct InventoryKeyResult {
    std::string key;
    KeyState    state = KeyState::ABSENT;
    int         type = -1;
    std::string reason;
};

struct InventoryValidation {
    std::vector<InventoryKeyResult> keys;
    std::set<std::string> valid_keys;
    std::set<std::string> absent_keys;
    std::set<std::string> ambiguous_keys;
    std::set<std::string> duplicated_keys;
    std::set<std::string> identity_null_keys;
    std::set<std::string> type_not_tq2_keys;
    // Poids TQ2_G128 du registre HORS inventaire : le perimetre declare n est
    // donc PAS complet.
    std::set<std::string> tq2_outside_inventory;
    int  n_inventory_entries = 0;
    int  n_registry_entries  = 0;
    bool inventory_globally_complete = false;
    std::string scope;
    std::string why_incomplete;
};

// L2 -- lecteur au format EXACT du producteur (f1a-capture.cpp `f1a_load_inventory`) :
//   ligne a ligne ; colonne 0 = tensor_name ; en-tete detecte par le PREFIXE
//   `tensor_name` (11 premiers caracteres) ; `#` = commentaire ; lignes vides
//   ignorees ; nom = texte avant la premiere virgule ; nom vide ignore ;
//   DOUBLON -> compteur ET capture DESACTIVEE.
struct InventoryRead {
    std::vector<std::string> names;   // dans l ordre du fichier, doublons INCLUS
    std::set<std::string>    unique_names;
    int  n_lines = 0, n_skipped_header = 0, n_skipped_comment = 0,
         n_skipped_empty_name = 0, n_duplicates = 0;
    bool header_seen = false;
    bool capture_disabled_by_duplicates = false;
    std::string why;
};
InventoryRead read_inventory_source(const std::string & bytes, bool file_existed);

InventoryValidation validate_inventory(const std::vector<std::string> & inventory_keys,
                                       const std::vector<NativeTensor> & registry,
                                       int tq2_type_value);

// ============================================================ J1
// Invocations ATTENDUES : une invocation = un appel a `build_lora_mm` dont le
// poids (src0) est eligible. Reference INDEPENDANTE des captures reussies.
// Elle exige les operations du GRAPHE SOUMIS identifiees par leur operande
// natif. Sans cette enumeration : UNKNOWN.
struct MatmulOp {
    std::string  weight_name;       // nom EXACT du poids en src0
    const void * weight_ptr = nullptr;
};
struct ExpectedInvocations {
    bool        known = false;              // false => UNKNOWN
    std::string method;
    std::string scope;
    std::string why;                        // si UNKNOWN : le maillon manquant
    int         n_invocations = 0;          // total d appels attendus
    std::set<std::string> invoked_weights;  // poids reellement invoques
    std::set<std::string> weights_invoked_twice;  // invocations multiples
};
ExpectedInvocations compute_expected_invocations(const std::set<std::string> & eligible_weights,
                                                 const std::vector<MatmulOp> & submitted_ops,
                                                 bool graph_enumeration_available);

// ============================================================ Reconciliation J1
struct InvocationReconciliation {
    bool expected_known = false;
    int  n_eligible_weights = 0;
    int  n_expected_invocations = -1;
    int  n_captured_targets = 0;
    int  n_relations_available = 0;
    std::set<std::string> invoked_never_captured;   // invocation attendue sans capture
    std::set<std::string> captured_never_invoked;   // capture inattendue
    std::set<std::string> ambiguous_references;
    bool complete = false;
    std::string verdict;
};
InvocationReconciliation reconcile_invocations(const ExpectedInvocations & exp,
                                               const std::set<std::string> & eligible_weights,
                                               const std::set<std::string> & captured_weight_names,
                                               int n_captured_targets,
                                               int n_relations_available);
