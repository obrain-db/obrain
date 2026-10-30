/**
 * LE RACCORD D'UNE RÉPONSE REPRISE — fonctions PURES, donc vérifiables.
 *
 * Défaut mesuré le 2026-09-13 (chat du 8432, reprise après coupure du service) :
 * on redonne au modèle la réponse DÉJÀ affichée en lui demandant de continuer.
 * Deux comportements observés, dans la même journée, sur la même question :
 *
 *   1. il CONTINUE exactement — « …16, 17, 1 » puis « 8, 19, 20 » (le compte
 *      reprend en plein « 18 » et finit) : c'est le cas attendu ;
 *   2. il RECOMMENCE la réponse — « 1, 2, 3, … 20 » — et le raccord collé à
 *      l'écran donne « …16, 17, 1, 1, 2, 3, … » : un doublon, qui se lit comme
 *      un défaut du logiciel alors que le modèle a simplement régénéré.
 *
 * La consigne envoyée au modèle interdit la répétition, mais on ne compte pas sur
 * elle : ces deux fonctions rendent le raccord DÉTERMINISTE dans les deux cas.
 * Elles ne touchent jamais à du texte qui n'a pas déjà été lu : la première ne
 * retire qu'un préfixe déjà présent à la fin de l'affichage, la seconde ne
 * remplace que si la suite se présente comme une réponse ENTIÈRE (son début
 * répète le début de l'analyse déjà affichée).
 */

/** Bornes de décision : au-delà, on ne cherche plus de recouvrement. */
const RECOUVREMENT_MAX = 240;
/** En dessous, un recouvrement n'est pas significatif (« 1 » n'est pas un doublon). */
const RECOUVREMENT_MIN = 12;
/** Longueur du témoin de début comparé pour reconnaître une réponse RÉGÉNÉRÉE. */
const TEMOIN = 24;

/** Normalise pour comparer : espaces écrasés, comparaison insensible à la casse. */
const normaliser = (s: string) => s.replace(/\s+/g, " ").trim().toLowerCase();

/**
 * Le plus long préfixe de `suite` qui est DÉJÀ la fin de `deja`, retiré.
 *
 * C'est le cas « le modèle répète ses derniers mots avant de continuer ». On ne
 * retire rien quand le recouvrement est court : « 1 » ou « la » se répètent
 * légitimement dans une phrase.
 */
export function sansRecouvrement(deja: string, suite: string): string {
  const max = Math.min(RECOUVREMENT_MAX, deja.length, suite.length);
  for (let n = max; n >= RECOUVREMENT_MIN; n--) {
    if (deja.endsWith(suite.slice(0, n))) return suite.slice(n);
  }
  return suite;
}

/**
 * La suite est-elle une RÉGÉNÉRATION de la réponse (elle repart du début) ?
 *
 * On compare le témoin de tête à la version normalisée : « 1, 2, 3, 4, 5 » contre
 * « 1, 2, 3, 4, 5 » — l'espacement du modèle ne doit pas faire échouer la
 * reconnaissance, sinon le doublon s'afficherait.
 */
export function estRegeneration(deja: string, suite: string): boolean {
  const a = normaliser(deja).slice(0, TEMOIN);
  const b = normaliser(suite).slice(0, TEMOIN);
  return a.length >= RECOUVREMENT_MIN && b.length >= RECOUVREMENT_MIN && a === b;
}

/**
 * Le raccord à appliquer, décidé en un seul endroit :
 *   - `remplace` : la suite est une réponse entière régénérée → elle REMPLACE le
 *     texte affiché (le montrer deux fois serait un doublon, le garder tronqué
 *     serait mentir sur ce que le modèle a produit) ;
 *   - `ajout` : le texte à AJOUTER à ce qui est déjà affiché.
 */
export function raccorder(
  deja: string,
  suite: string,
): { remplace: boolean; ajout: string } {
  if (estRegeneration(deja, suite)) return { remplace: true, ajout: suite };
  return { remplace: false, ajout: sansRecouvrement(deja, suite) };
}

/** Au-delà de cette longueur, on ne patiente plus pour décider du raccord. */
export const ATTENTE_RACCORD = 200;
