"""ObrainGraph: a LangChain ``GraphStore`` backed by an embedded Obrain database."""

from __future__ import annotations

import hashlib
from typing import Any, Dict, List, Optional

from langchain_community.graphs.graph_document import GraphDocument
from langchain_community.graphs.graph_store import GraphStore
from obrain import ObrainDB

from langchain_obrain._identifiers import check_identifier

BASE_ENTITY_LABEL = "__Entity__"
SOURCE_LABEL = "Document"
_INTERNAL_KEYS = ("_id", "_labels")


def _user_props(node: Dict[str, Any]) -> Dict[str, Any]:
    """Drop the engine's internal keys from a node returned as a map."""
    return {k: v for k, v in node.items() if k not in _INTERNAL_KEYS}


def _cypher_type(value: Any) -> str:
    """Type name of a property value, in the vocabulary of the Neo4j schema helpers."""
    if isinstance(value, bool):
        return "BOOLEAN"
    if isinstance(value, int):
        return "INTEGER"
    if isinstance(value, float):
        return "FLOAT"
    if isinstance(value, str):
        return "STRING"
    if isinstance(value, (list, tuple)):
        return "LIST"
    return "UNKNOWN"


class ObrainGraph(GraphStore):
    """Graph store over an Obrain database, queried with GQL/Cypher.

    Args:
        db: an open ``obrain.ObrainDB``. When omitted, a new database is created
            (in memory, or at ``path`` if given).
        path: location of a persistent database, used only when ``db`` is None.
        base_entity_label: when True, every node written by
            :meth:`add_graph_documents` also carries the ``__Entity__`` label.

    Query parameters use the ``$name`` syntax of the engine::

        graph.query("MATCH (p:Person {name: $name}) RETURN p.age AS age",
                    {"name": "Alix"})
    """

    def __init__(
        self,
        db: Optional[ObrainDB] = None,
        *,
        path: Optional[str] = None,
        base_entity_label: bool = False,
    ) -> None:
        if db is not None and path is not None:
            raise ValueError("passer soit db, soit path, pas les deux")
        self._db = db if db is not None else ObrainDB(path)
        self._base_entity_label = base_entity_label
        self._structured_schema: Dict[str, Any] = {}
        self._schema_str = ""
        self.refresh_schema()

    @property
    def db(self) -> ObrainDB:
        """The underlying ``ObrainDB`` handle."""
        return self._db

    def query(self, query: str, params: dict = {}) -> List[Dict[str, Any]]:
        """Run a GQL/Cypher query with named parameters and return its rows as dicts.

        The cached schema is not refreshed by this method: call
        :meth:`refresh_schema` after writes made through ``query``.
        """
        return self._db.execute(query, params or None).to_list()

    @property
    def get_schema(self) -> str:
        """Human-readable schema, as used in LLM prompts (labels, properties, relationships)."""
        return self._schema_str

    @property
    def get_structured_schema(self) -> Dict[str, Any]:
        """Schema as a dict: ``node_props``, ``rel_props`` and ``relationships``."""
        return self._structured_schema

    def refresh_schema(self) -> None:
        """Recompute the schema from the current database content."""
        self._structured_schema = self._collect_structured_schema()
        self._schema_str = self._format_schema(self._structured_schema)

    def add_graph_documents(
        self, graph_documents: List[GraphDocument], include_source: bool = False
    ) -> None:
        """Write nodes and relationships of each ``GraphDocument``.

        Nodes are merged on their ``id`` property, so adding the same document
        twice does not duplicate nodes. With ``include_source``, the source
        document is stored as a ``Document`` node linked by ``MENTIONS``.
        """
        for graph_doc in graph_documents:
            for node in graph_doc.nodes:
                self._merge_node(node.type, str(node.id), dict(node.properties))
            for rel in graph_doc.relationships:
                self._merge_relationship(rel)
            if include_source and graph_doc.source is not None:
                self._merge_source(graph_doc)
        self.refresh_schema()

    # ----- writes -------------------------------------------------------

    def _node_labels(self, label: str) -> str:
        labels = ":" + check_identifier(label or "Node", "label de nœud")
        if self._base_entity_label:
            labels += f":{BASE_ENTITY_LABEL}"
        return labels

    def _merge_node(self, label: str, node_id: str, props: Dict[str, Any]) -> None:
        query = f"MERGE (n{self._node_labels(label)} {{id: $id}}) SET n += $props"
        self._db.execute(query, {"id": node_id, "props": props})

    def _merge_relationship(self, rel: Any) -> None:
        src = check_identifier(rel.source.type or "Node", "label de nœud")
        dst = check_identifier(rel.target.type or "Node", "label de nœud")
        rtype = check_identifier(rel.type, "type de relation")
        query = (
            f"MATCH (a:{src} {{id: $source}}), (b:{dst} {{id: $target}}) "
            f"MERGE (a)-[r:{rtype}]->(b) SET r += $props"
        )
        self._db.execute(
            query,
            {
                "source": str(rel.source.id),
                "target": str(rel.target.id),
                "props": dict(rel.properties or {}),
            },
        )

    def _merge_source(self, graph_doc: GraphDocument) -> None:
        text = graph_doc.source.page_content
        doc_id = hashlib.sha256(text.encode("utf-8")).hexdigest()
        self._db.execute(
            f"MERGE (d:{SOURCE_LABEL} {{id: $id}}) SET d.text = $text",
            {"id": doc_id, "text": text},
        )
        for node in graph_doc.nodes:
            label = check_identifier(node.type or "Node", "label de nœud")
            self._db.execute(
                f"MATCH (d:{SOURCE_LABEL} {{id: $doc}}), (n:{label} {{id: $node}}) "
                "MERGE (d)-[:MENTIONS]->(n)",
                {"doc": doc_id, "node": str(node.id)},
            )

    # ----- schema -------------------------------------------------------

    def _collect_structured_schema(self) -> Dict[str, Any]:
        summary = self._db.schema()
        # Same shape as the Neo4j store, which GraphCypherQAChain expects:
        # node_props maps a label to [{"property": name, "type": TYPE}, ...].
        node_props: Dict[str, List[Dict[str, str]]] = {}
        for entry in summary.get("labels", []):
            label = check_identifier(entry["name"], "label de nœud")
            types: Dict[str, str] = {}
            for row in self._db.execute(f"MATCH (n:{label}) RETURN n AS node").to_list():
                for key, value in _user_props(row["node"]).items():
                    types.setdefault(key, _cypher_type(value))
            node_props[label] = [
                {"property": key, "type": types[key]} for key in sorted(types)
            ]

        triples = self._db.execute(
            "MATCH (a)-[r]->(b) RETURN labels(a) AS src, type(r) AS t, labels(b) AS dst"
        ).to_list()
        relationships = []
        for row in triples:
            triple = {
                "start": (row["src"] or ["Node"])[0],
                "type": row["t"],
                "end": (row["dst"] or ["Node"])[0],
            }
            if triple not in relationships:
                relationships.append(triple)
        return {"node_props": node_props, "rel_props": {}, "relationships": relationships}

    @staticmethod
    def _format_schema(structured: Dict[str, Any]) -> str:
        lines = ["Node labels and properties:"]
        for label, props in structured["node_props"].items():
            described = ", ".join(f"{p['property']}: {p['type']}" for p in props)
            lines.append(f"  {label} {{{described}}}")
        lines.append("Relationships:")
        for rel in structured["relationships"]:
            lines.append(f"  (:{rel['start']})-[:{rel['type']}]->(:{rel['end']})")
        return "\n".join(lines)
