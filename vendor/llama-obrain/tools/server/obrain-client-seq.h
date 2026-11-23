// obrain-client-seq.h -- [obrain P1] REGISTRE CLIENT -> SEQUENCE.
//
// ═══════════════════════════════════════════════════════════════════════════
// LE DEFAUT QUE CE FICHIER FERME, ET POURQUOI IL EST SILENCIEUX
// ═══════════════════════════════════════════════════════════════════════════
//
// `jspace_reserved_slot_id()` (tools/server/server-context.cpp) reserve LA
// DERNIERE sequence (`n_parallel - 1`) aux taches J-space : c'est la sequence
// sur laquelle l'extraction d'etat fait `llama_memory_seq_rm(...)` avant de
// decoder le fait qu'elle veut extraire. Le commentaire qui l'accompagne
// affirme que cette sequence n'est « never touched by normal completion
// traffic ».
//
// CETTE AFFIRMATION EST FAUSSE DES QU'UN CLIENT DEMANDE UN SLOT.
// L'exclusion de la sequence reservee n'existe que dans les DEUX boucles de
// SELECTION AUTOMATIQUE de `get_available_slot` (similarite de prefixe, puis
// LRU). Le chemin `task.id_slot != -1` -- celui qu'emprunte TOUT client qui
// epingle sa conversation -- appelle `get_slot_by_id()`, qui ignore la
// reserve : un client qui demande le slot `n_parallel - 1` L'OBTIENT.
//
// Et le chat obrain demande exactement cela : `id_slot = hash(cid) %
// total_slots` (wt-chat-kv-splice, `api/src/chat/inference.rs`, `ChatSlot`),
// `total_slots` etant publie par la route `/props` comme `params.n_parallel`.
// Avec `--parallel 4`, un fil sur quatre tombe donc sur la sequence reservee.
//
// CONSEQUENCE, ET C'EST LE CŒUR DU P1 : le fil s'installe sur cette sequence
// (son KV y vit, `slots[3].prompt` croit son prefixe en cache), puis la
// PREMIERE extraction J-space efface ce KV -- et elle a le droit de le faire,
// c'est SA sequence par construction. La requete suivante du fil ne re-decode
// rien (le serveur croit le prefixe present) et lit un cache VIDE : reponse
// fausse, sans erreur, sans journal, de facon reproductible.
//
// ═══════════════════════════════════════════════════════════════════════════
// CE QUE LE REGISTRE GARANTIT
// ═══════════════════════════════════════════════════════════════════════════
//
//  1. Le serveur n'offre JAMAIS la sequence reservee a un client. Chaque
//     client (identifie par son `client_id` s'il en fournit un, sinon par le
//     `id_slot` qu'il demande -- la seule identite dont dispose le serveur
//     aujourd'hui) recoit une sequence NON RESERVEE.
//  2. L'attribution est STABLE : la meme cle garde la meme sequence tant
//     qu'elle est utilisee. C'est ce qui preserve l'epinglage du prefixe, qui
//     est la raison d'etre de `id_slot` (sans stabilite, chaque etape
//     re-prefillerait tout le contexte).
//  3. Un client qui demande la sequence reservee est RE-MAPPE (il garde un
//     epinglage, simplement pas sur la sequence interdite), et non pas ignore :
//     l'ignorer le ferait retomber dans la selection LRU du serveur, donc
//     perdre son prefixe a chaque etape -- un correctif qui coute le benefice
//     de la fonctionnalite n'est pas un correctif.
//  4. `slots_offerts()` est le nombre a publier comme `total_slots` : l'espace
//     de hachage du client (`hash(cid) % total_slots`) coincide alors avec le
//     pool reel, et le cas de saturation ne se produit plus pour un client
//     conforme.
//
// POURQUOI UN EN-TETE SEPARE ET PAS DU CODE DANS server-context.cpp : meme
// doctrine que obrain-kv-cache.h -- le serveur ET les tests incluent le MEME
// code, donc les tests n'exercent pas une copie simplifiee de la politique
// d'attribution. Une seule implementation, donc pas de « regle du test » a
// cote de la « regle du serveur ».
//
// NOTE D'ETANCHEITE : le pool offert a `n_slots - 1` elements pour au plus
// `n_slots` cles distinctes (le hachage du client a `n_slots` valeurs
// possibles). Le principe des tiroirs interdit une injection : au moins deux
// cles se partagent une sequence. C'est la raison de la politique de saturation
// ci-dessous, et du point 4 (publier `slots_offerts` supprime le probleme pour
// tout client conforme).
#pragma once

#include <algorithm>
#include <cstdint>
#include <functional>
#include <list>
#include <mutex>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

namespace obrain_seq {

// Resultat d'une attribution : ce que le serveur a le droit de faire, et
// pourquoi. Les drapeaux servent au journal et aux compteurs d'observation --
// jamais a modifier la decision, qui est deja prise ici.
struct Attribution {
    int32_t     seq_id    = -1;    // sequence SERVEUR a utiliser (-1 = aucune contrainte)
    bool        nouvelle  = false; // premiere attribution pour cette cle
    bool        remap     = false; // la sequence rendue differe de celle DEMANDEE
    bool        reaffecte = false; // une AUTRE cle (INACTIVE) a perdu sa sequence
    bool        sature    = false; // pool sature ET toutes les sequences actives : rien attribue
    std::string victime;           // cle qui a perdu sa sequence (si reaffecte)
};

// Etat publie (journal, /props). Copie, donc lisible sans tenir le verrou.
struct Etat {
    int32_t  n_slots       = 0;
    int32_t  reserve       = -1;
    int32_t  slots_offerts = 0;
    uint64_t n_attrib      = 0;   // attributions NOUVELLES (premier contact d'une cle)
    uint64_t n_reutilise   = 0;   // hits : la cle avait deja sa sequence
    uint64_t n_remap       = 0;   // sequence rendue != sequence demandee
    uint64_t n_reaffecte   = 0;   // reprises d'une sequence detenue par une cle INACTIVE
    uint64_t n_sature      = 0;   // refus d'attribution : toutes les sequences etaient ACTIVES
    size_t   taille        = 0;   // cles actuellement attribuees
    std::vector<std::pair<std::string, int32_t>> mapping; // cle -> sequence (trie)
};

class Registre {
public:
    // La configuration n'est connue qu'APRES la creation des slots : elle est
    // donc posee separement de la construction. `configurer` REMET LE REGISTRE
    // A ZERO : elle decrit l'etat initial, elle ne le corrige pas. L'appeler au
    // demarrage (une fois les slots connus) est le seul usage correct.
    void configurer(int32_t n_slots, int32_t reserve) {
        std::lock_guard<std::mutex> lk(mtx_);
        n_slots_ = std::max<int32_t>(0, n_slots);
        reserve_ = (reserve >= 0 && reserve < n_slots_) ? reserve : -1;
        // Sans sequence offerte, le registre ne peut rien garantir : il se
        // DESACTIVE (comportement historique) plutot que d'attribuer la
        // sequence reservee ou une sequence inexistante.
        if (n_slots_ - (reserve_ >= 0 ? 1 : 0) <= 0) {
            n_slots_ = 0;
            reserve_ = -1;
        }
        attr_.clear();
        lru_.clear();
        pris_.assign((size_t) n_slots_, std::string());
        n_attrib_ = n_reutilise_ = n_remap_ = n_reaffecte_ = n_sature_ = 0;
    }

    int32_t n_slots() const       { std::lock_guard<std::mutex> lk(mtx_); return n_slots_; }
    int32_t reserve() const       { std::lock_guard<std::mutex> lk(mtx_); return reserve_; }
    int32_t slots_offerts() const { std::lock_guard<std::mutex> lk(mtx_); return offerts(); }

    bool est_reservee(int32_t seq) const {
        std::lock_guard<std::mutex> lk(mtx_);
        return reserve_ >= 0 && seq == reserve_;
    }

    bool est_offerte(int32_t seq) const {
        std::lock_guard<std::mutex> lk(mtx_);
        return offerte(seq);
    }

    // ATTRIBUTION. `cle` vide => aucune identite => selection automatique du
    // serveur, inchangee (-1). `demande` = le slot reclame par le client
    // (-1 = aucune preference numerique, cas d'un `client_id` seul).
    //
    // POURQUOI LA SEQUENCE DEMANDEE EST RENDUE TELLE QUELLE QUAND AUCUNE
    // RESERVATION N'EST ACTIVE : sans reservation (`--parallel < 3`), il n'y a
    // rien a proteger et le comportement doit rester BIT A BIT celui d'avant.
    // Un correctif qui change le comportement d'une configuration qui n'a pas
    // le defaut est une regression qui s'ignore.
    // `seq_utilisable` : ORACLE D'OCCUPATION fourni par l'appelant -- « cette
    // sequence est-elle libre MAINTENANT ? ». Le registre ne connait pas les
    // slots (il ne depend pas de llama.h), donc l'etat vivant lui est INJECTE
    // plutot que duplique : une seule politique, exercee par le serveur ET par
    // les tests. Sans oracle (test), toute sequence est censee libre.
    using OccupationFn = std::function<bool(int32_t)>;

    Attribution attribuer(const std::string & cle, int32_t demande,
                          const OccupationFn & seq_utilisable = OccupationFn()) {
        Attribution a;
        if (cle.empty()) {
            return a;                       // aucune identite : rien a attribuer
        }
        std::lock_guard<std::mutex> lk(mtx_);
        auto libre_maintenant = [&](int32_t s) {
            return !seq_utilisable || seq_utilisable(s);
        };

        if (reserve_ < 0 || offerts() <= 0) {
            a.seq_id = (demande >= 0) ? demande : -1;
            return a;                       // fonctionnalite NON activee
        }

        auto it = attr_.find(cle);
        if (it != attr_.end()) {
            toucher(cle);
            n_reutilise_++;
            a.seq_id = it->second;
            a.remap  = (demande >= 0) && (a.seq_id != normalise(demande));
            if (a.remap) n_remap_++;
            return a;
        }

        // ── ATTRIBUTION NOUVELLE ────────────────────────────────────────────
        // Preference = la sequence que le client visait, SI elle est offerte.
        // Un client qui visait la sequence RESERVEE n'a aucune preference
        // utilisable : on lui donnera la plus froide, jamais la reservee.
        const int32_t d    = normalise(demande);
        const int32_t pref = offerte(d) ? d : -1;

        int32_t seq = -1;
        if (pref >= 0 && libre(pref)) {
            seq = pref;                     // sa preference est libre : la lui donner
        } else {
            // Balayage DETERMINISTE des sequences offertes, a partir de la
            // preference : la meme suite de demandes donne toujours la meme
            // carte (un test peut donc l'affirmer, et un incident se rejoue).
            // Le balayage ne considere QUE des sequences offertes : c'est ce
            // qui rend la sequence reservee inatteignable par construction --
            // un `pris_[reserve]` resterait vide a jamais et la donnerait.
            const int32_t n = offerts();
            int32_t s = (pref >= 0) ? pref : premiere_offerte();
            for (int32_t k = 0; k < n && seq < 0; ++k) {
                if (offerte(s) && libre(s)) { seq = s; break; }
                s = suivante(s);
            }
        }

        if (seq < 0) {
            // ── POOL SATURE : PLUS DE CLIENTS DISTINCTS QUE DE SEQUENCES ────
            // Toutes les sequences offertes sont prises par d'autres cles. C'est
            // le cas d'un serveur qui recoit plus de conversations epinglees que
            // de sequences utilisables (le principe des tiroirs, cf. l'en-tete),
            // ou d'un client qui hache encore modulo l'ANCIEN `total_slots`.
            //
            // ON REPREND LA SEQUENCE DE LA CLE LA PLUS FROIDE -- MAIS SEULEMENT
            // SI ELLE EST INACTIVE. POURQUOI CE « SEULEMENT », ET CE QU'IL
            // CORRIGE (defaut trouve en tracant ma PREMIERE version, qui
            // reprenait la plus froide sans condition) : avec N clients actifs
            // sur un pool de N-1 sequences, la plus froide est toujours celle
            // qui vient de demander le moins recemment -- donc la reprise
            // faisait ROTATION. Chaque client perdait sa sequence (et re-prefillait
            // tout son contexte) a chaque tour, y compris les clients qui
            // n'avaient rien demande. Une rotation qui detruit le benefice de la
            // fonctionnalite pour TOUT LE MONDE est pire que le refus d'un seul.
            //
            // DEUXIEME PIEGE, symetrique : ne jamais reprendre une sequence a une
            // cle INACTIVE rendrait le registre inutilisable des la premiere
            // poignee de conversations terminees (le chat cree un cid par fil) --
            // les sequences resteraient attribuees a des fils morts et les fils
            // VIVANTS n'auraient plus jamais d'epinglage. C'est pourquoi la
            // reprise est reservee aux cles inactives : les fils termines
            // liberent, les fils en cours de generation sont intouchables.
            //
            // Si AUCUNE sequence n'est repriseable (toutes actives), on rend -1 :
            // le client supplementaire retombe sur la selection AUTOMATIQUE du
            // serveur, qui exclut deja la sequence reservee et ne choisit qu'un
            // slot libre (`is_processing`). Il perd son epinglage -- il n'y avait
            // physiquement pas de sequence pour lui --, jamais sa justesse.
            std::string victime;
            for (const auto & k : lru_) {          // front = plus recente : on garde la DERNIERE eligible
                auto v = attr_.find(k);
                if (v == attr_.end())              continue;
                if (!libre_maintenant(v->second))  continue;
                victime = k;
            }
            if (!victime.empty()) {
                auto v = attr_.find(victime);
                seq = v->second;
                if (seq >= 0 && seq < n_slots_) pris_[(size_t) seq].clear();
                attr_.erase(v);
                for (auto it = lru_.begin(); it != lru_.end(); ++it) {
                    if (*it == victime) { lru_.erase(it); break; }
                }
                a.reaffecte = true;
                a.victime   = victime;
                n_reaffecte_++;
            } else {
                n_sature_++;
                a.sature = true;
                return a;                      // seq_id reste -1 : selection automatique
            }
        }

        if (seq < 0) {
            // Ne peut arriver qu'avec un pool vide, ecarte par `configurer`.
            // On le dit : rendre -1 fait retomber sur la selection automatique
            // du serveur, qui exclut deja la sequence reservee.
            return a;
        }

        attr_[cle]         = seq;
        pris_[(size_t) seq] = cle;
        lru_.push_front(cle);
        n_attrib_++;
        a.seq_id   = seq;
        a.nouvelle = true;
        a.remap    = (demande >= 0) && (seq != d);
        if (a.remap) n_remap_++;
        return a;
    }

    // La cle qui occupe cette sequence, ou "" si elle est libre. Sert au
    // garde-fou de l'extraction : « la sequence reservee appartient-elle a un
    // client ? » doit avoir une reponse VERIFIABLE, pas une deduction.
    std::string occupant(int32_t seq) const {
        std::lock_guard<std::mutex> lk(mtx_);
        if (seq < 0 || seq >= n_slots_) return std::string();
        return pris_[(size_t) seq];
    }

    Etat etat() const {
        std::lock_guard<std::mutex> lk(mtx_);
        Etat e;
        e.n_slots       = n_slots_;
        e.reserve       = reserve_;
        e.slots_offerts = offerts();
        e.n_attrib      = n_attrib_;
        e.n_reutilise   = n_reutilise_;
        e.n_remap       = n_remap_;
        e.n_reaffecte   = n_reaffecte_;
        e.n_sature      = n_sature_;
        e.taille        = attr_.size();
        e.mapping.reserve(attr_.size());
        for (const auto & kv : attr_) e.mapping.push_back(kv);
        std::sort(e.mapping.begin(), e.mapping.end());
        return e;
    }

private:
    int32_t offerts() const { return n_slots_ - (reserve_ >= 0 ? 1 : 0); }

    bool offerte(int32_t seq) const {
        return seq >= 0 && seq < n_slots_ && seq != reserve_;
    }

    bool libre(int32_t seq) const {
        return offerte(seq) && seq < (int32_t) pris_.size() && pris_[(size_t) seq].empty();
    }

    // Le slot reclame par le client peut sortir de l'intervalle : `get_slot_by_id`
    // du serveur faisait deja un modulo, on reproduit exactement la meme
    // convention pour ne pas changer la sequence visee par un client existant.
    int32_t normalise(int32_t demande) const {
        if (demande < 0 || n_slots_ <= 0) return -1;
        return demande % n_slots_;
    }

    int32_t premiere_offerte() const {
        for (int32_t s = 0; s < n_slots_; ++s) if (offerte(s)) return s;
        return -1;
    }

    // Sequence offerte suivante, en ordre croissant circulaire.
    int32_t suivante(int32_t s) const {
        if (n_slots_ <= 0) return s;
        int32_t x = (s < 0) ? 0 : s;
        for (int32_t k = 0; k < n_slots_ + 1; ++k) {
            x = (x + 1) % n_slots_;
            if (offerte(x)) return x;
        }
        return s;
    }

    void toucher(const std::string & cle) {
        for (auto it = lru_.begin(); it != lru_.end(); ++it) {
            if (*it == cle) { lru_.erase(it); break; }
        }
        lru_.push_front(cle);
    }

    // `mutable` : les accesseurs d'observation (etat, slots_offerts) sont
    // const et pris depuis les threads HTTP (/props) pendant que le thread de
    // boucle attribue. Le verrou est INDISPENSABLE ici : sans lui, /props
    // lirait le registre en cours de modification.
    mutable std::mutex mtx_;
    int32_t n_slots_ = 0;
    int32_t reserve_ = -1;
    std::unordered_map<std::string, int32_t> attr_;  // cle -> sequence
    std::vector<std::string>                 pris_;  // sequence -> cle ("" = libre)
    std::list<std::string>                   lru_;   // front = plus recemment attribuee
    uint64_t n_attrib_ = 0, n_reutilise_ = 0, n_remap_ = 0, n_reaffecte_ = 0, n_sature_ = 0;
};

} // namespace obrain_seq
