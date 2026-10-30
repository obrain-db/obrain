# Design — Pont mémoire zero-copy inférence ↔ obrain (O1)

2026-09-10 · harness-O1 · branche `feat/zero-copy-views` (worktree `banc-obrain-zerocopy`)

## Problème

Le PoC agent-rs lit le graphe in-process (2–72 µs/requête) mais chaque valeur
traverse encore : `execute(GQL)` → `Value` possédées → `serde_json::to_string`
→ parse → re-tokenisation. Pour un moteur d'inférence embarquant `obrain`,
chaque octet de texte est copié 3 à 4 fois alors qu'il dort déjà dans une page
mmap du substrate.

Copies identifiées sur le chemin props (chaud) :

1. `PropsZone::read_heap_page` — copie 4 KiB entiers (`Ok(*bytemuck::from_bytes::<HeapPage>(..))`).
2. `PropsZone::read_heap` — `to_vec()` du payload.
3. `props_codec::decode_string_ref` — `ArcStr::from(s)` (3e copie).
4. `QueryResult` → JSON → parse (4e et 5e).

Côté blobcol (>256 B), `BlobColumnRegistry::read` copie dans un `Arc<[u8]>`
(« the writer's mmap borrow cannot escape the lock guard ») — c'est
précisément ce verrou-échappement que ce design résout proprement.

## Les vues exposées

### 1. `StrView<'store>` — la brique (PoC implémenté)

```rust
pub enum StrView<'a> {
    /// Slice UTF-8 emprunté au heap mmap de la PropsZone, sous garde de lecture.
    Zone(MappedRwLockReadGuard<'a, str>),
    /// Colonne blob (>256 B) : garde Arc (parking_lot arc_lock) + slot ;
    /// le slice est re-dérivé à chaque deref (bornes + UTF-8 revalidés, safe).
    Blob(BlobStrView),
    /// Chemin hérité (DashMap) : ArcStr cloné — bump de refcount, zéro copie d'octets.
    Shared(ArcStr),
}
impl Deref for StrView<'_> { type Target = str; }
```

- `SubstrateStore::node_prop_str_view(&self, id: NodeId, key: &str)
  -> SubstrateResult<Option<StrView<'_>>>` — ordre de résolution identique à
  `lire_propriete_noeud_vivant` : blobcol → PropsZone v2 (LWW, tombstone) →
  DashMap héritée. Vivacité vérifiée d'abord (`is_live_on_disk`).
- `Session::prop_str_view(&self, id, key)` (obrain-engine) délègue au
  `substrate_store()` déjà câblé ; réexporté par la crate faîtière `obrain`.

### 2. `FactView<'store>` — la vue métier du seeding

```rust
pub struct FactView<'a> { pub node: NodeId, pub text: StrView<'a> }
```
Sucre au-dessus de (1) : le texte d'un fait entre dans le tokenizer du moteur
par `&str` emprunté — dans engine-spec (E2), `seed(&fact.text)` sans copie.

### 3. Itérateur de props par référence (design, impl différée)

`SubstrateStore::for_each_prop_view(&self, id, f: impl FnMut(&str, ValueView<'_>))`
— callback sous UNE prise de la garde PropsZone, `ValueView` empruntant
scalaires décodés et `&str`/`&[u8]` du heap. Le style callback évite les
gardes auto-référentes pour le multi-entrées ; l'itérateur `lending` viendra
avec les GAT si besoin.

### 4. Buffer de résultat réutilisable (design, impl différée)

`ResultBuf` : arène possédée par l'appelant, remplie par
`execute_into(&mut ResultBuf)` — les Strings de `QueryResult` deviennent des
offsets dans l'arène, amorti sur des milliers de requêtes. Indépendant des
vues 1–3 ; élimine les allocations restantes quand une matérialisation est
inévitable (agrégats, expressions).

## Contrat de lifetimes

- **Une vue = un bail de lecture.** `StrView<'a>` emprunte `&'a SubstrateStore`
  et tient LA garde de lecture de la zone d'où viennent ses octets
  (`props_zone: RwLock<PropsZone>` ou `RwLock<BlobColumnWriter>` de la colonne).
- **Pourquoi c'est sûr** : toute croissance/remap du mmap passe par `&mut`
  (`as_slice_mut`, `append_*`), donc par la garde d'ÉCRITURE du même RwLock.
  Garde de lecture tenue ⇒ pas de remap ⇒ le slice reste valide. Le borrow
  checker interdit de faire survivre la vue au store (doc-test `compile_fail`).
- **Cohabitation lock RO partagé** : store ouvert `open_read_only`, plusieurs
  lecteurs concurrents OK (gardes partagées). Les vues sont des baux COURTS :
  ne pas tenir une vue en appelant une écriture sur le même store (le writer
  attendrait la garde) — même règle que les commentaires VERROUS existants
  (`lookup_*_v2` relâche avant de rendre ; ici le relâchement est le drop de
  la vue, borne par le borrow de `&self`).
- **Aucun deadlock nouveau** : jamais deux gardes de zones tenues à la fois
  pendant la résolution (blob relâché avant PropsZone, comme l'existant).

## Stratégie multi-arch (aarch64 / x86_64 aujourd'hui, wasm32 visé)

- **Little-endian doré** : les formats v2 sont DÉJÀ LE explicites sur le chemin
  lu (`HeapPage::read_at` : `u32::from_le_bytes` sur la longueur ; index blob :
  `try_pod_read_unaligned::<BlobSlotEntry>`). Les trois cibles (x86_64,
  aarch64, wasm32) sont LE ; un test de round-trip fige la représentation
  octet-à-octet pour qu'un portage BE futur casse un test, pas des données.
- **Alignement vérifié, jamais supposé** : `bytemuck::try_from_bytes`
  (vérification runtime d'alignement + taille) pour poser une réf `&HeapPage`
  sur la page mmap — pas de transmute, pas de packed-deref. Côté blob, lecture
  NON alignée assumée (`try_pod_read_unaligned`). wasm32 : pas de mmap, mais
  les mêmes octets chargés en mémoire linéaire passent par le même code sûr.
- **Zéro unsafe nouveau** : les vues n'ajoutent AUCUN bloc unsafe ; tout
  l'emprunt est porté par `parking_lot` (`RwLockReadGuard::try_map`,
  `read_arc` — feature `arc_lock` activée, additive).

## PoC mesuré (livrable 2)

Bench criterion `crates/obrain/benches/zero_copy.rs`, store réel = COPIE de
`t.obrain` (462 M, props v2 achevée) ouverte `open_read_only`, n ≥ 1000 :
(a) `execute` GQL + `to_string` JSON + re-parse (chemin agent-rs actuel) ;
(b) `get_node_property` (Value possédée, sans JSON) ;
(c) `prop_str_view` (vue zero-copy).
## Résultats mesurés (n3, x86_64, release, criterion 100 échantillons)

**Store réel** : copie de `chantiers-harness.obrain` (163 nœuds Event porteurs
de `msg`, sources des vues : 66 PropsZone + 97 blobcol, 0 héritée) :

| Chemin | µs/lecture | vs (a) |
|---|---|---|
| (a) `execute` + JSON + re-parse (agent-rs actuel) | 1,65 µs/ligne (268,3 µs / 163 lignes) | 1× |
| (b) `get_node_property` (Value + ArcStr possédés) | 1,20 µs | 1,4× |
| (c) `prop_str_view` (vue zero-copy) | **0,223 µs** | **7,4×** |
| (c bis) vue consommée octet à octet (~300 o) | 0,304 µs | 5,4× |

**Store synthétique** (1 200 faits, 1 000 lectures échantillonnées 100 % PropsZone) :
(a) 1,52 µs/ligne (1,824 ms / 1 200) ; (b) 0,804 µs ; (c) **0,225 µs** ;
(c bis, ~150 o) 0,251 µs. Vue : 6,7× vs JSON, 3,6× vs possédée.

Criterion : 100 échantillons, jusqu à 17 M itérations par bench (n ≫ 1000).
Piège documenté : la copie de `t.obrain` (46 522 nœuds) rend Null sur TOUTES
ses valeurs de props — y compris via le binaire prod et `execute` — store
dégradé préexistant, écarté du bench (sonde du 2026-09-10).

## Comparatif n3 vs DGX (extension du 2026-09-10)

Mêmes benchs rejoués sur la cible d inférence réelle : DGX GB10 aarch64
(spark-c578, 20 cœurs, build+bench `nice -n 10`, 81 Go libres, benchs
CPU-only — R1 non perturbé). Branche portée par `git fetch` ssh depuis n3,
worktree dédié, tests multi-arch 9/9 + `compile_fail` verts sur aarch64.

**Store réel** (copies RO : n3 = chantiers-harness 163 Events 66z/97b ;
DGX = kb-ergaster 36 Events 27z/9b, props vérifiées non-Null) :

| Chemin | n3 x86_64 | DGX aarch64 | DGX/n3 |
|---|---|---|---|
| (a) execute+JSON+re-parse, par ligne | 1 646 ns | 651 ns | 2,5× |
| (b) `get_node_property` possédée | 1 205 ns | 745 ns | 1,6× |
| (c) `prop_str_view` zero-copy | **223 ns** | **168 ns** | 1,3× |
| (c bis) vue consommée octet à octet | 304 ns | 182 ns | — |
| **ratio vue vs JSON** | **7,4×** | **3,9×** | |

**Synthétique** (1 200 faits, lectures 100 % PropsZone) :

| Chemin | n3 x86_64 | DGX aarch64 |
|---|---|---|
| (a) par ligne | 1 520 ns | 607 ns |
| (b) possédée | 804 ns | 575 ns |
| (c) vue | 225 ns | 177 ns (JSON/vue = 3,4×) |
| (c bis) | 251 ns | 185 ns |

Lecture : le GB10 avale mieux le chemin JSON (execute 2,5× plus rapide),
mais la vue reste le plancher sur les DEUX architectures — ~170-225 ns la
lecture, sous la microseconde partout, et l écart c bis n3 (304 ns) vient
des textes plus longs du store chantiers (97 blobs > 256 o contre 9).

## Démo seeding (livrable 4)

`crates/obrain/examples/seeding_zero_copy.rs` : deux faits (102 o PropsZone,
378 o blobcol) tokenisés DEPUIS la vue (`&fait.text`) et seedés dans un
miroir de `engine_spec::DraftProposer::seed(&[Token])` — zéro String
intermédiaire, garde relâchée au drop de la vue. Sortie : 480 tokens indexés.

## Tests (2026-09-10)

9 tests d intégration `crates/obrain-substrate/tests/zero_copy_views.rs`
(zone, blob, LWW, tombstone, nœud mort, goldens LE, alignement refusé)
+ 1 doc-test `compile_fail` (la vue ne survit pas au store) = 10 verts.
Aucun bloc unsafe ajouté sur la branche (vérifié par grep du diff).
