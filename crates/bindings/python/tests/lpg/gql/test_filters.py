"""GQL implementation of filter and direct lookup tests.

Tests filter operations and direct lookup APIs with GQL for setup/verification.
"""

from tests.bases.test_filters import BaseFilterAndLookupTest


class TestGQLFilters(BaseFilterAndLookupTest):
    """GQL implementation of filter and lookup tests."""

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
        """Filter using GQL WHERE clause."""
        result = db.execute(f"MATCH (p:Person) WHERE p.age = {age} RETURN p")
        return list(result)

    def filter_by_age_range(self, db, min_age: int, max_age: int) -> list:
        """Filter using GQL range comparison."""
        result = db.execute(
            f"MATCH (p:Person) WHERE p.age > {min_age} AND p.age < {max_age} RETURN p"
        )
        return list(result)

    def filter_by_city(self, db, city: str) -> list:
        """Filter using GQL string comparison."""
        result = db.execute(f"MATCH (p:Person) WHERE p.city = '{city}' RETURN p")
        return list(result)

    def filter_compound_and(self, db, city: str, min_age: int) -> list:
        """Filter using compound GQL WHERE clause."""
        result = db.execute(
            f"MATCH (p:Person) WHERE p.city = '{city}' AND p.age > {min_age} RETURN p"
        )
        return list(result)


class TestGQLFilterVerification:
    """Additional GQL-specific filter tests with verification."""

    def test_filter_with_relationship(self, db):
        """Test filtering with relationship pattern."""
        # Create people and relationships
        alix = db.create_node(["Person"], {"name": "Alix", "age": 30})
        gus = db.create_node(["Person"], {"name": "Gus", "age": 25})
        vincent = db.create_node(["Person"], {"name": "Vincent", "age": 35})

        db.create_edge(alix.id, gus.id, "KNOWS", {})
        db.create_edge(alix.id, vincent.id, "KNOWS", {})

        # Filter friends of Alix who are over 30
        result = db.execute(
            "MATCH (a:Person {name: 'Alix'})-[:KNOWS]->(friend:Person) "
            "WHERE friend.age > 30 "
            "RETURN friend.name"
        )
        friends = list(result)
        assert len(friends) == 1, "Should find 1 friend over 30"

    def test_filter_or_condition(self, db):
        """Test OR filter condition."""
        db.create_node(["Person"], {"name": "Alix", "age": 30, "city": "NYC"})
        db.create_node(["Person"], {"name": "Gus", "age": 25, "city": "LA"})
        db.create_node(["Person"], {"name": "Vincent", "age": 35, "city": "Chicago"})

        # Filter by city = NYC OR age < 30
        result = db.execute("MATCH (p:Person) WHERE p.city = 'NYC' OR p.age < 30 RETURN p.name")
        matches = list(result)
        assert len(matches) == 2, "Should find Alix (NYC) and Gus (age < 30)"

    def test_filter_inequality(self, db):
        """Test inequality filter condition."""
        db.create_node(["Person"], {"name": "Alix", "age": 30, "city": "NYC"})
        db.create_node(["Person"], {"name": "Gus", "age": 25, "city": "LA"})

        # Filter by city <> NYC (not equal)
        result = db.execute("MATCH (p:Person) WHERE p.city <> 'NYC' RETURN p.name")
        matches = list(result)
        assert len(matches) == 1, "Should find only Gus (not in NYC)"

    def test_filter_less_than(self, db):
        """Test less than filter condition."""
        db.create_node(["Person"], {"name": "Alix", "age": 30})
        db.create_node(["Person"], {"name": "Gus", "age": 25})

        # Filter by age < 28
        result = db.execute("MATCH (p:Person) WHERE p.age < 28 RETURN p.name")
        matches = list(result)
        assert len(matches) == 1, "Should find only Gus (age 25)"


class TestGQLMultipleNotExists:
    """Tests for multiple NOT EXISTS subqueries in a single WHERE clause."""

    def test_two_not_exists(self, db):
        """Two NOT EXISTS in the same WHERE clause."""
        alix = db.create_node(["Person"], {"name": "Alix"})
        gus = db.create_node(["Person"], {"name": "Gus"})
        vincent = db.create_node(["Person"], {"name": "Vincent"})
        db.create_edge(alix.id, gus.id, "KNOWS")
        db.create_edge(gus.id, vincent.id, "LIKES")
        # Vincent has no outgoing KNOWS and no outgoing LIKES
        result = list(
            db.execute(
                "MATCH (p:Person) "
                "WHERE NOT EXISTS { MATCH (p)-[:KNOWS]->() } "
                "AND NOT EXISTS { MATCH (p)-[:LIKES]->() } "
                "RETURN p.name"
            )
        )
        names = {r["p.name"] for r in result}
        assert names == {"Vincent"}

    def test_three_not_exists(self, db):
        """Three NOT EXISTS in the same WHERE clause."""
        alix = db.create_node(["Person"], {"name": "Alix"})
        gus = db.create_node(["Person"], {"name": "Gus"})
        vincent = db.create_node(["Person"], {"name": "Vincent"})
        jules = db.create_node(["Person"], {"name": "Jules"})
        db.create_edge(alix.id, gus.id, "KNOWS")
        db.create_edge(gus.id, vincent.id, "LIKES")
        db.create_edge(vincent.id, jules.id, "FOLLOWS")
        # Jules has none of the three outgoing edge types
        result = list(
            db.execute(
                "MATCH (p:Person) "
                "WHERE NOT EXISTS { MATCH (p)-[:KNOWS]->() } "
                "AND NOT EXISTS { MATCH (p)-[:LIKES]->() } "
                "AND NOT EXISTS { MATCH (p)-[:FOLLOWS]->() } "
                "RETURN p.name"
            )
        )
        names = {r["p.name"] for r in result}
        assert names == {"Jules"}

    def test_mixed_exists_and_not_exists(self, db):
        """Mixing EXISTS and NOT EXISTS in the same WHERE clause."""
        alix = db.create_node(["Person"], {"name": "Alix"})
        gus = db.create_node(["Person"], {"name": "Gus"})
        vincent = db.create_node(["Person"], {"name": "Vincent"})
        db.create_edge(alix.id, gus.id, "KNOWS")
        db.create_edge(alix.id, vincent.id, "KNOWS")
        db.create_edge(gus.id, vincent.id, "LIKES")
        # People who KNOW someone but do not LIKE anyone: Alix
        result = list(
            db.execute(
                "MATCH (p:Person) "
                "WHERE EXISTS { MATCH (p)-[:KNOWS]->() } "
                "AND NOT EXISTS { MATCH (p)-[:LIKES]->() } "
                "RETURN p.name"
            )
        )
        names = {r["p.name"] for r in result}
        assert names == {"Alix"}

    def test_not_exists_with_complex_inner_and_simple(self, db):
        """NOT EXISTS with a complex inner WHERE combined with a simple NOT EXISTS."""
        alix = db.create_node(["Person"], {"name": "Alix", "age": 30})
        gus = db.create_node(["Person"], {"name": "Gus", "age": 25})
        vincent = db.create_node(["Person"], {"name": "Vincent", "age": 35})
        mia = db.create_node(["Person"], {"name": "Mia", "age": 28})
        db.create_edge(alix.id, gus.id, "KNOWS")
        db.create_edge(gus.id, vincent.id, "KNOWS")
        db.create_edge(vincent.id, mia.id, "LIKES")
        # People who do not know anyone over 30, and who do not LIKE anyone
        result = list(
            db.execute(
                "MATCH (p:Person) "
                "WHERE NOT EXISTS { MATCH (p)-[:KNOWS]->(q:Person) WHERE q.age > 30 } "
                "AND NOT EXISTS { MATCH (p)-[:LIKES]->() } "
                "RETURN p.name"
            )
        )
        names = {r["p.name"] for r in result}
        # Alix knows Gus (age 25, not >30): passes first check, has no LIKES: passes second
        # Gus knows Vincent (age 35, >30): fails first check
        # Vincent: no outgoing KNOWS (passes first), LIKES Mia (fails second)
        # Mia: no outgoing KNOWS (passes first), no outgoing LIKES (passes second)
        assert names == {"Alix", "Mia"}
