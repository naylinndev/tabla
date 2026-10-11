-- Test fixture for the SQLite integration tests.
CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT NOT NULL, body TEXT, score REAL, data BLOB);
INSERT INTO notes (title, body, score, data) VALUES ('hello', 'world', 1.5, x'0102'), ('two', NULL, NULL, NULL);
