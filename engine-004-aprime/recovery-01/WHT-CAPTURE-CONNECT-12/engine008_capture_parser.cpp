#include "engine008_capture_parser.h"
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <set>
#include <sstream>

const char * file_state_name(FileState s) {
    switch (s) {
        case FileState::ABSENT:    return "ABSENT";
        case FileState::EMPTY:     return "EMPTY";
        case FileState::PARTIAL:   return "PARTIAL";
        case FileState::MALFORMED: return "MALFORMED";
        default:                   return "VALID";
    }
}

static std::vector<std::string> split(const std::string & l, char sep) {
    std::vector<std::string> o; std::string c;
    for (char ch : l) { if (ch == sep) { o.push_back(c); c.clear(); } else if (ch != '\r') c += ch; }
    o.push_back(c); return o;
}
// Entier STRICT : refuse toute conversion silencieuse.
static bool parse_i64(const std::string & s, long long & out) {
    if (s.empty()) return false;
    size_t i = 0; bool neg = false;
    if (s[0] == '-') { neg = true; i = 1; if (s.size() == 1) return false; }
    for (; i < s.size(); ++i) if (s[i] < '0' || s[i] > '9') return false;
    errno = 0;
    long long v = strtoll(s.c_str(), nullptr, 10);
    if (errno != 0) return false;
    out = neg ? v : v;
    return true;
}
static bool parse_u64(const std::string & s, unsigned long long & out) {
    if (s.empty()) return false;
    for (char c : s) if (c < '0' || c > '9') return false;
    errno = 0; out = strtoull(s.c_str(), nullptr, 10);
    return errno == 0;
}
// Pointeur : accepte "0x..." ou "NA" ; JAMAIS converti en entier exploitable.
static bool looks_pointer_or_na(const std::string & s) {
    if (s == "NA") return true;
    if (s.size() < 3 || s[0] != '0' || (s[1] != 'x' && s[1] != 'X')) return false;
    for (size_t i = 2; i < s.size(); ++i) {
        char c = s[i];
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'))) return false;
    }
    return true;
}
// Empreinte de contenu : sert a detecter troncature ou remplacement.
static std::string digest(const std::string & b) {
    unsigned long long h = 1469598103934665603ULL;
    for (unsigned char c : b) { h ^= c; h *= 1099511628211ULL; }
    char buf[32]; snprintf(buf, sizeof buf, "%016llx", h); return std::string(buf);
}

LedgerFile parse_ledger(const std::string & bytes, bool file_existed) {
    LedgerFile f;
    if (!file_existed) { f.state = FileState::ABSENT; f.why = "fichier absent"; return f; }
    if (bytes.empty()) { f.state = FileState::EMPTY; f.why = "fichier vide"; return f; }
    std::istringstream is(bytes); std::string line; bool hdr = false;
    std::vector<std::string> H; int i_kind = -1, i_seq = -1, i_sub = -1, i_gf = -1,
        i_fp = -1, i_ep = -1, i_el = -1, i_ca = -1, i_ds = -1, i_run = -1;
    std::set<unsigned long long> seqs;
    while (std::getline(is, line)) {
        if (line.empty()) continue;
        auto v = split(line, ',');
        if (!hdr) {
            hdr = true; H = v;
            for (size_t i = 0; i < v.size(); ++i) {
                if (v[i] == "event_kind") i_kind = (int) i;
                else if (v[i] == "event_sequence_id") i_seq = (int) i;
                else if (v[i] == "compute_submission_id") i_sub = (int) i;
                else if (v[i] == "graph_ptr") i_gf = (int) i;
                else if (v[i] == "graph_fingerprint") i_fp = (int) i;
                else if (v[i] == "capture_epoch_id") i_ep = (int) i;
                else if (v[i] == "eligible_target_count") i_el = (int) i;
                else if (v[i] == "captured_target_count") i_ca = (int) i;
                else if (v[i] == "decode_step") i_ds = (int) i;
                else if (v[i] == "run_id") i_run = (int) i;
            }
            // colonnes REQUISES : sans elles, aucune valeur n'est supposee
            if (i_kind < 0 || i_seq < 0 || i_fp < 0) {
                f.state = FileState::MALFORMED;
                f.why = "en-tete du ledger : colonnes requises absentes";
                return f;
            }
            continue;
        }
        if (v.size() != H.size()) {          // ligne partielle : regle EXPLICITE
            f.state = FileState::PARTIAL;    // on conserve ce qui a ete lu et on le dit
            f.why = "ligne de largeur differente de l'en-tete";
            return f;
        }
        LedgerEvent e;
        if (i_run >= 0) e.run_id = v[i_run];
        if (!parse_u64(v[i_seq], e.seq)) { f.state = FileState::MALFORMED; f.why = "sequence non numerique"; return f; }
        e.kind = v[i_kind];
        if (i_ds >= 0) { long long d; if (!parse_i64(v[i_ds], d)) { f.state = FileState::MALFORMED; f.why = "decode_step non numerique"; return f; } e.decode_step = (int) d; }
        if (i_sub >= 0) { unsigned long long u; if (!parse_u64(v[i_sub], u)) { f.state = FileState::MALFORMED; f.why = "submission non numerique"; return f; } e.submission_id = u; }
        if (i_gf >= 0) { if (!looks_pointer_or_na(v[i_gf])) { f.state = FileState::MALFORMED; f.why = "graph_ptr illisible"; return f; } e.graph_ptr = v[i_gf]; }
        e.fingerprint = v[i_fp];
        if (i_ep >= 0) e.capture_epoch = v[i_ep];      // "NA" est une valeur legitime
        if (i_el >= 0) { long long d; if (!parse_i64(v[i_el], d)) { f.state = FileState::MALFORMED; f.why = "eligible non numerique"; return f; } e.eligible = d; }
        if (i_ca >= 0) { long long d; if (!parse_i64(v[i_ca], d)) { f.state = FileState::MALFORMED; f.why = "captured non numerique"; return f; } e.captured = d; }
        if (!seqs.insert(e.seq).second) f.duplicate_seq++;
        f.rows.push_back(e);
        if (std::find(f.kinds.begin(), f.kinds.end(), e.kind) == f.kinds.end()) f.kinds.push_back(e.kind);
    }
    if (!hdr) { f.state = FileState::EMPTY; f.why = "aucune ligne"; return f; }
    if (f.rows.empty()) { f.state = FileState::EMPTY; f.why = "en-tete seul"; return f; }
    f.state = FileState::VALID;
    return f;
}

SnapshotFile parse_snapshots(const std::string & bytes, bool file_existed) {
    SnapshotFile f;
    if (!file_existed) { f.state = FileState::ABSENT; f.why = "fichier absent"; return f; }
    if (bytes.empty()) { f.state = FileState::EMPTY; f.why = "fichier vide"; return f; }
    std::istringstream is(bytes); std::string line; bool hdr = false; size_t ncol = 0;
    std::set<unsigned long long> ids;
    while (std::getline(is, line)) {
        if (line.empty()) continue;
        auto v = split(line, ',');
        if (!hdr) {
            hdr = true; ncol = v.size();
            if (ncol != 5 || v[0] != "submission_id" || v[1] != "gf" || v[2] != "n_nodes" ||
                v[3] != "submission_rc" || v[4] != "fingerprint") {
                f.state = FileState::MALFORMED; f.why = "en-tete des snapshots inattendu"; return f;
            }
            continue;
        }
        if (v.size() != ncol) { f.state = FileState::PARTIAL; f.why = "ligne partielle"; return f; }
        SnapshotRow r;
        if (!parse_u64(v[0], r.submission_id)) { f.state = FileState::MALFORMED; f.why = "submission_id non numerique"; return f; }
        if (!looks_pointer_or_na(v[1])) { f.state = FileState::MALFORMED; f.why = "gf illisible"; return f; }
        r.gf = v[1];
        long long n, rc;
        if (!parse_i64(v[2], n)) { f.state = FileState::MALFORMED; f.why = "n_nodes non numerique"; return f; }
        if (!parse_i64(v[3], rc)) { f.state = FileState::MALFORMED; f.why = "submission_rc non numerique"; return f; }
        r.n_nodes = (int) n; r.submission_rc = (int) rc; r.fingerprint = v[4];
        if (!ids.insert(r.submission_id).second) f.duplicate_submission_id++;
        f.rows.push_back(r);
    }
    if (!hdr) { f.state = FileState::EMPTY; f.why = "aucune ligne"; return f; }
    if (f.rows.empty()) { f.state = FileState::EMPTY; f.why = "en-tete seul"; return f; }
    f.state = FileState::VALID;
    return f;
}

static bool read_all(const std::string & path, std::string & out) {
    std::ifstream f(path, std::ios::binary);
    if (!f) return false;
    std::ostringstream ss; ss << f.rdbuf(); out = ss.str(); return true;
}
LedgerFile read_ledger_file(const std::string & p) {
    std::string b; bool ex = read_all(p, b); return parse_ledger(b, ex);
}
SnapshotFile read_snapshots_file(const std::string & p) {
    std::string b; bool ex = read_all(p, b); return parse_snapshots(b, ex);
}

// ---------------------------------------------------------------- CURSEUR
WindowCursor arm_window(const std::string & lp, const std::string & sp) {
    WindowCursor c;
    std::string b;
    bool ex = read_all(lp, b);
    LedgerFile l = parse_ledger(b, ex);
    c.ledger_state_before = l.state; c.ledger_rows_before = l.rows.size();
    c.ledger_digest_before = ex ? digest(b) : "";
    c.ledger_prefix = ex ? b : "";
    b.clear(); ex = read_all(sp, b);
    SnapshotFile s = parse_snapshots(b, ex);
    c.snap_state_before = s.state; c.snap_rows_before = s.rows.size();
    c.snap_digest_before = ex ? digest(b) : "";
    c.armed = true;
    return c;
}

WindowObservation observe_window(const WindowCursor & cur,
                                 const std::string & lp, const std::string & sp) {
    WindowObservation o;
    std::string b; bool ex = read_all(lp, b);
    LedgerFile l = parse_ledger(b, ex);
    o.ledger_state = l.state; o.malformed_why = l.why;
    if (l.state == FileState::PARTIAL) o.last_line_partial = true;
    if (l.state == FileState::MALFORMED || l.state == FileState::PARTIAL) return o;  // aucune conclusion
    // troncature ou remplacement : detecte par l'empreinte ET le compte
    if (cur.armed && ex && !cur.ledger_digest_before.empty()) {
        if (l.rows.size() < cur.ledger_rows_before) o.truncated_or_replaced = true;
    }
    if (l.duplicate_seq > 0) o.duplicated_ids = true;
    // Le NOMBRE de lignes ne suffit PAS : on compare le PREFIXE deja observe.
    // On relit le contenu precedent pour en extraire les lignes deja vues.
    if (cur.armed && !cur.ledger_prefix.empty()) {
        std::istringstream pis(cur.ledger_prefix);
        std::string pl; std::vector<std::string> prev;
        while (std::getline(pis, pl)) if (!pl.empty()) prev.push_back(pl);
        std::istringstream nis(b);
        std::string nl; std::vector<std::string> now;
        while (std::getline(nis, nl)) if (!nl.empty()) now.push_back(nl);
        if (now.size() >= prev.size()) {
            for (size_t i = 0; i < prev.size(); ++i)
                if (prev[i] != now[i]) { o.prefix_modified = true; break; }
        }
        if (now.size() >= cur.ledger_rows_before && (int) cur.ledger_rows_before > 0 &&
            !prev.empty() && cur.ledger_rows_before <= prev.size()) {
            // remplacement a nombre egal ou superieur, detecte par le prefixe
            if (o.prefix_modified) o.replaced_equal_or_more = true;
        }
    }
    // NOUVEAUX evenements : tout ce qui depasse le compte initial. Aucune
    // hypothese de contiguite des identifiants.
    if (cur.armed) {
        for (size_t i = cur.ledger_rows_before; i < l.rows.size(); ++i) o.new_events.push_back(l.rows[i]);
    } else {
        o.new_events = l.rows;
    }
    SnapshotFile s = read_snapshots_file(sp);
    if (s.state == FileState::VALID) {
        if (cur.armed) { for (size_t i = cur.snap_rows_before; i < s.rows.size(); ++i) o.new_snapshots.push_back(s.rows[i]); }
        else o.new_snapshots = s.rows;
        if (s.duplicate_submission_id > 0) o.duplicated_ids = true;
        for (const auto & r : o.new_snapshots) if (r.submission_rc != 0) o.submission_error_present = true;
    }
    // soumissions NOUVELLES : identifiants distincts parmi les nouveaux evenements
    std::set<unsigned long long> ids;
    for (const auto & e : o.new_events) if (e.submission_id != 0) ids.insert(e.submission_id);
    for (const auto & r : o.new_snapshots) ids.insert(r.submission_id);
    o.n_new_submissions = (int) ids.size();
    return o;
}
