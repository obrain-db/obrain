// obrain-client-seq-tests -- TESTS DU REGISTRE CLIENT -> SEQUENCE (P1).
//
// Inclut obrain-client-seq.h, le MEME en-tete que server-context.cpp : les
// regles testees sont celles du serveur, pas une copie simplifiee. Le
// garde-fou « l'extraction n'efface pas le KV d'un client » ne peut pas etre
// prouve au niveau HTTP sans moteur ; ce qui EST prouvable ici, et qui le
// fonde, c'est que l'ensemble des sequences attribuees aux clients et la
// sequence reservee sont DISJOINTS -- l'extraction ne peut donc, par
// construction, purger que la sienne.
#include "obrain-client-seq.h"
#include <cstdio>
#include <string>
using namespace obrain_seq;

static int failed = 0, passed = 0;
static void ck(const char * n, bool ok) {
    printf("  %-70s %s\n", n, ok ? "PASS" : "FAIL");
    if (ok) passed++; else failed++;
}

int main() {
    printf("OBRAIN_CLIENT_SEQ_TESTS -- registre client -> sequence (P1)\n");

    printf("\n=== A. CONFIGURATION (ce que le serveur publie et protege) ===\n");
    {
        Registre r;
        r.configurer(4, 3);                       // --parallel 4, sequence reservee = 3
        ck("4 slots, reservee 3 -> 3 sequences offertes", r.slots_offerts() == 3);
        ck("la sequence 3 est reconnue RESERVEE", r.est_reservee(3));
        ck("les sequences 0..2 sont offertes", r.est_offerte(0) && r.est_offerte(1) && r.est_offerte(2));
        ck("la sequence reservee n'est PAS offerte", !r.est_offerte(3));
        ck("hors intervalle -> ni reservee ni offerte", !r.est_reservee(9) && !r.est_offerte(9) && !r.est_offerte(-1));
    }
    {
        Registre r;
        r.configurer(2, -1);                      // --parallel 2 : aucune reservation
        ck("sans reservation : offertes = n_slots", r.slots_offerts() == 2 && r.reserve() == -1);
        ck("sans reservation : est_reservee(x) est faux partout", !r.est_reservee(0) && !r.est_reservee(1));
    }
    {
        Registre r;
        r.configurer(1, 0);                       // pool vide une fois la reserve posee
        ck("pool vide apres reservation -> registre DESACTIVE", r.slots_offerts() == 0 && r.reserve() == -1);
    }
    {
        Registre r;
        r.configurer(4, 7);                       // reserve hors intervalle : refusee
        ck("reserve hors intervalle -> ignoree (aucune protection fantome)", r.reserve() == -1);
    }

    printf("\n=== B. DEUX CLIENTS -> DEUX SEQUENCES, AUCUNE RESERVEE ===\n");
    {
        Registre r;
        r.configurer(4, 3);
        const Attribution a = r.attribuer("slot:0", 0);
        const Attribution b = r.attribuer("slot:1", 1);
        const Attribution c = r.attribuer("slot:2", 2);
        ck("client 'slot:0' -> sequence 0", a.seq_id == 0);
        ck("client 'slot:1' -> sequence 1", b.seq_id == 1);
        ck("client 'slot:2' -> sequence 2", c.seq_id == 2);
        ck("deux clients DIFFERENTS -> deux sequences DISTINCTES",
           a.seq_id != b.seq_id && b.seq_id != c.seq_id && a.seq_id != c.seq_id);
        ck("AUCUNE attribution sur la sequence reservee",
           !r.est_reservee(a.seq_id) && !r.est_reservee(b.seq_id) && !r.est_reservee(c.seq_id));
        ck("3 attributions NOUVELLES comptees", r.etat().n_attrib == 3);
    }

    printf("\n=== C. STABILITE (ce qui preserve l'epinglage du prefixe) ===\n");
    {
        Registre r;
        r.configurer(4, 3);
        const int32_t s0 = r.attribuer("slot:2", 2).seq_id;
        const int32_t s1 = r.attribuer("slot:2", 2).seq_id;   // etape suivante du meme fil
        const int32_t s2 = r.attribuer("slot:2", 2).seq_id;
        ck("meme client -> MEME sequence a chaque etape", s0 == s1 && s1 == s2 && s0 == 2);
        const Etat e = r.etat();
        ck("1 seule attribution nouvelle, 2 reutilisations", e.n_attrib == 1 && e.n_reutilise == 2);
        ck("le mapping publie est stable (1 entree)", e.taille == 1 && e.mapping.size() == 1);
        ck("le mapping nomme la cle et sa sequence",
           e.mapping[0].first == "slot:2" && e.mapping[0].second == 2);
        ck("la sequence 2 est occupee par 'slot:2' (verifiable)",
           r.occupant(2) == "slot:2");
    }

    printf("\n=== D. LE CLIENT QUI VISE LA SEQUENCE RESERVEE EST RE-MAPPE ===\n");
    {
        Registre r;
        r.configurer(4, 3);
        const Attribution a = r.attribuer("slot:3", 3);      // hash(cid) % 4 == 3
        ck("le client qui demande le slot 3 NE recoit PAS 3", a.seq_id != 3);
        ck("il recoit une sequence OFFERTE", r.est_offerte(a.seq_id));
        ck("l'attribution est marquee comme RE-MAPPAGE", a.remap && a.nouvelle);
        ck("aucune victime (aucun autre client n'existait)", !a.reaffecte);
        const Attribution b = r.attribuer("slot:3", 3);      // etape suivante
        ck("le re-mappage reste STABLE d'une etape a l'autre", b.seq_id == a.seq_id);
        ck("compteur n_remap = 2 (une par requete)", r.etat().n_remap == 2);
        printf("       [diag] slot demande 3 -> sequence %d (reservee=3, offertes=%d)\n",
               a.seq_id, r.slots_offerts());
    }

    printf("\n=== E. INVARIANT : LA RESERVEE N'EST JAMAIS ATTRIBUEE, MEME SATURE ===\n");
    {
        Registre r;
        r.configurer(4, 3);
        bool jamais_reservee = true, toujours_offerte = true;
        for (int i = 0; i < 50; ++i) {                       // 50 clients distincts pour 3 sequences
            const std::string cle = "slot:" + std::to_string(i % 4) + "#" + std::to_string(i);
            const int32_t s = r.attribuer(cle, i % 4).seq_id;
            if (r.est_reservee(s)) jamais_reservee = false;
            if (!r.est_offerte(s)) toujours_offerte = false;
        }
        ck("50 attributions (pool de 3) : jamais la sequence reservee", jamais_reservee);
        ck("50 attributions : toujours une sequence offerte", toujours_offerte);
        ck("la sequence reservee reste LIBRE dans le registre", r.occupant(3).empty());
        ck("compteur de reprises LRU non nul (pool sature)", r.etat().n_reaffecte > 0);
        ck("le registre ne detient jamais plus de cles que de sequences",
           r.etat().taille <= (size_t) r.slots_offerts());
        printf("       [diag] attrib=%llu reutilise=%llu remap=%llu reaffecte=%llu taille=%zu\n",
               (unsigned long long) r.etat().n_attrib, (unsigned long long) r.etat().n_reutilise,
               (unsigned long long) r.etat().n_remap,  (unsigned long long) r.etat().n_reaffecte,
               r.etat().taille);
    }

    printf("\n=== F. SATURATION : QUI CEDE SA SEQUENCE, ET QUI NE LA CEDE JAMAIS ===\n");
    {
        // F1 -- sans oracle d'occupation (toutes les sequences censees libres) :
        // c'est la plus FROIDE qui cede.
        Registre r;
        r.configurer(4, 3);
        // 3 clients remplissent le pool, puis on RAFRAICHIT 'A' : l'ordre LRU
        // devient A (plus recente), C, B (plus froide).
        const int32_t s0 = r.attribuer("A", 0).seq_id;
        const int32_t s1 = r.attribuer("B", 1).seq_id;
        const int32_t s2 = r.attribuer("C", 2).seq_id;
        r.attribuer("A", 0);                                  // A redevient la plus recente
        // 4e client distinct : le pool est sature, la victime doit etre la plus froide.
        const Attribution d = r.attribuer("D", 3);
        ck("4e client : une AUTRE cle a perdu sa sequence", d.reaffecte);
        ck("la victime est la plus FROIDE ('B', jamais retouchee depuis)",
           d.victime == "B");
        ck("la cle fraichement TOUCHEE ('A') survit", d.victime != "A");
        ck("le 4e client reprend la sequence liberee", d.seq_id == s1);
        ck("il ne prend NI la sequence de A NI celle de C", d.seq_id != s0 && d.seq_id != s2);
        ck("aucune sequence reservee n'a ete touchee", r.occupant(3).empty() && d.seq_id != 3);
        ck("le client evince a bien PERDU son mapping", r.occupant(s1) == "D");
        ck("les clients frais gardent le leur", r.occupant(s0) == "A" && r.occupant(s2) == "C");
        ck("compteur n_reaffecte = 1", r.etat().n_reaffecte == 1);
    }
    {
        // F2 -- LA PLUS FROIDE GENERE. C'est le defaut que la premiere version
        // avait : reprendre la plus froide sans regarder si elle travaille fait
        // ROTATION entre clients actifs (chacun re-prefille tout, a chaque tour).
        // Ici la plus froide ('B') est declaree OCCUPEE : elle doit etre SAUTEE,
        // au profit de la plus froide des sequences REELLEMENT libres.
        Registre r;
        r.configurer(4, 3);
        const int32_t sA = r.attribuer("A", 0).seq_id;
        const int32_t sB = r.attribuer("B", 1).seq_id;
        const int32_t sC = r.attribuer("C", 2).seq_id;
        auto occupee = [&](int32_t s) { return s != sB; };    // tout est libre SAUF la sequence de B
        const Attribution d = r.attribuer("D", 3, occupee);
        ck("la plus froide GENERE : elle n'est PAS reprise", d.victime != "B");
        ck("le client actif garde sa sequence", r.occupant(sB) == "B");
        ck("c'est la plus froide des LIBRES qui cede ('A')", d.victime == "A" && d.seq_id == sA);
        ck("le 4e client a bien une sequence offerte", r.est_offerte(d.seq_id));
        ck("le pool reste coherent (3 cles pour 3 sequences)", r.etat().taille == 3);
        ck("C, plus recente que A, survit", r.occupant(sC) == "C");
    }
    {
        // F3 -- TOUTES ACTIVES : aucun client actif ne perd sa sequence. On rend
        // -1, ce qui laisse le serveur choisir automatiquement (il exclut DEJA la
        // sequence reservee et ne prend qu'un slot libre).
        Registre r;
        r.configurer(4, 3);
        const int32_t sA = r.attribuer("A", 0).seq_id;
        const int32_t sB = r.attribuer("B", 1).seq_id;
        const int32_t sC = r.attribuer("C", 2).seq_id;
        auto toutes_actives = [](int32_t) { return false; };
        const Attribution d = r.attribuer("D", 3, toutes_actives);
        ck("toutes les sequences sont ACTIVES -> aucune attribution (-1)", d.seq_id == -1);
        ck("le drapeau 'sature' est pose, aucune victime", d.sature && !d.reaffecte);
        ck("AUCUN client actif n'a perdu sa sequence",
           r.occupant(sA) == "A" && r.occupant(sB) == "B" && r.occupant(sC) == "C");
        ck("le registre n'a pas grandi", r.etat().taille == 3);
        ck("compteur n_sature = 1", r.etat().n_sature == 1);
        ck("toujours AUCUNE attribution sur la reservee", r.occupant(3).empty());
    }
    {
        // F4 -- FILS TERMINES : les sequences doivent etre RECYCLEES. Le chat cree
        // un cid par conversation : si le registre ne rendait jamais une sequence
        // detenue par un fil mort, l'epinglage disparaitrait pour tout le monde
        // apres les premiers fils (un oracle qui dit « inactif » est la realite
        // de toute conversation terminee).
        Registre r;
        r.configurer(4, 3);
        auto inactives = [](int32_t) { return true; };        // tout est INACTIF : reprise possible
        bool toujours_offerte = true;
        for (int i = 0; i < 20; ++i) {                        // 20 conversations successives
            const int32_t s = r.attribuer("conv:" + std::to_string(i), 0, inactives).seq_id;
            if (!r.est_offerte(s)) toujours_offerte = false;
        }
        ck("20 conversations successives : sequence toujours offerte", toujours_offerte);
        ck("le registre ne fuit pas (jamais plus de cles que de sequences)",
           r.etat().taille == (size_t) r.slots_offerts());
        ck("17 reprises de fils termines (20 - 3 sequences)", r.etat().n_reaffecte == 17);
        ck("la DERNIERE conversation est bien enregistree",
           r.occupant(r.attribuer("conv:19", 0, inactives).seq_id) == "conv:19");
    }

    printf("\n=== G. SANS RESERVATION, LE COMPORTEMENT HISTORIQUE EST INTACT ===\n");
    {
        Registre r;
        r.configurer(4, -1);
        ck("demande 3 rendue TELLE QUELLE (aucun re-mappage)", r.attribuer("slot:3", 3).seq_id == 3);
        ck("demande 0 rendue telle quelle", r.attribuer("slot:0", 0).seq_id == 0);
        ck("aucun registre tenu (carte inchangee)", r.etat().taille == 0);
        ck("aucun re-mappage compte", r.etat().n_remap == 0);
    }
    {
        Registre r;
        r.configurer(4, 3);
        ck("cle vide -> -1 (selection automatique du serveur inchangee)",
           r.attribuer("", -1).seq_id == -1);
        ck("cle vide ne consomme AUCUNE sequence", r.etat().taille == 0);
    }

    printf("\n=== H. DEMANDE HORS INTERVALLE (convention de get_slot_by_id) ===\n");
    {
        Registre r;
        r.configurer(4, 3);
        // Le serveur faisait `id_slot % slots.size()` : on reproduit la meme
        // convention, donc 7 vise la sequence 3 -- la reservee. Le registre doit
        // la re-mapper, pas la rendre.
        const Attribution a = r.attribuer("slot:7", 7);
        ck("id_slot 7 (= 3 modulo 4) NE recoit PAS la reservee", a.seq_id != 3);
        ck("id_slot 7 recoit une sequence offerte", r.est_offerte(a.seq_id));
        const Attribution b = r.attribuer("slot:9", 9);       // 9 % 4 = 1, offerte
        ck("id_slot 9 (= 1 modulo 4) recoit la sequence 1", b.seq_id == 1);
        ck("id_slot 9 n'est pas vu comme un re-mappage", !b.remap);
    }

    printf("\n=== I. COHERENCE ENTRE occupant() ET LES ATTRIBUTIONS ===\n");
    {
        Registre r;
        r.configurer(5, 4);
        const int32_t s = r.attribuer("cid:abc", -1).seq_id;   // identite seule, sans preference
        ck("client_id seul (sans id_slot) recoit une sequence offerte", r.est_offerte(s));
        ck("son occupant est la cle 'cid:abc'", r.occupant(s) == "cid:abc");
        ck("les trois autres sequences offertes sont LIBRES",
           r.occupant(1).empty() && r.occupant(2).empty() && r.occupant(3).empty());
        ck("la sequence reservee (4) est libre", r.occupant(4).empty());
        ck("occupant(hors intervalle) est vide", r.occupant(99).empty());
    }

    printf("\nOBRAIN_CLIENT_SEQ_TESTS_RESULT=%s\n", failed == 0 ? "PASS" : "FAIL");
    printf("PASSED=%d FAILED=%d\n", passed, failed);
    return failed == 0 ? 0 : 1;
}
