"""Gremlin implementation of filter and direct lookup tests.

Tests filter operations and direct lookup APIs with Gremlin for setup/verification.

Note: Gremlin filter syntax may not be fully implemented yet.
"""

import pytest

from tests.bases.test_filters import BaseFilterAndLookupTest


class TestGremlinFilters(BaseFilterAndLookupTest):
    """Gremlin implementation of filter and lookup tests."""

    def _execute_gremlin(self, db, query: str):
        """Execute Gremlin query, skip if not supported."""
        try:
            return db.execute_gremlin(query)
        except AttributeError:
            pytest.skip("Gremlin support not available")
            return None
        except NotImplementedError:
            pytest.skip("Gremlin not implemented")
            return None

    def create_person_nodes(self, db, count: int = 1000) -> list:
        """Create Person nodes using direct API (faster than queries)."""
        cities = ["NYC", "LA", "Chicago", "Boston", "Utrecht"]
        node_ids = []

        for i in range(count):
            node = db.create_node(
                ["Person"],
                {
                    "name": f"Person{i}",
                    "age": i % 100,  # Ages 0-99, cycling
                    "city": cities[i % len(cities)],
                },
            )
            node_ids.append(node.id)

        return node_ids

    def filter_by_age_equals(self, db, age: int) -> list:
        """Filter using Gremlin has() step."""
        result = self._execute_gremlin(db, f"g.V().hasLabel('Person').has('age', {age})")
        return list(result)

    def filter_by_age_range(self, db, min_age: int, max_age: int) -> list:
        """Filter using Gremlin range predicates."""
        result = self._execute_gremlin(
            db,
            f"g.V().hasLabel('Person').has('age', gt({min_age})).has('age', lt({max_age}))",
        )
        return list(result)

    def filter_by_city(self, db, city: str) -> list:
        """Filter using Gremlin string comparison."""
        result = self._execute_gremlin(db, f"g.V().hasLabel('Person').has('city', '{city}')")
        return list(result)

    def filter_compound_and(self, db, city: str, min_age: int) -> list:
        """Filter using compound Gremlin has() steps (implicit AND)."""
        result = self._execute_gremlin(
            db,
            f"g.V().hasLabel('Person').has('city', '{city}').has('age', gt({min_age}))",
        )
        return list(result)


class TestGremlinFilterVerification:
    """Additional Gremlin-specific filter tests with verification."""

    def _execute_gremlin(self, db, query: str):
        """Execute Gremlin query, skip if not supported."""
        try:
            return db.execute_gremlin(query)
        except AttributeError:
            pytest.skip("Gremlin support not available")
            return None
        except NotImplementedError:
            pytest.skip("Gremlin not implemented")
            return None

    def test_filter_with_traversal(self, db):
        """Test filtering with traversal steps."""
        alix = db.create_node(["Person"], {"name": "Alix", "age": 30})
        gus = db.create_node(["Person"], {"name": "Gus", "age": 25})
        vincent = db.create_node(["Person"], {"name": "Vincent", "age": 35})

        db.create_edge(alix.id, gus.id, "knows", {})
        db.create_edge(alix.id, vincent.id, "knows", {})

        # Find friends of Alix who are over 30
        result = self._execute_gremlin(
            db, "g.V().has('name', 'Alix').out('knows').has('age', gt(30))"
        )
        friends = list(result)
        assert len(friends) == 1, "Should find 1 friend over 30"

    def test_filter_between(self, db):
        """Test Gremlin between predicate."""
        db.create_node(["Person"], {"name": "Alix", "age": 30})
        db.create_node(["Person"], {"name": "Gus", "age": 25})
        db.create_node(["Person"], {"name": "Vincent", "age": 35})

        result = self._execute_gremlin(db, "g.V().hasLabel('Person').has('age', between(26, 34))")
        matches = list(result)
        assert len(matches) == 1, "Should find only Alix (age 30)"

    def test_filter_within(self, db):
        """Test Gremlin within predicate."""
        db.create_node(["Person"], {"name": "Alix", "city": "NYC"})
        db.create_node(["Person"], {"name": "Gus", "city": "LA"})
        db.create_node(["Person"], {"name": "Vincent", "city": "Chicago"})

        result = self._execute_gremlin(
            db, "g.V().hasLabel('Person').has('city', within('NYC', 'LA'))"
        )
        matches = list(result)
        assert len(matches) == 2, "Should find Alix and Gus"

    def test_filter_values_select(self, db):
        """Test Gremlin values() step with filter."""
        db.create_node(["Person"], {"name": "Alix", "age": 30})
        db.create_node(["Person"], {"name": "Gus", "age": 25})

        result = self._execute_gremlin(
            db, "g.V().hasLabel('Person').has('age', gt(28)).values('name')"
        )
        names = list(result)
        assert len(names) == 1, "Should find only Alix's name"
