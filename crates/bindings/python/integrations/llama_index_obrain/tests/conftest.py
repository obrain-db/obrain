"""Shared fixtures. Tests never touch the network: sockets are blocked for the whole session."""

import socket

import pytest


class NetworkForbidden(RuntimeError):
    pass


@pytest.fixture(autouse=True)
def _no_network(monkeypatch):
    """Any outbound connection in a test fails loudly instead of reaching the network."""

    def _refuse(*args, **kwargs):
        raise NetworkForbidden("réseau interdit dans les tests")

    monkeypatch.setattr(socket.socket, "connect", _refuse)
    monkeypatch.setattr(socket.socket, "connect_ex", _refuse)
    monkeypatch.setattr(socket, "create_connection", _refuse)
    monkeypatch.setattr(socket, "getaddrinfo", _refuse)


@pytest.fixture(autouse=True)
def _offline_llama_settings(monkeypatch):
    """Route LlamaIndex's default LLM and embedding to fakes, so nothing resolves to OpenAI.

    LlamaIndex components built without an explicit model (for example the
    default sub-retrievers) read ``Settings``. Without this, they would try to
    import the OpenAI integration. Settings are restored after each test.
    """
    from llama_index.core import Settings
    from llama_index.core.embeddings import MockEmbedding
    from llama_index.core.llms import MockLLM

    monkeypatch.setattr(Settings, "_llm", MockLLM(max_tokens=None))
    monkeypatch.setattr(Settings, "_embed_model", MockEmbedding(embed_dim=8))
