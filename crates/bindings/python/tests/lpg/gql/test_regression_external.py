"""Regression tests inspired by bugs found in other graph databases.

Covers MERGE semantics, pattern matching, aggregation edge cases,
Unicode handling, and query correctness.

Run: pytest tests/lpg/gql/test_regression_external.py -v
"""


# =============================================================================
# MERGE + UNWIND tuple count
# =============================================================================


class TestMergeUnwind:
    """MERGE inside UNWIND must produce one row per input."""

    def test_unwind_merge_returns_row_per_input(self, db):
        """UNWIND [1,1,1] + MERGE should produce 3 rows."""
        result = list(db.execute("UNWIND [1, 1, 1] AS i MERGE (:Item {val: i}) RETURN i"))
        assert len(result) == 3, "MERGE must emit one row per UNWIND input"

    def test_unwind_merge_creates_single_node(self, db):
        """Duplicate UNWIND values should create only one node."""
        db.execute("UNWIND [1, 1, 1] AS i MERGE (:Item {val: i})")
        result = list(db.execute("MATCH (n:Item) RETURN count(n) AS cnt"))
        assert result[0]["cnt"] == 1

    def test_unwind_merge_distinct_creates_multiple(self, db):
        """Distinct UNWIND values each create a node."""
        db.execute("UNWIND [1, 2, 3] AS i MERGE (:Item {val: i})")
        result = list(db.execute("MATCH (n:Item) RETURN count(n) AS cnt"))
        assert result[0]["cnt"] == 3

    def test_unwind_merge_mixed_create_and_match(self, db):
        """MERGE matches existing and creates new in one UNWIND."""
        db.execute("INSERT (:Item {val: 2})")
        db.execute("UNWIND [1, 2, 3] AS i MERGE (:Item {val: i})")
        result = list(db.execute("MATCH (n:Item) RETURN count(n) AS cnt"))
        assert result[0]["cnt"] == 3

    def test_unwind_merge_on_create_on_match(self, db):
        """ON CREATE SET and ON MATCH SET apply correctly per-row."""
        db.execute("INSERT (:Item {val: 1, status: 'old'})")
        db.execute(
            "UNWIND [1, 2] AS i "
            "MERGE (n:Item {val: i}) "
            "ON CREATE SET n.status = 'new' "
            "ON MATCH SET n.status = 'updated'"
        )
        result = list(db.execute("MATCH (n:Item) RETURN n.val, n.status ORDER BY n.val"))
        assert len(result) == 2
        # val=1 existed: matched and updated
        assert result[0]["n.val"] == 1
        assert result[0]["n.status"] == "updated"
        # val=2 was new: created
        assert result[1]["n.val"] == 2
        assert result[1]["n.status"] == "new"

    def test_unwind_merge_identity_stable(self, db):
        """Same node returned each time MERGE matches it."""
        result = list(
            db.execute("UNWIND [1, 1, 1] AS i MERGE (n:Item {val: i}) RETURN id(n) AS nid")
        )
        ids = [r["nid"] for r in result]
        assert len(set(ids)) == 1, "All rows should reference the same node"


# =============================================================================
# MERGE with composite keys
# =============================================================================


class TestMergeCompositeKeys:
    """MERGE with multiple property keys must match on the full composite."""

    def test_identical_composite_no_duplicate(self, db):
        """Same two-property key should not create a second node."""
        db.execute("MERGE (:City {name: 'Amsterdam', country: 'NL'})")
        db.execute("MERGE (:City {name: 'Amsterdam', country: 'NL'})")
        result = list(db.execute("MATCH (n:City) RETURN count(n) AS cnt"))
        assert result[0]["cnt"] == 1

    def test_partial_match_creates_new(self, db):
        """Same name, different country: must create a new node."""
        db.execute("MERGE (:City {name: 'Amsterdam', country: 'NL'})")
        db.execute("MERGE (:City {name: 'Amsterdam', country: 'US'})")
        result = list(db.execute("MATCH (n:City) RETURN count(n) AS cnt"))
        assert result[0]["cnt"] == 2

    def test_three_property_composite(self, db):
        """Three-property composite key deduplication."""
        db.execute("MERGE (:Place {city: 'Berlin', country: 'DE', district: 'Mitte'})")
        db.execute("MERGE (:Place {city: 'Berlin', country: 'DE', district: 'Mitte'})")
        db.execute("MERGE (:Place {city: 'Berlin', country: 'DE', district: 'Kreuzberg'})")
        result = list(db.execute("MATCH (n:Place) RETURN count(n) AS cnt"))
        assert result[0]["cnt"] == 2


# =============================================================================
# Relationship isomorphism
# =============================================================================


class TestRelationshipIsomorphism:
    """Same relationship must not be matched twice in a single pattern."""

    def test_single_edge_cannot_satisfy_two_hop(self, db):
        """One edge cannot be used as both r1 and r2 in a two-hop pattern."""
        db.execute("INSERT (:N {name: 'Alix'})-[:R]->(:N {name: 'Gus'})")
        result = list(db.execute("MATCH (a:N)-[r1:R]->(b:N)-[r2:R]->(c:N) RETURN count(*) AS cnt"))
        assert result[0]["cnt"] == 0


# =============================================================================
# OPTIONAL MATCH order independence
# =============================================================================


class TestOptionalMatchOrder:
    """Swapping OPTIONAL MATCH clauses must not change results."""

    def test_swapped_optional_match_same_results(self, db):
        db.execute("INSERT (:Person {name: 'Alix'})-[:KNOWS]->(:Person {name: 'Gus'})")
        db.execute("INSERT (:Person {name: 'Alix'})-[:WORKS_AT]->(:Company {name: 'Acme'})")
        r1 = list(
            db.execute(
                "MATCH (p:Person {name: 'Alix'}) "
                "OPTIONAL MATCH (p)-[:KNOWS]->(friend:Person) "
                "OPTIONAL MATCH (p)-[:WORKS_AT]->(co:Company) "
                "RETURN p.name, friend.name, co.name"
            )
        )
        r2 = list(
            db.execute(
                "MATCH (p:Person {name: 'Alix'}) "
                "OPTIONAL MATCH (p)-[:WORKS_AT]->(co:Company) "
                "OPTIONAL MATCH (p)-[:KNOWS]->(friend:Person) "
                "RETURN p.name, friend.name, co.name"
            )
        )
        assert len(r1) == len(r2), "Swapping OPTIONAL MATCH order must not change row count"


# =============================================================================
# Aggregation inside CALL subqueries
# =============================================================================


class TestAggregationInSubquery:
    """Aggregation functions inside CALL subqueries must work correctly."""

    def test_count_inside_call(self, db):
        db.create_node(["Person"], {"name": "Alix"})
        db.create_node(["Person"], {"name": "Gus"})
        db.create_node(["Person"], {"name": "Vincent"})
        result = list(db.execute("CALL { MATCH (n:Person) RETURN count(n) AS cnt } RETURN cnt"))
        assert len(result) == 1
        assert result[0]["cnt"] == 3


# =============================================================================
# COLLECT() completeness
# =============================================================================


class TestCollect:
    """COLLECT must gather all values without dropping any."""

    def test_collect_returns_all(self, db):
        for name in ["Vincent", "Alix", "Gus", "Jules", "Mia"]:
            db.execute(f"INSERT (:Person {{name: '{name}'}})")
        result = list(db.execute("MATCH (p:Person) RETURN collect(p.name) AS names"))
        assert len(result) == 1
        names = sorted(result[0]["names"])
        assert names == ["Alix", "Gus", "Jules", "Mia", "Vincent"]


# =============================================================================
# GROUP BY expression order independence
# =============================================================================


class TestGroupByExpressionOrder:
    """Column order in RETURN must not change GROUP BY results."""

    def test_column_order_does_not_affect_grouping(self, db):
        db.execute("INSERT (:Person {name: 'Alix', city: 'Amsterdam'})")
        db.execute("INSERT (:Person {name: 'Gus', city: 'Amsterdam'})")
        db.execute("INSERT (:Person {name: 'Vincent', city: 'Berlin'})")

        r1 = list(
            db.execute("MATCH (p:Person) RETURN p.city AS city, count(p) AS cnt ORDER BY city")
        )
        r2 = list(
            db.execute("MATCH (p:Person) RETURN count(p) AS cnt, p.city AS city ORDER BY city")
        )
        cities1 = [r["city"] for r in r1]
        cities2 = [r["city"] for r in r2]
        assert cities1 == cities2


# =============================================================================
# WHERE filter must not corrupt projected values
# =============================================================================


class TestWhereFilterAndProjection:
    """WHERE must not replace returned property values with booleans."""

    def test_is_not_null_preserves_return_value(self, db):
        db.execute("INSERT (:Sensor {name: 'Temp', reading: 42.5})")
        db.execute("INSERT (:Sensor {name: 'Humidity'})")
        result = list(
            db.execute("MATCH (s:Sensor) WHERE s.reading IS NOT NULL RETURN s.name, s.reading")
        )
        assert len(result) == 1
        assert result[0]["s.name"] == "Temp"
        assert result[0]["s.reading"] == 42.5  # Must NOT be True


# =============================================================================
# Unicode and emoji in property values
# =============================================================================


class TestUnicodeEmoji:
    """Storing and reading back Unicode, emoji, and CJK characters."""

    def test_emoji_roundtrip(self, db):
        db.execute("INSERT (:Tag {symbol: '\U0001f389', name: 'party'})")
        result = list(db.execute("MATCH (t:Tag) RETURN t.symbol, t.name"))
        assert result[0]["t.symbol"] == "\U0001f389"  # 🎉

    def test_cjk_roundtrip(self, db):
        db.execute("INSERT (:City {name: '\u6771\u4eac'})")  # 東京
        result = list(db.execute("MATCH (c:City) RETURN c.name"))
        assert result[0]["c.name"] == "\u6771\u4eac"


# =============================================================================
# Self-loop pattern matching
# =============================================================================


class TestSelfLoop:
    """Self-referencing edges must be created and matched correctly."""

    def test_create_and_match_self_loop(self, db):
        db.execute("INSERT (a:Node {name: 'Alix'})-[:SELF]->(a)")
        result = list(db.execute("MATCH (a:Node)-[r:SELF]->(a) RETURN a.name"))
        assert len(result) == 1
        assert result[0]["a.name"] == "Alix"

    def test_self_loop_counted_once(self, db):
        db.execute("INSERT (a:Node {name: 'Alix'})-[:SELF]->(a)")
        result = list(db.execute("MATCH (:Node)-[r:SELF]->() RETURN count(r) AS cnt"))
        assert result[0]["cnt"] == 1


# =============================================================================
# Deleted node access in same session
# =============================================================================


class TestDeletedNodeAccess:
    """Deleted nodes must not be visible in subsequent queries."""

    def test_delete_then_match(self, db):
        db.execute("INSERT (:Temp {name: 'ephemeral'})")
        db.execute("MATCH (n:Temp) DELETE n")
        result = list(db.execute("MATCH (n:Temp) RETURN n.name"))
        assert len(result) == 0

    def test_detach_delete_clears_edges(self, db):
        db.execute("INSERT (:A {name: 'a'})-[:R]->(:B {name: 'b'})")
        db.execute("MATCH (a:A) DETACH DELETE a")
        result = list(db.execute("MATCH ()-[r:R]->() RETURN count(r) AS cnt"))
        assert result[0]["cnt"] == 0


# =============================================================================
# Nested map/list properties
# =============================================================================


class TestNestedProperties:
    """Deeply nested maps and heterogeneous lists must roundtrip correctly."""

    def test_nested_map_via_params(self, db):
        meta = {"b": {"c": 42}}
        db.execute("INSERT (:Data {meta: $meta})", {"meta": meta})
        result = list(db.execute("MATCH (d:Data) RETURN d.meta"))
        assert result[0]["d.meta"]["b"]["c"] == 42

    def test_heterogeneous_list_via_params(self, db):
        items = [1, "two", True, None]
        db.execute("INSERT (:Data {items: $items})", {"items": items})
        result = list(db.execute("MATCH (d:Data) RETURN d.items"))
        assert len(result[0]["d.items"]) == 4


# =============================================================================
# Constant folding in WHERE
# =============================================================================


class TestConstantFolding:
    """Constant WHERE clauses must not error."""

    def test_where_false_returns_empty(self, db):
        db.execute("INSERT (:N {val: 1})")
        result = list(db.execute("MATCH (n:N) WHERE 1 = 2 RETURN n"))
        assert len(result) == 0

    def test_where_true_returns_all(self, db):
        db.execute("INSERT (:N {val: 1})")
        db.execute("INSERT (:N {val: 2})")
        result = list(db.execute("MATCH (n:N) WHERE 1 = 1 RETURN n.val"))
        assert len(result) == 2


# =============================================================================
# Cyclic graph traversal
# =============================================================================


class TestCyclicTraversal:
    """Variable-length paths through cycles must terminate."""

    def test_triangle_vle_terminates(self, db):
        db.execute(
            "INSERT (a:N {name: 'Alix'})-[:R]->(b:N {name: 'Gus'})"
            "-[:R]->(c:N {name: 'Vincent'})-[:R]->(a)"
        )
        result = list(db.execute("MATCH (a:N {name: 'Alix'})-[:R*1..5]->(b:N) RETURN b.name"))
        assert len(result) > 0, "Should find reachable nodes"
        assert len(result) < 100, f"Should not explode: got {len(result)} rows"


# =============================================================================
# MERGE with NULL node reference
# =============================================================================


class TestMergeNullReference:
    """MERGE on a NULL variable (from OPTIONAL MATCH) should error."""

    def test_merge_rel_with_null_source_errors(self, db):
        """OPTIONAL MATCH that matches nothing + MERGE should fail."""
        import pytest

        with pytest.raises(RuntimeError):
            db.execute("OPTIONAL MATCH (n:NonExistent) MERGE (n)-[:R]->(m:Target {name: 'Alix'})")

    def test_merge_rel_with_null_target_errors(self, db):
        """NULL target in MERGE relationship should fail."""
        import pytest

        db.execute("INSERT (:Source {name: 'Gus'})")
        with pytest.raises(RuntimeError):
            db.execute(
                "MATCH (a:Source {name: 'Gus'}) OPTIONAL MATCH (b:NonExistent) MERGE (a)-[:R]->(b)"
            )

    def test_standalone_merge_unaffected(self, db):
        """Standalone MERGE (no OPTIONAL MATCH) should still work."""
        db.execute("MERGE (:Person {name: 'Mia'})")
        result = list(db.execute("MATCH (n:Person {name: 'Mia'}) RETURN n.name"))
        assert len(result) == 1
