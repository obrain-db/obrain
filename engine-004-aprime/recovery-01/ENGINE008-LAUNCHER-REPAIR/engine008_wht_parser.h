// ENGINE-008 -- WHT-CAPTURE-CONNECT-12 : surfaces WHT reelles.
// Aucune dependance a ggml. Pointeurs des captures = valeurs ARCHIVEES.
#pragma once
#include <cstdint>
#include <set>
#include <string>
#include <utility>
#include <vector>
#include "engine008_capture_parser.h"

// ---------------------------------------------------------------- GRAPH_CAPTURE_MANIFEST.csv
// 24 colonnes. Porte `rotation_applied` ET le lien vers la soumission.
struct WhtTargetRow {
    std::string        run_id;
    int                epoch = -1;
    int                target_id = -1;
    int                capture_id = -1;
    std::string        weight_name;
    int                family = -1;
    int                expected_K = -1;
    int                rotation_applied = -1;      // 1 / 0 / -1 (absent)
    std::string        kind;
    std::string        path;
    int                decode_step = -1;
    unsigned long long compute_submission_id = 0;
};
struct WhtManifestFile {
    FileState state = FileState::ABSENT;
    std::string why;
    std::vector<WhtTargetRow> rows;
    long long duplicate_target_id = 0;
    long long missing_rotation_field = 0;
    long long rotation_true = 0, rotation_false = 0;   // en LIGNES
    int n_distinct_targets = 0;      // (run, epoch, target_id, capture_id)
    int n_distinct_identities = 0;   // + kind : l identite REELLE d une ligne
    int rotation_true_targets = 0;   // cibles distinctes portant rotation_applied=1
    std::vector<std::string> kinds;  // types de lignes reellement presents
};

// ---------------------------------------------------------------- GRAPH_ROLE_RELATIONSHIPS.csv
struct RoleRelRow {
    int  epoch = -1; int target_id = -1; std::string weight_name;
    int  sub_at_build = -1;                 // le tenseur etait-il dans le graphe SOUMIS a la construction
    int  cur_op = -1, cur_src0_is_effective = -1;
    int  mulmat_op = -1, mulmat_src0_is_weight = -1, mulmat_src1_is_cur = -1;
    int  mmcap_op = -1, mmcap_src0_is_mmraw = -1;
    int  downstream_reach = -1;
    int  role_ok = -1;                      // <= verdict du producteur
    std::string effect_class;
};
struct RoleRelFile {
    FileState state = FileState::ABSENT; std::string why;
    std::vector<RoleRelRow> rows;
    long long duplicate_target_id = 0;
};

// ---------------------------------------------------------------- TARGET_EFFECT_PATH.csv
struct EffectPathRow {
    int epoch = -1; int target_id = -1; std::string weight_name;
    std::string role, effect_class;
    int src0_is_weight = -1, src1_is_cur = -1, dst_is_cur = -1;
    int effect_ok = -1;                     // <= verdict du producteur
};
struct EffectPathFile {
    FileState state = FileState::ABSENT; std::string why;
    std::vector<EffectPathRow> rows;
};

// ---------------------------------------------------------------- RESIDENCY_MANIFEST.csv
struct ResidencyRow {
    int target_id = -1, capture_id = -1;
    std::string role, tensor_ptr, tensor_name;
    int op = -1, type = -1;
    long long ne0 = -1, nb0 = -1, nbytes = -1;
    int contiguous = -1;
    std::string buffer_ptr;
    int resident = -1;
    int in_submitted_graph = -1;            // <= appartenance au graphe SOUMIS
    std::string weight_name;
};
struct ResidencyFile {
    FileState state = FileState::ABSENT; std::string why;
    std::vector<ResidencyRow> rows;
};

WhtManifestFile parse_wht_manifest(const std::string & bytes, bool existed);
RoleRelFile     parse_role_relations(const std::string & bytes, bool existed);
EffectPathFile  parse_effect_path(const std::string & bytes, bool existed);
ResidencyFile   parse_residency(const std::string & bytes, bool existed);
// ---------------------------------------------------------------- N1 : CIBLES ATTENDUES
// INDEPENDANT des captures reussies : derive de l'inventaire valide ET des types
// REELS du modele charge, pas des lignes du manifeste.
enum class N1State { KNOWN, UNKNOWN };
struct ExpectedTargets {
    N1State     state = N1State::UNKNOWN;
    std::string method;      // comment N1 a ete obtenu
    std::string scope;       // perimetre COUVERT par cet inventaire
    std::string why;         // si UNKNOWN : ce qui manque
    std::set<std::string> identities;   // noms de poids attendus, DISTINCTS
    int n_inventory_names = 0;          // noms dans l inventaire
    int n_model_tq2g128   = 0;          // tenseurs TQ2_G128 du modele
};
// Construit N1 depuis les noms d'inventaire et les (nom, type) du modele.
// type_tq2 == 3. UNKNOWN si l'un des deux est vide.
ExpectedTargets compute_n1(const std::vector<std::string> & inventory_names,
                           const std::vector<std::pair<std::string,int>> & model_tensors,
                           const std::string & inventory_path, int tq2_type_value);

// ---------------------------------------------------------------- RECONCILIATION
struct CoverageReconciliation {
    int n1 = -1;                 // -1 = UNKNOWN
    int n2 = 0;                  // cibles capturees (couples epoch/target)
    int n3 = 0;                  // cibles avec relations requises
    int n_lines = 0;             // lignes du manifeste (jamais un compte de cibles)
    int n_missing = -1;          // N1 - N2, si N1 connu
    int n_unexpected = 0;        // captures HORS N1
    std::vector<std::string> missing_identities;
    std::vector<std::string> unexpected_identities;
    bool complete = false;
    std::string verdict;
};
CoverageReconciliation reconcile_coverage(const ExpectedTargets & exp,
                                          const WhtManifestFile & m,
                                          const RoleRelFile & r,
                                          const EffectPathFile & e);

WhtManifestFile read_wht_manifest(const std::string & dir);
RoleRelFile     read_role_relations(const std::string & dir);
EffectPathFile  read_effect_path(const std::string & dir);
ResidencyFile   read_residency(const std::string & dir);

// ---------------------------------------------------------------- VERDICT, PORTEE EXACTE
// Les cinq etats sont DISTINCTS et ne se confondent pas.
enum class WhtVerdict {
    WHT_CONSTRUCTED,              // ggml_wht insere dans le graphe construit
    WHT_CONSUMED_IN_SUBMITTED,    // le resultat WHT alimente une op du graphe SOUMIS
    SUBMISSION_OK,                // la soumission qui porte ces cibles a reussi
    EXECUTION_NOT_ESTABLISHED,    // aucune surface auditee n'observe l'execution
    INCOMPLETE_COVERAGE,          // couverture insuffisante pour conclure
    AMBIGUOUS_ASSOCIATION,        // plusieurs candidates ou jointure non unique
    SURFACE_MISSING_OR_MALFORMED  // une surface requise est absente ou illisible
};
const char * wht_verdict_name(WhtVerdict v);

struct WhtCoverage {
    int n_targets_expected = 0;      // cibles attendues = target_id distincts du manifeste
    int n_targets_with_rotation = 0; // lignes portant une valeur de rotation exploitable
    int n_rotation_true = 0, n_rotation_false = 0;
    int n_in_submitted_graph = 0;
    int n_role_ok = 0, n_effect_ok = 0;
    // Dénominatif EXPLICITE : cibles du manifeste ayant une couverture
    // roles/effet, par opposition au total attendu.
    int n_targets_with_roles = 0;
    int n_targets_with_effect = 0;
    int n_targets_with_residency = 0;
    bool manifest_present = false, roles_present = false, effect_present = false, residency_present = false;
    std::string limits;
};

struct WhtAssessment {
    WhtVerdict  verdict = WhtVerdict::SURFACE_MISSING_OR_MALFORMED;
    WhtCoverage coverage;
    CoverageReconciliation recon;
    std::string scope;         // portee EXACTE atteinte
    std::vector<std::string> blockers;
    void limits_placeholder() {}
};

// Evalue la chaine a partir des quatre surfaces. Ne conclut JAMAIS au-dela de
// ce que les surfaces observent.
WhtAssessment assess_wht(const WhtManifestFile & m, const RoleRelFile & r,
                         const EffectPathFile & e, const ResidencyFile & s,
                         const ExpectedTargets & exp = ExpectedTargets());
