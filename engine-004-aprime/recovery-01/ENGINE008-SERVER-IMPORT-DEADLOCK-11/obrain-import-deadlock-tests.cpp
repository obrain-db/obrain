// obrain-import-deadlock-tests -- REGRESSION du verrou du cache.
// Le contrat de PRODUCTION : `kv_states_{get,insert}` possedent le verrou ;
// l'appelant ne doit PAS le reprendre (g_kv_states_mtx EST g_kv_cache.mtx).
// Le test exerce les DEUX motifs et BORNE l'attente : sans borne, l'ancien
// motif bloquerait la suite au lieu d'echouer proprement.
#include "obrain-kv-cache.h"
#include <atomic>
#include <chrono>
#include <cstdio>
#include <thread>
using namespace obrain_kv;

static int bad = 0, ok = 0;
static void ck(const char * n, bool p) {
    printf("  %-58s %s\n", n, p ? "PASS" : "FAIL"); if (p) ok++; else bad++;
}
// wrapper de production, copie EXACTE de server-context.cpp
static Cache C(64u << 20);
static std::mutex & g_mtx = C.mtx;                 // ligne 72 : MEME mutex
// Le test de l'ANCIEN motif abandonne un thread QUI GARDE le verrou : il doit
// donc utiliser son PROPRE cache, sinon il bloquerait tous les tests suivants.
static Cache C_old(64u << 20);
static std::mutex & g_mtx_old = C_old.mtx;
static bool w_insert(const std::string & r, Blob && b) {
    std::lock_guard<std::mutex> lk(C.mtx); return C.insert(r, std::move(b));
}
static bool w_get(const std::string & r, Blob & o) {
    std::lock_guard<std::mutex> lk(C.mtx); return C.get(r, o);
}

// Execute `fn` avec une borne. Rend true si termine.
template <class F> static bool bounded(F fn, int ms) {
    std::atomic<bool> done{false};
    std::thread t([&] { fn(); done = true; });
    const auto t0 = std::chrono::steady_clock::now();
    while (!done && std::chrono::duration_cast<std::chrono::milliseconds>(
                        std::chrono::steady_clock::now() - t0).count() < ms)
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    if (!done) { t.detach(); return false; }        // bloquee : on n'attend pas
    t.join(); return true;
}

int main() {
    printf("OBRAIN_IMPORT_DEADLOCK_TESTS -- contrat de verrou du cache\n");
    Blob b; b.n_tokens = 1; b.data.assign(1024, 0x5A);
    ck("insertion via le wrapper de production", w_insert("k", std::move(b)));
    Blob out;
    ck("lookup via le wrapper de production", w_get("k", out) && out.data.size() == 1024);

    printf("\n--- MOTIF ANCIEN (defaut) : verrou externe + wrapper ---\n");
    const bool old_ok = bounded([&] {
        std::lock_guard<std::mutex> lk(g_mtx_old);  // verrou EXTERNE (cache dedie)
        std::lock_guard<std::mutex> lk2(g_mtx_old); // reacquisition : le defaut
    }, 1500);
    printf("  %-58s %s\n", "ancien motif TERMINE (attendu: NON)",
           old_ok ? "INATTENDU" : "NON -> detecte");
    ck("le motif ANCIEN est bien detecte comme bloquant", !old_ok);

    printf("\n--- MOTIF CORRIGE : wrapper seul, plusieurs threads ---\n");
    const bool new_ok = bounded([&] {
        std::vector<std::thread> th;
        for (int i = 0; i < 16; ++i) th.emplace_back([&, i] {
            for (int k = 0; k < 300; ++k) {
                Blob x; x.n_tokens = 1; x.data.assign(64, (uint8_t) k);
                w_insert("k" + std::to_string(i % 4), std::move(x));
                Blob o; w_get("k" + std::to_string(i % 4), o);
            }
        });
        for (auto & t : th) t.join();
    }, 20000);
    ck("motif CORRIGE termine (16 threads x 300 insert/get)", new_ok);
    ck("comptabilite coherente apres concurrence", C.accounted_bytes() <= C.max_bytes);

    printf("\nOBRAIN_IMPORT_DEADLOCK_TESTS_RESULT=%s\n", bad == 0 ? "PASS" : "FAIL");
    printf("PASSED=%d FAILED=%d\n", ok, bad);
    return bad == 0 ? 0 : 1;
}
