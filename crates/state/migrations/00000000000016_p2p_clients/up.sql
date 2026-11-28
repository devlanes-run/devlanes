-- On a box: clients paired by ticket, by their peer key (64 lowercase hex).
-- One row per client: pairing again with a new key replaces the client's row.
CREATE TABLE p2p_clients (
    peer_key TEXT PRIMARY KEY NOT NULL,
    client_id TEXT NOT NULL,
    name TEXT NOT NULL,
    paired_at BIGINT NOT NULL,
    last_seen BIGINT
);
CREATE INDEX p2p_clients_client ON p2p_clients (client_id);
