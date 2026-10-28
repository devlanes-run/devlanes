-- On a client: which box serves a lane several boxes run, by lane reference
-- (`acme@0`). One box per lane; a new choice replaces the old one. Apart from
-- remote_hosts, so a host row written after the choice does not matter, and
-- retention and `remote disconnect` delete a box's choices themselves.
CREATE TABLE remote_lane_choices (
    lane TEXT PRIMARY KEY NOT NULL,
    host TEXT NOT NULL,
    chosen_at BIGINT NOT NULL
);
CREATE INDEX remote_lane_choices_host ON remote_lane_choices (host);
