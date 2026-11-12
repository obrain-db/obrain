"""ObrainPropertyGraphStore: a LlamaIndex ``PropertyGraphStore`` backed by Obrain.

Storage layout:

* every entity or chunk is a node whose Cypher label is the sanitized
  LlamaIndex label; the original label is kept in ``llama_label``;
* the node ``id`` property is the LlamaIndex id; ``name`` and ``text`` hold the
  entity name or chunk text; ``properties_json`` holds the free-form properties;
* every relation is a Cypher relationship typed with the sanitized label, the
  original label in ``llama_label`` and its properties in ``properties_json``;
* chunks with an embedding are indexed by an HNSW vector index.

LLM-generated labels may contain spaces or punctuation. They are sanitized for
Cypher (``works at`` becomes ``works_at``) and restored on read, so no triplet
is lost or altered.
"""

from __future__ import annotations

import json
import re
from typing import Any, Dict, List, Optional, Sequence, Tuple

from llama_index.core.graph_stores.types import (
    ChunkNode,
    EntityNode,
    LabelledNode,
    PropertyGraphStore,
    Relation,
    Triplet,
)
from llama_index.core.schema import BaseNode, TextNode
from llama_index.core.vector_stores.types import VectorStoreQuery
from obrain import ObrainDB

_NON_WORD = re.compile(r"[^A-Za-z0-9_]")
_IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
LABEL_KEY = "llama_label"
PROPERTIES_KEY = "properties_json"
DEFAULT_CHUNK_LABEL = "Chunk"


def cypher_name(raw: str) -> str:
    """Sanitize a LlamaIndex label or relation type into a Cypher identifier."""
    name = _NON_WORD.sub("_", raw or "node")
    if not name or name[0].isdigit():
        name = "_" + name
    return name


def _check_property_key(key: str) -> str:
    if not _IDENT.match(key):
        raise ValueError(f"nom de propriété invalide pour une requête Cypher : {key!r}")
    return key


class ObrainPropertyGraphStore(PropertyGraphStore):
    """Property graph store over an Obrain database.

    Args:
        db: an open ``obrain.ObrainDB``. When omitted, a new database is created
            (in memory, or at ``path`` if given).
        path: location of a persistent database, used only when ``db`` is None.
        chunk_label: label of the chunk nodes indexed for vector queries.
        embedding_key: property holding the chunk embedding.
        metric: distance metric of the vector index, ``"cosine"`` or ``"euclidean"``.
    """

    supports_structured_queries: bool = True
    supports_vector_queries: bool = True

    def __init__(
        self,
        db: Optional[ObrainDB] = None,
        *,
        path: Optional[str] = None,
        chunk_label: str = DEFAULT_CHUNK_LABEL,
        embedding_key: str = "embedding",
        metric: str = "cosine",
    ) -> None:
        if db is not None and path is not None:
            raise ValueError("passer soit db, soit path, pas les deux")
        if metric not in ("cosine", "euclidean"):
            raise ValueError(f"metric inconnue : {metric!r} (cosine ou euclidean)")
        self._db = db if db is not None else ObrainDB(path)
        self._chunk_label = cypher_name(chunk_label)
        self._embedding_key = _check_property_key(embedding_key)
        self._metric = metric
        self._vector_indexed = False

    @property
    def client(self) -> Any:
        return self._db

    # ----- writes -------------------------------------------------------

    def upsert_nodes(self, nodes: Sequence[LabelledNode]) -> None:
        for node in nodes:
            props = self._node_props(node)
            if node.embedding is not None:
                props[self._embedding_key] = list(node.embedding)
            self._db.execute(
                f"MERGE (n:{cypher_name(node.label)} {{id: $id}}) SET n += $props",
                {"id": node.id, "props": props},
            )

    def upsert_relations(self, relations: List[Relation]) -> None:
        for rel in relations:
            self._db.execute(
                f"MATCH (a {{id: $src}}), (b {{id: $dst}}) "
                f"MERGE (a)-[r:{cypher_name(rel.label)}]->(b) SET r += $props",
                {
                    "src": rel.source_id,
                    "dst": rel.target_id,
                    "props": {
                        LABEL_KEY: rel.label,
                        PROPERTIES_KEY: json.dumps(rel.properties, default=str),
                    },
                },
            )

    def upsert_triplet(self, subj: str, rel: str, obj: str) -> None:
        """Add the triplet ``(subj)-[rel]->(obj)``, creating missing entities."""
        subj_node = EntityNode(name=subj)
        obj_node = EntityNode(name=obj)
        self.upsert_nodes([subj_node, obj_node])
        self.upsert_relations(
            [Relation(label=rel, source_id=subj_node.id, target_id=obj_node.id)]
        )

    def upsert_llama_nodes(self, llama_nodes: List[BaseNode]) -> None:
        """Store text chunks (with their embeddings when present) as chunk nodes."""
        for node in llama_nodes:
            self._db.execute(
                f"MATCH (n:{self._chunk_label} {{id: $id}}) DETACH DELETE n",
                {"id": node.node_id},
            )
            props: Dict[str, Any] = {
                "id": node.node_id,
                "text": node.get_content(),
                LABEL_KEY: "text_chunk",
                PROPERTIES_KEY: json.dumps(node.metadata, default=str),
            }
            embedding = node.get_embedding() if node.embedding is not None else None
            if embedding is not None:
                self._ensure_vector_index(len(embedding))
                props[self._embedding_key] = list(embedding)
            self._db.create_node([self._chunk_label], props)

    def delete(
        self,
        entity_names: Optional[List[str]] = None,
        relation_names: Optional[List[str]] = None,
        properties: Optional[dict] = None,
        ids: Optional[List[str]] = None,
    ) -> None:
        if entity_names or ids:
            self._db.execute(
                "MATCH (n) WHERE n.id IN $ids OR n.name IN $names DETACH DELETE n",
                {"ids": list(ids or []), "names": list(entity_names or [])},
            )
        if relation_names:
            self._db.execute(
                f"MATCH ()-[r]->() WHERE r.{LABEL_KEY} IN $rels DELETE r",
                {"rels": list(relation_names)},
            )

    def delete_llama_nodes(
        self,
        node_ids: Optional[List[str]] = None,
        ref_doc_ids: Optional[List[str]] = None,
    ) -> None:
        if ref_doc_ids:
            raise NotImplementedError("suppression par ref_doc_id non prise en charge")
        if node_ids:
            self._db.execute(
                "MATCH (n) WHERE n.id IN $ids DETACH DELETE n", {"ids": list(node_ids)}
            )

    # ----- reads --------------------------------------------------------

    def get(
        self,
        properties: Optional[dict] = None,
        ids: Optional[List[str]] = None,
    ) -> List[LabelledNode]:
        if ids:
            rows = self._db.execute(
                "MATCH (n) WHERE n.id IN $ids RETURN n AS node", {"ids": list(ids)}
            ).to_list()
        elif properties:
            clauses = []
            params: Dict[str, Any] = {}
            for i, (key, value) in enumerate(properties.items()):
                clauses.append(f"n.{_check_property_key(key)} = $p{i}")
                params[f"p{i}"] = value
            rows = self._db.execute(
                f"MATCH (n) WHERE {' AND '.join(clauses)} RETURN n AS node", params
            ).to_list()
        else:
            rows = self._db.execute("MATCH (n) RETURN n AS node").to_list()
        return [self._to_node(row["node"]) for row in rows]

    def get_triplets(
        self,
        entity_names: Optional[List[str]] = None,
        relation_names: Optional[List[str]] = None,
        properties: Optional[dict] = None,
        ids: Optional[List[str]] = None,
    ) -> List[Triplet]:
        clauses = []
        params: Dict[str, Any] = {}
        if entity_names:
            clauses.append("(a.id IN $names OR b.id IN $names)")
            params["names"] = list(entity_names)
        if relation_names:
            clauses.append(f"r.{LABEL_KEY} IN $rels")
            params["rels"] = list(relation_names)
        where = f" WHERE {' AND '.join(clauses)}" if clauses else ""
        rows = self._db.execute(
            "MATCH (a)-[r]->(b)"
            + where
            + f" RETURN a AS source, r.{LABEL_KEY} AS rel_label, "
            f"r.{PROPERTIES_KEY} AS rel_props, b AS target",
            params or None,
        ).to_list()
        return [self._row_to_triplet(row) for row in rows]

    def get_rel_map(
        self,
        graph_nodes: List[LabelledNode],
        depth: int = 2,
        limit: int = 30,
        ignore_rels: Optional[List[str]] = None,
    ) -> List[Triplet]:
        """Triplets reachable from ``graph_nodes`` within ``depth`` hops, in either direction."""
        ignored = set(ignore_rels or [])
        frontier = {node.id for node in graph_nodes}
        visited = set(frontier)
        seen_edges: set = set()
        result: List[Triplet] = []
        for _ in range(depth):
            if not frontier:
                break
            next_frontier: set = set()
            # "a" anchors outgoing relations of the frontier, "b" incoming ones.
            for anchor in ("a", "b"):
                rows = self._db.execute(
                    f"MATCH (a)-[r]->(b) WHERE {anchor}.id IN $ids "
                    f"RETURN a AS source, r.{LABEL_KEY} AS rel_label, "
                    f"r.{PROPERTIES_KEY} AS rel_props, b AS target",
                    {"ids": list(frontier)},
                ).to_list()
                for row in rows:
                    triplet = self._row_to_triplet(row)
                    key = (triplet[0].id, triplet[1].label, triplet[2].id)
                    if triplet[1].label in ignored or key in seen_edges:
                        continue
                    seen_edges.add(key)
                    result.append(triplet)
                    for endpoint in (triplet[0].id, triplet[2].id):
                        if endpoint not in visited:
                            next_frontier.add(endpoint)
                    if len(result) >= limit:
                        return result
            visited |= next_frontier
            frontier = next_frontier
        return result

    def structured_query(self, query: str, param_map: Optional[Dict[str, Any]] = None) -> Any:
        return self._db.execute(query, param_map or None).to_list()

    def vector_query(
        self, query: VectorStoreQuery, **kwargs: Any
    ) -> Tuple[List[LabelledNode], List[float]]:
        if query.query_embedding is None:
            raise ValueError("VectorStoreQuery sans query_embedding")
        hits = self._db.vector_search(
            self._chunk_label, self._embedding_key, list(query.query_embedding), query.similarity_top_k
        )
        nodes: List[LabelledNode] = []
        scores: List[float] = []
        for node_id, distance in hits:
            nodes.append(self._to_node(self._db.get_node(node_id).properties()))
            scores.append(1.0 - float(distance) if self._metric == "cosine" else 1.0 / (1.0 + float(distance)))
        return nodes, scores

    def get_llama_nodes(self, node_ids: List[str]) -> List[BaseNode]:
        rows = self._db.execute(
            f"MATCH (n:{self._chunk_label}) WHERE n.id IN $ids RETURN n AS node",
            {"ids": list(node_ids)},
        ).to_list()
        nodes: List[BaseNode] = []
        for row in rows:
            props = row["node"]
            metadata = json.loads(props.get(PROPERTIES_KEY) or "{}")
            nodes.append(TextNode(id_=props["id"], text=props["text"], metadata=metadata))
        return nodes

    def get_schema(self, refresh: bool = False) -> Any:
        return self._db.schema()

    # ----- helpers ------------------------------------------------------

    def _ensure_vector_index(self, dimensions: int) -> None:
        if self._vector_indexed:
            return
        self._db.create_vector_index(
            self._chunk_label, self._embedding_key, dimensions=dimensions, metric=self._metric
        )
        self._vector_indexed = True

    @staticmethod
    def _node_props(node: LabelledNode) -> Dict[str, Any]:
        props: Dict[str, Any] = {
            "id": node.id,
            LABEL_KEY: node.label,
            PROPERTIES_KEY: json.dumps(node.properties, default=str),
        }
        if isinstance(node, EntityNode):
            props["name"] = node.name
        if isinstance(node, ChunkNode):
            props["text"] = node.text
        return props

    @staticmethod
    def _to_node(props: Dict[str, Any]) -> LabelledNode:
        label = props.get(LABEL_KEY) or "node"
        properties = json.loads(props.get(PROPERTIES_KEY) or "{}")
        if "text" in props:
            return ChunkNode(text=props["text"], id_=props["id"], label=label, properties=properties)
        return EntityNode(name=props.get("name", props["id"]), label=label, properties=properties)

    def _row_to_triplet(self, row: Dict[str, Any]) -> Triplet:
        source = self._to_node(row["source"])
        target = self._to_node(row["target"])
        relation = Relation(
            label=row["rel_label"] or "related",
            source_id=source.id,
            target_id=target.id,
            properties=json.loads(row["rel_props"] or "{}"),
        )
        return (source, relation, target)
