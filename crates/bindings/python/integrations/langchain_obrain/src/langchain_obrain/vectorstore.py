"""ObrainVectorStore: a LangChain ``VectorStore`` on an Obrain HNSW index.

Each document is a node with a label (``Document`` by default), a text
property, an embedding property, a ``doc_id`` property and the metadata as
scalar properties. Similarity search uses the vector index; hybrid search
calls the ``obrain.hybrid_search`` procedure (BM25 on the text index fused with
the vector index).
"""

from __future__ import annotations

import uuid
from typing import Any, Callable, Iterable, List, Optional, Sequence, Tuple

from langchain_core.documents import Document
from langchain_core.embeddings import Embeddings
from langchain_core.vectorstores import VectorStore
from obrain import ObrainDB

from langchain_obrain._identifiers import check_identifier

HYBRID_PROCEDURE = "obrain.hybrid_search"
_RESERVED = ("doc_id",)


class HybridSearchUnavailableError(RuntimeError):
    """The engine does not provide the ``obrain.hybrid_search`` procedure."""


def hybrid_search_available(db: ObrainDB) -> bool:
    """True when this engine build lists ``obrain.hybrid_search`` among its procedures."""
    rows = db.execute("CALL obrain.procedures() YIELD name RETURN name").to_list()
    return any(row["name"] == HYBRID_PROCEDURE for row in rows)


class ObrainVectorStore(VectorStore):
    """Vector store over an Obrain database.

    Args:
        db: an open ``obrain.ObrainDB``.
        embedding: the LangChain ``Embeddings`` used for documents and queries.
        label: node label holding the documents.
        text_property: property holding the document text (also text-indexed,
            which the hybrid search needs).
        embedding_property: property holding the embedding vector.
        metric: distance metric of the vector index, ``"cosine"`` or ``"euclidean"``.

    Indexes are created on the first :meth:`add_texts` call, once the embedding
    dimension is known. Index creation is idempotent.
    """

    def __init__(
        self,
        db: ObrainDB,
        embedding: Embeddings,
        *,
        label: str = "Document",
        text_property: str = "text",
        embedding_property: str = "embedding",
        metric: str = "cosine",
    ) -> None:
        if metric not in ("cosine", "euclidean"):
            raise ValueError(f"metric inconnue : {metric!r} (cosine ou euclidean)")
        self._db = db
        self._embedding = embedding
        self._label = check_identifier(label, "label")
        self._text_property = check_identifier(text_property, "propriété texte")
        self._embedding_property = check_identifier(embedding_property, "propriété vecteur")
        self._metric = metric
        self._indexed = False

    @property
    def db(self) -> ObrainDB:
        return self._db

    @property
    def embeddings(self) -> Embeddings:
        return self._embedding

    # ----- writes -------------------------------------------------------

    def add_texts(
        self,
        texts: Iterable[str],
        metadatas: Optional[List[dict]] = None,
        *,
        ids: Optional[List[str]] = None,
        **kwargs: Any,
    ) -> List[str]:
        """Embed and store the texts; return their ids."""
        texts = list(texts)
        metadatas = list(metadatas) if metadatas is not None else [{} for _ in texts]
        ids = list(ids) if ids is not None else [str(uuid.uuid4()) for _ in texts]
        if not (len(texts) == len(metadatas) == len(ids)):
            raise ValueError("texts, metadatas et ids doivent avoir la même longueur")
        if not texts:
            return []

        vectors = self._embedding.embed_documents(texts)
        self._ensure_indexes(len(vectors[0]))
        for doc_id, text, meta, vector in zip(ids, texts, metadatas, vectors):
            clash = [k for k in meta if k in _RESERVED or k in (self._text_property, self._embedding_property)]
            if clash:
                raise ValueError(f"clés de métadonnées réservées : {clash}")
            props = {
                **meta,
                self._text_property: text,
                self._embedding_property: list(vector),
                "doc_id": doc_id,
            }
            self._db.create_node([self._label], props)
        return ids

    def delete(self, ids: Optional[List[str]] = None, **kwargs: Any) -> Optional[bool]:
        """Delete the documents with the given ids."""
        if not ids:
            return False
        self._db.execute(
            f"MATCH (n:{self._label}) WHERE n.doc_id IN $ids DETACH DELETE n",
            {"ids": list(ids)},
        )
        return True

    def get_by_ids(self, ids: Sequence[str], /) -> List[Document]:
        """Return the documents with the given ids, in the order requested."""
        rows = self._db.execute(
            f"MATCH (n:{self._label}) WHERE n.doc_id IN $ids RETURN n AS node",
            {"ids": list(ids)},
        ).to_list()
        by_id = {}
        for row in rows:
            doc = self._to_document(row["node"])
            by_id[doc.id] = doc
        return [by_id[i] for i in ids if i in by_id]

    # ----- reads --------------------------------------------------------

    def similarity_search(self, query: str, k: int = 4, **kwargs: Any) -> List[Document]:
        return [doc for doc, _ in self.similarity_search_with_score(query, k, **kwargs)]

    def similarity_search_with_score(
        self, query: str, k: int = 4, filter: Optional[dict] = None, **kwargs: Any
    ) -> List[Tuple[Document, float]]:
        """Vector search. The score is the engine's distance (lower is closer)."""
        vector = self._embedding.embed_query(query)
        hits = self._db.vector_search(
            self._label, self._embedding_property, vector, k, None, filter
        )
        results = []
        for node_id, distance in hits:
            node = self._db.get_node(node_id)
            results.append((self._to_document(node.properties()), float(distance)))
        return results

    def hybrid_search(self, query: str, k: int = 4, *, fusion: str = "rrf") -> List[Document]:
        """Hybrid BM25 + vector search through ``CALL obrain.hybrid_search``.

        The score is stored in ``metadata["score"]``. Raises
        :class:`HybridSearchUnavailableError` when the engine lacks the
        procedure: the store never falls back silently to plain vector search.
        """
        if not hybrid_search_available(self._db):
            raise HybridSearchUnavailableError(
                f"{HYBRID_PROCEDURE} n'est pas disponible dans ce binding obrain"
            )
        vector = self._embedding.embed_query(query)
        rows = self._db.execute(
            f"CALL {HYBRID_PROCEDURE}($label, $text_prop, $vec_prop, $query, $vector, "
            "{k: $k, fusion: $fusion}) YIELD node_id, score RETURN node_id, score",
            {
                "label": self._label,
                "text_prop": self._text_property,
                "vec_prop": self._embedding_property,
                "query": query,
                "vector": list(vector),
                "k": k,
                "fusion": fusion,
            },
        ).to_list()
        docs = []
        for row in rows:
            doc = self._to_document(self._db.get_node(row["node_id"]).properties())
            doc.metadata["score"] = row["score"]
            docs.append(doc)
        return docs

    def search(self, query: str, search_type: str, **kwargs: Any) -> List[Document]:
        if search_type == "hybrid":
            return self.hybrid_search(query, k=kwargs.get("k", 4), fusion=kwargs.get("fusion", "rrf"))
        return super().search(query, search_type, **kwargs)

    # ----- construction -------------------------------------------------

    @classmethod
    def from_texts(
        cls,
        texts: List[str],
        embedding: Embeddings,
        metadatas: Optional[List[dict]] = None,
        *,
        db: Optional[ObrainDB] = None,
        ids: Optional[List[str]] = None,
        **kwargs: Any,
    ) -> "ObrainVectorStore":
        """Build a store on ``db`` (a new in-memory database when omitted) and add the texts."""
        if db is None:
            db = ObrainDB()
        store = cls(db, embedding, **kwargs)
        store.add_texts(texts, metadatas, ids=ids)
        return store

    # ----- helpers ------------------------------------------------------

    def _ensure_indexes(self, dimensions: int) -> None:
        if self._indexed:
            return
        self._db.create_text_index(self._label, self._text_property)
        self._db.create_vector_index(
            self._label, self._embedding_property, dimensions=dimensions, metric=self._metric
        )
        self._indexed = True

    def _to_document(self, props: dict) -> Document:
        props = dict(props)
        doc_id = props.pop("doc_id")
        text = props.pop(self._text_property, "")
        props.pop(self._embedding_property, None)
        props.pop("_id", None)
        props.pop("_labels", None)
        return Document(id=doc_id, page_content=text, metadata=props)

    def _select_relevance_score_fn(self) -> Callable[[float], float]:
        if self._metric == "cosine":
            return lambda distance: 1.0 - distance
        return lambda distance: 1.0 / (1.0 + distance)
