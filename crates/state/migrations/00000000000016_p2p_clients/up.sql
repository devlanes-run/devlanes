-- On a box: clients paired by ticket, one row per peer key (64 lowercase hex).
-- Pairing a key again replaces its row; a client keeps several keys, and the
-- daemon drops its oldest beyond its limit.
CREATE TABLE p2p_clients (
    peer_key TEXT PRIMARY KEY NOT NULL,
    client_id TEXT NOT NULL,
    name TEXT NOT NULL,
    paired_at BIGINT NOT NULL,
    last_seen BIGINT
);
CREATE INDEX p2p_clients_client ON p2p_clients (client_id);
