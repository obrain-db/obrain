// ENGINE-008 -- PREFLIGHT-CAPTURE-PARSER-07 : lecture des surfaces f1a REELLES.
// Aucune dependance a ggml ni a libllama : valeurs et chaines seulement.
// Les pointeurs des captures historiques sont des VALEURS ARCHIVEES, jamais
// des objets a dereferencer dans le processus courant.
#pragma once
#include <cstdint>
#include <string>
#include <vector>

// Etat d'un fichier de capture. Distingue ABSENT / VIDE / PARTIEL / MALFORME / VALIDE.
enum class FileState { ABSENT, EMPTY, PARTIAL, MALFORMED, VALID };
const char * file_state_name(FileState s);

struct LedgerEvent {
    std::string        run_id;
    unsigned long long seq = 0;
    std::string        kind;
    int                decode_step = -1;
    unsigned long long submission_id = 0;
    std::string        graph_ptr;      // valeur ARCHIVEE, non dereferencable
    std::string        fingerprint;
    std::string        capture_epoch;  // peut valoir "NA"
    long long          eligible = -1, captured = -1;
};

struct SnapshotRow {
    unsigned long long submission_id = 0;
    std::string        gf;
    int                n_nodes = -1;
    int                submission_rc = -1;
    std::string        fingerprint;
};

struct LedgerFile {
    FileState state = FileState::ABSENT;
    std::string why;
    std::vector<LedgerEvent> rows;
    // types d'evenements preserves, sans filtrage
    std::vector<std::string> kinds;
    long long duplicate_seq = 0;
};

struct SnapshotFile {
    FileState state = FileState::ABSENT;
    std::string why;
    std::vector<SnapshotRow> rows;
    long long duplicate_submission_id = 0;
};

// Lecture depuis des OCTETS : les tests peuvent injecter des fixtures, et la
// lecture reelle passe simplement le contenu du fichier.
LedgerFile   parse_ledger(const std::string & bytes, bool file_existed);
SnapshotFile parse_snapshots(const std::string & bytes, bool file_existed);
// Lecture disque : absent -> ABSENT, jamais une exception.
LedgerFile   read_ledger_file(const std::string & path);
SnapshotFile read_snapshots_file(const std::string & path);

// ---------------------------------------------------------------- CURSEUR DE FENETRE
// Releve avant/apres. Ne suppose AUCUNE contiguite des identifiants.
struct WindowCursor {
    // Releve AVANT : etat des surfaces et empreinte du contenu.
    FileState ledger_state_before = FileState::ABSENT;
    size_t    ledger_rows_before  = 0;
    std::string ledger_digest_before;      // empreinte de troncature/replacement
    FileState snap_state_before   = FileState::ABSENT;
    size_t    snap_rows_before     = 0;
    std::string snap_digest_before;
    bool      armed = false;
};
struct WindowObservation {
    bool truncated_or_replaced = false;   // le fichier a change sous nos pieds
    bool duplicated_ids        = false;
    FileState ledger_state = FileState::ABSENT;
    std::string malformed_why;
    std::vector<LedgerEvent> new_events;                 // evenements nouveaux
    std::vector<SnapshotRow> new_snapshots;              // snapshots nouveaux
    int n_new_submissions = 0;
    bool submission_error_present = false;
};
WindowCursor arm_window(const std::string & ledger_path, const std::string & snap_path);
WindowObservation observe_window(const WindowCursor & cur,
                                 const std::string & ledger_path,
                                 const std::string & snap_path);
