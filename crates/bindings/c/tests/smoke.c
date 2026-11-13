/* Fumée de l'ABI C d'Obrain, bout en bout, depuis un vrai programme C :
 *   ouvre une base temporaire en écriture, INSERT + MATCH en GQL, lit
 *   obrain_result_json / obrain_result_row_count, vérifie le bail (un second
 *   écrivain est refusé), ferme, rouvre en lecture seule, libère tout.
 * Compilé et lancé par le test Rust `smoke_c` (tests/smoke_c.rs).
 * Usage : smoke <répertoire de la base> ; sortie 0 si tout est conforme. */
#include "obrain.h"

#include <stdio.h>
#include <string.h>

static int echec(const char *etape) {
    const char *msg = obrain_last_error();
    fprintf(stderr, "ECHEC %s : %s\n", etape, msg ? msg : "(sans message)");
    return 1;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage : %s <répertoire de la base>\n", argv[0]);
        return 2;
    }
    const char *chemin = argv[1];
    printf("obrain %s\n", obrain_version());

    ObrainDatabase *db = obrain_open_write(chemin);
    if (!db) return echec("obrain_open_write");

    ObrainResult *r = obrain_execute(
        db, "INSERT (:Personne {nom: 'Alix', age: 30}), (:Personne {nom: 'Gus', age: 25})");
    if (!r) return echec("INSERT");
    obrain_free_result(r);

    r = obrain_execute(db, "MATCH (p:Personne) RETURN p.nom AS nom, p.age AS age ORDER BY age");
    if (!r) return echec("MATCH");
    size_t lignes = obrain_result_row_count(r);
    const char *json = obrain_result_json(r);
    printf("lignes=%zu json=%s\n", lignes, json ? json : "(null)");
    if (lignes != 2 || !json || !strstr(json, "\"Gus\"") || !strstr(json, "\"Alix\"")) {
        fprintf(stderr, "ECHEC résultat inattendu\n");
        return 1;
    }
    obrain_free_result(r);

    /* Bail : un second écrivain sur la même base est refusé tant que le
     * premier handle vit. */
    ObrainDatabase *second = obrain_open_write(chemin);
    if (second) {
        fprintf(stderr, "ECHEC bail : second écrivain accepté\n");
        return 1;
    }
    printf("bail refusé : %s\n", obrain_last_error());

    if (obrain_close(db) != OBRAIN_STATUS_OK) return echec("obrain_close");
    obrain_free_database(db);

    /* Lecture seule : les écritures journalisées sont visibles, les mutations
     * refusées. */
    ObrainDatabase *ro = obrain_open(chemin);
    if (!ro) return echec("obrain_open (lecture seule)");
    r = obrain_execute(ro, "MATCH (p:Personne) RETURN count(p) AS n");
    if (!r) return echec("MATCH en lecture seule");
    printf("relecture json=%s\n", obrain_result_json(r));
    if (obrain_result_row_count(r) != 1 || !strstr(obrain_result_json(r), "2")) {
        fprintf(stderr, "ECHEC relecture\n");
        return 1;
    }
    obrain_free_result(r);
    r = obrain_execute(ro, "INSERT (:Personne {nom: 'Harm'})");
    if (r) {
        fprintf(stderr, "ECHEC une mutation a réussi en lecture seule\n");
        return 1;
    }
    printf("mutation refusée en lecture seule : %s\n", obrain_last_error());
    if (obrain_close(ro) != OBRAIN_STATUS_OK) return echec("obrain_close (ro)");
    obrain_free_database(ro);

    printf("SMOKE_C_OK\n");
    return 0;
}
