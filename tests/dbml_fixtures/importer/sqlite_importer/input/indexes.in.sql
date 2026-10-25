CREATE TABLE events (
  id INTEGER PRIMARY KEY,
  user_id int,
  kind varchar(40)
);
CREATE UNIQUE INDEX events_kind_unique ON events (kind);
CREATE INDEX idx_multi ON events (user_id, kind);
CREATE INDEX idx_partial ON events (user_id) WHERE deleted = 0;
