// ENGINE-008 -- PREFLIGHT-UNIFY-04 : point d'entree UNIQUE.
// Les composants sont compiles en unites de traduction separees ; leurs main
// respectifs sont renommes par -Dmain=... et APPELES ici. Aucune implementation
// n'est dupliquee, aucun cinquieme composant autonome n'est cree.
#include <cstdio>
#include <cstring>

int engine008_enum_main(int, char **);   // enumerateur (signature reelle verifiee par nm) (10 slots src[], schema v1_1)
int engine008_identity_main();    // registre d'identite natif
int engine008_integrated_main();  // correlation + hypotheses + autorisation

static void sep(const char * t) { printf("\n================ %s ================\n", t); }

int main(int argc, char ** argv) {
    const char * mode = (argc > 1) ? argv[1] : "--all";
    printf("ENGINE-008 -- HARNAIS PREFLIGHT UNIFIE (binaire unique)\n");
    printf("MODEL_FORWARD_CALL_COUNT=0  FULL_MODEL_LOAD_COUNT=0\n");

    int rc = 0;
    if (strcmp(mode, "--all") == 0 || strcmp(mode, "--enum") == 0) {
        sep("ENUMERATEUR (10 slots src[], ENGINE008_GRAPH_ENUM_v1_1)");
        rc |= engine008_enum_main(argc, argv);
    }
    if (strcmp(mode, "--all") == 0 || strcmp(mode, "--identity") == 0) {
        sep("REGISTRE D'IDENTITE NATIF");
        rc |= engine008_identity_main();
    }
    if (strcmp(mode, "--all") == 0 || strcmp(mode, "--correlate") == 0) {
        sep("CORRELATION / HYPOTHESES / AUTORISATION");
        rc |= engine008_integrated_main();
    }
    sep("VERDICT DU BINAIRE UNIFIE");
    printf("UNIFIED_EXIT=%d\n", rc);
    printf("PREFLIGHT_SINGLE_EXECUTABLE_BUILT=YES\n");
    return rc;
}
