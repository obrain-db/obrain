# CARTOGRAPHIE DU CHEMIN RÉEL DE L'ENGINE — RÉSULTAT D'INSPECTION

**Aucune supposition : tout ce qui suit est lu dans les sources, avec numéros de
ligne.** Le fork qualifié : `/home/triviere/projects/llama-obrain-cuda-f1a`.

---

## 1. LE CHEMIN DE REQUÊTE RÉEL EST `tools/server/server-context.cpp`

**Ce que j'ai d'abord écarté, et pourquoi :**
- `tools/obrain/` ne contient **qu'un seul fichier**, `pstate-exp0a.cpp` (194 Ko),
  et **aucun binaire `obrain` n'existe** dans `build-cuda/bin/`. Ce n'est **pas**
  le chemin de requête : c'est un outil d'expérience.
- Les autres fichiers `*obrain*` du fork sont des utilitaires
  (`src/obrain-rotation.cpp`, `tools/server/obrain-*.h`, tests) : **pas** un
  moteur de requête.

**Le chemin réel est le serveur `llama-server`**, dont l'implémentation vit dans
`tools/server/server-context.cpp` (~7 000 lignes) et se construit en
`libllama-server-impl.so` (présent dans `build-cuda/bin/`).

**Et ce chemin contient DÉJÀ l'intégration K1/K2 du transfert d'état hybride.**

## 2. L'INTÉGRATION EXISTANTE, LUE LIGNE À LIGNE

### Le cache
```cpp
// server-context.cpp:62-65
// [obrain K1/K2] états K/V extraits (contexte 100 % dispo, injection ultra-rapide).
struct kv_state_blob { uint32_t n_tokens = 0; std::vector<uint8_t> data; };
static std::mutex g_kv_states_mtx;
static std::unordered_map<std::string, kv_state_blob> g_kv_states;
```
Un **cache mémoire global**, protégé par mutex, indexé par `std::string`.

### La clé de cache — **content-addressed**
```cpp
// server-context.cpp:66-71
static std::string kv_ref_of(const std::vector<uint8_t> & data) {
    uint64_t h = 0xcbf29ce484222325ULL;
    for (uint8_t b : data) { h ^= b; h *= 0x100000001b3ULL; }   // FNV-1a 64 bits
    char buf[17]; snprintf(buf, sizeof buf, "%016llx", (unsigned long long) h);
    return std::string(buf);
}
```

### L'EXTRACTION (K1) — `server-context.cpp:3680-3700`
```cpp
if (kv_extract) {
    // K1 : extraire l'état complet (KV+recurrent) du fait.
    const size_t blob_size = llama_state_seq_get_size(ctx, seq_id);
    std::vector<uint8_t> data(blob_size);
    if (blob_size == 0 || llama_state_seq_get_data(ctx, data.data(), blob_size, seq_id) != blob_size) {
        // -> erreur "failed to extract fact state", aucune entree de cache
    }
    const std::string ref = kv_ref_of(data);
    { std::lock_guard<std::mutex> lk(g_kv_states_mtx);
      g_kv_states[ref] = kv_state_blob{(uint32_t) n_fact, data}; }
    // K1b : persistance disque
}
```
Activé par `task.jspace_extract_only` (`:3597`), exposé en JSON par
`body.value("extract_only", false)` (`:6844`).

### L'INJECTION (K2) — `server-context.cpp:3722-3752`
```cpp
} else {
    // K2 : injecter l'état persisté (copie mémoire, pas de prefill).
    kv_state_blob blob;
    { std::lock_guard<std::mutex> lk(g_kv_states_mtx);
      auto it = g_kv_states.find(task.jspace_state_ref);
      if (it != g_kv_states.end()) blob = it->second; }
    if (blob.data.empty()) {
        // K1b : fallback disque
        std::ifstream f(task.jspace_kv_dir + "/" + task.jspace_state_ref + ".bin", std::ios::binary);
        if (!f) { /* "unknown state_ref ... (extract d'abord)" */ }
        uint32_t hdr = 0; f.read((char *) &hdr, 4);
        std::vector<uint8_t> payload(...);
        blob = kv_state_blob{hdr, std::move(payload)};
        g_kv_states[task.jspace_state_ref] = blob;
    }
    const size_t rc = llama_state_seq_set_data(ctx, blob.data.data(), blob.data.size(), seq_id);
    if (rc == 0) { /* "failed to load state (set_data)" */ }
    n_fact = (int32_t) blob.n_tokens;
}
```
**Le commentaire du producteur dit littéralement « copie mémoire, pas de
prefill ».** Le paramètre JSON est `state_ref` (`:6864`) :
`task.jspace_state_ref = body.value("state_ref", std::string());`.

## 3. LA CLÉ ACTUELLE SATISFAIT-ELLE L'EXIGENCE §2 ?

L'instruction exige une clé liant **au minimum** : identité du modèle et du
calcul Q1 · IDs exacts du préfixe et longueur · configuration de contexte ·
types KV et mode de sérialisation · format/version du blob · positions et
convention de séquence.

**Verdict : OUI, parce que la clé est dérivée du BLOB LUI-MÊME.**

Tous ces éléments sont **à l'intérieur** du blob sérialisé : le magic
`0xaf143cd9` porte la **version de format**, les sections attention et récurrente
portent les **types et les cellules** (donc les positions), la longueur du
préfixe détermine le contenu, et le contenu dépend du **modèle** et de la
**configuration de contexte** (types KV, mode `OBRAIN_STATE_TYPE`, `n_ctx`,
`n_batch`, `seq_id`). Deux états compatibles au sens de §2 ont donc le **même
blob**, donc la **même clé** — et toute incompatibilité produit un blob
différent, donc un **miss**.

**C'est une identité content-addressed, et c'est la forme la plus stricte
disponible sans inventer un schéma parallèle.** Le tokenizer, le template et le
rendu n'entrent que par les **IDs** qu'ils produisent : c'est correct, car c'est
la seule chose qui entre dans le calcul.

## 4. LES DEUX DÉFAUTS RÉELS

### DÉFAUT 1 — **aucune borne mémoire, aucune éviction** (le trou concret)
`g_kv_states` est un `std::unordered_map` **sans limite et sans éviction** :
`g_kv_states.erase` **n'apparaît nulle part** dans `server-context.cpp`, et aucune
constante de capacité n'existe. Chaque extraction ajoute une entrée de
**160 697 016 octets** (mesuré). **Une poignée d'extractions suffit à épuiser la
mémoire**, et rien ne le signale : la seule issue est l'OOM du processus.

L'instruction exige explicitement : « Prévoir une limite explicite de mémoire et
une éviction. » — **c'est ce qui manque, et c'est ce que j'implémente.**

### DÉFAUT 2 — **clé FNV-1a 64 bits, non résistante aux collisions**
`kv_ref_of` utilise FNV-1a 64 bits, **non cryptographique**. Sur des blobs de
160 Mo, une collision accidental n'est pas exclue, et une collision **servirait
le mauvais état** : la destination recevrait un état qui n'est pas le sien, et
`set_data` **réussirait** (le blob serait bien formé). Aucun contrôle
d'intégrité ne rattraperait l'erreur.

L'instruction dit : « Toute compatibilité inconnue interdit le hit. » Un hachage
non résistant **présume** la compatibilité au lieu de l'établir.

**Et sur le chemin disque**, le blob est lu **sans vérification d'intégrité** :
le nom de fichier est la clé, mais **rien ne confirme que le contenu correspond**
à cette clé. Un fichier altéré serait chargé et injecté.

## 5. CE QUI RESTE À FAIRE POUR CLORE L'INTÉGRATION
1. Implémenter la **borne mémoire + éviction** sur `g_kv_states`.
2. **Vérifier l'intégrité** du blob au chargement depuis le disque (recalcul de
   la clé et comparaison), et **refuser** en cas de discordance.
3. **Vérifier que la clé recalculée correspond** à celle demandée, sur les deux
   chemins (mémoire et disque).
4. Tests couvrant : hit exact, miss sur divergence, blob tronqué ou altéré,
   `PARTIAL_ONLY` refusé, retour d'import nul ou partiel, libération du contexte
   après échec, aucun decode du préfixe sur hit, aucune entrée après export échoué.
5. Smoke test borné par le chemin serveur : `extract_only` puis `state_ref`.
