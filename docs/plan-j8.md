# NATIF-J8 — Garde ORDER BY / min-max consciente de l'epoque de l'index

## Doctrine
« Tout doit etre instantane » — MAIS **la justesse prime sur la vitesse,
toujours**. Mieux vaut le repli balayage (lent, jamais faux) qu'un index perime
qui rendrait un resultat faux.

## Le probleme (mesure)
La route `ORDER BY n.<prop> LIMIT k` via index ordonne-valeur
(`try_plan_order_by_index`, `project.rs`) et le fast-path `min`/`max`
(`try_plan_minmax_shortcut`, `aggregate.rs`) engageaient l'index
`substrate.props.ord.idx.v2` des que :

    has_pending_writes() == false   (+ vue au present, pour ORDER BY)

Or `has_pending_writes()` (== `attente_noeuds_active`) n'est arme QUE par les
noeuds d'une transaction explicite ouverte. Une ecriture **auto-commit**
(`TransactionId::SYSTEM` — le mode de tout write hors `BEGIN`/`COMMIT`, dont les
bridges writers) ne l'arme JAMAIS (store.rs, la garde `if transaction_id !=
TransactionId::SYSTEM`). Elle commite pourtant immediatement et fait avancer
l'offset WAL.

Cote index, le drapeau `fiable` (props_ord_index.rs) n'est (re)confronte qu'a
**l'ouverture** (`confronter_index_ord_valeur`) et (re)pose FRAIS qu'aux
**flush/checkpoint** (`fusionner_index_ord_valeur`). Entre deux flush, il reste
fige. Le chemin de requete (`pk_ord_valeur_pret`) faisait confiance a ce
`fiable` CACHE.

Consequence : apres une ecriture auto-commit non encore flushee, la route
servait un index en RETARD sur la requete → **resultat faux**.

## Preuve du bug (contre-epreuve sur base 2a3b402d)
Test `crates/obrain-engine/tests/orderby_epoque_j8.rs`, lance sur la BASE, AVANT
correction :

- `correction_nouveau_min_sous_ecriture` : apres `INSERT (:N {v:0})` auto-commit
  (nouveau min), `... ORDER BY n.v ASC LIMIT 1` rendait **1** (ancien min de
  l'index perime) au lieu de **0**. ROUGE.
- `correction_min_supprime_sous_ecriture` : apres suppression du min (v=1),
  la meme requete rendait **Null** (slot du noeud supprime encore pointe par
  l'index). ROUGE — pire qu'une valeur perimee : une ligne fantome.
- `instant_quand_a_jour`, `instant_apres_flush`, `non_regression_sans_index` :
  VERTS (le regime instantane et le repli sans index etaient deja corrects).

## La correction — garde consciente de l'epoque
L'epoque de l'index EST son couple d'ancres `(ancre_wal, ancre_pages)` : l'offset
WAL et le nombre de pages v2 au moment de sa derniere fusion. L'etat committe
courant est `(writer.wal().offset(), props_zone.allocated_page_count())`.

> Regle : engager l'index SI ET SEULEMENT SI ses ancres correspondent ENCORE a
> l'etat committe courant. Toute ecriture commitee depuis la derniere fusion
> fait diverger les ancres → l'index est en retard sur la requete → repli
> balayage OBLIGATOIRE.

C'est l'analogue exact du `viewing_epoch >= epoque_index` de la mission, mais sur
la granularite fine et EXACTE deja portee par le format (offset WAL + pages),
plutot qu'un compteur d'epoque separe qu'il aurait fallu ajouter.

### Deux etages, defense en profondeur
1. **Filet de securite (store.rs `pk_ord_valeur_pret`)** : re-confronte
   FRAICHEMENT les ancres a CHAQUE requete, en lecture seule, au lieu de faire
   confiance au `fiable` cache. Rend `None` des que l'ancre diverge. Ce seul
   changement rend `min`/`max` ET `ORDER BY` surs, quel que soit l'etage
   planificateur, car TOUS les accesseurs ordonnes passent par lui.
2. **Garde planificateur explicite (project.rs, aggregate.rs)** : nouvelle
   methode de trait `index_ord_valeur_couvre_courant()` (defaut `false`,
   surchargee par substrate) appelee dans la garde, pour un repli PRECOCE et une
   intention lisible.

### Ce qui est CONSERVE, et pourquoi
- `has_pending_writes()` : GARDE. Une ecriture non commitee d'une transaction
  ouverte est visible a SA PROPRE session (MVCC) mais absente de l'index (bati
  sur disque). Les ancres ne divergent pas forcement (l'ecriture peut n'etre que
  tamponnee en memoire), donc la seule confrontation d'ancres ne suffirait pas :
  on garde ce garde pour ne jamais rendre a une session un index amputee de ses
  propres ecritures en vol.
- `viewing_epoch != current_epoch()` (time-travel) : GARDE pour ORDER BY,
  AJOUTE pour min/max. Un index bati au present contient des lignes nees APRES
  l'epoque visee ; le servir a une vue passee serait faux. Repli obligatoire.

La garde finale, pour les deux routes :

    if has_pending_writes()                          // ecritures en vol
       || viewing_epoch != current_epoch()           // time-travel
       || !index_ord_valeur_couvre_courant()         // index en retard (LE FIX)
    { return None; }  // repli balayage, jamais faux

## Contrat de tests (`orderby_epoque_j8.rs`)
- **Correction sous ecriture** (le plus important) : nouveau min insere ; min
  existant supprime — sans rebati d'index. Resultat toujours CORRECT.
- **Instant quand a jour** : index frais, vue au present → 0-slot (mesure par
  `compteurs::mesurer_slots`).
- **Instant apres flush** : ecriture puis `checkpoint()` (hook J6) → index
  re-engage, 0-slot, resultat correct incluant l'ecriture.
- **Non-regression** : base sans index → balayage, tri complet correct.

## Livre vs differe
- LIVRE : garde consciente de l'epoque via confrontation d'ancres (ORDER BY +
  min/max), filet `pk_ord_valeur_pret`, ajout du garde time-travel a min/max,
  contrat de tests adversaire vert.
- DIFFERE (pas necessaire) : aucun compteur d'epoque separe n'a ete ajoute — les
  ancres `(wal, pages)` du format v2 le portaient deja, exactement et
  gratuitement. Aucune maintenance incrementale de l'index sur le chemin
  d'ecriture (hors sujet ; la re-synchro reste au flush/checkpoint J6). Aucune
  clause de re-essai/attente de flush (non prouvee sure, et le repli balayage
  suffit a la justesse).
