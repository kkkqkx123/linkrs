package linkrs

import (
	"testing"
)

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestLifecycle(t *testing.T) {
	if LibVersion() == "" {
		t.Fatal("expected a non-empty library version")
	}
	db, err := OpenMemory()
	must(t, err)
	defer func() { must(t, db.Close()) }()

	if db.LastCode() != CodeOK {
		t.Fatalf("unexpected last code: %d", db.LastCode())
	}

	must(t, db.CreateSpace("g", 1, 1, nil, nil))
	names, err := db.ListSpaces()
	must(t, err)
	if len(names) != 1 || names[0] != "g" {
		t.Fatalf("unexpected space list: %v", names)
	}
	space, err := db.GetSpace("g")
	must(t, err)
	if space == nil {
		t.Fatal("expected space description")
	}
	missing, err := db.GetSpace("missing")
	must(t, err)
	if missing != nil {
		t.Fatalf("expected nil for unknown space, got %v", missing)
	}

	props := []any{map[string]any{
		"name": "name", "data_type": "String",
		"nullable": true, "default": nil, "comment": nil,
	}}
	must(t, db.CreateTag("g", "user", props))
	tags, err := db.ListTags("g")
	must(t, err)
	if len(tags) != 1 || tags[0] != "user" {
		t.Fatalf("unexpected tag list: %v", tags)
	}
	tag, err := db.GetTag("g", "user")
	must(t, err)
	if tag == nil {
		t.Fatal("expected tag description")
	}
	edgeProps := []any{map[string]any{
		"name": "since", "data_type": "BigInt",
		"nullable": true, "default": nil, "comment": nil,
	}}
	must(t, db.CreateEdgeType("g", "knows", edgeProps))
	edges, err := db.ListEdgeTypes("g")
	must(t, err)
	if len(edges) != 1 || edges[0] != "knows" {
		t.Fatalf("unexpected edge type list: %v", edges)
	}

	session, err := db.Session()
	must(t, err)
	defer func() { must(t, session.Close()) }()

	must(t, session.UseSpace("g"))
	current, err := session.CurrentSpace()
	must(t, err)
	if current == nil || *current != "g" {
		t.Fatalf("unexpected current space: %v", current)
	}
	must(t, session.ValidateQuery("SHOW SPACES"))

	result, err := session.Execute("SHOW SPACES")
	must(t, err)
	rows, err := result.Rows()
	must(t, err)
	count, err := result.RowCount()
	must(t, err)
	if len(rows) != count || count < 1 {
		t.Fatalf("unexpected row count: %d vs %d", len(rows), count)
	}
	first, err := result.Row(0)
	must(t, err)
	if len(first) == 0 {
		t.Fatal("expected a non-empty first row")
	}
	columns, err := result.ColumnNames()
	must(t, err)
	if len(columns) == 0 {
		t.Fatal("expected column names")
	}
	must(t, result.Close())

	explained, err := session.ExplainQuery("SHOW SPACES")
	must(t, err)
	must(t, explained.Close())

	stats, err := session.Statistics()
	must(t, err)
	if stats == nil {
		t.Fatal("expected statistics")
	}

	txn, err := session.Begin()
	must(t, err)
	inner, err := txn.Execute("SHOW SPACES")
	must(t, err)
	must(t, inner.Close())
	save, err := txn.Savepoint("sp1")
	must(t, err)
	must(t, txn.RollbackToSavepoint(save))
	must(t, txn.ReleaseSavepoint(save))
	must(t, txn.Commit())
	must(t, txn.Close())
	if err := txn.Commit(); !IsMisuse(err) {
		t.Fatalf("expected misuse on double commit, got %v", err)
	}

	ro, err := session.BeginReadonly()
	must(t, err)
	must(t, ro.Rollback())
	must(t, ro.Close())

	must(t, db.DropEdgeType("g", "knows"))
	must(t, db.DropTag("g", "user"))
	must(t, db.DropSpace("g"))
	names, err = db.ListSpaces()
	must(t, err)
	if len(names) != 0 {
		t.Fatalf("unexpected space list after drop: %v", names)
	}

	if err := session.Close(); err != nil {
		t.Fatalf("second close must succeed: %v", err)
	}
}

func TestErrorMapping(t *testing.T) {
	db, err := OpenMemory()
	must(t, err)
	defer func() { must(t, db.Close()) }()

	session, err := db.Session()
	must(t, err)
	defer func() { must(t, session.Close()) }()

	if err := session.UseSpace("no_such_space"); CodeOf(err) != CodeIO {
		t.Fatalf("expected IO error for missing space, got %v", err)
	}
	if err := session.ValidateQuery("!!! not a query !!!"); CodeOf(err) != CodeError || ExtendedOf(err) != 1000 {
		t.Fatalf("expected general error with syntax extended code, got %v", err)
	}
	if _, err := session.Execute("SHOW SPACES"); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}
