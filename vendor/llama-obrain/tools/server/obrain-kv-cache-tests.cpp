// obrain-kv-cache-tests -- TESTS SYNTHETIQUES DES FONCTIONS DE PRODUCTION.
// Inclut obrain-kv-cache.h, le MEME en-tete que server-context.cpp : les regles
// testees sont celles du serveur, pas une copie simplifiee.
#include "obrain-kv-cache.h"
#include <cstdio>
#include <string>
using namespace obrain_kv;

static int failed = 0, passed = 0;

// [P1] Blob de sortie jetable pour les lectures de verification : les
// tests de budget n'ont besoin que du BOOLEEN rendu par get().
static Blob & out_unused() {
    static Blob b;
    return b;
}
static void ck(const char * n, bool ok) {
    printf("  %-64s %s\n", n, ok ? "PASS" : "FAIL");
    if (ok) passed++; else failed++;
}

// Manifeste COMPLET de reference.
static Manifest mkm(const std::string & model_sha = "MODEL_A", const std::string & q1 = "Q1_A") {
    Manifest m;
    m.model_desc = "Qwen3.8-27B-OBRAIN-v3"; m.model_sha256 = model_sha;
    m.model_bytes = 16000000000ULL; m.model_params = 27000000000ULL;
    m.n_vocab = 248320; m.n_layer = 64; m.n_embd = 5120;
    m.q1_sha256 = q1; m.format_magic = 0xaf143cd9u;
    m.n_tokens = 58; m.type_k = 1; m.type_v = 1;
    m.n_ctx = 1024; m.n_batch = 512; m.n_ubatch = 512; m.n_seq_max = 1;
    m.state_mode = 0; m.seq_id = 0;
    m.prefix_ids.assign(58, 7);
    m.pos_min = 0; m.pos_max = 57;
    return m;
}
// [obrain SPLICE-BOUND] Construction SYNTHETIQUE d'un blob d'etat, octet par
// octet, dans le format ecrit par `llama_context::state_write_data` :
//   magic | seq_id | n_stream { cellules (pos, n_seq_id, [ext], ids) | v_trans |
//   n_layer { type K, ligne K, donnees } [idem V] } | section recurrente.
// Un blob synthetique permet d'exercer le LECTEUR sur des cas qu'un vrai modele
// ne fabrique pas a la demande : aucune couche recurrente, blob tronque, plus de
// cellules que de jetons.
struct BlobFab {
    std::vector<uint8_t> d;
    void u32(uint32_t v) { for (int i = 0; i < 4; ++i) d.push_back((uint8_t) ((v >> (8 * i)) & 0xFF)); }
    void u64(uint64_t v) { for (int i = 0; i < 8; ++i) d.push_back((uint8_t) ((v >> (8 * i)) & 0xFF)); }
    void zeros(size_t n) { for (size_t i = 0; i < n; ++i) d.push_back(0); }
    void entete(uint32_t seq, uint32_t n_stream) { u32(0xaf143cd9u); u32(seq); u32(n_stream); }
    // meta de cellule : pos (4) + n_seq_id (4) + extension MROPE (8, si mrope) + 1 id de sequence (4)
    void kv_ext(uint32_t cellules, uint32_t n_layer, uint64_t ligne_k, uint64_t ligne_v, bool mrope) {
        u32(cellules);
        for (uint32_t i = 0; i < cellules; ++i) {
            u32(i); u32(1);
            if (mrope) u64(0);
            u32(3);
        }
        u32(0);            // v_trans
        u32(n_layer);
        for (uint32_t l = 0; l < n_layer; ++l) { u32(1); u64(ligne_k); zeros((size_t) (ligne_k * cellules)); }
        for (uint32_t l = 0; l < n_layer; ++l) { u32(1); u64(ligne_v); zeros((size_t) (ligne_v * cellules)); }
    }
    // Section recurrente : `couches_ecrites` entrees R puis autant d'entrees S
    // (les couches NON recurrentes n'ecrivent RIEN — c'est pourquoi le nombre
    // d'entrees n'est pas n_layer, et pourquoi le lecteur les enumere).
    void recurrent(uint32_t cellules, uint32_t n_layer, uint64_t ligne_r, uint64_t ligne_s, uint32_t couches_ecrites) {
        u32(cellules); u32(0); u32(0);      // 1 cellule, pos = 0, n_seq_id = 0
        u32(0); u32(n_layer); u32(0);       // s_trans, n_layer, k4 = natif f32
        for (uint32_t l = 0; l < couches_ecrites; ++l) { u32(0); u64(ligne_r); zeros((size_t) (ligne_r * cellules)); }
        for (uint32_t l = 0; l < couches_ecrites; ++l) { u32(0); u64(ligne_s); zeros((size_t) (ligne_s * cellules)); }
    }
};

static Blob mkb(const std::vector<uint8_t> & d, std::shared_ptr<Manifest> m) {
    Blob b; b.n_tokens = (uint32_t)(m ? m->n_tokens : 0); b.data = d; b.manifest = m;
    if (m) { m->blob_bytes = d.size(); m->blob_sha256 = sha256_hex(d.data(), d.size()); }
    return b;
}

int main() {
    printf("OBRAIN_KV_CACHE_TESTS -- fonctions de production\n\n");
    const Manifest cur = mkm();

    printf("=== A. MANIFESTE ABSENT / INCOMPLET / INCOMPATIBLE ===\n");
    { std::string why; Blob b = mkb(std::vector<uint8_t>(1024, 0x41), nullptr);
      ck("manifeste ABSENT -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::NO_MANIFEST); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm());
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      b.manifest->blob_sha256.clear();   // efface APRES mkb (qui le recalcule)
      ck("manifeste INCOMPLET (sha vide) -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm());
      m->model_sha256.clear();
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      ck("manifeste INCOMPLET (identite modele absente) -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE); }

    printf("\n=== B. MODELE DIFFERENT malgre description ET topologie IDENTIQUES ===\n");
    { std::string why; auto m = std::make_shared<Manifest>(mkm("MODEL_B", "Q1_A"));
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      const bool r = decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE;
      ck("SHA256 modele different, desc/topologie identiques -> REFUSE", r);
      printf("       raison : %s\n", why.c_str()); }

    printf("\n=== C. Q1, TYPES, MODE, CONFIG, SEQUENCE ===\n");
    { std::string why; auto m = std::make_shared<Manifest>(mkm("MODEL_A", "Q1_B"));
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      ck("identite du calcul Q1 differente -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm()); m->type_v = 2;
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      ck("type KV different -> REFUSE", decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm()); m->state_mode = 1;
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      ck("mode de serialisation different -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm()); m->n_ctx = 512;
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      ck("configuration de contexte differente -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm()); m->seq_id = 3;
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      ck("convention de sequence differente -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm()); m->format_magic = 0;
      Blob b = mkb(std::vector<uint8_t>(1024, 0x41), m);
      ck("format d'etat inconnu -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::INCOMPATIBLE); }

    printf("\n=== D. BLOB TRONQUE / ALTERE / SHA256 DISCORDANT ===\n");
    { std::string why; auto m = std::make_shared<Manifest>(mkm());
      Blob b = mkb(std::vector<uint8_t>(4096, 0x42), m);
      b.data.resize(2048);                          // TRONQUE apres le SHA
      ck("blob TRONQUE -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::BAD_SIZE); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm());
      Blob b = mkb(std::vector<uint8_t>(4096, 0x42), m);
      b.data[100] ^= 0xFF;                          // ALTERE, meme taille
      ck("blob ALTERE (meme taille) -> SHA256 discordant -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::BAD_SHA256); }
    { std::string why; auto m = std::make_shared<Manifest>(mkm());
      Blob b = mkb(std::vector<uint8_t>(4096, 0x42), m);
      b.manifest->blob_sha256 = std::string(64, '0');
      ck("SHA256 stocké falsifie -> REFUSE",
         decide_import(b, cur, why) == ImportDecision::BAD_SHA256); }

    printf("\n=== E. CAS ACCEPTE + SET_DATA_CALLS ===\n");
    { std::string why; auto m = std::make_shared<Manifest>(mkm());
      Blob b = mkb(std::vector<uint8_t>(4096, 0x42), m);
      Cache cache(1u << 20);
      const auto d = decide_import(b, cur, why);
      ck("manifeste complet et concordant -> ACCEPT", d == ImportDecision::ACCEPT);
      if (d == ImportDecision::ACCEPT) cache.set_data_calls++;
      ck("SET_DATA_CALLS=1 sur ACCEPT", cache.set_data_calls == 1); }
    { // chaque refus doit laisser SET_DATA_CALLS a 0
      Cache cache(1u << 20);
      struct Case { const char * n; Blob b; };
      std::vector<Blob> bad;
      { std::string w; bad.push_back(mkb(std::vector<uint8_t>(512, 1), nullptr)); }
      { auto m = std::make_shared<Manifest>(mkm("MODEL_B")); bad.push_back(mkb(std::vector<uint8_t>(512, 1), m)); }
      { auto m = std::make_shared<Manifest>(mkm()); Blob b = mkb(std::vector<uint8_t>(512, 1), m);
        b.data[0] ^= 1; bad.push_back(b); }
      { auto m = std::make_shared<Manifest>(mkm()); Blob b = mkb(std::vector<uint8_t>(512, 1), m);
        b.data.resize(256); bad.push_back(b); }
      int n_ref = 0;
      for (auto & b : bad) { std::string w;
        if (decide_import(b, cur, w) != ImportDecision::ACCEPT) n_ref++; else cache.set_data_calls++; }
      ck("SET_DATA_CALLS=0 sur les 4 refus", cache.set_data_calls == 0 && n_ref == 4); }

    printf("\n=== F. BORNE MEMOIRE ET REFUS D'ENTREE TROP GROSSE ===\n");
    { Cache c(4096);                                   // borne 4096 octets
      Blob big = mkb(std::vector<uint8_t>(8192, 7), std::make_shared<Manifest>(mkm()));
      const bool ok = c.insert("big", std::move(big));
      ck("entree PLUS GROSSE que la borne -> REFUSEE", !ok);
      ck("compteur n_refuse_too_big=1", c.n_refuse_too_big == 1);
      ck("aucune entree stockee", c.size() == 0 && c.accounted_bytes() == 0); }

    printf("\n=== G. LRU : ORDRE, EVICTION, COMPTABILITE ===\n");
    { Cache c(300);                                    // 3 entrees de 100
      auto mk = [](uint8_t f) { return std::vector<uint8_t>(100, f); };
      c.insert("A", Blob{1, mk(1), nullptr});
      c.insert("B", Blob{1, mk(2), nullptr});
      c.insert("C", Blob{1, mk(3), nullptr});
      ck("3 insertions -> 3 entrees, 300 octets", c.size() == 3 && c.accounted_bytes() == 300);
      Blob out;
      ck("get(A) touche A", c.get("A", out));
      c.insert("D", Blob{1, mk(4), nullptr});          // doit EVINCER le moins recent
      ck("insertion D evince B (le moins recemment utilise)", !c.get("B", out));
      ck("n_evict=1 apres l insertion de D", c.n_evict == 1);
      ck("A survit (recemment utilise)", c.get("A", out));
      ck("C survit", c.get("C", out));
      ck("D present", c.get("D", out));
      ck("comptabilite apres eviction = 300", c.accounted_bytes() == 300);

      // remplacement de la MEME cle : la comptabilite ne doit pas doubler
      c.insert("A", Blob{1, mk(9), nullptr});
      ck("remplacement meme cle : taille inchangee", c.size() == 3);
      ck("remplacement meme cle : octets inchanges", c.accounted_bytes() == 300);
      printf("       [diag] n_replace=%llu n_evict=%llu size=%zu bytes=%zu\n",
             (unsigned long long) c.n_replace, (unsigned long long) c.n_evict, c.size(), c.accounted_bytes());
      ck("remplacement : AUCUNE eviction non liee (n_evict reste 1)",
         c.n_evict == 1);
      ck("compteur n_replace=1", c.n_replace == 1);
      ck("valeur remplacee", c.get("A", out) && out.data[0] == 9);
      c.insert("A", Blob{1, std::vector<uint8_t>(400, 5), nullptr});
      ck("remplacement par PLUS GROS que la borne -> refuse",
         c.n_refuse_too_big == 1 && c.accounted_bytes() == 300); }

    printf("\n=== G2. LE BUDGET VIENT BIEN DE L'ENVIRONNEMENT, EN OCTETS ===\n");
    // POURQUOI CE TEST EXISTE : le chat affirme que le cache du moteur est borne
    // par `OBRAIN_KV_STATES_MAX_BYTES` (2 Gio par defaut). Une affirmation sur
    // une variable d'environnement se verifie en la posant, pas en la relisant.
    { setenv("OBRAIN_KV_STATES_MAX_BYTES", "12345678", 1);
      ck("OBRAIN_KV_STATES_MAX_BYTES=12345678 -> borne de 12345678 OCTETS",
         Cache::default_max_bytes() == 12345678u);
      setenv("OBRAIN_KV_STATES_MAX_BYTES", "0", 1);
      ck("valeur 0 (invalide) -> repli sur le defaut", Cache::default_max_bytes() == (size_t) 2 * 1024 * 1024 * 1024);
      unsetenv("OBRAIN_KV_STATES_MAX_BYTES");
      ck("variable absente -> defaut 2 Gio", Cache::default_max_bytes() == (size_t) 2 * 1024 * 1024 * 1024); }

    printf("\n=== G3. BUDGET SOUS CHARGE : N INSERTIONS, BORNE JAMAIS DEPASSEE ===\n");
    // POURQUOI CE TEST EXISTE : la garantie qui compte n'est pas « une insertion
    // de trop est evincee » mais « AUCUNE suite d'insertions ne fait depasser le
    // budget ». Une croissance monotone de la RAM est exactement le defaut qu'on
    // ferme (chaque extraction epinglait un blob a jamais).
    { const size_t budget = 1000;
      Cache c(budget);
      size_t max_vu = 0; bool borne_tenue = true;
      for (int i = 0; i < 64; ++i) {
          const std::string ref = "ref" + std::to_string(i);
          c.insert(ref, Blob{1, std::vector<uint8_t>(120, (uint8_t) i), nullptr});
          max_vu = std::max(max_vu, c.accounted_bytes());
          if (c.accounted_bytes() > budget) borne_tenue = false;
      }
      ck("64 insertions de 120 o dans un budget de 1000 o : borne JAMAIS depassee", borne_tenue);
      ck("le cache est plein mais borne (8 entrees)", c.size() == 8 && c.accounted_bytes() == 960);
      ck("56 evictions comptees (64 - 8)", c.n_evict == 56);
      ck("les 8 DERNIERES refs sont presentes", c.get("ref63", out_unused()) && c.get("ref56", out_unused()));
      ck("les plus anciennes ont ete evincees", !c.get("ref0", out_unused()) && !c.get("ref55", out_unused()));
      ck("octets rendus apres evictions (pas de fuite comptable)", c.accounted_bytes() == 8 * 120);
      printf("       [diag] max_vu=%zu budget=%zu entrees=%zu evictions=%llu\n",
             max_vu, budget, c.size(), (unsigned long long) c.n_evict); }
    // Et la borne se tient AUSSI quand les tailles varient (eviction en cascade).
    { Cache c(1000);
      for (int i = 0; i < 20; ++i) {
          c.insert("v" + std::to_string(i), Blob{1, std::vector<uint8_t>(90 + (size_t) (i % 7) * 10, 1), nullptr});
      }
      ck("tailles variables : borne respectee", c.accounted_bytes() <= 1000);
      ck("tailles variables : une seule entree trop grosse est REFUSEE, pas stockee",
         !c.insert("trop", Blob{1, std::vector<uint8_t>(1001, 1), nullptr}) && c.n_refuse_too_big >= 1);
      ck("le refus n'a rien evince au passage", c.accounted_bytes() <= 1000 && c.size() > 0); }

    printf("\n=== H. PERSISTANCE DISQUE : ALLER-RETOUR ET REFUS ===\n");
    { auto m = std::make_shared<Manifest>(mkm());
      Blob b = mkb(std::vector<uint8_t>(2048, 0x5A), m);
      const std::string p = "/tmp/obrain_kvtest.bin";
      ck("save_state_file ecrit", save_state_file(p, b));
      DiskLoad L = load_state_file(p);
      ck("rechargement : fichier lu", L.ok);
      ck("rechargement : octets IDENTIQUES", L.ok && L.blob.data == b.data);
      ck("rechargement : SHA256 du manifeste concorde",
         L.ok && L.blob.manifest && L.blob.manifest->blob_sha256 == m->blob_sha256);
      // §4 : le PREFIXE EXACT doit etre persiste et restaure
      ck("rechargement : PREFIXE EXACT restaure a l identique",
         L.ok && L.blob.manifest && L.blob.manifest->prefix_ids == m->prefix_ids);
      ck("rechargement : manifeste COMPLET apres restauration",
         L.ok && L.blob.manifest && L.blob.manifest->complete());
      ck("rechargement : contexte (n_ctx) restaure",
         L.ok && L.blob.manifest && L.blob.manifest->n_ctx == m->n_ctx);
      ck("rechargement : topologie restauree",
         L.ok && L.blob.manifest && L.blob.manifest->n_layer == m->n_layer &&
         L.blob.manifest->n_embd == m->n_embd && L.blob.manifest->n_vocab == m->n_vocab);
      ck("rechargement : positions restaurees",
         L.ok && L.blob.manifest && L.blob.manifest->pos_min == m->pos_min &&
         L.blob.manifest->pos_max == m->pos_max);
      { std::string why;
        ck("rechargement avec manifeste VALIDE -> ACCEPT",
           L.ok && decide_import(L.blob, cur, why) == ImportDecision::ACCEPT);
        printf("       [diag] decision rechargement=%s\n",
               L.ok ? decision_name(decide_import(L.blob, cur, why)) : "N/A"); }
      // un manifeste RENDU incomplet (prefixe efface) doit etre refuse
      { Blob bad = L.blob; bad.manifest = std::make_shared<Manifest>(*L.blob.manifest);
        bad.manifest->prefix_ids.clear(); std::string why;
        ck("prefixe efface -> manifeste INCOMPLET -> REFUSE",
           decide_import(bad, cur, why) == ImportDecision::INCOMPATIBLE); }
      // fichier TRONQUE
      { std::ifstream fi(p, std::ios::binary); std::string all((std::istreambuf_iterator<char>(fi)), {});
        const std::string p2 = "/tmp/obrain_kvtest_trunc.bin";
        { std::ofstream fo(p2, std::ios::binary); fo.write(all.data(), (std::streamsize)(all.size() / 2)); }
        DiskLoad T = load_state_file(p2);
        ck("fichier TRONQUE -> refus au chargement", !T.ok);
        printf("       raison : %s\n", T.why.c_str()); }
      // fichier SANS manifeste
      { const std::string p3 = "/tmp/obrain_kvtest_noman.bin";
        { std::ofstream fo(p3, std::ios::binary); fo.write("OBKV", 4);
          uint32_t v = 1; fo.write((const char *) &v, 4); fo.write("END\n", 4);
          uint64_t n = 16; fo.write((const char *) &n, 8);
          std::vector<uint8_t> d(16, 0); fo.write((const char *) d.data(), 16); }
        DiskLoad N = load_state_file(p3);
        ck("fichier SANS manifeste -> refus", !N.ok); }
      // fichier ALTERE apres ecriture
      { std::string all; { std::ifstream fi(p, std::ios::binary); all.assign((std::istreambuf_iterator<char>(fi)), {}); }
        all[all.size() / 2] ^= 0xFF;   // au MILIEU : dans le payload
        const std::string p4 = "/tmp/obrain_kvtest_alt.bin";
        { std::ofstream fo(p4, std::ios::binary); fo.write(all.data(), (std::streamsize) all.size()); }
        DiskLoad A = load_state_file(p4);
        ck("fichier ALTERE : lecture ok mais SHA256 discordant", A.ok);
        if (!A.ok) printf("       [diag] raison chargement : %s\n", A.why.c_str());
        std::string why;
        const auto dA = A.ok ? decide_import(A.blob, cur, why) : ImportDecision::NO_MANIFEST;
        printf("       [diag] decision=%s why=%s\n", decision_name(dA), why.c_str());
        ck("fichier ALTERE -> import REFUSE (le refus a lieu)", dA != ImportDecision::ACCEPT);
        ck("fichier ALTERE -> refus par SHA256 (le manifeste est desormais COMPLET)",
           dA == ImportDecision::BAD_SHA256); }
    }

    printf("\n=== [SPLICE-BOUND] LECTURE DU BLOB : CE QUI DEPEND DES JETONS, ET CE QUI N'EN DEPEND PAS ===\n");
    // Blob synthetique : 4 cellules KV, 2 couches, lignes de 32 octets, AUCUNE
    // couche recurrente ecrite (modele a attention seule).
    { BlobFab f; f.entete(3, 1); f.kv_ext(4, 2, 32, 32, true);
      const BlobLayout l = blob_layout(f.d.data(), f.d.size());
      ck("blob KV seul : lu jusqu'au DERNIER octet", l.ok);
      ck("blob KV seul : 4 cellules KV (= 4 jetons)", l.attn_cells == 4);
      ck("blob KV seul : part KV = en-tetes + 4 cellules x (20 o de meta + 2 x 32 o)",
         l.attn_bytes == 12 + 4 + 4 * 20 + 8 + 2 * (12 + 4 * 32) + 2 * (12 + 4 * 32));
      ck("blob KV seul : aucune part recurrente", l.rec_bytes == 0); }
    // Cellules SANS extension MROPE (n_pos_per_embd = 1) : l'autre branche du
    // lecteur — le blob ne dit pas laquelle s'applique, seule la consommation
    // EXACTE du blob tranche.
    { BlobFab f; f.entete(3, 1); f.kv_ext(4, 2, 32, 32, false);
      const BlobLayout l = blob_layout(f.d.data(), f.d.size());
      ck("blob sans extension MROPE : reconnu (consommation exacte)", l.ok && l.attn_cells == 4);
      ck("blob sans extension MROPE : meta de 12 octets par cellule",
         l.attn_bytes == 12 + 4 + 4 * 12 + 8 + 2 * (12 + 4 * 32) + 2 * (12 + 4 * 32)); }
    // LE FAIT MESURE : la part recurrente ne bouge PAS avec le nombre de jetons.
    { BlobFab a; a.entete(3, 1); a.kv_ext(4,  2, 32, 32, true); a.recurrent(1, 8, 16, 64, 3);
      BlobFab b; b.entete(3, 1); b.kv_ext(40, 2, 32, 32, true); b.recurrent(1, 8, 16, 64, 3);
      const BlobLayout la = blob_layout(a.d.data(), a.d.size());
      const BlobLayout lb = blob_layout(b.d.data(), b.d.size());
      ck("blob hybride : les deux lus", la.ok && lb.ok);
      ck("blob hybride : les cellules KV SUIVENT les jetons", la.attn_cells == 4 && lb.attn_cells == 40);
      ck("blob hybride : la part recurrente NE SUIT PAS les jetons", la.rec_bytes == lb.rec_bytes);
      ck("blob hybride : part recurrente = 3 entrees R (16 o) + 3 entrees S (64 o)",
         la.rec_bytes == 24 + 3 * (12 + 16) + 3 * (12 + 64));
      ck("blob hybride : entrees R et S comptees separement", la.rec_layers_r == 3 && la.rec_layers_s == 3);
      printf("       4 jetons  : KV %zu o + recurrent %zu o = %zu o\n",
             la.attn_bytes, la.rec_bytes, la.attn_bytes + la.rec_bytes);
      printf("       40 jetons : KV %zu o + recurrent %zu o = %zu o\n",
             lb.attn_bytes, lb.rec_bytes, lb.attn_bytes + lb.rec_bytes); }
    // Un blob tronque d'UN octet n'est pas « presque bon » : il est refuse.
    { BlobFab f; f.entete(3, 1); f.kv_ext(4, 2, 32, 32, true); f.recurrent(1, 8, 16, 64, 3);
      const BlobLayout l = blob_layout(f.d.data(), f.d.size() - 1);
      ck("blob TRONQUE d'un octet -> ILLISIBLE (jamais accepte)", !l.ok);
      printf("       raison : %s\n", l.why.c_str()); }
    // Magic d'un autre format.
    { BlobFab f; f.entete(3, 1); f.kv_ext(4, 2, 32, 32, true);
      f.d[0] = 0x00;
      ck("magic inconnu -> ILLISIBLE", !blob_layout(f.d.data(), f.d.size()).ok); }
    // ── [obrain APPEND] FLUX VIDE DANS UN BLOB MULTI-FLUX ───────────────────
    //
    // POURQUOI CE TEST EXISTE : il reproduit un defaut REEL, trouve en mesurant la
    // route d'ajout sur un serveur a plusieurs sequences (2026-10-05). Avec
    // `--parallel 3`, `n_stream` vaut 3 et les flux des autres sequences sont
    // VIDES. L'ecrivain n'ecrit alors QUE le nombre de leurs cellules (0) et passe
    // au flux suivant ; le lecteur, lui, attendait une section de donnees apres
    // CHAQUE flux. Il se desalignait donc, et refusait un blob VALIDE en annoncant
    // « topologie d'attention invraisemblable » -- un message qui accusait le
    // blob au lieu du lecteur. Sur un serveur mono-flux (n_stream = 1) le defaut
    // n'existait pas : c'est ce qui l'a rendu invisible jusqu'ici.
    { BlobFab f; f.entete(3, 3);
      f.u32(0);                       // flux 0 : VIDE (le compte, et rien d'autre)
      f.kv_ext(4, 2, 32, 32, false);  // flux 1 : 4 cellules
      f.u32(0);                       // flux 2 : VIDE
      const BlobLayout l = blob_layout(f.d.data(), f.d.size());
      ck("multi-flux avec flux VIDES : blob lu jusqu'au dernier octet", l.ok);
      if (!l.ok) { printf("       raison : %s\n", l.why.c_str()); }
      ck("multi-flux : n_stream annonce = 3", l.n_stream == 3);
      ck("multi-flux : les cellules des flux vides ne comptent PAS", l.attn_cells == 4);
      std::string why;
      ck("multi-flux : bornage accepte (4 cellules pour 4 jetons)", extraction_bornee(l, 4, why));
      ck("multi-flux : 4 cellules pour 2 jetons -> REFUSE", !extraction_bornee(l, 2, why)); }
    // Le cas hybride reellement rencontre : un flux vide ET la section recurrente.
    { BlobFab f; f.entete(3, 2);
      f.kv_ext(4, 2, 32, 32, true);
      f.u32(0);                       // flux 1 vide
      f.recurrent(1, 8, 16, 64, 3);
      const BlobLayout l = blob_layout(f.d.data(), f.d.size());
      ck("multi-flux + recurrent : blob lu", l.ok);
      if (!l.ok) { printf("       raison : %s\n", l.why.c_str()); }
      ck("multi-flux + recurrent : part recurrente comptee", l.rec_bytes > 0 && l.rec_layers_r == 3); }
    // Ce qui doit RESTER refuse : un flux vide dans un blob tronque.
    { BlobFab f; f.entete(3, 3); f.u32(0); f.kv_ext(4, 2, 32, 32, false); f.u32(0);
      const BlobLayout l = blob_layout(f.d.data(), f.d.size() - 1);
      ck("multi-flux tronque d'un octet -> ILLISIBLE", !l.ok); }
    // LA REGLE DE BORNAGE, exercee directement (c'est elle que la route appelle).
    { BlobFab f; f.entete(3, 1); f.kv_ext(4, 2, 32, 32, true);
      const BlobLayout l = blob_layout(f.d.data(), f.d.size());
      std::string why;
      ck("4 cellules pour 4 jetons -> ACCEPTE", extraction_bornee(l, 4, why));
      ck("4 cellules pour 2 jetons -> REFUSE (extraction NON BORNEE)", !extraction_bornee(l, 2, why));
      printf("       raison : %s\n", why.c_str());
      ck("moins de cellules que de jetons -> ACCEPTE (fenetre glissante legitime)",
         extraction_bornee(l, 9, why));
      ck("blob illisible -> REFUSE quel que soit n_fact", !extraction_bornee(BlobLayout{}, 4, why)); }

    printf("\nOBRAIN_KV_CACHE_TESTS_RESULT=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("PASSED=%d FAILED=%d\n", passed, failed);
    return failed == 0 ? 0 : 1;
}
