-- Boxes connected with `remote connect` and not disconnected since, which a
-- restarted daemon connects again. Rows from earlier daemons start unwanted.
ALTER TABLE remote_hosts ADD COLUMN wanted INTEGER NOT NULL DEFAULT 0 CHECK(wanted IN (0, 1));

-- On a box: the clients it paired with, one row per client id. Only the
-- SHA-256 of each remote token is kept, as lowercase hex.
CREATE TABLE remote_pairings (
    client_id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    token_sha256 TEXT NOT NULL,
    paired_at BIGINT NOT NULL,
    last_seen BIGINT
);

-- On a client: the remote token each box gave this machine, by the name it
-- connects to the box with. Apart from remote_hosts, whose rows retention
-- deletes, so an old box keeps its pairing.
CREATE TABLE remote_tokens (
    host TEXT PRIMARY KEY NOT NULL,
    token TEXT NOT NULL,
    paired_at BIGINT NOT NULL
);
