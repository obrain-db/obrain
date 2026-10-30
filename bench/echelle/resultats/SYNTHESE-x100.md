# Banc à l'échelle ×100 — T7 (2026-08-30, M4 128 Go, binaire tronc)

Corpus : carto-dump (37 678 nœuds / 246 934 arêtes) amplifié ×100 par
`gen_xn.py` → 3 767 800 nœuds / 24 693 400 arêtes, ids renumérotés CONTIGUS,
labels/propriétés/degrés conservés. JSONL ×100 : 3,8 Go.

## Load et empreinte

| Mesure                   | 1×        | ×100        | Facteur |
|--------------------------|-----------|-------------|---------|
| data load (réel)         | 2,13 s    | 1 048,2 s   | ×492 (~×4,9 au-delà du linéaire) |
| RSS pic du load          | 395,7 Mo  | 15,62 Go    | ×39,5   |
| Empreinte mémoire pic    | —         | 31,5 Go     |         |
| Base sur disque          | 257 Mo    | 17 Go       | ×66     |

Le WAL monte à ~3,3 Go pendant le load puis est auto-tronqué en fin
(checkpoint) : −4,2 Go, `substrate.wal` final = 40 octets. La croissance
détaillée est dans `croissance-load-x100.log` (t, Mo) et
`croissance-fichiers-x100.log` (par zone).

Profil du load (`sample`, phases nœuds/arêtes) :
- phase nœuds : 67 % du fil principal dans `set_node_property`, dont 26 %
  dans le syscall `write` de `WalWriter::append` (wal_io.rs:123 — un write(2)
  NON bufferisé par enregistrement, sous `Mutex<File>`) et ~24 % dans
  `crc32fast` (rescellement d'une page 4 Ko entière par valeur chaîne
  internée — props_zone.rs:717 `intern_bytes`) ;
- phase arêtes : ~81 % du fil principal dans `write` (2 write par arête :
  WAL + zone).
Le load est mono-cœur et I/O-lié (CPU 7-27 % d'un cœur).

## Les 5 formes de requêtes (CLI complet = ouverture + requête, minima)

1× : minima sur 7 passes ; ×100 : minima sur 7 passes, cache chaud.
Justesse : les 5 formes rendent exactement 100× les comptes de 1×.

| Forme                       | 1× (ms) | ×100 (ms) | Facteur |
|-----------------------------|---------|-----------|---------|
| q1 lookup nom               | 123     | 20 931    | ×170    |
| q2 appelants 1 saut         | 126     | 3 955     | ×31     |
| q3 COUNT label              | 110     | 545       | ×5      |
| q4 ORDER BY DESC LIMIT      | 128     | 3 599     | ×28     |
| q5 scan filtré (start_line) | 133     | 85 994    | ×647    |

## LE POINT DE DÉCROCHAGE

**q5 (scan filtré par comparaison) dégrade le plus : ×647.** Ensuite q1
(lookup par égalité, ×170). q3 reste quasi plat (×5 : bitmap de labels,
sous-linéaire par rapport aux ×100 nœuds).

Cause, profilée (échantillon q1 : 88 % des échantillons du fil) :
sans index déclaré, le planificateur prend la branche « label-first » de
`try_plan_filter_with_property_index`
(obrain-engine/src/query/planner/lpg/filter.rs:1307) et HYDRATE CHAQUE NŒUD
du label entier — `get_node_at_epoch` → `noeud_brut` (store.rs:2826) →
`read_all_properties_for_node` (store.rs:8885) →
`PropsZone::collect_live_props_by_key` — soit 1 154 800 traversées de
chaînes de pages PAR INVOCATION, sans pushdown de la seule propriété testée.
q5 emprunte le même moteur de traversée (`balayer_v2_par_predicat`,
store.rs:1613 : « le balayage est repayé à CHAQUE appel »).

Le pendant égalité est le grief tmn : `batir_index_depuis_v2` (store.rs:1677),
appelé à l'ouverture pour toutes les clés du catalogue (store.rs:4510) et à la
première requête par clé (store.rs:10656) — passe complète PAR PROCESSUS,
jamais persistée. → step f4fb348f (index d'égalité persisté).

Croisement avec la note PO 0a00b306 (revue moteur 2026-08-17) : voir la note
PO T7 finale — (a) confirmé à 88 %, (d) confirmé (write(2) sous mutex WAL
26-81 %), (b) INFIRMÉ sur le tronc actuel (WAL auto-tronqué), (c)/(e) non
exercés par ces formes.
