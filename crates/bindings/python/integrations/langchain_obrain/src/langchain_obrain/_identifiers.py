"""Validation of identifiers interpolated into Cypher text.

Labels, relationship types and property names cannot be bound as query
parameters, so they are checked against a strict pattern before being put in
a query string. Anything else raises ``ValueError`` instead of being escaped.
"""

from __future__ import annotations

import re

_IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def check_identifier(value: str, kind: str) -> str:
    """Return ``value`` if it is a safe Cypher identifier, else raise ``ValueError``."""
    if not isinstance(value, str) or not _IDENTIFIER.match(value):
        raise ValueError(
            f"{kind} invalide pour une requête Cypher : {value!r} "
            "(attendu [A-Za-z_][A-Za-z0-9_]*)"
        )
    return value
