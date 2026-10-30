# NATIF-J9 — Phase C / priorite 3 : Protobuf (ANALYSE + schema, DIFFERE)

Statut : **differe** (clause anti-enlisement). A + priorite 1 (stream priorise)
+ priorite 2 (abonnement vivant + deltas) + client (repli) sont LIVRES ET
PROUVES ; ils portent le gain de tete (time-to-first-paint, graphe vivant).
Protobuf attaque le RESIDUEL (CPU serialize/parse + ~2-3x le fil apres gzip sur
la part numerique). Cette note fige la decision et livre le blueprint pret a
poser, pour que l orchestrateur (ou une session J10) l execute sans re-concevoir.

## Pourquoi differe (cout mesure vs gain)
- `prost` / `prost-build` NE sont PAS dans Cargo.lock (verifie : deps directes
  obrain-cli = serde, serde_json, flate2 ; obrain-viz = serde, serde_json,
  thiserror). Les ajouter = nouvelles deps + un `build.rs` qui compile le
  `.proto` (protoc ou prost-build vendored) => gros rebuild.
- Contexte d execution ce soir : disque a 95-96 % (wami build en //, sature a
  100 % plusieurs fois), lien ssh instable (retries frequents). Un rebuild lourd
  avec deps neuves est precisement le risque que la clause anti-enlisement
  interdit d encourir alors que le gain de tete est deja acquis.
- Cote client : il faut un decodeur (protobuf.js runtime, ou ts-proto genere) +
  la negociation. Risque front non nul pour un gain secondaire.
- Le gain de tete (peindre en ~13-30 ms au lieu d attendre 180 ms-3 s ; deltas
  au lieu de re-fetch) est INDEPENDANT de l encodage : il est deja livre en JSON.
  Protobuf ne le change pas — il rogne le CPU et le fil residuels.

Conclusion : on POSE le schema et le plan ; on n execute pas le rebuild ce soir.

## Schema `.proto` versionne (propose : crates/obrain-viz/proto/viz_stream.proto)
```proto
syntax = "proto3";
package obrain.viz.stream.v1;

// Enveloppe de frame (1 message par frame WS binaire).
message Frame {
  oneof body {
    Hello   hello   = 1;
    Nodes   nodes   = 2;
    Edges   edges   = 3;
    End     end     = 4;
    Live    live    = 5;
    Delta   delta   = 6;
    Epoch   epoch   = 7;   // avance d epoque sans changement visible
    Error   error   = 8;
  }
}

message Hello {                 // == frame JSON "begin"
  string schema   = 1;          // "obrain/json-graph@1"
  uint64 n_nodes  = 2;
  uint64 n_edges  = 3;
  bool   truncated = 4;
  string order    = 5;          // "importance:degree" | "canonical"
}

message Node {                  // champs optionnels proto3 -> presence via optional
  string id = 1;
  optional string qn = 2;
  optional string label = 3;
  optional string file_path = 4;
  optional double energy = 5;
  optional double curvature = 6;
  optional uint64 scc_id = 7;
  optional uint64 community_id = 8;
  optional string engram_state = 9;
  optional string engram_id = 10;
  optional double fsrs_stability = 11;
  optional double fsrs_difficulty = 12;
  optional string kind = 13;
  optional string color_hint = 14;
  optional double size_hint = 15;
}

message Edge {
  string src = 1;
  string dst = 2;
  string type = 3;
  optional double weight = 4;
  optional double ricci = 5;
  optional bool   stale = 6;
  optional double pheromone = 7;
  optional string color_hint = 8;
  optional double size_hint = 9;
}

message Nodes { repeated Node items = 1; }   // lot (WS_CHUNK)
message Edges { repeated Edge items = 1; }
message End {}
message Live  { uint64 epoch = 1; }
message Epoch { uint64 epoch = 1; }
message Error { string detail = 1; }

message EdgeKey { string src = 1; string dst = 2; string type = 3; }
message Delta {
  uint64 epoch = 1;
  repeated Node    added_nodes   = 2;  // upsert par id
  repeated string  removed_nodes = 3;  // ids
  repeated Edge    added_edges   = 4;
  repeated EdgeKey removed_edges = 5;
}
```
Le schema est un miroir EXACT de VizNode/VizEdge et des frames JSON actuelles :
l identite reste verifiable (decoder proto -> memes ids/champs que le JSON).

## Negociation (JSON reste le repli)
- Le client annonce protobuf via le sous-protocole WS
  (`Sec-WebSocket-Protocol: obrain.viz.pb.v1`) OU un param d URL `enc=pb`.
- Le handshake maison (viz_ws.rs) sait deja lire les en-tetes : ajouter la
  lecture de `Sec-WebSocket-Protocol` et renvoyer l en-tete accepte si `pb`
  demande ET supporte ; sinon on ne renvoie rien -> le client sait qu il aura du
  JSON. `send_text` (opcode 0x1) reste pour JSON ; ajouter `send_binary`
  (opcode 0x2) pour les frames proto. Une seule branche a l emission :
  `if pb { send_binary(frame.encode()) } else { send_text(json) }`.
- Aucune regression : sans `enc=pb`, comportement identique a aujourd hui.

## Cote client
- Option A (rapide) : `protobuf.js` (runtime + .proto charge) — decode Frame.
- Option B (type) : `ts-proto` genere des types TS a partir du .proto (build).
- fetchSubgraph : si `enc=pb` accepte, brancher un decodeur binaire sur
  `ws.onmessage` (ArrayBuffer) au lieu de `JSON.parse` ; meme accumulation
  nodes[]/edges[]. Repli JSON inchange.

## Ordre d execution recommande (J10)
1. Poser le .proto + `build.rs` (prost-build) dans obrain-viz, derriere une
   feature Cargo `proto` (compilation optionnelle -> pas de cout si off).
2. `send_binary` dans viz_ws.rs + negociation `enc=pb` dans api_subgraph_stream.
3. Client : decodeur (option A d abord), negociation, repli JSON.
4. Preuve : memes ids/champs que JSON (identite) ; octets sur le fil (~2-3x <
   JSON+gzip sur la part numerique) ; CPU parse client (chrono).
