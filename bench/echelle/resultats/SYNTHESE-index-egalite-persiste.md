# Index d'égalité persisté — vérification T7 (2026-08-30, M4 128 Go)

Le grief : `batir_index_depuis_v2` (store.rs) reconstruit les index
d'égalité PAR PROCESSUS — à l'ouverture pour toutes les clés du catalogue,
à la première requête pour les autres. Sur les bases larges de tmn :
23 minutes de démarrage, repayées à chaque processus.

La réponse : zone `substrate.props.eq.idx.v2` (module
`crates/obrain-substrate/src/props_eq_index.rs`, gabarit de
`props_index.rs`) — séries triées de couples (hachage FNV-1a 64 de la
valeur, slot), 16 o/entrée, CRC32 par page, réécriture atomique
tmp+rename, ancres de couverture (offset WAL + pages v2) posées à la
fusion (fin de `flush()`/`checkpoint()`), confrontées à l'ouverture.
Zone fiable ⇒ clés déclarées COUVERTES ⇒ zéro reconstruction ;
`indexed_candidates` lit zone ∪ delta DashMap de session et REVÉRIFIE
chaque candidat (le chemin existant) — collisions et entrées périmées ne
produisent que des faux positifs écartés. Doute (ancres divergentes, CRC
faux, version autre) ⇒ zone jetée ⇒ reconstruction comme avant.
Échappatoire : `OBRAIN_PROPS_INDEX_EGALITE=0`.

Le hachage est PAR VARIANTE d'`IndexValueKey` (pas de `hacher(&Value)`) :
l'équivalence de domaine avec l'index RAM est vraie par construction.
La clé lossy 12 o de l'index ordonné aurait été inutilisable ici : tous
les `qualified_name` d'un projet partagent leur préfixe de 11 octets.

## Mesures — base ×100 (3 767 800 nœuds), catalogue déclaré sur `name`

| Régime | requête d'égalité CLI (ouverture comprise) |
|---|---|
| AVANT (témoin sans zone, catalogue déclaré) | min 7 491 ms/processus (passe complète 3,77M chaînes à CHAQUE ouverture) |
| T7, première ouverture (paie la passe + fusionne) | 11 882 ms, une seule fois — zone écrite : 58 Mo |
| T7, régime de croisière | **min 1 533 ms** (passes : 1589/1645/1555/1684/1533) |
| Préuve tracée | `index d'égalité persisté FIABLE — zéro reconstruction par processus` |

Gain : **×4,9 par processus** ; le coût de construction est payé UNE fois
puis amorti tant que les ancres tiennent. À noter : 1 533 ms bat aussi le
régime « sans catalogue » (2 431 ms, branche label-first du planificateur).

Sur le smoke 1× (37 678 nœuds) : avant 407 ms → après 220 ms ;
zone 600 Ko ; checkpoint 0,31 s.

## Cycle de vie vérifié (traces)

- fusion → réouverture : FIABLE, génération stable, ancre == offset WAL
  (vérifié octet à octet sur la racine après chaque type de processus :
  requête, wal checkpoint).
- toute écriture depuis la fusion ⇒ ancres divergentes ⇒ NON FIABLE ⇒
  reconstruction (conservateur, jamais l'inverse) ; le processus suivant
  refusionne à sa fermeture ⇒ re-couverture. Pire cas : UN processus de
  reconstruction après une fermeture sans flush final — coût borné.
- suites : substrate --lib 745/745, engine --lib 676/676 ; 12 tests neufs
  dans props_eq_index (stabilité du hachage figée par témoins, monotonie
  des frontières de pages, ancres, corruption, non-report d'une série
  douteuse, séparation des magics avec l'index ordonné).
