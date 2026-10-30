# ADDENDUM A RUN-02 -- ce qui etait prouve, et ce qui ne l'etait pas

**Artefacts RUN-02 : CONSERVES INTACTS.** Cet addendum precise leur portee.

| Element | Portee REELLE |
|---|---|
| quatre processus `verify-nominal` lances DIRECTEMENT | **PASS** |
| sequence nominale TRAVERSANT LE LANCEUR | **NOT_VERIFIED** (a l'epoque) |
| `baseline1` invoquee par le lanceur reel | **refus AVANT calcul** |
| defaut | **drapeau enfant absent** de `build_command` |

**Limite du mecanisme gele telle qu'elle etait** : `build_command` codait en dur
`--mode real-preflight` et n'offrait **aucun** mode sans calcul. Le lanceur gele
ne pouvait donc pas exercer de sequence nominale moche : les quatre processus
`verify-nominal` de RUN-02 ont ete lances **a la main**, pas par le lanceur.

**Conclusion de disponibilite du gel v2 : INVALIDEE.**
`ENGINE008_READY_FOR_PREFLIGHT_AUTHORIZATION=YES` (manifeste `1736b341...`) est
retire : le lanceur ne pouvait pas ouvrir le calcul reel.

**Identites du gel v2** (`c4fe4dd1...` harnais, `f3c32092...` lanceur) :
**HISTORIQUES**, conservees.

**OLD_AUTHORIZATIONS_REUSED=NO** -- l'autorisation RUN-02 (`6b8753c4...`)
reference l'ancienne identite de harnais et n'est pas reutilisable.
