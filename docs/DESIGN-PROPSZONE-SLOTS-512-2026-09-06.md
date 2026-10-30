# DESIGN — PropsZone v2 « unités 512 o » (option B de la RFC propszone-pages-partagees)

**Date** : 2026-09-06 · branche `feat/zorder` · HEAD `9f29d3df` · **aucun code — design d'architecture**.
RFC mère : `docs/rfc/substrate/propszone-pages-partagees.md`. Cartographie layout : rapport de lecture (session) — page 4K/header 24 B/CRC/chaînage par owner, tête U48 = page u32 (16 bits hauts libres), frontière live/padding par recherche binaire sur le magic, RMW 4K par append, pas de compaction, `SUBSTRATE_FORMAT_VERSION = 1`.

## 0. Objectif et périmètre

Remplacer l'unité d'allocation **4K par début de chaîne** et le **RMW 4K par append** par des **unités de 512 o (slots)** logées 7 par page 4K (l'unité 0 du 4K sert d'en-tête de carrier) (« carrier »), sans casser la lecture des pages v2 existantes ni exiger de réécrire les données des bases actuelles. Cible : workloads à nœuds frais + props (CREATE, imports) — remplissage 0,45 % → ~3 %, 1ʳᵉ prop ~5,7 → ~3,5-4 µs (est.), append chaude ~0,8 → ~0,5 µs (est.).

## 1. Modèle d'adressage

- **Carrier** = page 4K de la zone (`substrate.props.v2`), alignée à 4096. Deux sortes : carrier **légacy** (exactement les pages actuelles, magic NODE/EDGE) et carrier **packé** (nouveau magic ; unité 0 = contrôle, 7 slots de 512).
- **Unité logique** (« page » du point de vue des chaînes) = soit un carrier légacy entier (lisible tel quel), soit un slot 512 dans un carrier packé. Une chaîne peut **mélanger** les deux via `next_page` : son adresse est une **UnitéAddr**.
- **UnitéAddr (48 bits, dans les U48 existants)** :
  - bit 47 = 1 → slot packé : bits 46..4 = index du carrier (carrier ≤ 2^43−1), bits 3..0 → numéro de slot (0..7) — en réalité slot = 3 bits (bits 3..1), bit 0 réservé/alignement. Ex. `(1<<47) | ((carrier as u64) << 4) | slot`.
  - bit 47 = 0 → carrier légacy (comportement actuel : valeur = page_idx u32 ; **compatibilité totale** : toutes les adresses existantes ont les 16 bits hauts à 0).
  - `0` = pas de chaîne (inchangé).
  - Les records (32/36 o) ne changent **pas** : `first_prop_off`/`next_page` restent des U48.
- **Header de slot (512 o)** : même gabarit que le header 4K, réduit : magic packé (par slot OU par carrier, cf. §2), `node_id u32` (owner), `crc32 u32`, `next_page U48`, `entry_count u16`, `free_offset u16`, `tombstones u16` = 24 o → payload 488 o (~40 entrées Int64 par slot). CRC **par slot** (512 o), re-scellé à chaque append du slot.

## 2. Magic, frontière, ouverture

- **Contrainte dure** : `compute_next_page_idx` (recherche binaire) compte « live » tout slot 4K dont les 4 premiers octets sont un magic connu ; un magic inconnu = padding (un vieux binaire réallouerait par-dessus). Deux magics 4K : NODE `0xF5075FA6`, EDGE `0xE5075FA6` (page.rs:16/21).
- **Décision de magic** : le carrier packé porte en tête de 4K un magic **nouveau** `0xC5A2_50C8` (« CArrier Slots 512 ») + header carrier (48 o : magic, nb_slots_utilises u8, padding, crc carrier de contrôle — le CRC de chaque slot restant l'autorité de contenu). La recherche binaire inclut le nouveau magic dans `VALID_MAGICS` (dans le NOUVEAU binaire). Le numéro de slot maximal utilisé du carrier est maintenu par le carrier header (écrit à chaque allocation de slot).
- **Gouvernance (inchangée depuis la RFC)** : un vieux binaire ne connaissant pas `0xC5A2_50C8` le compterait comme padding → **toute base écrite par le nouveau binaire doit être verrouillée contre les vieux binaires** : bump `SUBSTRATE_FORMAT_VERSION` (2) + refus d'ouverture (file.rs:939) + `obrain-migrate` pour les bases existantes à ouvrir ensuite… **OU** garde « carriers packés seulement sur zones créées par le nouveau binaire » + échec d'ouverture explicite des vieux binaires (message migration). **Aucune réécriture de données n'est requise** : les carriers légacy restent lisibles pour toujours (l'unité légacy est une UnitéAddr à bit 47 = 0).

## 3. Allocation

- **Nouveau début de chaîne** (1ʳᵉ prop d'un owner) : allouer un slot dans le **carrier packé ouvert** courant (celui dont `nb_slots_utilises < 8 (unités 1..=7)`), sinon créer un carrier packé (grow ×1,5 inchangé, mmap). L'owner-stamp du slot = owner. → 7 petits chaînages partagent un 4K (densité ×7 sur le motif CREATE).
- **Append sur chaîne existante** : unité de tête (legacy OU slot) : RMW de l'unité (512 si slot : memcpy 2×512 + CRC512) ; si pleine → nouvelle unité chaînée : **slot** si le slot courant se remplit ET un slot libre existe dans le carrier ouvert, sinon carrier packé neuf, sinon carrier légacy (stratégie : préférer slot ; un chaînage volumineux finit en carrier légacy 4K pour limiter la profondeur de chaîne) — `next_page` = UnitéAddr de l'ancienne tête.
- **Frontière d'ouverture** : recherche binaire sur les magics 4K (carriers légacy + packés) ; à l'intérieur du dernier carrier, `nb_slots_utilises` du carrier header donne le curseur (jamais d'allocation dans un carrier partiel antérieur — l'ordre reste monotone). Un slot append à moitié écrit (crash) a un CRC invalide → ignoré au walk ; `nb_slots_utilises` (header carrier, CRC de contrôle) fait autorité.
- **Reads bulk/sweeps** (`tetes_de_chaines_v2_impl`, index, relevés) : itèrent les UnitéAddr comme aujourd'hui les page_idx (helper `decode_unite` commun) — un seul point d'encodage/décodage (remplace `encode_page_id`/`decode_page_id`, props_zone.rs:97-105).

## 4. Lecteurs / walkers (impact minimal)

- `walk_chain`/`collect_live_props_by_key`/`get_latest_for_key` (props_zone.rs:571-739) ne connaissent que « magic + next_page » : la généralisation = une fonction `lire_unite(addr) -> (header, &[u8] payload)` qui sert soit un carrier légacy entier (4096−24), soit un slot (512−24). Le filtrage par magic/owner actuel est inchangé ; aucune entrée ne porte d'owner (l'owner reste celui de la chaîne — propriété du header d'unité).
- Vérifs lecture : magic seulement (comme aujourd'hui) ; CRC vérifié par les outils/audit/tests uniquement.
- `read_all_properties_for_*` (fusion 4 passes) et le store (`lookup_*_v2`) ne changent que via le helper d'adresse.

## 5. Stamps owner & remap (leçons P0 intégrées)

- Le slot porte `node_id` = owner (comme aujourd'hui) → le **risque remap s'applique aux slots** : le fix P0 (`d7f1a303` live + `57e185f1` rejeu) doit marcher sur les **Unités** (slots ET carriers légacy) — c.-à-d. `rafraichir_owners_de_chaine`/`restamper_apres_rejeu` itèrent des UnitéAddr.
- Alternative structurelle à trancher au design : **supprimer l'owner-stamp** et rendre la garde d'append inutile (l'append ne se fait QUE dans l'unité de tête de la chaîne du record — l'unité est atteinte par l'adresse du record, pas par un slot libre) : la garde « page d'un autre propriétaire » servait à détecter les têtes parasites du régime hérité (19/08) ; sur bases finalisées elle ne protège plus que le remap-stale — qu'on répare désormais. Si l'owner-stamp disparaît des slots, plus aucun refresh remap nécessaire. **Décision : garder l'owner-stamp (audit/outils) et conserver les fixes P0** — coût faible, filet conservé.

## 6. Rejeu / WAL / checkpoint (inchangés de principe)

- `update_*_first_prop_off` (WAL-first, 6 o) et le replay des têtes (replay.rs:1229) restent identiques (ils manipulent l'U48 brut).
- `HilbertRepermuteV2`/`CompactCommunityV2` + auto-guérison (57e185f1) inchangés, adaptés aux Unités.
- `flush`/`msync`/checkpoint : la liste des zones à msync inclut le carrier packé (même fichier `substrate.props.v2`) — rien de nouveau. Exception THP : le fichier reste hors HUGEPAGE (file.rs:287-289).
- `compute_next_page_idx` (recherche binaire) : nouveau magic ajouté à la liste des magics vivants ; l'audit d'ancres post-rejeu (replay.rs:234-323) étend sa règle : une UnitéAddr slot est valide si carrier < frontière ET slot < nb_slots_utilises du carrier (sinon « dangling » compté).

## 7. Compaction (reportée, cohérente)

Pas de compaction de contenu (tombstones accumulés, chaînes append-only) — inchangé ; le chantier compaction reste séparé (futur). Le shrink du padding de fin de fichier (compact CLI) s'applique pareil (frontière par carrier).

## 8. Tests / DoD (repris de la RFC)

- Nouveaux : codecs UnitéAddr (roundtrip, discriminator, bornes) ; carrier packé (magic, nb_slots_utilises, allocation monotone, crash mid-slot → slot CRC invalide ignoré) ; chaînes mixtes (legacy ↔ slot) ; densité (7 chaînages/carrier) ; walkers/LWW/tombstones sur slots ; kill/regime_v2 sur slots ; remap sur slots (fix P0 étendu) ; migration/ouverture (garde version) ; audit d'ancres étendu.
- Existant (garde) : page.rs:606-926, props_zone.rs:819-1425, writer.rs:3735-3891, store intégration 21082-21788, kill `*_regime_v2`, migration/finalize, hilbert/slot_remap, churn.
- Suites : substrate lib, engine lib, core, invariants, intégrations replay/remap — 0 rouge.
- Bench daté avant/après (DoD RFC) : CREATE mono/@8/@16, chaude, import bulk, sonde waste (remplissage %), RMW 512 vs 4096 réel (l'étape 0 proxy donne déjà ×8 sur copie+checksum).

## 9. Ordre d'implémentation (chantier, après validation + M6 au calme)

1. `UnitéAddr` codec + helpers (+ garde anti-confusion legacy/slot) — unit tests.
2. Carrier packé + header + CRC512 (page.rs/props_zone.rs) — tests format.
3. Allocateur de slots (début de chaîne + overflow) + monotonicité + ouverture (frontière, nb_slots_utilises).
4. Walkers/lecteurs généralisés à l'unité + store + relevés ; audit d'ancres étendu.
5. Fix remap (P0) étendu aux slots + tests.
6. Gouvernance : bump/version + garde vieux binaires + `obrain-migrate` (parité QC bases harness).
7. DoD complet : suites + benches + sonde waste + ledger.
