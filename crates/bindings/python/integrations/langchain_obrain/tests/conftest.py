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
