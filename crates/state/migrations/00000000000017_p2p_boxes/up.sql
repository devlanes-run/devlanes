-- On a client: boxes paired by ticket, by the name `remote connect` uses.
-- Apart from remote_hosts, whose rows retention deletes, so the pairing stays.
CREATE TABLE p2p_boxes (
    name TEXT PRIMARY KEY NOT NULL CHECK(name != ''),
    peer_key TEXT NOT NULL UNIQUE,
    relay_url TEXT,
    -- Socket addresses from the ticket, separated by spaces.
    direct_addrs TEXT NOT NULL DEFAULT '',
    ephemeral INTEGER NOT NULL DEFAULT 0 CHECK(ephemeral IN (0, 1)),
    paired_at BIGINT NOT NULL
);
