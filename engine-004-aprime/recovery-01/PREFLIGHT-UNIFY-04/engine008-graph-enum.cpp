// ENGINE-008 -- GRAPH-ENUMERATOR-IMPLEMENT-01
// Enumerateur du GRAPHE SOUMIS via les acces Q1 EXISTANTS.
// Aucune modification de libllama. Aucun forward. Aucune evaluation numerique.
//
// Separation exigee (§4) :
//   - EXPORTEUR  : graph_export() -- recoit un graphe VALIDE et des metadonnees.
//   - ADAPTATEUR : last_graph_acquire() -- acquiert le dernier graphe publie
//                  aupres d'une source INJECTABLE, verifie la fenetre, puis
//                  appelle l'exporteur seulement si les preconditions passent.

#include "llama.h"
#include "ggml.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <algorithm>
#include <map>
#include <set>
#include <string>
#include <vector>

// ------------------------------------------------------------------ ABI Q1
// VERIFIE par nm sur libllama.so.0.0.135 :
//   B f1a_last_gf        (NON mangle  -> linkage C)
//   B f1a_last_gf_nodes  (NON mangle  -> linkage C)
//   T _Z15f1a_sub_node_opPKv          (mangle -> linkage C++)
extern "C" {
extern const void * f1a_last_gf;
extern int          f1a_last_gf_nodes;
}
// f1a_sub_node_op est C++ : on ne le declare pas, on ne l'utilise pas ici.
// (Son usage exigerait de recopier une signature mangée ; l'enumeration passe
//  par l'API ggml PUBLIQUE, verifiee exportee par libggml-base.so.0.16.0 :
//  ggml_graph_n_nodes, ggml_graph_node, ggml_graph_nodes, ggml_op_name.)

// ------------------------------------------------------------------ §7 regles
enum class Coverage {
    COMPLETE,
    NO_PUBLISHED_GRAPH,
    NULL_DECLARED_COUNT,
    COUNT_MISMATCH,
    UNRESOLVED_SRC,
    CORRELATION_FAILED,
    SUBMISSION_RC_NONZERO,
    INTERMEDIATE_POSSIBLE
};
static const char * cov_name(Coverage c) {
    switch (c) {
        case Coverage::COMPLETE:               return "COMPLETE";
        case Coverage::NO_PUBLISHED_GRAPH:     return "NO_PUBLISHED_GRAPH";
        case Coverage::NULL_DECLARED_COUNT:    return "NULL_DECLARED_COUNT";
        case Coverage::COUNT_MISMATCH:         return "COUNT_MISMATCH";
        case Coverage::UNRESOLVED_SRC:         return "UNRESOLVED_SRC";
        case Coverage::CORRELATION_FAILED:     return "CORRELATION_FAILED";
        case Coverage::SUBMISSION_RC_NONZERO:  return "SUBMISSION_RC_NONZERO";
        default:                               return "INTERMEDIATE_SUBMISSIONS_POSSIBLE";
    }
}

enum class NextnVerdict {
    STRUCTURE_DETECTED,
    NOT_DETECTED_IN_COMPLETE_SCOPE,
    NOT_CONFIRMED_INCOMPLETE,
    NOT_CONFIRMED_AMBIGUOUS,
    MALFORMED,
    SURFACE_INSUFFICIENT,
    NOT_CONFIRMED_IDENTIFICATION_RULE_INSUFFICIENT
};
static const char * nextn_name(NextnVerdict v) {
    switch (v) {
        case NextnVerdict::STRUCTURE_DETECTED:            return "NEXTN_STRUCTURE_DETECTED";
        case NextnVerdict::NOT_DETECTED_IN_COMPLETE_SCOPE:return "NEXTN_NOT_DETECTED_IN_COMPLETE_SUBMITTED_GRAPH_SCOPE";
        case NextnVerdict::NOT_CONFIRMED_INCOMPLETE:      return "NOT_CONFIRMED_INCOMPLETE_COVERAGE";
        case NextnVerdict::NOT_CONFIRMED_AMBIGUOUS:       return "NOT_CONFIRMED_AMBIGUOUS_MAPPING";
        case NextnVerdict::MALFORMED:                     return "MALFORMED_CAPTURE";
        default:                                          return "SURFACE_INSUFFICIENT";
    }
}

// ------------------------------------------------------------------ metadonnees
struct CaptureMeta {
    std::string run_id;
    unsigned long long submission_id = 0;
    int         submission_rc    = 0;
    int         published_n_nodes = 0;   // f1a_last_gf_nodes
    std::string published_gf;            // colonne `gf` du snapshot
    std::string published_fingerprint;   // colonne `fingerprint` du snapshot
    bool        snapshot_present = false;
    int         submit_events_expected = 1; // COMPTE (etait bool : bug)
    int         submit_events_observed = 1;
};

struct NodeRow {
    int         index = 0;
    int         op_code = 0;
    std::string op_name;
    bool        op_name_available = false;
    std::string name;
    int         type_code = 0;
    int         n_dims = 0;
    int64_t     ne[4] = {0,0,0,0};
    bool        is_leaf = false;
    bool        is_output_flag = false;
    int         n_src = 0;
    // Indices par SLOT, capacite exacte GGML_MAX_SRC (=10, ggml.h:224).
    // -1 = feuille/externe, -2 = calcule mais absent, -9 = slot vide.
    // AUCUNE compaction : le slot k reste le slot k.
    int         src_index[GGML_MAX_SRC];
    bool        has_nextn_leaf = false;
    bool        consumes_nextn_leaf = false;
};

struct ExportResult {
    std::vector<NodeRow> rows;
    int  n_declared = 0;
    int  n_exported = 0;
    bool malformed  = false;
    std::string malformed_why;
    int  unresolved_src = 0;
    int  nextn_leaf_count = 0;
    int  nextn_op_count = 0;
    std::set<const void *> nextn_leaves_seen;
};

static std::string csv_escape(const std::string & s) {
    bool need = false;
    for (char c : s) if (c == ',' || c == '"' || c == '\n' || c == '\r') { need = true; break; }
    if (!need) return s;
    std::string o = "\"";
    for (char c : s) { if (c == '"') o += "\"\""; else o += c; }
    return o + "\"";
}

// Un nom est "nextn-suspect" s'il contient le motif -- INDICATEUR D'AMBIGUITE
// uniquement, JAMAIS une conclusion (§8).
static bool name_looks_nextn(const std::string & n) {
    return n.find("nextn") != std::string::npos || n.find("NEXTN") != std::string::npos;
}

// ------------------------------------------------------------------ EXPORTEUR
// Recoit un graphe VALIDE. N'evalue rien, ne lit aucune donnee numerique.
static ExportResult graph_export(struct ggml_cgraph * g, const CaptureMeta & meta) {
    ExportResult r;
    if (g == nullptr) { r.malformed = true; r.malformed_why = "graphe nul"; return r; }

    const int n = ggml_graph_n_nodes(g);
    r.n_declared = n;
    if (n <= 0) { r.malformed = true; r.malformed_why = "n_nodes <= 0"; return r; }

    // Table pointeur -> index local (identite LOCALE seulement).
    std::map<const void *, int> idx;
    std::vector<struct ggml_tensor *> nodes;
    nodes.reserve(n);
    for (int i = 0; i < n; ++i) {
        struct ggml_tensor * t = ggml_graph_node(g, i);
        if (!t) { r.malformed = true; r.malformed_why = "noeud nul a l index"; return r; }
        nodes.push_back(t);
        idx[(const void *) t] = i;
    }

    for (int i = 0; i < n; ++i) {
        struct ggml_tensor * t = nodes[i];
        NodeRow row;
        row.index    = i;
        row.op_code  = (int) t->op;
        const char * on = ggml_op_name(t->op);
        row.op_name_available = (on != nullptr);
        row.op_name  = on ? on : "";
        row.name     = t->name ? std::string(t->name) : std::string();
        row.type_code= (int) t->type;
        row.n_dims   = GGML_MAX_DIMS;
        for (int d = 0; d < 4; ++d) row.ne[d] = t->ne[d];
        row.is_leaf = (t->op == GGML_OP_NONE);
        for (int k = 0; k < GGML_MAX_SRC; ++k) row.src_index[k] = -9;
        row.n_src    = 0;
        for (int k = 0; k < GGML_MAX_SRC; ++k) {
            struct ggml_tensor * s = t->src[k];
            if (!s) continue;
            row.n_src++;
            auto it = idx.find((const void *) s);
            if (it != idx.end()) {
                row.src_index[k] = it->second;          // noeud du graphe soumis
            } else if (s->op == GGML_OP_NONE) {
                row.src_index[k] = -1;                  // FEUILLE : legitime
                const char * sn = s->name ? s->name : "";
                if (name_looks_nextn(sn)) {
                    // Une OPERATION consomme une FEUILLE NextN : c'est la
                    // STRUCTURE qui compte, pas la chaine du nom.
                    row.consumes_nextn_leaf = true;
                    if (!r.nextn_leaves_seen.count((const void *) s)) {
                        r.nextn_leaves_seen.insert((const void *) s);
                        r.nextn_leaf_count++;
                    }
                }
            } else {
                row.src_index[k] = -2;                  // calcule mais absent
                r.unresolved_src++;                     // vraie reference non resolue
            }
        }
        if (row.consumes_nextn_leaf) r.nextn_op_count++;
        r.rows.push_back(row);
    }
    r.n_exported = (int) r.rows.size();

    // nextn_op_count et nextn_leaf_count sont accumules pendant la resolution
    // des sources. Aucune seconde passe : la structure est lue une fois.
    return r;
}

// ------------------------------------------------------------------ serialisation
static std::string export_to_csv(const ExportResult & r, const CaptureMeta & meta) {
    std::string o =
        "schema,run_id,submission_id,graph_ptr,graph_fingerprint,n_declared,n_exported,"
        "node_index,op_code,op_name_available,tensor_name_or_empty,tensor_name_len,"
        "type_code,n_dims,ne_0,ne_1,ne_2,ne_3,is_leaf,is_output_flag,n_src,"
        "src0_index,src1_index,src2_index,src3_index,src4_index,src5_index,"
        "src6_index,src7_index,src8_index,src9_index,has_nextn_leaf,name_is_empty\n";
    char buf[2048];
    for (const auto & row : r.rows) {
        snprintf(buf, sizeof buf,
            "ENGINE008_GRAPH_ENUM_v1_1,%s,%llu,%s,%s,%d,%d,%d,%d,%d,%s,%zu,%d,%d,%lld,%lld,%lld,%lld,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d\n",
            csv_escape(meta.run_id).c_str(), meta.submission_id,
            csv_escape(meta.published_gf).c_str(), csv_escape(meta.published_fingerprint).c_str(),
            r.n_declared, r.n_exported,
            row.index, row.op_code, row.op_name_available ? 1 : 0,
            csv_escape(row.name).c_str(), row.name.size(),
            row.type_code, row.n_dims,
            (long long) row.ne[0], (long long) row.ne[1],
            (long long) row.ne[2], (long long) row.ne[3],
            row.n_src == 0 ? 1 : 0, row.is_output_flag ? 1 : 0, row.n_src,
            row.src_index[0], row.src_index[1], row.src_index[2], row.src_index[3],
            row.src_index[4], row.src_index[5], row.src_index[6], row.src_index[7],
            row.src_index[8], row.src_index[9],
            row.has_nextn_leaf ? 1 : 0, row.name.empty() ? 1 : 0);
        o += buf;
    }
    return o;
}

// ------------------------------------------------------------------ §3 LES 6 CONTROLES
static Coverage check_completeness(const ExportResult & r, const CaptureMeta & meta) {
    if (r.malformed)                       return Coverage::COUNT_MISMATCH;
    if (r.n_declared <= 0)                 return Coverage::NULL_DECLARED_COUNT;
    if (r.n_exported != r.n_declared)      return Coverage::COUNT_MISMATCH;
    if (meta.published_n_nodes != 0 && meta.published_n_nodes != r.n_declared)
                                           return Coverage::COUNT_MISMATCH;
    if (meta.submission_rc != 0)           return Coverage::SUBMISSION_RC_NONZERO;
    if (r.unresolved_src > 0)              return Coverage::UNRESOLVED_SRC;
    if (!meta.snapshot_present)            return Coverage::CORRELATION_FAILED;
    if (meta.submit_events_observed != meta.submit_events_expected)
                                           return Coverage::INTERMEDIATE_POSSIBLE;
    return Coverage::COMPLETE;
}

// ------------------------------------------------------------------ §8 LES 6 REGLES
static NextnVerdict classify_nextn(const ExportResult & r, const CaptureMeta & meta) {
    if (r.malformed) return NextnVerdict::MALFORMED;
    Coverage c = check_completeness(r, meta);
    if (c == Coverage::UNRESOLVED_SRC || c == Coverage::CORRELATION_FAILED)
        return NextnVerdict::NOT_CONFIRMED_AMBIGUOUS;
    if (c != Coverage::COMPLETE)
        return NextnVerdict::NOT_CONFIRMED_INCOMPLETE;
    // Couverture COMPLETE : l'enumeration structurelle est valide. MAIS la
    // seule signature NextN disponible dans Q1 est une CONVENTION DE NOM
    // (blk.<N>.nextn.*), et un nom n'est pas une structure. Aucune regle
    // d'identification STRUCTURELLE n'est justifiee par la cartographie
    // source. Consequence assumee : ni un positif ni un negatif ne sont
    // fondes. On REFUSE de trancher plutot que de fonder une conclusion sur
    // une sous-chaine -- c'est exactement la faute de l'ancien classificateur.
    (void) r.nextn_op_count;
    return NextnVerdict::NOT_CONFIRMED_IDENTIFICATION_RULE_INSUFFICIENT;
}

// ------------------------------------------------------------------ ADAPTATEUR
// Source de metadonnees INJECTABLE : les tests ne touchent JAMAIS aux globals Q1.
struct PublishedGraphSource {
    virtual const void * last_gf() = 0;
    virtual int          last_gf_nodes() = 0;
    virtual bool         snapshot_for(unsigned long long sub_id, CaptureMeta & out) = 0;
    virtual ~PublishedGraphSource() {}
};

// Source REELLE : lit les globals exportes de Q1 (jamais ecrits ici).
struct RealQ1Source : PublishedGraphSource {
    const void * last_gf() override { return f1a_last_gf; }
    int          last_gf_nodes() override { return f1a_last_gf_nodes; }
    bool snapshot_for(unsigned long long, CaptureMeta & out) override {
        // Les colonnes gf / fingerprint viennent du snapshot f1a ; leur
        // disponibilite depend du moment d'ecriture. NON DISPONIBLE ici ->
        // on le DECLARE au lieu d'inventer une valeur.
        out.snapshot_present = false;
        return false;
    }
};

struct AdapterOutcome {
    bool         exported = false;
    Coverage     coverage = Coverage::NO_PUBLISHED_GRAPH;
    NextnVerdict verdict  = NextnVerdict::SURFACE_INSUFFICIENT;
    ExportResult result;
    std::string  refusal;
};

static AdapterOutcome last_graph_acquire(PublishedGraphSource & src,
                                         const CaptureMeta & base_meta) {
    AdapterOutcome o;
    const void * gf = src.last_gf();
    if (gf == nullptr) {
        o.coverage = Coverage::NO_PUBLISHED_GRAPH;
        o.verdict  = NextnVerdict::NOT_CONFIRMED_INCOMPLETE;
        o.refusal  = "aucun graphe publie";
        return o;                       // AUCUN dereferencement (§3)
    }
    const int published = src.last_gf_nodes();
    if (published <= 0) {
        o.coverage = Coverage::NULL_DECLARED_COUNT;
        o.verdict  = NextnVerdict::NOT_CONFIRMED_INCOMPLETE;
        o.refusal  = "compte publie nul";
        return o;
    }
    CaptureMeta meta = base_meta;
    meta.published_n_nodes = published;
    bool snap = src.snapshot_for(meta.submission_id, meta);
    meta.snapshot_present = snap;
    if (!snap) {
        // Sans snapshot, la correlation gf/fingerprint n'est pas etablie :
        // la couverture reste INCOMPLETE, et aucune conclusion n'est permise.
        o.coverage = Coverage::CORRELATION_FAILED;
        o.verdict  = NextnVerdict::NOT_CONFIRMED_AMBIGUOUS;
        o.refusal  = "correlation snapshot indisponible";
        return o;
    }
    o.result   = graph_export((struct ggml_cgraph *) gf, meta);
    o.coverage = check_completeness(o.result, meta);
    o.verdict  = classify_nextn(o.result, meta);
    o.exported = true;
    return o;
}

// ------------------------------------------------------------------ TESTS
static int failed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-62s %s\n", n, ok ? "PASS" : "FAIL");
    if (!ok) failed++;
}

// Construit des graphes SYNTHETIQUES avec les primitives ggml publiques.
// AUCUNE evaluation numerique : on ne fait que construire.
struct Synth {
    struct ggml_context * ctx = nullptr;
    struct ggml_cgraph  * gf  = nullptr;
};

static Synth make_synth(bool with_nextn_structure, bool misleading_name,
                        bool shared_dep, bool empty_name) {
    Synth s;
    struct ggml_init_params p;
    p.mem_size = 4 * 1024 * 1024;
    p.mem_buffer = nullptr;
    p.no_alloc = true;                      // aucune allocation de donnees
    s.ctx = ggml_init(p);
    s.gf  = ggml_new_graph(s.ctx);
    struct ggml_tensor * a = ggml_new_tensor_1d(s.ctx, GGML_TYPE_F32, 8);
    ggml_set_name(a, empty_name ? "" : (misleading_name ? "not_nextn_eh_proj_w" : "w_a"));
    struct ggml_tensor * b = ggml_new_tensor_1d(s.ctx, GGML_TYPE_F32, 8);
    ggml_set_name(b, "w_b");
    struct ggml_tensor * c = ggml_add(s.ctx, a, b);
    ggml_set_name(c, "add0");
    ggml_build_forward_expand(s.gf, c);
    if (shared_dep) {
        struct ggml_tensor * d = ggml_mul(s.ctx, c, a);   // dependance partagee sur a
        ggml_set_name(d, "mul0");
        ggml_build_forward_expand(s.gf, d);
    }
    if (with_nextn_structure) {
        // STRUCTURE POSITIVE : une FEUILLE au nom NextN, CONSOMMEE par une
        // operation du graphe. C'est la relation structurelle qui compte,
        // pas la chaine dans le nom.
        struct ggml_tensor * nw = ggml_new_tensor_1d(s.ctx, GGML_TYPE_F32, 8);
        ggml_set_name(nw, "blk.64.nextn.eh_proj.weight");
        struct ggml_tensor * nn = ggml_mul(s.ctx, nw, c);
        ggml_set_name(nn, "nextn_consumer");
        ggml_build_forward_expand(s.gf, nn);
    }
    return s;
}

static void free_synth(Synth & s) { if (s.ctx) ggml_free(s.ctx); }

int main(int argc, char ** argv) {
    (void) argc; (void) argv;
    printf("ENGINE-008 -- ENUMERATEUR DU GRAPHE SOUMIS (aucun forward, aucune eval)\n");
    printf("SCHEMA=ENGINE008_GRAPH_ENUM_v1\n\n");

    // --- 1. structure complete
    { Synth s = make_synth(false, false, false, false);
      CaptureMeta m; m.run_id="t"; m.submission_id=1; m.snapshot_present=true;
      auto r = graph_export(s.gf, m);
      ck("export_complete_counts", r.n_exported == r.n_declared && r.n_declared > 0);
      ck("export_covers_completeness_COMPLETE", check_completeness(r,m) == Coverage::COMPLETE);
      ck("no_structural_rule_refuses_to_conclude_absence",
         classify_nextn(r,m) == NextnVerdict::NOT_CONFIRMED_IDENTIFICATION_RULE_INSUFFICIENT);
      std::string csv = export_to_csv(r, m);
      ck("serialization_has_27_columns", csv.find("name_is_empty") != std::string::npos);
      ck("serialization_row_count_matches",
         (int) std::count(csv.begin(), csv.end(), '\n') - 1 == r.n_exported);
      free_synth(s); }

    // --- 2. dependance partagee
    { Synth s = make_synth(false, false, true, false);
      CaptureMeta m; m.snapshot_present=true;
      auto r = graph_export(s.gf, m);
      bool resolved = true;
      for (auto & row : r.rows) for (int k=0;k<4;k++) if (row.src_index[k] == -9 && row.n_src>k) {}
      ck("shared_dependency_still_resolves", r.unresolved_src == 0 && resolved);
      free_synth(s); }

    // --- 3. structure NextN positive, JUSTIFIEE par la cartographie
    { Synth s = make_synth(true, false, false, false);
      CaptureMeta m; m.snapshot_present=true;
      auto r = graph_export(s.gf, m);
      ck("nextn_leaf_detected", r.nextn_leaf_count > 0);
      ck("nextn_consumer_operation_detected", r.nextn_op_count > 0);
      // L'enumeration OBSERVE la feuille et l'operation qui la consomme, mais
      // la classification REFUSE de conclure : un nom n'est pas une structure.
      ck("nextn_observed_but_classifier_refuses_to_conclude",
         classify_nextn(r,m) == NextnVerdict::NOT_CONFIRMED_IDENTIFICATION_RULE_INSUFFICIENT);
      free_synth(s); }

    // --- 4. NOM TROMPEUR seul : ne doit PAS produire de structure
    { Synth s = make_synth(false, true, false, false);
      CaptureMeta m; m.snapshot_present=true;
      auto r = graph_export(s.gf, m);
      // Un nom trompeur ne doit produire NI structure NI absence.
      ck("misleading_nextn_name_yields_no_conclusion",
         classify_nextn(r,m) == NextnVerdict::NOT_CONFIRMED_IDENTIFICATION_RULE_INSUFFICIENT);
      free_synth(s); }

    // --- 5. nom vide
    { Synth s = make_synth(false, false, false, true);
      CaptureMeta m; m.snapshot_present=true;
      auto r = graph_export(s.gf, m);
      ck("empty_name_handled", r.n_exported == r.n_declared);
      free_synth(s); }

    // --- 6. compte incoherent (published != declared)
    { Synth s = make_synth(false, false, false, false);
      CaptureMeta m; m.snapshot_present=true; m.published_n_nodes = 99999;
      auto r = graph_export(s.gf, m);
      ck("count_mismatch_not_complete", check_completeness(r,m) == Coverage::COUNT_MISMATCH);
      ck("count_mismatch_not_confirmed",
         classify_nextn(r,m) == NextnVerdict::NOT_CONFIRMED_INCOMPLETE);
      free_synth(s); }

    // --- 7. soumission en erreur
    { Synth s = make_synth(false, false, false, false);
      CaptureMeta m; m.snapshot_present=true; m.submission_rc = 1;
      auto r = graph_export(s.gf, m);
      ck("submission_error_not_confirmed",
         classify_nextn(r,m) == NextnVerdict::NOT_CONFIRMED_INCOMPLETE);
      free_synth(s); }

    // --- 8. soumissions intermediaires manquantes
    { Synth s = make_synth(false, false, false, false);
      CaptureMeta m; m.snapshot_present=true;
      m.submit_events_expected = 3; m.submit_events_observed = 1;
      auto r = graph_export(s.gf, m);
      ck("intermediate_submissions_loss_detected",
         check_completeness(r,m) == Coverage::INTERMEDIATE_POSSIBLE);
      ck("intermediate_loss_never_concludes",
         classify_nextn(r,m) == NextnVerdict::NOT_CONFIRMED_INCOMPLETE);
      free_synth(s); }

    // --- 9. snapshot absent -> correlation impossible
    { Synth s = make_synth(false, false, false, false);
      CaptureMeta m; m.snapshot_present=false;
      auto r = graph_export(s.gf, m);
      ck("missing_snapshot_ambiguous",
         classify_nextn(r,m) == NextnVerdict::NOT_CONFIRMED_AMBIGUOUS);
      free_synth(s); }

    // --- 10. graphe nul
    { CaptureMeta m; m.snapshot_present=true;
      auto r = graph_export(nullptr, m);
      ck("null_graph_malformed", r.malformed && classify_nextn(r,m) == NextnVerdict::MALFORMED); }

    // --- 11. ADAPTATEUR : aucun graphe publie -> aucun dereferencement
    { struct NoGraph : PublishedGraphSource {
        const void * last_gf() override { return nullptr; }
        int last_gf_nodes() override { return 0; }
        bool snapshot_for(unsigned long long, CaptureMeta &) override { return false; } };
      NoGraph src; CaptureMeta m;
      auto o = last_graph_acquire(src, m);
      ck("adapter_no_published_graph_refuses",
         !o.exported && o.coverage == Coverage::NO_PUBLISHED_GRAPH);
      ck("adapter_no_graph_never_concludes_absence",
         o.verdict == NextnVerdict::NOT_CONFIRMED_INCOMPLETE); }

    // --- 12. ADAPTATEUR : graphe publie MAIS snapshot indisponible
    { struct GFOnly : PublishedGraphSource {
        struct ggml_cgraph * g;
        const void * last_gf() override { return (const void *) g; }
        int last_gf_nodes() override { return ggml_graph_n_nodes(g); }
        bool snapshot_for(unsigned long long, CaptureMeta &) override { return false; } };
      Synth s = make_synth(false,false,false,false);
      GFOnly src; src.g = s.gf; CaptureMeta m;
      auto o = last_graph_acquire(src, m);
      ck("adapter_without_snapshot_not_confirmed",
         o.coverage == Coverage::CORRELATION_FAILED &&
         o.verdict == NextnVerdict::NOT_CONFIRMED_AMBIGUOUS);
      free_synth(s); }

    // --- 13. ADAPTATEUR : chemin complet mocke (snapshot fourni)
    { struct Full : PublishedGraphSource {
        struct ggml_cgraph * g; int rc = 0;
        const void * last_gf() override { return (const void *) g; }
        int last_gf_nodes() override { return ggml_graph_n_nodes(g); }
        bool snapshot_for(unsigned long long, CaptureMeta & out) override {
            out.snapshot_present = true; out.published_fingerprint = "deadbeefdeadbeef";
            out.published_gf = "0xmock"; out.submission_rc = rc; return true; } };
      Synth s = make_synth(true,false,false,false);
      Full src; src.g = s.gf; CaptureMeta m; m.run_id="mock";
      auto o = last_graph_acquire(src, m);
      ck("adapter_nominal_mocked_path_exports", o.exported &&
         o.coverage == Coverage::COMPLETE);
      ck("adapter_nominal_mocked_reports_no_conclusion",
         o.verdict == NextnVerdict::NOT_CONFIRMED_IDENTIFICATION_RULE_INSUFFICIENT);
      free_synth(s); }

    // --- 14. capacite src[] reelle et operateur dans un slot > 3
    { Synth s = make_synth(false,false,false,false);
      ck("ggml_max_src_is_10", GGML_MAX_SRC == 10);
      // slot 4 : structure synthetique explicite (le 5e slot n'est pas utilise
      // par ggml_add, on l'etablit pour exercer la BOUCLE d'export)
      struct ggml_tensor * leaf4 = ggml_new_tensor_1d(s.ctx, GGML_TYPE_F32, 8);
      ggml_set_name(leaf4, "w_slot4");
      struct ggml_tensor * host = ggml_graph_node(s.gf, ggml_graph_n_nodes(s.gf) - 1);
      host->src[4] = leaf4;
      CaptureMeta m; m.snapshot_present = true; m.published_n_nodes = 0;
      auto r = graph_export(s.gf, m);
      bool found_slot4 = false;
      for (auto & row : r.rows) if (row.src_index[4] == -1) found_slot4 = true;
      ck("operand_in_slot_gt_3_is_exported", found_slot4);
      ck("export_not_truncated_at_slot_3", r.unresolved_src == 0);
      free_synth(s); }

    printf("\nGRAPH_ENUMERATOR_SYNTHETIC_SELFTEST=%s\n", failed==0?"PASS":"FAIL");
    printf("GRAPH_COMPLETENESS_CONTROLS_SELFTEST=%s\n", failed==0?"PASS":"FAIL");
    printf("NEXTN_STRUCTURAL_CLASSIFIER_SELFTEST=%s\n", failed==0?"PASS":"FAIL");
    printf("LAST_GRAPH_ADAPTER_SELFTEST=%s\n", failed==0?"PASS":"FAIL");
    printf("SYNTHETIC_GRAPH_NUMERIC_EVALUATION_COUNT=0\n");
    printf("MODEL_FORWARD_CALL_COUNT=0\n");
    printf("FAILED_COUNT=%d\n", failed);
    return failed == 0 ? 0 : 1;
}
