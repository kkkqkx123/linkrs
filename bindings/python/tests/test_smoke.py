"""Smoke coverage for the ctypes binding against a fresh database."""

import unittest

from linkrs import (
    Database,
    LinkrsError,
    MisuseError,
)


class BindingSmokeTest(unittest.TestCase):
    def test_lifecycle(self):
        with Database.open_memory() as db:
            self.assertTrue(db.libversion())
            self.assertEqual(db.errcode, 0)

            db.create_space("g")
            self.assertEqual(db.list_spaces(), ["g"])
            self.assertIsNotNone(db.get_space("g"))
            self.assertIsNone(db.get_space("missing"))

            db.create_tag("g", "user", [
                {"name": "name", "data_type": "String", "nullable": True,
                 "default": None, "comment": None},
            ])
            self.assertEqual(db.list_tags("g"), ["user"])
            self.assertIsNotNone(db.get_tag("g", "user"))
            db.create_edge_type("g", "knows", [
                {"name": "since", "data_type": "BigInt", "nullable": True,
                 "default": None, "comment": None},
            ])
            self.assertEqual(db.list_edge_types("g"), ["knows"])
            self.assertIsNotNone(db.get_edge_type("g", "knows"))

            with db.session() as session:
                session.use_space("g")
                self.assertEqual(session.current_space(), "g")
                self.assertTrue(session.validate_query("SHOW SPACES"))

                with session.execute("SHOW SPACES") as result:
                    self.assertGreaterEqual(result.row_count(), 1)
                    self.assertTrue(result.column_names())
                    rows = result.rows()
                    self.assertEqual(len(rows), result.row_count())
                    self.assertEqual(result.row(0), rows[0])

                with session.explain_query("SHOW SPACES") as explained:
                    self.assertGreaterEqual(explained.row_count(), 0)

                stats = session.statistics()
                self.assertIn("total_changes", stats)

                with session.begin() as txn:
                    with txn.execute("SHOW SPACES") as inner:
                        self.assertGreaterEqual(inner.row_count(), 1)
                    save = txn.savepoint("sp1")
                    txn.rollback_to_savepoint(save)
                    txn.release_savepoint(save)

                with session.begin_readonly() as txn:
                    txn.rollback()

                with session.begin() as txn:
                    txn.commit()
                # Double commit must surface as misuse, not crash.
                with self.assertRaises(MisuseError):
                    txn.commit()

            db.drop_edge_type("g", "knows")
            db.drop_tag("g", "user")
            db.drop_space("g")
            self.assertEqual(db.list_spaces(), [])

        # Use after close must surface as misuse, not crash.
        with self.assertRaises(MisuseError):
            db.session()
        db.close()

    def test_error_mapping(self):
        with Database.open_memory() as db:
            with db.session() as session:
                # Missing space surfaces through the storage layer as IOERR.
                with self.assertRaises(LinkrsError) as ctx:
                    session.use_space("no_such_space")
                self.assertEqual(ctx.exception.code, 10)
                # Garbage text fails validation as a misuse error carrying
                # the syntax detail in the message.
                with self.assertRaises(MisuseError) as ctx:
                    session.validate_query("!!! not a query !!!")
                self.assertEqual(ctx.exception.code, 20)
                self.assertIn("syntax", str(ctx.exception).lower())
                self.assertTrue(db.last_message)


if __name__ == "__main__":
    unittest.main()
