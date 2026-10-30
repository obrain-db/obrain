// obrain-sha256-substitution-tests -- COMPARAISON reference vs OpenSSL.
// Les empreintes doivent rester STRICTEMENT identiques.
#include "obrain-sha256.h"
#include <algorithm>
#include <chrono>
#include <cstdio>
#include <fstream>
#include <string>
#include <vector>
using namespace obrain_sha;
static int bad = 0, ok = 0;
static void ck(const char * n, bool p, const std::string & a = "", const std::string & b = "") {
    printf("  %-56s %s\n", n, p ? "PASS" : "FAIL"); if (p) ok++; else { bad++;
        if (!a.empty()) printf("       ref %s\n       new %s\n", a.c_str(), b.c_str()); }
}
static double ms(std::chrono::steady_clock::time_point t) {
    return std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t).count(); }

int main(int argc, char ** argv) {
    printf("OBRAIN_SHA256_SUBSTITUTION_TESTS\n");
    printf("implementation active : %s\n",
#ifdef OBRAIN_USE_OPENSSL_SHA256
        "OpenSSL"
#else
        "REFERENCE (OpenSSL non active)"
#endif
    );
    printf("\n=== A. VECTEURS CONNUS : ref == new ===\n");
    struct V { const char * n; std::string s; };
    std::vector<V> vs = {
        {"vide", ""}, {"abc", "abc"},
        {"56 octets", "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"},
        {"1 000 000 x a", std::string(1000000, 'a')},
    };
    for (auto & v : vs) {
        const std::string a = hex_of_ref((const uint8_t *) v.s.data(), v.s.size());
        const std::string b = hex_of((const uint8_t *) v.s.data(), v.s.size());
        ck(v.n, a == b, a, b);
    }
    printf("\n=== B. FRONTIERES DE BLOCS (0..200 octets) ===\n");
    { bool all = true; std::string first_bad;
      for (size_t n = 0; n <= 200; ++n) {
          std::vector<uint8_t> d(n); for (size_t i = 0; i < n; ++i) d[i] = (uint8_t)(i * 31 + 7);
          const std::string a = hex_of_ref(d), b = hex_of(d);
          if (a != b) { all = false; if (first_bad.empty()) first_bad = "n=" + std::to_string(n); }
      }
      ck("0..200 octets : identiques", all, first_bad); }
    printf("\n=== C. BINAIRE ET TAILLES PARTICULIERES ===\n");
    { std::vector<uint8_t> b(256); for (int i = 0; i < 256; ++i) b[i] = (uint8_t) i;
      ck("binaire 0x00..0xFF", hex_of_ref(b) == hex_of(b), hex_of_ref(b), hex_of(b)); }
    for (size_t n : {64u, 55u, 56u, 63u, 65u, 119u, 120u, 4096u, 100000u}) {
        std::vector<uint8_t> d(n); for (size_t i = 0; i < n; ++i) d[i] = (uint8_t)(i % 251);
        ck(("taille " + std::to_string(n)).c_str(), hex_of_ref(d) == hex_of(d));
    }
    printf("\n=== D. LECTURE FRAGMENTEE DE FICHIER ===\n");
    { const char * p = "/tmp/shaperf_blk.bin";
      std::vector<uint8_t> d(3 * 1024 * 1024 + 137);
      for (size_t i = 0; i < d.size(); ++i) d[i] = (uint8_t)(i * 13);
      { std::ofstream f(p, std::ios::binary); f.write((const char *) d.data(), (std::streamsize) d.size()); }
      const std::string mem = hex_of(d);
      for (size_t blk : {1u << 20, 7u, 65536u, 4096u}) {
          const std::string a = hex_of_file_ref(p, blk), b = hex_of_file(p, blk);
          ck(("blocs de " + std::to_string(blk) + " : ref==new==memoire").c_str(),
             a == b && b == mem, a, b);
      } }
    printf("\n=== E. BLOB EXACT DE L'EXPERIENCE (160697024 octets) ===\n");
    { const char * p = "/tmp/exp12_blob.bin";
      std::ifstream f(p, std::ios::binary | std::ios::ate);
      if (!f) { printf("  %-56s SKIP (blob absent)\n", "blob present"); }
      else {
          const size_t n = (size_t) f.tellg(); f.close();
          printf("  blob trouve : %zu octets\n", n);
          const std::string a = hex_of_file_ref(p), b = hex_of_file(p);
          ck("blob de l'experience : ref == new", a == b, a, b);
          ck("blob == 7a40fc6b9f270a87... (journal BENCH-03, 160697016 octets)",
             a == "7a40fc6b9f270a87053ca6a352b68a2cbf83401b945b62f396eb70147cf34832", a); } }
    printf("\n=== F. FICHIER DE MODELE ===\n");
    if (argc > 1) {
        const std::string m = argv[1];
        auto t = std::chrono::steady_clock::now();
        const std::string a = hex_of_file_ref(m);
        const double ta = ms(t);
        t = std::chrono::steady_clock::now();
        const std::string b = hex_of_file(m);
        const double tb = ms(t);
        ck("modele : ref == new", a == b, a, b);
        printf("       ref %.1f ms | new %.1f ms | gain x%.2f\n", ta, tb, tb > 0 ? ta / tb : 0.0);
    } else printf("  (modele non fourni)\n");
    printf("\n=== G. BANC §4 : 5 mesures chacune, ORDRE ALTERNE PREENREGISTRE ===\n");
    printf("  ordre : R/N, N/R, R/N, N/R, R/N  (R=reference, N=OpenSSL)\n");
    { const char * bp = "/tmp/exp12_blob.bin";
      std::ifstream bf(bp, std::ios::binary | std::ios::ate);
      if (!bf) printf("  blob absent : banc ignore\n");
      else {
          const size_t bsz = (size_t) bf.tellg(); bf.close();
          std::vector<double> R, N; std::string hr, hn; bool ident = true;
          for (int i = 0; i < 5; ++i) {
              const bool r_first = (i % 2 == 0);
              auto run_R = [&]{ auto t0 = std::chrono::steady_clock::now();
                                std::string h = hex_of_file_ref(bp); double d = ms(t0);
                                R.push_back(d); if (hr.empty()) hr = h;
                                else if (h != hr) ident = false; };
              auto run_N = [&]{ auto t0 = std::chrono::steady_clock::now();
                                std::string h = hex_of_file(bp); double d = ms(t0);
                                N.push_back(d); if (hn.empty()) hn = h;
                                else if (h != hn) ident = false; };
              if (r_first) { run_R(); run_N(); } else { run_N(); run_R(); }
              printf("    paire %d (%s) : R=%.1f ms  N=%.1f ms\n", i + 1,
                     r_first ? "R/N" : "N/R", R.back(), N.back());
          }
          auto med = [](std::vector<double> v) { std::sort(v.begin(), v.end()); return v[v.size()/2]; };
          printf("  OLD_BLOB_SHA256_MEDIAN_MS=%.1f\n", med(R));
          printf("  NEW_BLOB_SHA256_MEDIAN_MS=%.1f\n", med(N));
          printf("  GAIN_MEDIAN=x%.2f\n", med(N) > 0 ? med(R)/med(N) : 0.0);
          printf("  TAILLE_BLOB=%zu octets\n", bsz);
          printf("  DEBIT_APPARENT_ref=%.0f Mo/s  new=%.0f Mo/s  (debit de HACHAGE, pas bande passante physique)\n",
                 bsz / 1e6 / (med(R)/1000.0), bsz / 1e6 / (med(N)/1000.0));
          ck("les 10 empreintes du banc sont IDENTIQUES entre elles", ident, hr, hn);
      } }
    printf("\nOBRAIN_SHA256_SUBSTITUTION_RESULT=%s\nPASSED=%d FAILED=%d\n", bad == 0 ? "PASS" : "FAIL", ok, bad);
    return bad == 0 ? 0 : 1;
}
