// obrain-kv-cache-tests -- TESTS SYNTHETIQUES DES FONCTIONS DE PRODUCTION.
// Inclut obrain-kv-cache.h, le MEME en-tete que server-context.cpp : les regles
// testees sont celles du serveur, pas une copie simplifiee.
#include "obrain-kv-cache.h"
#include <cstdio>
#include <string>
using namespace obrain_kv;

static int failed = 0, passed = 0;
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
      ck("rechargement : prefixe exact NON persiste -> manifeste INCOMPLET",
         L.ok && L.blob.manifest && !L.blob.manifest->complete());
      { std::string why;
        ck("rechargement : import REFUSE (manifeste incomplet)",
           L.ok && decide_import(L.blob, cur, why) == ImportDecision::INCOMPATIBLE); }
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
        ck("le refus tombe sur le MANIFESTE INCOMPLET (prefixe non persiste), AVANT le SHA256",
           dA == ImportDecision::INCOMPATIBLE); }
    }

    printf("\nOBRAIN_KV_CACHE_TESTS_RESULT=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("PASSED=%d FAILED=%d\n", passed, failed);
    return failed == 0 ? 0 : 1;
}
