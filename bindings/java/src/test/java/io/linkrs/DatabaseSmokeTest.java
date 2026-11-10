package io.linkrs;

import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/** Smoke coverage for the JNI binding against a fresh database. */
class DatabaseSmokeTest {
    private Database db;

    @BeforeEach
    void open() {
        db = Database.openMemory();
    }

    @AfterEach
    void close() {
        if (db != null) {
            db.close();
        }
    }

    private static Map<String, Object> props(String name, String dataType) {
        Map<String, Object> fields = new java.util.HashMap<>();
        fields.put("name", name);
        fields.put("data_type", dataType);
        fields.put("nullable", true);
        fields.put("default", null);
        fields.put("comment", null);
        return fields;
    }

    @Test
    void lifecycle() {
        assertTrue(Database.libversion() != null && !Database.libversion().isEmpty());
        assertEquals(0, db.lastCode());

        db.createSpace("g");
        assertEquals(List.of("g"), db.listSpaces());
        assertNotNull(db.getSpace("g"));
        assertNull(db.getSpace("missing"));

        db.createTag("g", "user", List.of(props("name", "String")));
        assertEquals(List.of("user"), db.listTags("g"));
        assertNotNull(db.getTag("g", "user"));
        db.createEdgeType("g", "knows", List.of(props("since", "BigInt")));
        assertEquals(List.of("knows"), db.listEdgeTypes("g"));
        assertNotNull(db.getEdgeType("g", "knows"));

        try (Session session = db.session()) {
            session.useSpace("g");
            assertEquals("g", session.currentSpace());
            session.validateQuery("SHOW SPACES");

            try (Result result = session.execute("SHOW SPACES")) {
                assertTrue(result.rowCount() >= 1);
                assertTrue(!result.columnNames().isEmpty());
                List<Map<String, Object>> rows = result.rows();
                assertEquals(result.rowCount(), rows.size());
                assertEquals(rows.get(0), result.row(0));
            }

            try (Result explained = session.explainQuery("SHOW SPACES")) {
                assertTrue(explained.rowCount() >= 0);
            }

            assertNotNull(session.statistics());

            try (Transaction txn = session.begin()) {
                try (Result inner = txn.execute("SHOW SPACES")) {
                    assertTrue(inner.rowCount() >= 1);
                }
                long save = txn.savepoint("sp1");
                txn.rollbackToSavepoint(save);
                txn.releaseSavepoint(save);
                txn.commit();
                assertThrows(MisuseError.class, txn::commit);
            }

            try (Transaction ro = session.beginReadonly()) {
                ro.rollback();
            }
        }

        db.dropEdgeType("g", "knows");
        db.dropTag("g", "user");
        db.dropSpace("g");
        assertEquals(List.of(), db.listSpaces());
    }

    @Test
    void errorMapping() {
        try (Session session = db.session()) {
            LinkrsError missing = assertThrows(LinkrsError.class,
                    () -> session.useSpace("no_such_space"));
            assertEquals(10, missing.getCode());
            LinkrsError syntax = assertThrows(LinkrsError.class,
                    () -> session.validateQuery("!!! not a query !!!"));
            assertEquals(1, syntax.getCode());
            assertEquals(1000, syntax.getExtended());
        }
    }
}
