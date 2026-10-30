#include "engine008_wht_parser.h"
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <algorithm>
#include <set>
#include <sstream>

static std::vector<std::string> sp(const std::string & l) {
    std::vector<std::string> o; std::string c;
    for (char ch : l) { if (ch == ',') { o.push_back(c); c.clear(); } else if (ch != '\r') c += ch; }
    o.push_back(c); return o;
}
static bool i64(const std::string & s, long long & out) {
    if (s.empty()) return false;
    for (char c : s) if (!((c >= '0' && c <= '9') || (c == '-' && s.size() > 1))) return false;
    errno = 0; out = strtoll(s.c_str(), nullptr, 10); return errno == 0;
}
static bool u64(const std::string & s, unsigned long long & out) {
    if (s.empty()) return false;
    for (char c : s) if (c < '0' || c > '9') return false;
    errno = 0; out = strtoull(s.c_str(), nullptr, 10); return errno == 0;
}
static bool read_all(const std::string & p, std::string & out) {
    std::ifstream f(p, std::ios::binary); if (!f) return false;
    std::ostringstream ss; ss << f.rdbuf(); out = ss.str(); return true;
}
static int col(const std::vector<std::string> & h, const char * n) {
    for (size_t i = 0; i < h.size(); ++i) if (h[i] == n) return (int) i;
    return -1;
}

// ---------------------------------------------------------------- MANIFESTE
WhtManifestFile parse_wht_manifest(const std::string & b, bool ex) {
    WhtManifestFile f;
    if (!ex) { f.state = FileState::ABSENT; f.why = "manifeste absent"; return f; }
    if (b.empty()) { f.state = FileState::EMPTY; f.why = "manifeste vide"; return f; }
    std::istringstream is(b); std::string line; bool hdr = false;
    std::vector<std::string> H; int c_run, c_ep, c_tid, c_cid, c_wn, c_fam, c_K, c_rot, c_kind, c_path, c_ds, c_sub;
    // target_id est PER-EPOCH : l identite est (run_id, epoch, target_id, capture_id).
    std::set<int> tid;
    std::set<std::string> ident;
    std::set<std::pair<int,int>> tgt;
    while (std::getline(is, line)) {
        if (line.empty()) continue;
        auto v = sp(line);
        if (!hdr) {
            hdr = true; H = v;
            c_run = col(H, "run_id");   c_ep = col(H, "epoch");   c_tid = col(H, "target_id");
            c_cid = col(H, "capture_id"); c_wn = col(H, "weight_name"); c_fam = col(H, "family");
            c_K   = col(H, "expected_K"); c_rot = col(H, "rotation_applied");
            c_kind= col(H, "kind");     c_path = col(H, "path");
            c_ds  = col(H, "decode_step"); c_sub = col(H, "compute_submission_id");
            if (c_tid < 0 || c_rot < 0 || c_sub < 0 || c_ds < 0) {
                f.state = FileState::MALFORMED;
                f.why = "manifeste : colonnes requises absentes (target_id, rotation_applied, compute_submission_id, decode_step)";
                return f;
            }
            continue;
        }
        if (v.size() != H.size()) { f.state = FileState::PARTIAL; f.why = "ligne de largeur differente"; return f; }
        WhtTargetRow r;
        if (c_run >= 0) r.run_id = v[c_run];
        long long x;
        if (!i64(v[c_tid], x)) { f.state = FileState::MALFORMED; f.why = "target_id non numerique"; return f; }
        r.target_id = (int) x;
        if (c_ep >= 0 && !v[c_ep].empty()) { if (!i64(v[c_ep], x)) { f.state = FileState::MALFORMED; f.why = "epoch non numerique"; return f; } r.epoch = (int) x; }
        if (c_cid >= 0 && !v[c_cid].empty()) { if (!i64(v[c_cid], x)) { f.state = FileState::MALFORMED; f.why = "capture_id non numerique"; return f; } r.capture_id = (int) x; }
        if (c_wn >= 0) r.weight_name = v[c_wn];
        if (c_fam >= 0 && !v[c_fam].empty()) { i64(v[c_fam], x); r.family = (int) x; }
        if (c_K >= 0 && !v[c_K].empty()) { i64(v[c_K], x); r.expected_K = (int) x; }
        // rotation_applied : CHAMP CLE. Une valeur non exploitable est COMPTEE, pas supposee.
        if (v[c_rot] == "1") r.rotation_applied = 1;
        else if (v[c_rot] == "0") r.rotation_applied = 0;
        else { r.rotation_applied = -1; f.missing_rotation_field++; }
        if (c_kind >= 0) r.kind = v[c_kind];
        if (c_path >= 0) r.path = v[c_path];
        if (!i64(v[c_ds], x)) { f.state = FileState::MALFORMED; f.why = "decode_step non numerique"; return f; }
        r.decode_step = (int) x;
        if (!u64(v[c_sub], r.compute_submission_id)) { f.state = FileState::MALFORMED; f.why = "compute_submission_id non numerique"; return f; }
        if (r.rotation_applied == 1) f.rotation_true++;
        else if (r.rotation_applied == 0) f.rotation_false++;
        // Identite REELLE : `kind` est indispensable -- un target porte une
        // ligne `activation` ET une ligne `base_output`.
        std::string key = r.run_id + "|" + std::to_string(r.epoch) + "|" +
                          std::to_string(r.target_id) + "|" + std::to_string(r.capture_id) +
                          "|" + r.kind;
        if (!ident.insert(key).second) f.duplicate_target_id++;
        tid.insert(r.target_id);
        tgt.insert({r.epoch, r.target_id});
        if (std::find(f.kinds.begin(), f.kinds.end(), r.kind) == f.kinds.end())
            f.kinds.push_back(r.kind);
        f.rows.push_back(r);
    }
    if (!hdr) { f.state = FileState::EMPTY; f.why = "aucune ligne"; return f; }
    if (f.rows.empty()) { f.state = FileState::EMPTY; f.why = "en-tete seul"; return f; }
    f.n_distinct_targets = (int) tgt.size();          // (epoch, target_id)
    f.n_distinct_identities = (int) ident.size();     // + kind
    { std::set<int> rt;
      for (const auto & x : f.rows)
          if (x.rotation_applied == 1) rt.insert(x.target_id);
      f.rotation_true_targets = (int) rt.size(); }
    f.state = FileState::VALID;
    return f;
}

RoleRelFile parse_role_relations(const std::string & b, bool ex) {
    RoleRelFile f;
    if (!ex) { f.state = FileState::ABSENT; f.why = "absent"; return f; }
    if (b.empty()) { f.state = FileState::EMPTY; f.why = "vide"; return f; }
    std::istringstream is(b); std::string line; bool hdr = false;
    std::vector<std::string> H; std::set<int> seen;
    while (std::getline(is, line)) {
        if (line.empty()) continue;
        auto v = sp(line);
        if (!hdr) { hdr = true; H = v;
            if (col(H, "target_id") < 0 || col(H, "role_ok") < 0 || col(H, "sub_at_build") < 0) {
                f.state = FileState::MALFORMED; f.why = "colonnes requises absentes"; return f; }
            continue; }
        if (v.size() != H.size()) { f.state = FileState::PARTIAL; f.why = "ligne partielle"; return f; }
        RoleRelRow r;
        auto gi = [&](const char * n) -> int { int c = col(H, n); if (c < 0 || v[c].empty()) return -1; long long x; return i64(v[c], x) ? (int) x : -1; };
        auto gs = [&](const char * n) -> std::string { int c = col(H, n); return (c < 0) ? std::string() : v[c]; };
        r.epoch = gi("epoch"); r.target_id = gi("target_id"); r.weight_name = gs("weight_name");
        r.sub_at_build = gi("sub_at_build"); r.cur_op = gi("cur_op");
        r.cur_src0_is_effective = gi("cur_src0_is_effective");
        r.mulmat_op = gi("mulmat_op"); r.mulmat_src0_is_weight = gi("mulmat_src0_is_weight");
        r.mulmat_src1_is_cur = gi("mulmat_src1_is_cur");
        r.mmcap_op = gi("mmcap_op"); r.mmcap_src0_is_mmraw = gi("mmcap_src0_is_mmraw");
        r.downstream_reach = gi("downstream_reach"); r.role_ok = gi("role_ok");
        r.effect_class = gs("effect_class");
        if (r.target_id < 0) { f.state = FileState::MALFORMED; f.why = "target_id illisible"; return f; }
        if (!seen.insert(r.target_id).second) f.duplicate_target_id++;
        f.rows.push_back(r);
    }
    if (!hdr) { f.state = FileState::EMPTY; f.why = "aucune ligne"; return f; }
    if (f.rows.empty()) { f.state = FileState::EMPTY; f.why = "en-tete seul"; return f; }
    f.state = FileState::VALID; return f;
}

EffectPathFile parse_effect_path(const std::string & b, bool ex) {
    EffectPathFile f;
    if (!ex) { f.state = FileState::ABSENT; f.why = "absent"; return f; }
    if (b.empty()) { f.state = FileState::EMPTY; f.why = "vide"; return f; }
    std::istringstream is(b); std::string line; bool hdr = false; std::vector<std::string> H;
    while (std::getline(is, line)) {
        if (line.empty()) continue;
        auto v = sp(line);
        if (!hdr) { hdr = true; H = v;
            if (col(H, "target_id") < 0 || col(H, "effect_ok") < 0) { f.state = FileState::MALFORMED; f.why = "colonnes requises absentes"; return f; }
            continue; }
        if (v.size() != H.size()) { f.state = FileState::PARTIAL; f.why = "ligne partielle"; return f; }
        EffectPathRow r;
        auto gi = [&](const char * n) -> int { int c = col(H, n); if (c < 0 || v[c].empty()) return -1; long long x; return i64(v[c], x) ? (int) x : -1; };
        auto gs = [&](const char * n) -> std::string { int c = col(H, n); return (c < 0) ? std::string() : v[c]; };
        r.epoch = gi("epoch"); r.target_id = gi("target_id"); r.weight_name = gs("weight_name");
        r.role = gs("role"); r.effect_class = gs("effect_class");
        r.src0_is_weight = gi("src0_is_weight"); r.src1_is_cur = gi("src1_is_cur");
        r.dst_is_cur = gi("dst_is_cur"); r.effect_ok = gi("effect_ok");
        if (r.target_id < 0) { f.state = FileState::MALFORMED; f.why = "target_id illisible"; return f; }
        f.rows.push_back(r);
    }
    if (!hdr) { f.state = FileState::EMPTY; f.why = "aucune ligne"; return f; }
    if (f.rows.empty()) { f.state = FileState::EMPTY; f.why = "en-tete seul"; return f; }
    f.state = FileState::VALID; return f;
}

ResidencyFile parse_residency(const std::string & b, bool ex) {
    ResidencyFile f;
    if (!ex) { f.state = FileState::ABSENT; f.why = "absent"; return f; }
    if (b.empty()) { f.state = FileState::EMPTY; f.why = "vide"; return f; }
    std::istringstream is(b); std::string line; bool hdr = false; std::vector<std::string> H;
    while (std::getline(is, line)) {
        if (line.empty()) continue;
        auto v = sp(line);
        if (!hdr) { hdr = true; H = v;
            if (col(H, "target_id") < 0 || col(H, "in_submitted_graph") < 0) { f.state = FileState::MALFORMED; f.why = "colonnes requises absentes"; return f; }
            continue; }
        if (v.size() != H.size()) { f.state = FileState::PARTIAL; f.why = "ligne partielle"; return f; }
        ResidencyRow r;
        auto gi = [&](const char * n) -> int { int c = col(H, n); if (c < 0 || v[c].empty()) return -1; long long x; return i64(v[c], x) ? (int) x : -1; };
        auto gl = [&](const char * n) -> long long { int c = col(H, n); if (c < 0 || v[c].empty()) return -1; long long x; return i64(v[c], x) ? x : -1; };
        auto gs = [&](const char * n) -> std::string { int c = col(H, n); return (c < 0) ? std::string() : v[c]; };
        r.target_id = gi("target_id"); r.capture_id = gi("capture_id");
        r.role = gs("role"); r.tensor_ptr = gs("tensor_ptr"); r.tensor_name = gs("tensor_name");
        r.op = gi("op"); r.type = gi("type");
        r.ne0 = gl("ne0"); r.nb0 = gl("nb0"); r.nbytes = gl("nbytes");
        r.contiguous = gi("contiguous"); r.buffer_ptr = gs("buffer_ptr");
        r.resident = gi("resident"); r.in_submitted_graph = gi("in_submitted_graph");
        r.weight_name = gs("weight_name");
        if (r.target_id < 0) { f.state = FileState::MALFORMED; f.why = "target_id illisible"; return f; }
        f.rows.push_back(r);
    }
    if (!hdr) { f.state = FileState::EMPTY; f.why = "aucune ligne"; return f; }
    if (f.rows.empty()) { f.state = FileState::EMPTY; f.why = "en-tete seul"; return f; }
    f.state = FileState::VALID; return f;
}

WhtManifestFile read_wht_manifest(const std::string & d) { std::string b; bool e = read_all(d + "/GRAPH_CAPTURE_MANIFEST.csv", b); return parse_wht_manifest(b, e); }
RoleRelFile     read_role_relations(const std::string & d) { std::string b; bool e = read_all(d + "/GRAPH_ROLE_RELATIONSHIPS.csv", b); return parse_role_relations(b, e); }
EffectPathFile  read_effect_path(const std::string & d) { std::string b; bool e = read_all(d + "/TARGET_EFFECT_PATH.csv", b); return parse_effect_path(b, e); }
ResidencyFile   read_residency(const std::string & d) { std::string b; bool e = read_all(d + "/RESIDENCY_MANIFEST.csv", b); return parse_residency(b, e); }

// ---------------------------------------------------------------- N1
ExpectedTargets compute_n1(const std::vector<std::string> & inv,
                           const std::vector<std::pair<std::string,int>> & model,
                           const std::string & inv_path, int tq2) {
    ExpectedTargets e;
    e.n_inventory_names = (int) inv.size();
    for (const auto & t : model) if (t.second == tq2) e.n_model_tq2g128++;
    if (inv.empty()) { e.state = N1State::UNKNOWN; e.why = "inventaire vide ou absent"; }
    else if (model.empty()) { e.state = N1State::UNKNOWN; e.why = "registre natif indisponible"; }
    else {
        e.state = N1State::KNOWN;
        e.method = "intersection(inventaire CSV valide, tenseurs du modele dont le TYPE REEL est TQ2_G128)";
        e.scope = "poids portant un nom de l inventaire ET le type TQ2_G128 ; inventaire : " + inv_path;
        for (const auto & t : model)
            if (t.second == tq2) {
                for (const auto & n : inv) if (n == t.first) { e.identities.insert(n); break; }
            }
    }
    return e;
}

// ---------------------------------------------------------------- RECONCILIATION
CoverageReconciliation reconcile_coverage(const ExpectedTargets & exp,
                                          const WhtManifestFile & m,
                                          const RoleRelFile & r,
                                          const EffectPathFile & e) {
    CoverageReconciliation c;
    c.n_lines = (int) m.rows.size();
    // N2 : couples (epoch,target) distincts -- JAMAIS la somme des kinds
    std::set<std::pair<int,int>> tgt;
    std::set<std::string> names;
    for (const auto & x : m.rows) { tgt.insert({x.epoch, x.target_id}); names.insert(x.weight_name); }
    c.n2 = (int) tgt.size();
    // N3 : cibles ayant role_ok ET effect_ok
    std::set<std::pair<int,int>> tr, te;
    for (const auto & x : r.rows) if (x.target_id >= 0 && x.sub_at_build == 1 && x.role_ok == 1) tr.insert({x.epoch, x.target_id});
    for (const auto & x : e.rows) if (x.target_id >= 0 && x.effect_ok == 1) te.insert({x.epoch, x.target_id});
    c.n3 = (int) std::min(tr.size(), te.size());

    if (exp.state != N1State::KNOWN) {
        c.n1 = -1;
        c.verdict = "N1_UNKNOWN_BLOCKS_COMPLETENESS";
        c.complete = false;      // N2 > 0 ou N3 == N2 ne suffisent JAMAIS
        return c;
    }
    c.n1 = (int) exp.identities.size();
    for (const auto & n : names) if (!exp.identities.count(n)) c.unexpected_identities.push_back(n);
    for (const auto & n : exp.identities) if (!names.count(n)) c.missing_identities.push_back(n);
    c.n_unexpected = (int) c.unexpected_identities.size();
    c.n_missing = (int) c.missing_identities.size();
    c.complete = (c.n_missing == 0 && c.n3 >= c.n2 && c.n2 > 0);
    c.verdict = c.complete ? "COMPLETE" :
        (c.n_missing > 0 ? "MISSING_EXPECTED_TARGETS" :
         (c.n3 < c.n2 ? "MISSING_REQUIRED_RELATIONS" : "EMPTY_CAPTURE"));
    return c;
}

const char * wht_verdict_name(WhtVerdict v) {
    switch (v) {
        case WhtVerdict::WHT_CONSTRUCTED:            return "WHT_CONSTRUCTED";
        case WhtVerdict::WHT_CONSUMED_IN_SUBMITTED:  return "WHT_CONSUMED_IN_SUBMITTED";
        case WhtVerdict::SUBMISSION_OK:              return "SUBMISSION_OK";
        case WhtVerdict::EXECUTION_NOT_ESTABLISHED:  return "EXECUTION_NOT_ESTABLISHED";
        case WhtVerdict::INCOMPLETE_COVERAGE:        return "INCOMPLETE_COVERAGE";
        case WhtVerdict::AMBIGUOUS_ASSOCIATION:      return "AMBIGUOUS_ASSOCIATION";
        default:                                     return "SURFACE_MISSING_OR_MALFORMED";
    }
}

// ---------------------------------------------------------------- EVALUATION
// La portee est construite maillon par maillon. Chaque maillon non etabli est
// NOMME, et l'execution n'est JAMAIS deduite d'une reussite de soumission.
WhtAssessment assess_wht(const WhtManifestFile & m, const RoleRelFile & r,
                         const EffectPathFile & e, const ResidencyFile & s,
                         const ExpectedTargets & exp) {
    WhtAssessment a;
    a.recon = reconcile_coverage(exp, m, r, e);
    a.coverage.manifest_present = (m.state == FileState::VALID);
    a.coverage.roles_present    = (r.state == FileState::VALID);
    a.coverage.effect_present   = (e.state == FileState::VALID);
    a.coverage.residency_present= (s.state == FileState::VALID);

    // 1. SURFACE PORTANT LA ROTATION : sans elle, rien n'est evaluable.
    if (m.state != FileState::VALID) {
        a.verdict = WhtVerdict::SURFACE_MISSING_OR_MALFORMED;
        a.blockers.push_back("GRAPH_CAPTURE_MANIFEST.csv : " + m.why);
        a.scope = "AUCUNE : la surface portant rotation_applied est absente ou illisible";
        return a;
    }
    a.coverage.n_targets_expected = m.n_distinct_targets;
    for (const auto & x : m.rows) if (x.rotation_applied >= 0) a.coverage.n_targets_with_rotation++;
    a.coverage.n_rotation_true  = (int) m.rotation_true;
    a.coverage.n_rotation_false = (int) m.rotation_false;
    if (m.missing_rotation_field > 0)
        a.blockers.push_back("champ rotation_applied non exploitable sur " +
                             std::to_string(m.missing_rotation_field) + " ligne(s)");
    if (m.duplicate_target_id > 0) {
        a.verdict = WhtVerdict::AMBIGUOUS_ASSOCIATION;
        a.blockers.push_back("couples (target_id, capture_id) dupliques : " +
                             std::to_string(m.duplicate_target_id));
        a.scope = "AUCUNE : association cible/capture non unique";
        return a;
    }
    if (a.coverage.n_rotation_true == 0) {
        // Aucune rotation observee : ce n'est PAS une preuve d'absence.
        a.verdict = WhtVerdict::INCOMPLETE_COVERAGE;
        a.blockers.push_back("rotation_applied n est vrai sur aucune cible");
        a.scope = "aucune rotation OBSERVEE -- jamais une preuve d absence";
        return a;
    }
    // 2. CONSTRUCTION : rotation_applied etablie sur au moins une cible.
    a.verdict = WhtVerdict::WHT_CONSTRUCTED;
    a.scope = "WHT CONSTRUITE : ggml_wht insere, activation consommee transformee";

    // 3. CONSOMMATION DANS LE GRAPHE SOUMIS.
    // Denominatif : les cibles du MANIFESTE. On ne compte jamais la couverture
    // relativement aux seules lignes deja jointes -- ce serait circulaire.
    std::set<std::pair<int,int>> tgt_all;
    for (const auto & x : m.rows) tgt_all.insert({x.epoch, x.target_id});
    std::set<std::pair<int,int>> tgt_roles, tgt_effect, tgt_res;
    if (r.state == FileState::VALID) {
        for (const auto & x : r.rows) {
            if (x.sub_at_build == 1 && x.role_ok == 1) a.coverage.n_role_ok++;
            if (x.target_id >= 0) tgt_roles.insert({x.epoch, x.target_id});
        }
    } else a.blockers.push_back("GRAPH_ROLE_RELATIONSHIPS.csv : " + r.why);
    if (e.state == FileState::VALID) {
        for (const auto & x : e.rows) {
            if (x.effect_ok == 1) a.coverage.n_effect_ok++;
            if (x.target_id >= 0) tgt_effect.insert({x.epoch, x.target_id});
        }
    } else a.blockers.push_back("TARGET_EFFECT_PATH.csv : " + e.why);
    if (s.state == FileState::VALID) {
        for (const auto & x : s.rows) {
            if (x.in_submitted_graph == 1) a.coverage.n_in_submitted_graph++;
            if (x.target_id >= 0) tgt_res.insert({x.target_id, 0});
        }
    } else a.blockers.push_back("RESIDENCY_MANIFEST.csv : " + s.why);
    a.coverage.n_targets_with_roles    = (int) tgt_roles.size();
    a.coverage.n_targets_with_effect   = (int) tgt_effect.size();
    a.coverage.n_targets_with_residency= (int) tgt_res.size();

    // N1 INDEPENDANT : s'il est inconnu, la completude est INTERDITE, quels que
    // soient N2 et N3. N2>0 ne suffit jamais ; N3==N2 non plus.
    if (a.recon.n1 < 0) {
        a.verdict = WhtVerdict::INCOMPLETE_COVERAGE;
        a.blockers.push_back("N1 INCONNU (" + exp.why + ") : la completude de capture est INTERDITE");
        a.scope += " ; N1 INCONNU : completude non etablie";
        return a;
    }
    if (a.recon.n_missing > 0) {
        a.verdict = WhtVerdict::INCOMPLETE_COVERAGE;
        a.blockers.push_back("cibles ATTENDUES non capturees : " + std::to_string(a.recon.n_missing) +
                             " sur " + std::to_string(a.recon.n1));
        a.scope += " ; CIBLES ATTENDUES MANQUANTES";
        return a;
    }
    if (a.recon.n_unexpected > 0) {
        a.verdict = WhtVerdict::AMBIGUOUS_ASSOCIATION;
        a.blockers.push_back("captures HORS N1 (inattendues) : " + std::to_string(a.recon.n_unexpected));
        a.scope += " ; CAPTURES INATTENDUES";
        return a;
    }
    // COUVERTURE PARTIELLE : elle interdit le verdict de chaine complete.
    if (a.coverage.n_targets_with_roles < a.coverage.n_targets_expected ||
        a.coverage.n_targets_with_effect < a.coverage.n_targets_expected) {
        a.verdict = WhtVerdict::INCOMPLETE_COVERAGE;
        a.blockers.push_back("couverture roles/effet PARTIELLE : " +
            std::to_string(std::min(a.coverage.n_targets_with_roles, a.coverage.n_targets_with_effect)) +
            " cibles couvertes sur " + std::to_string(a.coverage.n_targets_expected) +
            " attendues (denominateur = manifeste, jamais les lignes deja jointes)");
        a.scope += " ; COUVERTURE PARTIELLE : chaine complete NON etablie";
        return a;
    }
    if (a.coverage.n_in_submitted_graph > 0 && a.coverage.n_role_ok > 0 && a.coverage.n_effect_ok > 0) {
        a.verdict = WhtVerdict::WHT_CONSUMED_IN_SUBMITTED;
        a.scope += " ; RESULTAT WHT CONSOMME PAR UNE OPERATION DU GRAPHE SOUMIS"
                   " (in_submitted_graph, role_ok, effect_ok)";
    } else {
        a.verdict = WhtVerdict::INCOMPLETE_COVERAGE;
        a.scope += " ; appartenance au graphe SOUMIS NON etablie";
        a.blockers.push_back("maillon construction -> soumission incomplet");
        return a;
    }

    // 4. SOUMISSION : etablie par le ledger (surface distincte).
    a.verdict = WhtVerdict::SUBMISSION_OK;
    a.scope += " ; soumission a reussir (a confronter au ledger)";

    // 5. EXECUTION : AUCUNE surface auditee ne l'observe.
    //    Une reussite de soumission n'est PAS une trace d'execution par operation.
    a.verdict = WhtVerdict::EXECUTION_NOT_ESTABLISHED;
    a.scope += " ; EXECUTION NON ETABLIE : aucune surface auditee n observe "
               "l execution, et une reussite de soumission n est PAS une trace "
               "par operation";
    a.limits_placeholder();
    return a;
}
