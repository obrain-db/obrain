// obrain-kv-cache.h -- [obrain K1/K2] CACHE D'ETATS HYBRIDES.
// EXTRAIT de server-context.cpp pour que le serveur ET les tests incluent le
// MEME code : les tests n'exercent pas une copie simplifiee des regles.
// Aucune dependance a llama.h : les types d'etat sont des entiers.
#pragma once
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <list>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>
#include <unistd.h>
#include "obrain-sha256.h"

namespace obrain_kv {

// Manifeste de COMPATIBILITE. Un hash de contenu etablit que le contenu est LE
// MEME ; il n'etablit PAS qu'il est COMPATIBLE. Le manifeste porte ces elements,
// qui sont EXTERIEURS au blob.
struct Manifest {
    std::string model_desc;        // description du modele charge
    std::string model_sha256;      // IDENTITE EXACTE du fichier de modele
    size_t      model_bytes   = 0;
    uint64_t    model_params  = 0;
    int32_t     n_vocab = 0, n_layer = 0, n_embd = 0;
    std::string q1_sha256;         // identite du calcul Q1
    uint32_t    format_magic  = 0;
    size_t      blob_bytes    = 0;
    std::string blob_sha256;       // integrite cryptographique des octets
    int32_t     n_tokens      = 0;
    int32_t     type_k = 0, type_v = 0;
    uint32_t    n_ctx = 0, n_batch = 0, n_ubatch = 0, n_seq_max = 0;
    int32_t     state_mode    = 0;
    int32_t     seq_id        = 0;
    std::vector<int32_t> prefix_ids;
    int64_t     pos_min = 0, pos_max = 0;

    // [obrain APPEND] COUVERTURE DE LA PART RECURRENTE — `recr_from` = premiere
    // position couverte par l'etat recurrent contenu dans ce blob.
    //
    // POURQUOI CETTE DONNEE DOIT ETRE DANS LE MANIFESTE ET NULLE PART AILLEURS.
    // Sur un modele hybride, l'etat recurrent est un PLI (un resume) de toutes les
    // positions vues, et il n'est PAS indexe par position : le blob n'en porte que
    // la derniere position (`recr_pos_max` implicite = pos_max de la cellule).
    // Impossible donc, en relisant le blob, de savoir si ce pli couvre depuis 0 ou
    // seulement depuis la page qu'il accompagne. Or c'est exactement ce qui decide
    // si l'ajouter est CORRECT (le pli couvre les pages deja en place) ou FAUX (il
    // les ignore). Le manifeste est le seul endroit ou cette provenance survit.
    //
    // 0 = le pli couvre depuis le debut de la sequence : la page peut etre ajoutee
    //     a une sequence assemblee qui commence a 0.
    // > 0 = le pli ne couvre qu'a partir de cette position : l'ajouter a une
    //     sequence qui porte deja [0, k) produirait un etat recurrent qui IGNORE
    //     ces pages, sans aucun message. La route REFUSE alors explicitement.
    int64_t     recr_from = 0;

    // La couverture du pli ne fait PAS partie de la compatibilite : deux etats du
    // meme modele, du meme mode, a la meme position, restent interchangeables au
    // sens du format. C'est leur AJOUT qui est conditionne, pas leur validite.

    // Toute donnee necessaire INCONNUE => manifeste INCOMPLET => refus.
    bool complete() const {
        return !model_desc.empty() && !model_sha256.empty() && model_bytes != 0 &&
               model_params != 0 && n_vocab > 0 && n_layer > 0 && n_embd > 0 &&
               !q1_sha256.empty() && format_magic != 0 && blob_bytes != 0 &&
               !blob_sha256.empty() && n_tokens > 0 && type_k != 0 && type_v != 0 &&
               n_ctx > 0 && n_batch > 0 && n_ubatch > 0 && n_seq_max > 0 &&
               !prefix_ids.empty();
    }
};

struct Blob {
    uint32_t n_tokens = 0;
    std::vector<uint8_t> data;
    std::shared_ptr<Manifest> manifest;
};

// [obrain R2] SHA256 EN PROCESSUS. L'ancienne version creait un
// SOUS-PROCESSUS (`popen("sha256sum ...")`) et un FICHIER TEMPORAIRE pour
// chaque empreinte. Plus aucun fork, aucun shell, aucun fichier temporaire.
inline std::string sha256_hex(const uint8_t * d, size_t n) {
    return obrain_sha::hex_of(d, n);
}

// SHA256 d'un FICHIER (utilise pour l'identite du modele et de Q1).
inline std::string sha256_hex_file(const std::string & path) {
    return obrain_sha::hex_of_file(path);
}

inline uint64_t fnv1a64(const std::vector<uint8_t> & d) {
    uint64_t h = 0xcbf29ce484222325ULL;
    for (uint8_t b : d) { h ^= b; h *= 0x100000001b3ULL; }
    return h;
}

inline std::string ref_of(const std::vector<uint8_t> & data) {
    char buf[17]; snprintf(buf, sizeof buf, "%016llx", (unsigned long long) fnv1a64(data));
    return std::string(buf);
}

// Comparaison STRICTE du manifeste. Rend la raison du refus, ou "" si compatible.
inline std::string manifest_mismatch(const Manifest & stored, const Manifest & cur) {
    if (!stored.complete())          return "manifeste d'etat INCOMPLET (donnee necessaire inconnue)";
    if (stored.model_sha256.empty()) return "identite SHA256 du modele inconnue";
    if (stored.model_sha256 != cur.model_sha256)
                                     return "SHA256 du modele DIFFERENT (identite exacte)";
    if (stored.model_desc   != cur.model_desc)   return "description du modele differente";
    if (stored.model_bytes  != cur.model_bytes)  return "taille du modele differente";
    if (stored.model_params != cur.model_params) return "nombre de parametres different";
    if (stored.n_vocab != cur.n_vocab || stored.n_layer != cur.n_layer ||
        stored.n_embd  != cur.n_embd)            return "topologie du modele differente";
    if (stored.q1_sha256 != cur.q1_sha256)       return "identite du calcul Q1 differente";
    if (stored.format_magic == 0)                return "format d'etat inconnu";
    if (stored.type_k != cur.type_k || stored.type_v != cur.type_v)
                                                 return "types KV differents";
    if (stored.state_mode != cur.state_mode)     return "mode de serialisation different";
    if (stored.n_ctx != cur.n_ctx || stored.n_batch != cur.n_batch ||
        stored.n_ubatch != cur.n_ubatch || stored.n_seq_max != cur.n_seq_max)
                                                 return "configuration de contexte differente";
    if (stored.seq_id != cur.seq_id)             return "convention de sequence differente";
    return std::string();
}

// --------------------------------------------------- DECISION D'IMPORT
// FONCTION DE PRODUCTION : c'est ELLE que la route appelle avant set_data.
// Le test l'exerce directement, donc il n'existe qu'une seule implementation
// des regles de refus.
enum class ImportDecision { ACCEPT, NO_MANIFEST, INCOMPATIBLE, BAD_SHA256, BAD_SIZE };

inline ImportDecision decide_import(const Blob & b, const Manifest & cur, std::string & why) {
    if (!b.manifest)          { why = "state has NO compatibility manifest"; return ImportDecision::NO_MANIFEST; }
    const std::string m = manifest_mismatch(*b.manifest, cur);
    if (!m.empty())           { why = "state INCOMPATIBLE : " + m; return ImportDecision::INCOMPATIBLE; }
    if (b.data.size() != b.manifest->blob_bytes)
                              { why = "state size mismatch vs manifest"; return ImportDecision::BAD_SIZE; }
    if (sha256_hex(b.data.data(), b.data.size()) != b.manifest->blob_sha256)
                              { why = "state SHA256 MISMATCH"; return ImportDecision::BAD_SHA256; }
    why.clear();
    return ImportDecision::ACCEPT;
}
inline const char * decision_name(ImportDecision d) {
    switch (d) {
        case ImportDecision::ACCEPT:       return "ACCEPT";
        case ImportDecision::NO_MANIFEST:  return "REFUSE_NO_MANIFEST";
        case ImportDecision::INCOMPATIBLE: return "REFUSE_INCOMPATIBLE";
        case ImportDecision::BAD_SHA256:   return "REFUSE_BAD_SHA256";
        case ImportDecision::BAD_SIZE:     return "REFUSE_BAD_SIZE";
    }
    return "?";
}

// ------------------------------------------- PERSISTANCE DISQUE (blob+manifeste)
// Format : magic "OBKV" | uint32 version | uint32 n_fields | champs cle=valeur
// separes par \n | uint64 blob_bytes | octets du blob.
// Un lecteur REFUSE un fichier tronque, sans manifeste ou dont l'association
// blob/manifeste est discordante.
struct DiskLoad { bool ok = false; std::string why; Blob blob; };

inline bool save_state_file(const std::string & path, const Blob & b) {
    if (!b.manifest || b.data.empty()) return false;
    std::ofstream f(path, std::ios::binary);
    if (!f) return false;
    const char * magic = "OBKV"; const uint32_t ver = 1;
    f.write(magic, 4); f.write((const char *) &ver, sizeof ver);
    f << "blob_sha256=" << b.manifest->blob_sha256 << "\n"
      << "model_sha256=" << b.manifest->model_sha256 << "\n"
      << "q1_sha256=" << b.manifest->q1_sha256 << "\n"
      << "format_magic=" << b.manifest->format_magic << "\n"
      << "n_tokens=" << b.manifest->n_tokens << "\n"
      << "type_k=" << b.manifest->type_k << "\n"
      << "type_v=" << b.manifest->type_v << "\n"
      << "n_ctx=" << b.manifest->n_ctx << "\n"
      << "n_batch=" << b.manifest->n_batch << "\n"
      << "n_ubatch=" << b.manifest->n_ubatch << "\n"
      << "n_seq_max=" << b.manifest->n_seq_max << "\n"
      << "state_mode=" << b.manifest->state_mode << "\n"
      << "seq_id=" << b.manifest->seq_id << "\n"
      << "blob_bytes=" << b.manifest->blob_bytes << "\n"
      << "model_desc=" << b.manifest->model_desc << "\n"
      << "model_bytes=" << b.manifest->model_bytes << "\n"
      << "model_params=" << b.manifest->model_params << "\n"
      << "n_vocab=" << b.manifest->n_vocab << "\n"
      << "n_layer=" << b.manifest->n_layer << "\n"
      << "n_embd=" << b.manifest->n_embd << "\n"
      << "pos_min=" << b.manifest->pos_min << "\n"
      << "pos_max=" << b.manifest->pos_max << "\n"
      << "recr_from=" << b.manifest->recr_from << "\n"
      << "prefix_ids=";
    for (size_t i = 0; i < b.manifest->prefix_ids.size(); ++i) {
        if (i) f << ",";
        f << b.manifest->prefix_ids[i];
    }
    f << "\n" << "END\n";
    const uint64_t n = b.data.size();
    f.write((const char *) &n, sizeof n);
    f.write((const char *) b.data.data(), (std::streamsize) b.data.size());
    return (bool) f;
}

inline DiskLoad load_state_file(const std::string & path) {
    DiskLoad r;
    std::ifstream f(path, std::ios::binary);
    if (!f) { r.why = "fichier illisible"; return r; }
    char magic[4] = {0}; f.read(magic, 4);
    if (memcmp(magic, "OBKV", 4) != 0) { r.why = "magic invalide"; return r; }
    uint32_t ver = 0; f.read((char *) &ver, sizeof ver);
    if (ver != 1) { r.why = "version inconnue"; return r; }
    std::string line; std::unordered_map<std::string, std::string> kv;
    while (std::getline(f, line)) {
        if (line == "END") break;
        auto e = line.find('=');
        if (e != std::string::npos) kv[line.substr(0, e)] = line.substr(e + 1);
    }
    if (kv.empty()) { r.why = "manifeste ABSENT dans le fichier"; return r; }
    uint64_t n = 0; f.read((char *) &n, sizeof n);
    if (!f) { r.why = "en-tete de taille illisible"; return r; }
    std::vector<uint8_t> data(n);
    f.read((char *) data.data(), (std::streamsize) n);
    if ((uint64_t) f.gcount() != n) { r.why = "blob TRONQUE"; return r; }
    auto M = std::make_shared<Manifest>();
    M->blob_sha256  = kv["blob_sha256"];
    M->model_sha256 = kv["model_sha256"];
    M->q1_sha256    = kv["q1_sha256"];
    M->format_magic = (uint32_t) strtoul(kv["format_magic"].c_str(), nullptr, 10);
    M->n_tokens     = atoi(kv["n_tokens"].c_str());
    M->type_k       = atoi(kv["type_k"].c_str());
    M->type_v       = atoi(kv["type_v"].c_str());
    M->n_ctx        = (uint32_t) strtoul(kv["n_ctx"].c_str(), nullptr, 10);
    M->n_batch      = (uint32_t) strtoul(kv["n_batch"].c_str(), nullptr, 10);
    M->n_ubatch     = (uint32_t) strtoul(kv["n_ubatch"].c_str(), nullptr, 10);
    M->n_seq_max    = (uint32_t) strtoul(kv["n_seq_max"].c_str(), nullptr, 10);
    M->state_mode   = atoi(kv["state_mode"].c_str());
    M->seq_id       = atoi(kv["seq_id"].c_str());
    M->blob_bytes   = (size_t) strtoull(kv["blob_bytes"].c_str(), nullptr, 10);
    M->model_desc   = kv["model_desc"];
    M->model_bytes  = (size_t) strtoull(kv["model_bytes"].c_str(), nullptr, 10);
    M->model_params = (uint64_t) strtoull(kv["model_params"].c_str(), nullptr, 10);
    M->n_vocab      = atoi(kv["n_vocab"].c_str());
    M->n_layer      = atoi(kv["n_layer"].c_str());
    M->n_embd       = atoi(kv["n_embd"].c_str());
    M->pos_min      = (int64_t) strtoll(kv["pos_min"].c_str(), nullptr, 10);
    M->pos_max      = (int64_t) strtoll(kv["pos_max"].c_str(), nullptr, 10);
    // [obrain APPEND] couverture du pli recurrent. ABSENTE = 0 : c'est la bonne
    // valeur pour tout etat extrait par la version historique du serveur, qui
    // decodait TOUJOURS le fait a partir de la position 0 (man->pos_min = 0).
    M->recr_from    = kv.count("recr_from") ? (int64_t) strtoll(kv["recr_from"].c_str(), nullptr, 10) : 0;
    // PREFIXE EXACT, restaure tel quel. Sans lui le manifeste reste INCOMPLET
    // et l'import est refuse : aucun manifeste retroactif n'est fabrique.
    { const std::string & pids = kv["prefix_ids"];
      if (!pids.empty()) {
          size_t a = 0;
          while (a <= pids.size()) {
              size_t b2 = pids.find(',', a);
              const std::string tok = pids.substr(a, (b2 == std::string::npos) ? std::string::npos : b2 - a);
              if (tok.empty()) break;
              M->prefix_ids.push_back((int32_t) strtol(tok.c_str(), nullptr, 10));
              if (b2 == std::string::npos) break;
              a = b2 + 1;
          }
      } }
    r.blob.n_tokens = (uint32_t) M->n_tokens;
    r.blob.data = std::move(data);
    r.blob.manifest = M;
    r.ok = true;
    return r;
}

// ═══════════════════════════════════════════════════════════════════════════
// ═══════════════════════════════════════════════════════════════════════════
// [obrain SPLICE-BOUND] LECTURE DE LA STRUCTURE D'UN BLOB D'ETAT.
//
// POURQUOI CE LECTEUR EXISTE. On a cru que `llama_state_seq_get_size(ctx,
// seq_id)` rendait « l'etat du SLOT entier » (158 Mo pour un fait de 24
// jetons), et donc qu'un correctif consistait a n'extraire que les n_fact
// premiers jetons. La mesure sur un blob REEL dit autre chose.
//
// MESURE (2026-10-05, ~/kv-states/22455e65bd8fdde6.bin, Qwen3.8-27B-OBRAIN-v3
// hybride, manifeste n_tokens=14, positions [0,13], blob = 157 812 552 octets) :
//
//   magic 0xaf143cd9 | seq_id=3 | n_stream=1
//   ATTENTION : cell_count = 14 == n_fact ............    917 888 octets
//               16 couches, ligne K = ligne V = 2048 octets (f16)
//               => 65 536 octets par JETON + 20 octets de meta par cellule
//   RECURRENT : 1 cellule, 48 couches, R = 30 720 f32, S = 786 432 f32
//               par couche ............................ 156 894 336 octets
//   total exact ....................................... 157 812 552 octets
//
// Deux faits en sortent, et ils changent le diagnostic :
//
//  1. La partie qui depend des jetons EST DEJA bornee a la sequence du fait :
//     14 cellules KV pour 14 jetons decodes. Le defaut suppose (« le slot
//     entier ») n'existe pas sur ce chemin : `llama_state_seq_get_size` rend la
//     taille ALLOUEE POUR LA SERIALISATION de la sequence, et l'ecriture ne
//     retient que les cellules qui PORTENT cette sequence
//     (`cells.seq_has(i, seq_id)` cote KV, `cell.has_seq_id(seq_id)` cote
//     recurrent). Un fait de 14 jetons ne peut pas en produire plus de 14.
//
//  2. Les 99,4 % restants sont l'etat RECURRENT d'un modele hybride (48 couches
//     a attention lineaire). Cet etat est CONSTANT : il ne depend pas de la
//     longueur du prefixe, et il est INDISPENSABLE — c'est lui qui porte le
//     contenu du bloc pour ces couches, qui n'ont pas de cache KV. Tronquer le
//     blob a « n_fact x octets_par_jeton » ne le rendrait pas plus petit : cela
//     produirait un etat CORROMPU (state_read echouerait, ou pire, restaurerait
//     un etat partiel en silence).
//
// Ce que ce lecteur apporte, et rien de plus : la possibilite de VERIFIER le
// bornage au lieu de le supposer, et de NOMMER les deux parties dans la reponse
// et dans le journal. Un `blob_bytes` de 158 Mo ne dit pas si le defaut est
// « trop de jetons » ou « etat recurrent » : deux causes, deux correctifs
// opposes. Il ne modifie rien, ne prend aucun verrou et ne lit aucun etat du
// serveur : seulement les octets qu'on lui donne.
//
// Il ne connait AUCUNE constante du modele (dimensions, nombre de couches) : il
// avance en lisant le format ecrit par `llama_context::state_write_data`, ce qui
// le rend valable pour tout modele et tout mode de compression (k4 = 0/1/2).
struct BlobLayout {
    bool        ok          = false;
    std::string why;
    size_t      consomme    = 0;      // octets lus (doit valoir la taille du blob)
    uint32_t    n_stream    = 0;
    uint32_t    attn_cells  = 0;      // cellules KV de la SEQUENCE (doit valoir n_fact)
    size_t      attn_bytes  = 0;      // part PROPORTIONNELLE aux jetons
    uint32_t    rec_cells   = 0;
    size_t      rec_bytes   = 0;      // part CONSTANTE (etat recurrent hybride)
    uint32_t    rec_layers_r = 0;     // entrees R ecrites
    uint32_t    rec_layers_s = 0;     // entrees S ecrites
};

namespace blob_detail {

// Curseur BORNE : chaque lecture verifie d'abord qu'elle tient dans le blob.
// Un lecteur qui lit d'abord et verifie ensuite ne verifie rien.
struct Curseur {
    const uint8_t * d;
    size_t n;
    size_t o = 0;

    bool reste(size_t k) const { return k <= n - o; }
    bool u32(uint32_t & v) {
        if (!reste(4)) return false;
        memcpy(&v, d + o, 4);   // meme endianness que l'ecriture (meme machine)
        o += 4;
        return true;
    }
    bool u64(uint64_t & v) {
        if (!reste(8)) return false;
        memcpy(&v, d + o, 8);
        o += 8;
        return true;
    }
    bool sauter(uint64_t k) {
        if (k > (uint64_t) (n - o)) return false;
        o += (size_t) k;
        return true;
    }
};

// Une passe du format, pour une taille donnee d'extension de cellule
// (`n_pos_per_embd` : 8 octets quand le modele est en MROPE, 0 sinon).
// Rend true SEULEMENT si le blob est consomme EXACTEMENT : un parseur qui
// s'arrete « quelque part » dans un blob accepte n'importe quoi.
inline bool marche(const uint8_t * d, size_t n, size_t ext, BlobLayout & r, std::string & why) {
    blob_detail::Curseur c{d, n};
    uint32_t magic = 0, seq = 0, n_stream = 0;
    if (!c.u32(magic))       { why = "en-tete tronque"; return false; }
    if (magic != 0xaf143cd9u) { why = "magic d'etat inconnu"; return false; }
    if (!c.u32(seq))         { why = "seq_id illisible"; return false; }
    if (!c.u32(n_stream))    { why = "n_stream illisible"; return false; }
    if (n_stream == 0 || n_stream > 4096) { why = "n_stream invraisemblable"; return false; }
    r.n_stream = n_stream;

    for (uint32_t s = 0; s < n_stream; ++s) {
        uint32_t cellules = 0;
        if (!c.u32(cellules)) { why = "cell_count illisible"; return false; }
        if (cellules > (uint32_t) (n / 8)) { why = "cell_count invraisemblable"; return false; }
        r.attn_cells += cellules;

        // ── [obrain APPEND] FLUX VIDE : RIEN NE SUIT LE COMPTE ─────────────────
        // POURQUOI CE `continue` EST OBLIGATOIRE, ET CE QU'IL CORRIGE.
        // L'ECRIVAIN (llama_kv_cache::state_write_range) ecrit le nombre de
        // cellules du flux, puis `continue` quand ce nombre vaut 0 : un flux vide
        // n'a NI metadonnees de cellules NI section de donnees.
        // Le LECTEUR, lui, lisait la section de donnees (v_trans, n_layer,
        // descripteurs K/V) dans TOUS les cas. Des le premier flux vide il se
        // desalignait donc de quelques octets, lisait un `n_layer` qui n'en etait
        // pas un, et declarait le blob « ILLISIBLE (topologie d'attention
        // invraisemblable) » -- un etat PARFAITEMENT VALIDE refuse, avec un
        // message qui accusait le blob au lieu du lecteur.
        // Le defaut etait invisible sur un serveur mono-flux (n_stream = 1, donc
        // `--parallel 1`) : c'est pourquoi il a survecu jusqu'ici. Il frappait
        // TOUTE configuration multi-sequences des qu'une autre sequence que la
        // sequence visee occupait un flux distinct. Mesure : 2026-10-05, serveur
        // --parallel 3 (n_stream = 3), extraction d'etat refusee en HTTP 500 sur
        // un modele parfaitement sain.
        if (cellules == 0) {
            continue;
        }

        for (uint32_t i = 0; i < cellules; ++i) {
            uint32_t pos = 0, n_seq_id = 0;
            if (!c.u32(pos) || !c.u32(n_seq_id)) { why = "meta de cellule tronquee"; return false; }
            if (n_seq_id > 4096)                 { why = "n_seq_id invraisemblable"; return false; }
            if (!c.sauter(ext))                  { why = "extension de cellule tronquee"; return false; }
            if (!c.sauter((uint64_t) 4 * n_seq_id)) { why = "ids de sequence tronques"; return false; }
        }

        uint32_t v_trans = 0, n_layer = 0;
        if (!c.u32(v_trans) || !c.u32(n_layer)) { why = "en-tete de donnees illisible"; return false; }
        if (v_trans > 1 || n_layer == 0 || n_layer > 1024) {
            why = "topologie d'attention invraisemblable"; return false;
        }
        // K : n_layer x (type i32 + taille de ligne u64 + cellules x ligne).
        for (uint32_t l = 0; l < n_layer; ++l) {
            uint32_t type = 0; uint64_t ligne = 0;
            if (!c.u32(type) || !c.u64(ligne)) { why = "descripteur K tronque"; return false; }
            if (!c.sauter(ligne * (uint64_t) cellules)) { why = "donnees K tronquees"; return false; }
        }
        if (v_trans == 0) {
            for (uint32_t l = 0; l < n_layer; ++l) {
                uint32_t type = 0; uint64_t ligne = 0;
                if (!c.u32(type) || !c.u64(ligne)) { why = "descripteur V tronque"; return false; }
                if (!c.sauter(ligne * (uint64_t) cellules)) { why = "donnees V tronquees"; return false; }
            }
        } else {
            // V TRANSPOSE (v_trans = 1) : le format n'est pas « une ligne par
            // cellule » mais « un element par cellule, element par element »
            // (llama-kv-cache.cpp, branche v_trans). Le total reste calculable :
            // n_embd_v_gqa elements de v_size_el octets par cellule. Ne pas
            // traiter ce cas ferait REFUSER des blobs parfaitement valides — un
            // correctif qui casse un autre modele n'est pas un correctif.
            for (uint32_t l = 0; l < n_layer; ++l) {
                uint32_t type = 0, size_el = 0, n_embd_v = 0;
                if (!c.u32(type) || !c.u32(size_el) || !c.u32(n_embd_v)) {
                    why = "descripteur V transpose tronque"; return false;
                }
                if (!c.sauter((uint64_t) n_embd_v * size_el * (uint64_t) cellules)) {
                    why = "donnees V transposees tronquees"; return false;
                }
            }
        }
    }
    r.attn_bytes = c.o;
    if (c.o == n) { why.clear(); return true; }   // modele sans memoire recurrente

    // ── SECTION RECURRENTE (modele hybride : mamba/linear attention) ────────
    uint32_t rec_cellules = 0;
    if (!c.u32(rec_cellules)) { why = "cell_count recurrent illisible"; return false; }
    if (rec_cellules > (uint32_t) (n / 8)) { why = "cell_count recurrent invraisemblable"; return false; }
    r.rec_cells = rec_cellules;
    for (uint32_t i = 0; i < rec_cellules; ++i) {
        uint32_t pos = 0, n_seq_id = 0;
        if (!c.u32(pos) || !c.u32(n_seq_id)) { why = "meta recurrente tronquee"; return false; }
        if (n_seq_id > 4096)                 { why = "n_seq_id recurrent invraisemblable"; return false; }
        if (!c.sauter((uint64_t) 4 * n_seq_id)) { why = "ids de sequence recurrents tronques"; return false; }
    }
    uint32_t s_trans = 0, n_layer_r = 0, k4 = 0;
    if (!c.u32(s_trans) || !c.u32(n_layer_r) || !c.u32(k4)) { why = "en-tete recurrent illisible"; return false; }
    if (s_trans > 1 || n_layer_r == 0 || n_layer_r > 1024) {
        why = "topologie recurrente invraisemblable"; return false;
    }

    // Les entrees R puis S : les boucles d'ecriture font n_layer iterations
    // chacune, mais une couche NON recurrente n'ecrit RIEN. Le nombre d'entrees
    // n'est donc pas n_layer, et le format ne l'annonce pas : on enumere les
    // entrees jusqu'a epuiser le blob. La serie R se reconnait a sa taille de
    // ligne CONSTANTE (n_embd_r est un hparam global) ; le passage a S est le
    // premier CHANGEMENT de cette taille.
    uint64_t ligne_r = 0;
    bool premiere = true;
    while (true) {
        const size_t sauvegarde = c.o;
        uint32_t type = 0; uint64_t ligne = 0;
        if (!c.u32(type) || !c.u64(ligne)) { c.o = sauvegarde; break; }
        if (ligne == 0)                    { c.o = sauvegarde; break; }
        if (!c.sauter(ligne * (uint64_t) rec_cellules)) { c.o = sauvegarde; break; }
        if (premiere)                        { ligne_r = ligne; premiere = false; r.rec_layers_r = 1; }
        else if (ligne == ligne_r && r.rec_layers_s == 0) { r.rec_layers_r++; }
        else                                 { r.rec_layers_s++; }
    }
    r.rec_bytes = n - r.attn_bytes;
    if (c.o != n) { why = "octets residuels apres la section recurrente"; return false; }
    why.clear();
    return true;
}

} // namespace blob_detail

// Lit la structure d'un blob d'etat. `ok == false` ⇒ le blob n'est PAS celui
// qu'on croit (tronque, d'un autre format, ou d'une autre version) : l'appelant
// doit REFUSER, jamais improviser.
inline BlobLayout blob_layout(const uint8_t * d, size_t n) {
    BlobLayout a, b;
    std::string wa, wb;
    // `n_pos_per_embd` (MROPE => 4, donc une extension de 8 octets par cellule)
    // n'est PAS ecrit dans le blob. On essaie donc les deux tailles et on garde
    // celle qui consomme le blob EXACTEMENT : la verification porte sur le
    // total, pas sur un octet isole, donc une seule des deux peut passer.
    const bool oka = (n != 0) && blob_detail::marche(d, n, 0, a, wa);
    const bool okb = (n != 0) && blob_detail::marche(d, n, 8, b, wb);
    if (oka) { a.ok = true; a.consomme = n; return a; }
    if (okb) { b.ok = true; b.consomme = n; return b; }

    BlobLayout r;
    r.why = (a.attn_bytes >= b.attn_bytes) ? wa : wb;
    return r;
}

// ───────────────────────────────────────── LA REGLE DE BORNAGE D'UNE EXTRACTION
// FONCTION DE PRODUCTION : c'est ELLE que la route d'extraction appelle avant
// d'accepter un blob, et c'est elle que les tests exercent. Une seule
// implementation des regles, donc pas de « regle du test » a cote de la regle
// du serveur (meme choix que `decide_import`).
//
// Ce qui est exige : le nombre de cellules KV du blob ne peut pas DEPASSER le
// nombre de jetons effectivement decodes pour le fait. C'est la seule forme de
// bornage qui ait un sens ici : « extraire les n_fact premiers jetons » est
// exactement ce que fait deja l'ecriture (`seq_has`), et cette fonction le
// PROUVE au lieu de le supposer.
//
// Ce qui n'est PAS exige : l'egalite. Un modele a fenetre glissante peut
// legitimement ne pas serialiser les cellules masquees hors fenetre
// (`is_masked_swa` dans llama-kv-cache.cpp) : moins de cellules que de jetons
// est un cas NORMAL, plus de cellules ne l'est pas.
inline bool extraction_bornee(const BlobLayout & l, int32_t n_fact, std::string & why) {
    if (!l.ok) {
        why = "blob d'etat ILLISIBLE (" + l.why + ")";
        return false;
    }
    if (l.attn_cells > (uint32_t) n_fact) {
        why = "extraction NON BORNEE : " + std::to_string(l.attn_cells) +
              " cellules KV pour " + std::to_string(n_fact) + " jetons decodes" +
              " (le blob porte plus que le fait : " + std::to_string(l.attn_bytes) +
              " octets de KV, " + std::to_string(l.rec_bytes) + " octets recurrents)";
        return false;
    }
    why.clear();
    return true;
}
// ------------------------------------------------------------------ CACHE LRU
struct Cache {
    mutable std::mutex mtx;
    std::unordered_map<std::string, Blob> map;
    std::list<std::string> lru;   // front = plus recemment utilise
    std::unordered_map<std::string, std::list<std::string>::iterator> pos;
    size_t bytes = 0;
    // Compteurs d'observation (publiables dans les rapports).
    uint64_t n_insert = 0, n_replace = 0, n_evict = 0, n_refuse_too_big = 0;
    size_t   max_bytes = 0;
    // set_data_calls : incremente UNIQUEMENT par le chemin d'import REEL.
    // Permet de prouver SET_DATA_CALLS=0 sur un refus.
    mutable uint64_t set_data_calls = 0;

    // ATTENTION AUX UNITES : le parametre s'appelait `mb` alors que la valeur
    // est en OCTETS (`Cache(4096)` = 4096 octets, pas 4 Gio). Le nom invitait a
    // ecrire `Cache(2048)` en croyant demander 2 Gio, ce qui refusait ensuite
    // TOUTE insertion avec un message qui n'accusait pas l'unite. Renomme, et
    // l'unite est desormais dite a l'endroit ou on la lit.
    explicit Cache(size_t max_bytes_octets) : max_bytes(max_bytes_octets) {}
    static size_t default_max_bytes() {
        const char * e = std::getenv("OBRAIN_KV_STATES_MAX_BYTES");
        if (e && e[0]) { const long long x = atoll(e); if (x > 0) return (size_t) x; }
        return (size_t) 2 * 1024 * 1024 * 1024;   // 2 GiB, unites OCTETS
    }
    // false si l'entree est REFUSEE (vide, ou plus grosse que la borne totale).
    bool insert(const std::string & ref, Blob && b) {
        const size_t n = b.data.size();
        if (n == 0) return false;
        if (n > max_bytes) { n_refuse_too_big++; return false; }
        // [DEFAAUT CORRIGE] La cle existante doit etre retiree AVANT de calculer
        // le besoin : sinon un simple REMPLACEMENT evinçait une entree NON LIEE,
        // parce que sa propre taille comptait encore dans le total.
        auto ex = map.find(ref);
        if (ex != map.end()) {
            bytes -= ex->second.data.size();
            map.erase(ex);
            auto q = pos.find(ref);
            if (q != pos.end()) { lru.erase(q->second); pos.erase(q); }
            n_replace++;
        }
        while (bytes + n > max_bytes && !lru.empty()) {
            const std::string victim = lru.back();
            lru.pop_back();
            auto it = map.find(victim);
            if (it != map.end()) { bytes -= it->second.data.size(); map.erase(it); n_evict++; }
            pos.erase(victim);
        }
        bytes += n;
        map[ref] = std::move(b);
        lru.push_front(ref);
        pos[ref] = lru.begin();
        n_insert++;
        return true;
    }
    bool get(const std::string & ref, Blob & out) {
        auto it = map.find(ref);
        if (it == map.end()) return false;
        out = it->second;
        auto q = pos.find(ref);
        if (q != pos.end()) {
            lru.erase(q->second);
            lru.push_front(ref);
            q->second = lru.begin();
        }
        return true;
    }
    void clear() {
        map.clear(); lru.clear(); pos.clear(); bytes = 0;
    }
    size_t accounted_bytes() const { return bytes; }
    size_t size() const { return map.size(); }
};

} // namespace obrain_kv
