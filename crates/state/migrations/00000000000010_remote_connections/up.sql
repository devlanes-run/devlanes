-- Boxes connected with `remote connect` and not disconnected since, which a
-- restarted daemon connects again. Rows from earlier daemons start unwanted.
ALTER TABLE remote_hosts ADD COLUMN wanted INTEGER NOT NULL DEFAULT 0 CHECK(wanted IN (0, 1));

-- On a box: one row per pairing, so a client that pairs under two box names
-- keeps both. Only the SHA-256 of each remote token is kept, as lowercase hex.
CREATE TABLE remote_pairings (
    token_sha256 TEXT PRIMARY KEY NOT NULL,
    client_id TEXT NOT NULL,
    name TEXT NOT NULL,
    paired_at BIGINT NOT NULL,
    last_seen BIGINT
);
CREATE INDEX remote_pairings_client ON remote_pairings (client_id);

-- On a client: the remote token each box gave this machine, by the name it
-- connects to the box with. Apart from remote_hosts, whose rows retention
-- deletes, so an old box keeps its pairing.
CREATE TABLE remote_tokens (
    host TEXT PRIMARY KEY NOT NULL,
    token TEXT NOT NULL,
    paired_at BIGINT NOT NULL
);
