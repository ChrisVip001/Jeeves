CREATE TABLE executor_control (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    stopped BOOLEAN NOT NULL,
    generation INTEGER NOT NULL CHECK (generation BETWEEN 0 AND 2147483647)
);
INSERT INTO executor_control VALUES (1, FALSE, 0);
