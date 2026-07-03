---
title: Aggregations
description: Aggregation functions and grouping in GQL.
tags:
  - gql
  - aggregations
---

# Aggregations

GQL provides aggregation functions for computing summaries over query results.

## Aggregation Functions

| Function | Description |
|----------|-------------|
| `count()` | Count items |
| `sum()` | Sum of values |
| `avg()` | Average of values |
| `min()` | Minimum value |
| `max()` | Maximum value |
| `collect()` | Collect into list |
| `variance()` | Sample variance (aliases: `var_samp()`) |
| `var_pop()` | Population variance |
| `stdev()` | Sample standard deviation (aliases: `stddev()`, `stddev_samp()`) |
| `stdevp()` | Population standard deviation (aliases: `stddevp()`, `stddev_pop()`) |

## Count

```sql
-- Count all nodes
MATCH (n)
RETURN count(n)

-- Count with label
MATCH (p:Person)
RETURN count(p)

-- Count distinct
MATCH (p:Person)-[:LIVES_IN]->(c:City)
RETURN count(DISTINCT c)
```

## Sum, Avg, Min, Max

```sql
-- Sum
MATCH (o:Order)
RETURN sum(o.total)

-- Average
MATCH (p:Person)
RETURN avg(p.age)

-- Min and Max
MATCH (p:Product)
RETURN min(p.price), max(p.price)
```

## Collect

```sql
-- Collect into list
MATCH (p:Person)
RETURN collect(p.name)

-- Collect with limit
MATCH (p:Person)
RETURN collect(p.name)[0..5]
```

## Grouping

```sql
-- Group by property
MATCH (p:Person)
RETURN p.city, count(p) AS population
ORDER BY population DESC

-- Group by relationship target
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
RETURN c.name, count(p) AS employees

-- Multiple aggregations
MATCH (o:Order)
RETURN
    o.status,
    count(o) AS order_count,
    sum(o.total) AS total_revenue,
    avg(o.total) AS avg_order_value
```

## WITH Clause

The `WITH` clause allows intermediate aggregations:

```sql
-- Find people with more than 5 friends
MATCH (p:Person)-[:KNOWS]->(friend)
WITH p, count(friend) AS friend_count
WHERE friend_count > 5
RETURN p.name, friend_count
```

## Variance

```sql
-- Sample variance (divides by n-1)
MATCH (p:Person)
RETURN variance(p.age) AS age_var

-- var_samp() is an alias for variance()
MATCH (p:Person)
RETURN var_samp(p.age)

-- Population variance (divides by n)
MATCH (p:Person)
RETURN var_pop(p.age) AS age_var_pop
```

## Standard Deviation

```sql
-- Sample standard deviation (sqrt of sample variance)
MATCH (p:Person)
RETURN stdev(p.age) AS age_stdev

-- stddev() and stddev_samp() are aliases
MATCH (p:Person)
RETURN stddev_samp(p.age)

-- Population standard deviation (sqrt of population variance)
MATCH (p:Person)
RETURN stdevp(p.age) AS age_stdevp

-- stddev_pop() is an alias for stdevp()
MATCH (p:Person)
RETURN stddev_pop(p.age)
```

## Percentiles

```sql
-- Discrete percentile (returns the nearest actual value)
MATCH (p:Person)
RETURN percentile_disc(p.salary, 0.5) AS median_salary

-- Continuous percentile (interpolates between values)
MATCH (p:Person)
RETURN percentile_cont(p.salary, 0.5) AS median_salary

-- Multiple percentiles
MATCH (p:Person)
RETURN
    percentile_cont(p.salary, 0.25) AS p25,
    percentile_cont(p.salary, 0.50) AS median,
    percentile_cont(p.salary, 0.75) AS p75,
    percentile_cont(p.salary, 0.90) AS p90
```

## Explicit GROUP BY

GQL supports explicit `GROUP BY` as an alternative to implicit grouping via non-aggregated columns in `RETURN`:

```sql
-- Explicit GROUP BY
MATCH (p:Person)-[:LIVES_IN]->(c:City)
RETURN c.name, count(p) AS population
GROUP BY c.name

-- Multiple GROUP BY keys
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
RETURN c.name, p.department, count(p) AS headcount
GROUP BY c.name, p.department
```

## HAVING

Filter on aggregated results using `HAVING`. This applies after grouping, unlike `WHERE` which filters before grouping:

```sql
-- Cities with more than 100 people
MATCH (p:Person)-[:LIVES_IN]->(c:City)
RETURN c.name, count(p) AS population
GROUP BY c.name
HAVING count(p) > 100

-- Departments with above-average salary
MATCH (p:Person)
RETURN p.department,
    avg(p.salary) AS avg_salary,
    count(p) AS headcount
GROUP BY p.department
HAVING avg(p.salary) > 80000
ORDER BY avg_salary DESC
```
