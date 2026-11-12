"""GraphCypherQAChain end to end: a fake LLM writes the Cypher and answers, the graph really runs it."""

from langchain_community.chains.graph_qa.cypher import GraphCypherQAChain
from langchain_community.graphs.graph_document import GraphDocument, Node, Relationship
from langchain_core.documents import Document
from langchain_core.language_models import FakeListLLM

from langchain_obrain import ObrainGraph


def test_graph_cypher_qa_chain_runs_generated_query_end_to_end():
    alix = Node(id="Alix", type="Person", properties={"age": 30})
    gus = Node(id="Gus", type="Person", properties={"age": 41})
    graph = ObrainGraph()
    graph.add_graph_documents(
        [
            GraphDocument(
                nodes=[alix, gus],
                relationships=[Relationship(source=alix, target=gus, type="KNOWS", properties={})],
                source=Document(page_content="Alix knows Gus."),
            )
        ]
    )
    cypher = "MATCH (:Person {id: 'Alix'})-[:KNOWS]->(friend:Person) RETURN friend.id AS name"
    llm = FakeListLLM(responses=[cypher, "Alix knows Gus."])

    chain = GraphCypherQAChain.from_llm(
        llm=llm,
        graph=graph,
        allow_dangerous_requests=True,
        return_intermediate_steps=True,
        verbose=False,
    )
    out = chain.invoke({"query": "Who does Alix know?"})

    assert out["result"] == "Alix knows Gus."
    steps = out["intermediate_steps"]
    assert steps[0]["query"] == cypher
    assert steps[1]["context"] == [{"name": "Gus"}]
