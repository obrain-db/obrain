# ENGINE008_ADMISSIBILITY_MATRIX (preflight)

Colonnes : **preuve statique** deja acquise · **observation future** requise ·
**couverture necessaire** · verdict **autorise** · verdict **INCOMPLETE / NOT_CONFIRMED**.

| # | Propriete exigee | Preuve statique acquise | Observation future | Couverture necessaire | Verdict autorise si | INCOMPLETE / NOT_CONFIRMED si |
|---|---|---|---|---|---|---|
| P1 | Chemin **standard** qwen35 vs MTP | **OUI** : `build_arch_graph` (qwen35.cpp:129-134) selectionne `graph` ou `graph_mtp` selon `params.gtype` ; les consommateurs de `layer.nextn.*` sont tous dans `graph_mtp` (:535-565) | `ctx_type` et `embeddings_nextn` transmis | aucune (statique) | — | — |
| P2 | **NextN non utilise** dans le graphe soumis | **OUI, conditionnel** a A1/A2/A3 | A1/A2 acceptes + preuve de soumission reussie | soumissions reussies | les 3 conditions ci-dessous reunies | l'une des conditions manque -> `NOT_CONFIRMED` |
| P3 | **Rotation candidate** | non | WHT sur le candidat | fenetre de capture valide, ledger VALID | rotation detectee, couverture suffisante | ledger ABSENT/EMPTY/PARTIAL/MALFORME |
| P4 | **Propriete negative de rotation baseline** | non | WHT sur la baseline | idem | rotation absente AVEC couverture non nulle | couverture nulle -> `NOT_CONFIRMED` (jamais « absente ») |
| P5 | **Arret et limite de generation** | **OUI** : 7 controles du contrat d'arret, EOS/8/EOS-en-8/aucun-decode-apres/erreur/egalite/argmax | ids generes reels | 1 run | ids et raison d'arret coherents | logits invalides, erreur de decode |
| P6 | **Attribution des graphes aux soumissions** | **PARTIELLE** : correlation et confrontation implementees et testees ; `n_nodes` absent du ledger (Z2) ; snapshots non exhaustifs par construction (Z3) | fenetre reelle | **exhaustivite NON obtenable** de la surface snapshots | — | toute enumeration partielle -> `INCOMPLETE` |
| P7 | **Identite du calcul et des entrees** | **OUI** : SHA binaires/bibliotheques/modeles/template/protocole | — | — | identites concordent | toute divergence |
| P8 | **Collecte WHT reellement raccordee** | **NON VERIFIEE** | — | — | raccordement verifie dans le code final | **etat actuel** |

## Methode de preuve NextN (P2) -- les DEUX formes, explicitement distinguees

**Forme 1 -- conclusion fondee sur une enumeration EXHAUSTIVE.**
Exigerait que chaque soumission soit enumeratee. **NON OBTENABLE** avec l'acces
actuel : la surface snapshots est un dump a dernier ecrivain gagnant (Z3), et
`f1a_last_gf` ne conserve que la DERNIERE soumission. Cette forme est donc
**ECARTEE par le protocole**, pas contournee.

**Forme 2 -- conclusion CONDITIONNELLE**, adoptee :
1. valeurs de contexte **transmises et acceptees** (`ctx_type`, `embeddings_nextn`) ;
2. **dispatch statique verifie** du fork qualifie (`qwen35.cpp:129-134`) ;
3. **preuve de soumission et de succes** des operations concernees.

Portee exacte : « sous ces conditions, les operations NextN pertinentes ne sont
pas incluses dans ce graphe ».
**INTERDIT d'ecrire** « absence directement observee dans tous les graphes ».
`gtype` reste `GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING` : aucune surface
native ne l'expose. `h_nextn` n'est PAS un temoin d'operation MTP (le
constructeur standard produit cet etat du tronc, `qwen35.cpp:225-226`).

## Z4 -- preenregistrement (non exige comme resultat deja obtenu)

Critere de succes de la validation de l'observateur sur modele reel : le ledger
est ecrit, VALID, avec au moins un `COMPUTE_SUBMIT` d'`rc` nul, et le snapshot
correspondant est joignable.
Arret de la sequence en cas d'echec : **un defaut d'observation apres calcul
n'autorise NI un PASS automatique, NI des repetitions supplementaires non
autorisees.** La sequence s'arrete et le point est rapporte.
