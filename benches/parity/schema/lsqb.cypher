-- LSQB projected-fk schema + COPY script for Linkrs.
-- q1-q9 only count vertices and edges, so tags carry just the `id` primary
-- key (the storage layer rejects an empty property list for vertex tags) and
-- edge types use empty `()` lists. CSVs are pipe-delimited; the COPY header
-- rewrite keeps vid/src/dst and strips the `:type` suffixes of property
-- columns. See docs/release/benchmark/parity-benchmark.md.
--
-- Dialect rules (Linkrs-specific):
--   * `Comment` collides with the reserved word COMMENT -> `Comment_`.
--   * Edge types MUST declare `FROM <tag> TO <tag>`: without an endpoint
--     pair the edge table is not registered for checkpoint recovery and all
--     edges silently vanish across close/reopen.
--   * One edge type per endpoint pair: `ALTER EDGE ADD FROM ... TO ...` only
--     updates the schema, it never registers the physical table. This also
--     matches Ladybug's own LSQB layout (separate rel tables per pair).

CREATE SPACE IF NOT EXISTS lsqb (vid_type=INT64);
USE lsqb;

CREATE TAG IF NOT EXISTS Person(id: INT64);
CREATE TAG IF NOT EXISTS Comment_(id: INT64);
CREATE TAG IF NOT EXISTS Post(id: INT64);
CREATE TAG IF NOT EXISTS Forum(id: INT64);

CREATE EDGE IF NOT EXISTS KNOWS() FROM Person TO Person;
CREATE EDGE IF NOT EXISTS HASCREATOR_COMMENT() FROM Comment_ TO Person;
CREATE EDGE IF NOT EXISTS HASCREATOR_POST() FROM Post TO Person;
CREATE EDGE IF NOT EXISTS REPLYOF_COMMENT() FROM Comment_ TO Comment_;
CREATE EDGE IF NOT EXISTS REPLYOF_POST() FROM Comment_ TO Post;
CREATE EDGE IF NOT EXISTS CONTAINEROF() FROM Forum TO Post;

-- Vertices first (edge endpoints must exist).
COPY TAG Person FROM 'Person.csv' WITH (HEADER true, DELIMITER '|');
COPY TAG Comment_ FROM 'Comment.csv' WITH (HEADER true, DELIMITER '|');
COPY TAG Post FROM 'Post.csv' WITH (HEADER true, DELIMITER '|');
COPY TAG Forum FROM 'Forum.csv' WITH (HEADER true, DELIMITER '|');

COPY EDGE KNOWS FROM 'Person_knows_Person.csv' WITH (HEADER true, DELIMITER '|');
COPY EDGE HASCREATOR_COMMENT FROM 'Comment_hasCreator_Person.csv' WITH (HEADER true, DELIMITER '|');
COPY EDGE HASCREATOR_POST FROM 'Post_hasCreator_Person.csv' WITH (HEADER true, DELIMITER '|');
COPY EDGE REPLYOF_COMMENT FROM 'Comment_replyOf_Comment.csv' WITH (HEADER true, DELIMITER '|');
COPY EDGE REPLYOF_POST FROM 'Comment_replyOf_Post.csv' WITH (HEADER true, DELIMITER '|');
COPY EDGE CONTAINEROF FROM 'Forum_containerOf_Post.csv' WITH (HEADER true, DELIMITER '|');
