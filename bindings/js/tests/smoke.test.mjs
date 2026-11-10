import { describe, it, before, after } from "node:test";
import assert from "node:assert/strict";
import { Database, LinkrsError, MisuseError } from "../src/index.js";

describe("linkrs js binding smoke", () => {
  let db;

  before(() => {
    db = Database.openMemory();
  });

  after(() => {
    db?.close();
  });

  it("covers schema, session, query and transaction lifecycle", () => {
    assert.ok(Database.libversion());
    assert.equal(db.lastCode, 0);

    db.createSpace("g");
    assert.deepEqual(db.listSpaces(), ["g"]);
    assert.ok(db.getSpace("g"));
    assert.equal(db.getSpace("missing"), null);

    db.createTag("g", "user", [
      { name: "name", data_type: "String", nullable: true, default: null, comment: null },
    ]);
    assert.deepEqual(db.listTags("g"), ["user"]);
    assert.ok(db.getTag("g", "user"));
    db.createEdgeType("g", "knows", [
      { name: "since", data_type: "BigInt", nullable: true, default: null, comment: null },
    ]);
    assert.deepEqual(db.listEdgeTypes("g"), ["knows"]);
    assert.ok(db.getEdgeType("g", "knows"));

    const session = db.session();
    try {
      session.useSpace("g");
      assert.equal(session.currentSpace(), "g");
      assert.equal(session.validateQuery("SHOW SPACES"), true);

      const result = session.execute("SHOW SPACES");
      try {
        assert.ok(result.rowCount() >= 1);
        assert.ok(result.columnNames().length > 0);
        const rows = result.rows();
        assert.equal(rows.length, result.rowCount());
        assert.deepEqual(result.row(0), rows[0]);
      } finally {
        result.close();
      }

      const explained = session.explainQuery("SHOW SPACES");
      try {
        assert.ok(explained.rowCount() >= 0);
      } finally {
        explained.close();
      }

      const stats = session.statistics();
      assert.ok("totalChanges" in stats);

      const txn = session.begin();
      try {
        const inner = txn.execute("SHOW SPACES");
        try {
          assert.ok(inner.rowCount() >= 1);
        } finally {
          inner.close();
        }
        const save = txn.savepoint("sp1");
        txn.rollbackToSavepoint(save);
        txn.releaseSavepoint(save);
      } finally {
        txn.commit();
        txn.close();
      }
      assert.throws(() => txn.commit(), MisuseError);

      const ro = session.beginReadonly();
      try {
        ro.rollback();
      } finally {
        ro.close();
      }
    } finally {
      session.close();
    }

    db.dropEdgeType("g", "knows");
    db.dropTag("g", "user");
    db.dropSpace("g");
    assert.deepEqual(db.listSpaces(), []);
  });

  it("maps errors to typed exceptions", () => {
    const session = db.session();
    try {
      assert.throws(() => session.useSpace("no_such_space"), LinkrsError);
      assert.throws(() => session.validateQuery("!!! not a query !!!"), (err) => {
        assert.ok(err instanceof MisuseError);
        assert.equal(err.code, 20);
        assert.match(String(err), /syntax/i);
        return true;
      });
      assert.ok(db.lastMessage.length >= 0);
    } finally {
      session.close();
    }
  });
});
